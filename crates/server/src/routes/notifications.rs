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
         WHERE n.user_id = $1 AND (p.id IS NULL OR p.deleted_at IS NULL) ORDER BY n.created_at DESC LIMIT 50",
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

pub async fn watch(State(state): State<AppState>, CurrentUser(user): CurrentUser, Path(id): Path<Uuid>) -> ApiResult<StatusCode> {
    let p = Principal::load(&state, Some(user.clone())).await?;
    require_page(&state, &p, id, Need::View).await?;
    sqlx::query("INSERT INTO watches (user_id, page_id) VALUES ($1, $2) ON CONFLICT DO NOTHING").bind(user.id).bind(id).execute(&state.db).await?;
    Ok(StatusCode::NO_CONTENT)
}

pub async fn unwatch(State(state): State<AppState>, CurrentUser(user): CurrentUser, Path(id): Path<Uuid>) -> ApiResult<StatusCode> {
    sqlx::query("DELETE FROM watches WHERE user_id = $1 AND page_id = $2").bind(user.id).bind(id).execute(&state.db).await?;
    Ok(StatusCode::NO_CONTENT)
}

/// Notifies users newly @mentioned in a page body (each user once per page) and adds them as
/// watchers. Users who can't view the page are skipped.
pub async fn notify_page_mentions(state: &AppState, page_id: Uuid, doc: &Node, actor: Option<Uuid>) -> anyhow::Result<()> {
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
    sqlx::query(
        "INSERT INTO notifications (id, user_id, kind, page_id, actor_id, data)
         SELECT gen_random_uuid(), w.user_id, 'page_updated', $1, $2, $3 FROM watches w JOIN users u ON u.id = w.user_id
         WHERE w.page_id = $1 AND u.status = 'active' AND w.user_id IS DISTINCT FROM $2",
    )
    .bind(page_id)
    .bind(editor)
    .bind(json!({ "version": version }))
    .execute(&state.db)
    .await?;
    Ok(())
}
