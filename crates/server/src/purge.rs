//! Permanent deletion of trashed pages: on request (Delete forever, Empty trash) and after the
//! workspace's trash retention period (`trashRetentionDays`, 0 = keep forever).
//!
//! A purged page takes its trashed sub-pages, versions, comments, attachments (all versions),
//! likes, watches and links along (foreign keys cascade). Attachment files are stored once per
//! content hash, so a file is only removed from storage when no attachment and no workspace logo
//! refers to it any more.

use std::collections::HashSet;

use serde_json::json;
use uuid::Uuid;

use crate::app::AppState;

/// Deletes `roots` and their trashed descendants for good; returns how many pages went.
/// Only pages that are in the trash are touched.
pub async fn purge_pages(state: &AppState, roots: &[Uuid]) -> anyhow::Result<Vec<Uuid>> {
    if roots.is_empty() {
        return Ok(vec![]);
    }
    let mut tx = state.db.begin().await?;
    let ids: Vec<Uuid> = sqlx::query_scalar(
        "WITH RECURSIVE sub AS (
             SELECT id FROM pages WHERE id = ANY($1) AND deleted_at IS NOT NULL
             UNION SELECT p.id FROM pages p JOIN sub ON p.parent_id = sub.id WHERE p.deleted_at IS NOT NULL
         ) SELECT id FROM sub",
    )
    .bind(roots)
    .fetch_all(&mut *tx)
    .await?;
    if ids.is_empty() {
        return Ok(vec![]);
    }
    let keys: Vec<String> = sqlx::query_scalar(
        "SELECT a.blob_key FROM attachments a WHERE a.page_id = ANY($1) AND a.blob_key IS NOT NULL
         UNION SELECT v.blob_key FROM attachment_versions v JOIN attachments a ON a.id = v.attachment_id WHERE a.page_id = ANY($1) AND v.blob_key IS NOT NULL",
    )
    .bind(&ids)
    .fetch_all(&mut *tx)
    .await?;
    sqlx::query("DELETE FROM import_mappings WHERE source_type = 'page' AND target_id = ANY($1)").bind(&ids).execute(&mut *tx).await?;
    sqlx::query("DELETE FROM pages WHERE id = ANY($1)").bind(&ids).execute(&mut *tx).await?;
    tx.commit().await?;
    for id in &ids {
        state.rooms.evict(*id);
        state.search.enqueue(*id);
    }
    collect_blobs(state, keys).await;
    Ok(ids)
}

/// Removes files nothing refers to any more (row first, under the foreign keys, then the file).
async fn collect_blobs(state: &AppState, keys: Vec<String>) {
    let keys: HashSet<String> = keys.into_iter().collect();
    for key in keys {
        let deleted = sqlx::query(
            "DELETE FROM blobs b WHERE b.key = $1
               AND NOT EXISTS (SELECT 1 FROM attachments WHERE blob_key = $1)
               AND NOT EXISTS (SELECT 1 FROM attachment_versions WHERE blob_key = $1)
               AND NOT EXISTS (SELECT 1 FROM workspaces WHERE settings->'logo'->>'key' = $1)
               AND NOT EXISTS (SELECT 1 FROM users WHERE avatar_key = $1)",
        )
        .bind(&key)
        .execute(&state.db)
        .await;
        match deleted {
            Ok(r) if r.rows_affected() == 1 => {
                if let Err(e) = state.blobs.delete_blob(&key).await {
                    tracing::warn!(key, "removing an unused file failed: {e:#}");
                }
            }
            Ok(_) => {}
            // referenced again meanwhile (an upload of the same file): keep it
            Err(e) => tracing::debug!(key, "file still in use: {e}"),
        }
    }
}

/// Purges pages that have been in the trash longer than their workspace's retention period.
pub async fn purge_expired(state: &AppState) -> anyhow::Result<usize> {
    let rows: Vec<(Uuid, Uuid)> = sqlx::query_as(
        "SELECT p.id, p.workspace_id FROM pages p JOIN workspaces w ON w.id = p.workspace_id
          LEFT JOIN pages parent ON parent.id = p.parent_id
          WHERE p.deleted_at IS NOT NULL
            AND COALESCE((w.settings->>'trashRetentionDays')::int, 0) > 0
            AND p.deleted_at < now() - make_interval(days => (w.settings->>'trashRetentionDays')::int)
            AND (parent.id IS NULL OR parent.deleted_at IS NULL)
          LIMIT 200",
    )
    .fetch_all(&state.db)
    .await?;
    let mut total = 0;
    for (root, ws) in rows {
        let gone = purge_pages(state, &[root]).await?;
        if !gone.is_empty() {
            total += gone.len();
            crate::audit::log(state, ws, None, "page.purge", Some("page"), Some(root), json!({"count": gone.len(), "reason": "retention"})).await;
        }
    }
    Ok(total)
}
