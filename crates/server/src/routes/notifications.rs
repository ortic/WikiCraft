//! In-app notifications and page watching.

use std::collections::HashSet;

use axum::Json;
use axum::extract::{Path, State};
use axum::http::StatusCode;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use uuid::Uuid;
use wikicraft_doc_model::{Node, nodes, text};

use crate::app::{AppState, new_id};
use crate::auth::CurrentUser;
use crate::authz::{Need, Principal, page_access, require_page};
use crate::error::ApiResult;

#[derive(Serialize, sqlx::FromRow)]
#[serde(rename_all = "camelCase")]
pub struct NotificationRow {
    id: Uuid,
    kind: String,
    page_id: Option<Uuid>,
    page_title: Option<String>,
    actor_name: Option<String>,
    data: Value,
    created_at: chrono::DateTime<chrono::Utc>,
    read_at: Option<chrono::DateTime<chrono::Utc>>,
}

pub async fn list(State(state): State<AppState>, CurrentUser(user): CurrentUser) -> ApiResult<Json<Value>> {
    let rows = sqlx::query_as::<_, NotificationRow>(
        "SELECT n.id, n.kind, n.page_id, p.title AS page_title, a.name AS actor_name, n.data, n.created_at, n.read_at
         FROM notifications n LEFT JOIN pages p ON p.id = n.page_id LEFT JOIN users a ON a.id = n.actor_id
         WHERE n.user_id = $1 AND (p.id IS NULL OR p.deleted_at IS NULL)
           AND NOT (n.kind = ANY (COALESCE((SELECT muted_notifications FROM users WHERE id = $1), '{}')))
         ORDER BY n.created_at DESC LIMIT 50",
    )
    .bind(user.id)
    .fetch_all(&state.db)
    .await?;
    // don't leak titles of pages the user can no longer see
    let p = Principal::load(&state, Some(user.clone())).await?;
    let mut out = Vec::with_capacity(rows.len());
    for r in rows {
        if let Some(pid) = r.page_id {
            match page_access(&state, &p, pid).await {
                Ok((a, _)) if a.view => {}
                _ => continue,
            }
        }
        out.push(r);
    }
    let unread = out.iter().filter(|r| r.read_at.is_none()).count();
    Ok(Json(json!({ "unread": unread, "items": out })))
}

#[derive(Deserialize)]
pub struct MarkRead {
    ids: Option<Vec<Uuid>>,
}

pub async fn mark_read(State(state): State<AppState>, CurrentUser(user): CurrentUser, Json(req): Json<MarkRead>) -> ApiResult<StatusCode> {
    match req.ids {
        Some(ids) => {
            sqlx::query("UPDATE notifications SET read_at = now() WHERE user_id = $1 AND id = ANY($2) AND read_at IS NULL")
                .bind(user.id)
                .bind(&ids)
                .execute(&state.db)
                .await?
        }
        None => sqlx::query("UPDATE notifications SET read_at = now() WHERE user_id = $1 AND read_at IS NULL").bind(user.id).execute(&state.db).await?,
    };
    Ok(StatusCode::NO_CONTENT)
}

#[derive(Deserialize, Default)]
#[serde(rename_all = "camelCase", default)]
pub struct WatchReq {
    /// also the pages below it: their changes and new pages
    include_children: bool,
}

/// `PUT /pages/{id}/watch` — watch the page (body optional: `{"includeChildren": true}` also
/// watches everything below it).
pub async fn watch(State(state): State<AppState>, CurrentUser(user): CurrentUser, Path(id): Path<Uuid>, body: axum::body::Bytes) -> ApiResult<StatusCode> {
    let req: WatchReq = if body.iter().all(u8::is_ascii_whitespace) {
        WatchReq::default()
    } else {
        serde_json::from_slice(&body).map_err(|e| crate::error::ApiError::bad(format!("invalid body: {e}")))?
    };
    let p = Principal::load(&state, Some(user.clone())).await?;
    require_page(&state, &p, id, Need::View).await?;
    sqlx::query(
        "INSERT INTO watches (user_id, page_id, include_children) VALUES ($1, $2, $3)
         ON CONFLICT (user_id, page_id) DO UPDATE SET include_children = EXCLUDED.include_children",
    )
    .bind(user.id)
    .bind(id)
    .bind(req.include_children)
    .execute(&state.db)
    .await?;
    sqlx::query("DELETE FROM page_unwatched WHERE user_id = $1 AND page_id = $2").bind(user.id).bind(id).execute(&state.db).await?;
    Ok(StatusCode::NO_CONTENT)
}

pub async fn unwatch(State(state): State<AppState>, CurrentUser(user): CurrentUser, Path(id): Path<Uuid>) -> ApiResult<StatusCode> {
    sqlx::query("DELETE FROM watches WHERE user_id = $1 AND page_id = $2").bind(user.id).bind(id).execute(&state.db).await?;
    // remembered, so editing the page later doesn't sign them up again
    sqlx::query("INSERT INTO page_unwatched (user_id, page_id) SELECT $1, $2 WHERE EXISTS (SELECT 1 FROM pages WHERE id = $2) ON CONFLICT DO NOTHING")
        .bind(user.id)
        .bind(id)
        .execute(&state.db)
        .await?;
    Ok(StatusCode::NO_CONTENT)
}

/// People who edit a page watch it from then on, unless they stopped watching it on purpose.
pub async fn auto_watch(state: &AppState, page_id: Uuid, editor: Uuid) -> sqlx::Result<()> {
    sqlx::query(
        "INSERT INTO watches (user_id, page_id) SELECT $1, $2
          WHERE NOT EXISTS (SELECT 1 FROM page_unwatched WHERE user_id = $1 AND page_id = $2)
         ON CONFLICT DO NOTHING",
    )
    .bind(editor)
    .bind(page_id)
    .execute(&state.db)
    .await?;
    Ok(())
}

/// Notifies users newly @mentioned in a page body (each user once per page) and adds them as
/// watchers. Users who can't view the page are skipped.
pub async fn notify_page_mentions(state: &AppState, page_id: Uuid, doc: &Node, actor: Option<Uuid>) -> anyhow::Result<()> {
    if crate::authz::in_draft(state, page_id).await {
        return Ok(()); // mentioned people are told when the draft is published
    }
    let mut ids: HashSet<Uuid> = HashSet::new();
    doc.walk(&mut |n| {
        if n.kind == nodes::MENTION
            && let Some(id) = n.attr_str("id").and_then(|s| s.parse().ok())
        {
            ids.insert(id);
        }
    });
    if ids.is_empty() {
        return Ok(());
    }
    let ids: Vec<Uuid> = ids.into_iter().collect();
    let new: Vec<Uuid> = sqlx::query_scalar(
        "INSERT INTO page_mentions (page_id, user_id) SELECT $1, u.id FROM users u WHERE u.id = ANY($2) AND u.status = 'active' ON CONFLICT DO NOTHING RETURNING user_id",
    )
    .bind(page_id)
    .bind(&ids)
    .fetch_all(&state.db)
    .await?;
    let excerpt = text::excerpt(doc, 140);
    for uid in new {
        if Some(uid) == actor {
            continue;
        }
        let user = sqlx::query_as::<_, crate::auth::User>(sqlx::AssertSqlSafe(format!("SELECT {} FROM users WHERE id = $1", crate::auth::USER_COLUMNS)))
            .bind(uid)
            .fetch_optional(&state.db)
            .await?;
        let Some(user) = user else { continue };
        let p = Principal::load(state, Some(user)).await.map_err(|e| anyhow::anyhow!("{e}"))?;
        let can_view = page_access(state, &p, page_id).await.map(|(a, _)| a.view).unwrap_or(false);
        if !can_view {
            continue;
        }
        sqlx::query("INSERT INTO notifications (id, user_id, kind, page_id, actor_id, data) VALUES ($1, $2, 'page_mention', $3, $4, $5)")
            .bind(new_id())
            .bind(uid)
            .bind(page_id)
            .bind(actor)
            .bind(json!({ "excerpt": excerpt }))
            .execute(&state.db)
            .await?;
        sqlx::query("INSERT INTO watches (user_id, page_id) VALUES ($1, $2) ON CONFLICT DO NOTHING").bind(uid).bind(page_id).execute(&state.db).await?;
    }
    Ok(())
}

/// Notifies watchers that a page got a new version.
pub async fn notify_page_updated(state: &AppState, page_id: Uuid, version: i32, editor: Option<Uuid>) -> anyhow::Result<()> {
    if crate::authz::in_draft(state, page_id).await {
        return Ok(());
    }
    // watchers of the page, of a page above it (with the pages below) and of its space (the
    // list only shows pages they can see)
    sqlx::query(sqlx::AssertSqlSafe(format!(
        "WITH RECURSIVE {ANCESTORS}
         INSERT INTO notifications (id, user_id, kind, page_id, actor_id, data)
         SELECT gen_random_uuid(), x.user_id, 'page_updated', $1, $2, $3
         FROM (SELECT w.user_id FROM watches w WHERE w.page_id = $1
               UNION {SUBTREE_WATCHERS}
               UNION SELECT sw.user_id FROM space_watches sw JOIN pages p ON p.space_id = sw.space_id WHERE p.id = $1) x
         JOIN users u ON u.id = x.user_id
         WHERE u.status = 'active' AND x.user_id IS DISTINCT FROM $2"
    )))
    .bind(page_id)
    .bind(editor)
    .bind(json!({ "version": version }))
    .execute(&state.db)
    .await?;
    Ok(())
}

/// `ancestors(id)`: the pages above `$1`.
const ANCESTORS: &str = "ancestors AS (
    SELECT parent_id AS id, 1 AS depth FROM pages WHERE id = $1 AND parent_id IS NOT NULL
    UNION ALL SELECT p.parent_id, a.depth + 1 FROM pages p JOIN ancestors a ON p.id = a.id WHERE p.parent_id IS NOT NULL AND a.depth < 200)";

/// People watching a page above `$1` with the pages below, unless they stopped watching `$1`.
const SUBTREE_WATCHERS: &str = "SELECT w.user_id FROM watches w JOIN ancestors a ON a.id = w.page_id
    WHERE w.include_children AND NOT EXISTS (SELECT 1 FROM page_unwatched pu WHERE pu.user_id = w.user_id AND pu.page_id = $1)";

/// Tells the people watching a space (or a page above the new one, with the pages below) about
/// a new page.
pub async fn notify_page_created(state: &AppState, page_id: Uuid, creator: Uuid) -> anyhow::Result<()> {
    if crate::authz::in_draft(state, page_id).await {
        return Ok(());
    }
    // watchers of the space and of a page above it (with the pages below)
    sqlx::query(sqlx::AssertSqlSafe(format!(
        "WITH RECURSIVE {ANCESTORS}
         INSERT INTO notifications (id, user_id, kind, page_id, actor_id, data)
         SELECT gen_random_uuid(), x.user_id, 'page_created', $1, $2, '{{}}'::jsonb
         FROM (SELECT sw.user_id FROM space_watches sw JOIN pages p ON p.space_id = sw.space_id WHERE p.id = $1
               UNION {SUBTREE_WATCHERS}) x
         JOIN users u ON u.id = x.user_id
         WHERE u.status = 'active' AND x.user_id <> $2"
    )))
    .bind(page_id)
    .bind(creator)
    .execute(&state.db)
    .await?;
    Ok(())
}
