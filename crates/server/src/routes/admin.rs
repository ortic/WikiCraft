use axum::Json;
use axum::extract::{Query, State};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use uuid::Uuid;

use crate::app::AppState;
use crate::auth::{CurrentUser, require_admin};
use crate::error::{ApiError, ApiResult};

#[derive(Serialize, sqlx::FromRow)]
#[serde(rename_all = "camelCase")]
pub struct AuditRow {
    id: i64,
    actor_id: Option<Uuid>,
    actor_name: Option<String>,
    action: String,
    target_type: Option<String>,
    target_id: Option<Uuid>,
    data: Value,
    created_at: chrono::DateTime<chrono::Utc>,
}

#[derive(Deserialize)]
pub struct AuditQuery {
    before: Option<i64>,
    action: Option<String>,
}

pub async fn audit(State(state): State<AppState>, CurrentUser(user): CurrentUser, Query(q): Query<AuditQuery>) -> ApiResult<Json<Vec<AuditRow>>> {
    require_admin(&user)?;
    Ok(Json(
        sqlx::query_as::<_, AuditRow>(
            "SELECT a.id, a.actor_id, u.name AS actor_name, a.action, a.target_type, a.target_id, a.data, a.created_at FROM audit_events a LEFT JOIN users u ON u.id = a.actor_id
             WHERE a.workspace_id = $1 AND ($2::bigint IS NULL OR a.id < $2) AND ($3::text IS NULL OR a.action LIKE $3 || '%') ORDER BY a.id DESC LIMIT 200",
        )
        .bind(user.workspace_id)
        .bind(q.before)
        .bind(q.action)
        .fetch_all(&state.db)
        .await?,
    ))
}

pub async fn stats(State(state): State<AppState>, CurrentUser(user): CurrentUser) -> ApiResult<Json<Value>> {
    require_admin(&user)?;
    let ws = user.workspace_id;
    let one = |sql: &'static str| {
        let db = state.db.clone();
        async move { sqlx::query_scalar::<_, i64>(sql).bind(ws).fetch_one(&db).await }
    };
    Ok(Json(json!({
        "users": one("SELECT count(*) FROM users WHERE workspace_id = $1 AND status = 'active'").await?,
        "placeholderUsers": one("SELECT count(*) FROM users WHERE workspace_id = $1 AND status = 'placeholder'").await?,
        "spaces": one("SELECT count(*) FROM spaces WHERE workspace_id = $1 AND archived_at IS NULL").await?,
        "pages": one("SELECT count(*) FROM pages WHERE workspace_id = $1 AND deleted_at IS NULL").await?,
        "versions": one("SELECT count(*) FROM page_versions v JOIN pages p ON p.id = v.page_id WHERE p.workspace_id = $1").await?,
        "comments": one("SELECT count(*) FROM comments c JOIN pages p ON p.id = c.page_id WHERE p.workspace_id = $1 AND c.deleted_at IS NULL").await?,
        "attachments": one("SELECT count(*) FROM attachments WHERE workspace_id = $1 AND deleted_at IS NULL").await?,
        "storageBytes": one("SELECT COALESCE(sum(size), 0)::bigint FROM attachments WHERE workspace_id = $1 AND deleted_at IS NULL").await?,
        "liveRooms": state.rooms.all().len(),
    })))
}

pub async fn reindex(State(state): State<AppState>, CurrentUser(user): CurrentUser) -> ApiResult<Json<Value>> {
    require_admin(&user)?;
    let n = state.search.reindex_all(&state.db).await.map_err(ApiError::Internal)?;
    Ok(Json(json!({ "indexed": n })))
}

pub async fn health(State(state): State<AppState>) -> ApiResult<Json<Value>> {
    let db_ok = sqlx::query_scalar::<_, i32>("SELECT 1").fetch_one(&state.db).await.is_ok();
    Ok(Json(json!({ "status": if db_ok { "ok" } else { "degraded" }, "database": db_ok, "version": env!("CARGO_PKG_VERSION") })))
}

/// Readiness probe: 503 while shutting down or when the database is unreachable.
pub async fn ready(State(state): State<AppState>) -> (axum::http::StatusCode, Json<Value>) {
    let db_ok = sqlx::query_scalar::<_, i32>("SELECT 1").fetch_one(&state.db).await.is_ok();
    let shutting_down = state.shutting_down.load(std::sync::atomic::Ordering::SeqCst);
    let ok = db_ok && !shutting_down;
    (
        if ok { axum::http::StatusCode::OK } else { axum::http::StatusCode::SERVICE_UNAVAILABLE },
        Json(json!({ "ready": ok, "database": db_ok, "shuttingDown": shutting_down })),
    )
}
