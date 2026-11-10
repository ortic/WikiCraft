//! Archiving pages: an archived page and everything under it is read-only and leaves the page
//! tree and search; it stays readable by link and can be restored.

use axum::Json;
use axum::extract::{Path, State};
use axum::http::StatusCode;
use serde_json::{Value, json};
use uuid::Uuid;

use crate::app::AppState;
use crate::audit;
use crate::auth::CurrentUser;
use crate::authz::{Need, Principal, page_access, require_page};
use crate::error::{ApiError, ApiResult};
use crate::routes::spaces::load_space;

/// `POST /api/pages/{id}/archive` (people who may edit the page)
pub async fn archive(State(state): State<AppState>, CurrentUser(user): CurrentUser, Path(id): Path<Uuid>) -> ApiResult<StatusCode> {
    let p = Principal::load(&state, Some(user.clone())).await?;
    let (access, space) = require_page(&state, &p, id, Need::View).await?;
    if access.archived_page.is_some() {
        return Err(ApiError::Conflict("the page is already archived".into()));
    }
    if !access.edit {
        return Err(ApiError::Forbidden);
    }
    let home: Option<Uuid> = sqlx::query_scalar("SELECT home_page_id FROM spaces WHERE id = $1").bind(space).fetch_one(&state.db).await?;
    if home == Some(id) {
        return Err(ApiError::bad("the space's home page can't be archived; archive the space instead"));
    }
    sqlx::query("UPDATE pages SET archived_at = now(), archived_by = $2 WHERE id = $1").bind(id).bind(user.id).execute(&state.db).await?;
    audit::log(&state, user.workspace_id, Some(user.id), "page.archive", Some("page"), Some(id), json!({})).await;
    Ok(StatusCode::NO_CONTENT)
}

/// `POST /api/pages/{id}/unarchive`
pub async fn unarchive(State(state): State<AppState>, CurrentUser(user): CurrentUser, Path(id): Path<Uuid>) -> ApiResult<StatusCode> {
    let p = Principal::load(&state, Some(user.clone())).await?;
    let (access, _) = require_page(&state, &p, id, Need::View).await?;
    match access.archived_page {
        None => return Err(ApiError::Conflict("the page isn't archived".into())),
        Some(a) if a != id => return Err(ApiError::bad("the page is archived together with a page above it; restore that page")),
        _ => {}
    }
    if !access.admin {
        return Err(ApiError::Forbidden);
    }
    sqlx::query("UPDATE pages SET archived_at = NULL, archived_by = NULL WHERE id = $1").bind(id).execute(&state.db).await?;
    audit::log(&state, user.workspace_id, Some(user.id), "page.unarchive", Some("page"), Some(id), json!({})).await;
    Ok(StatusCode::NO_CONTENT)
}

/// `GET /api/spaces/{key}/archived`: the archived pages of a space the caller can see.
pub async fn list(State(state): State<AppState>, CurrentUser(user): CurrentUser, Path(key): Path<String>) -> ApiResult<Json<Vec<Value>>> {
    let p = Principal::load(&state, Some(user)).await?;
    let (space, _) = load_space(&state, &p, &key).await?;
    #[allow(clippy::type_complexity)]
    let rows: Vec<(Uuid, String, chrono::DateTime<chrono::Utc>, Option<String>, i64)> = sqlx::query_as(
        "SELECT p.id, p.title, p.archived_at, u.name,
                (WITH RECURSIVE d AS (SELECT id FROM pages WHERE parent_id = p.id AND deleted_at IS NULL
                                      UNION ALL SELECT c.id FROM pages c JOIN d ON c.parent_id = d.id WHERE c.deleted_at IS NULL)
                 SELECT count(*) FROM d) AS descendants
         FROM pages p LEFT JOIN users u ON u.id = p.archived_by
         WHERE p.space_id = $1 AND p.archived_at IS NOT NULL AND p.deleted_at IS NULL ORDER BY p.archived_at DESC LIMIT 500",
    )
    .bind(space.id)
    .fetch_all(&state.db)
    .await?;
    let mut out = Vec::new();
    for (id, title, at, by, descendants) in rows {
        let Ok((a, _)) = page_access(&state, &p, id).await else { continue };
        if !a.view {
            continue;
        }
        out.push(json!({ "id": id, "title": title, "archivedAt": at, "archivedBy": by, "descendants": descendants, "canRestore": a.admin && a.archived_page == Some(id) }));
    }
    Ok(Json(out))
}
