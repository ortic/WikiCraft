//! Brings pages stored in an older document format ([`wikicraft_doc_model::schema`]) up to date
//! in the background, in small batches, so exports, shared pages and the API see the current
//! format even for pages nobody opens. Opening a page upgrades it too (see `collab`).
//!
//! Each page is upgraded under its row lock: the stored Yjs state gets the change as CRDT
//! operations (so a live session on another replica merges it), the JSON is derived from it,
//! and `updated_at` / versions are left alone: it isn't an edit.

use std::time::Duration;

use uuid::Uuid;
use wikicraft_doc_model::{Node, schema, ydoc};

use crate::app::AppState;

const BATCH: i64 = 50;

pub fn spawn(state: &AppState) {
    let state = state.clone();
    tokio::spawn(async move {
        loop {
            match upgrade_batch(&state, BATCH).await {
                Ok(0) => break,
                Ok(n) => {
                    tracing::info!(pages = n, "upgraded pages to document format {}", schema::VERSION);
                    tokio::time::sleep(Duration::from_millis(200)).await;
                }
                Err(e) => {
                    tracing::warn!("document format upgrade failed, retrying later: {e:#}");
                    tokio::time::sleep(Duration::from_secs(60)).await;
                }
            }
        }
    });
}

/// Upgrades up to `limit` outdated pages; returns how many were handled (0 = all done).
pub async fn upgrade_batch(state: &AppState, limit: i64) -> anyhow::Result<usize> {
    let ids: Vec<Uuid> = sqlx::query_scalar("SELECT id FROM pages WHERE schema_version < $1 ORDER BY id LIMIT $2")
        .bind(schema::VERSION)
        .bind(limit)
        .fetch_all(&state.db)
        .await?;
    for id in &ids {
        upgrade_page(state, *id).await?;
    }
    Ok(ids.len())
}

async fn upgrade_page(state: &AppState, id: Uuid) -> anyhow::Result<()> {
    let mut tx = state.db.begin().await?;
    let row: Option<(serde_json::Value, Option<Vec<u8>>, i32)> =
        sqlx::query_as("SELECT content, ydoc, schema_version FROM pages WHERE id = $1 AND schema_version < $2 FOR UPDATE")
            .bind(id)
            .bind(schema::VERSION)
            .fetch_optional(&mut *tx)
            .await?;
    // another replica (or the page being opened) got there first
    let Some((content, ystate, from)) = row else { return Ok(()) };
    match ystate.filter(|b| !b.is_empty()) {
        Some(bytes) => {
            let doc = ydoc::new_doc();
            ydoc::apply_update(&doc, &bytes)?;
            if schema::upgrade_ydoc(&doc, from) {
                sqlx::query("UPDATE pages SET ydoc = $2, content = $3, schema_version = $4 WHERE id = $1")
                    .bind(id)
                    .bind(ydoc::encode_state(&doc))
                    .bind(ydoc::doc_to_pm(&doc).to_json())
                    .bind(schema::VERSION)
                    .execute(&mut *tx)
                    .await?;
            } else {
                sqlx::query("UPDATE pages SET schema_version = $2 WHERE id = $1").bind(id).bind(schema::VERSION).execute(&mut *tx).await?;
            }
        }
        None => {
            // never opened for editing: only JSON
            let mut doc = Node::from_json(&content).unwrap_or_else(|_| Node::empty_doc());
            schema::upgrade(&mut doc, from);
            sqlx::query("UPDATE pages SET content = $2, schema_version = $3 WHERE id = $1")
                .bind(id)
                .bind(doc.to_json())
                .bind(schema::VERSION)
                .execute(&mut *tx)
                .await?;
        }
    }
    tx.commit().await?;
    Ok(())
}
