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

/// `GET /api/admin/system`: version, database, job queue, search index, mail, storage and
/// cluster state, for the admin overview.
pub async fn system(State(state): State<AppState>, CurrentUser(user): CurrentUser) -> ApiResult<Json<Value>> {
    require_admin(&user)?;
    let (pg_version, db_size): (String, i64) =
        sqlx::query_as("SELECT current_setting('server_version'), pg_database_size(current_database())").fetch_one(&state.db).await?;
    let migration: Option<i64> = sqlx::query_scalar("SELECT max(version) FROM _sqlx_migrations WHERE success").fetch_one(&state.db).await.ok().flatten();
    let (queued, running, failed_day): (i64, i64, i64) = sqlx::query_as(
        "SELECT count(*) FILTER (WHERE status = 'queued'), count(*) FILTER (WHERE status = 'running'),
                count(*) FILTER (WHERE status = 'failed' AND finished_at > now() - interval '1 day')
         FROM jobs WHERE workspace_id = $1",
    )
    .bind(user.workspace_id)
    .fetch_one(&state.db)
    .await?;
    let pages: i64 = sqlx::query_scalar("SELECT count(*) FROM pages WHERE deleted_at IS NULL").fetch_one(&state.db).await?;
    let indexed = state.search.indexed_documents(&state.db).await.ok();
    Ok(Json(json!({
        "version": env!("CARGO_PKG_VERSION"),
        "node": state.node_id,
        "cluster": state.relay.is_some(),
        "database": {
            "version": pg_version,
            "sizeBytes": db_size,
            "schema": migration,
            "connections": state.db.size(),
            "idleConnections": state.db.num_idle(),
        },
        "jobs": { "queued": queued, "running": running, "failedLastDay": failed_day },
        "search": { "backend": state.search.backend_name(), "indexed": indexed, "pages": pages },
        "mail": { "enabled": state.mailer.enabled() },
        "storage": { "backend": state.blobs.backend_name() },
        "liveRooms": state.rooms.all().len(),
        "signIn": state.providers.iter().map(|p| json!({"id": p.id(), "kind": p.kind(), "name": p.display_name()})).collect::<Vec<_>>(),
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

#[derive(Serialize, sqlx::FromRow)]
#[serde(rename_all = "camelCase")]
pub struct SpaceOverview {
    id: Uuid,
    key: String,
    name: String,
    icon: Option<String>,
    default_role: String,
    anonymous_access: bool,
    archived_at: Option<chrono::DateTime<chrono::Utc>>,
    pages: i64,
    attachments: i64,
    storage_bytes: i64,
    last_updated: Option<chrono::DateTime<chrono::Utc>>,
    personal: bool,
    owner_id: Option<Uuid>,
    owner_name: Option<String>,
    /// the owner's account is active (otherwise the space needs a new owner)
    owner_active: bool,
}

/// `GET /api/admin/spaces` — every space, archived ones included, with size and activity.
pub async fn spaces(State(state): State<AppState>, CurrentUser(user): CurrentUser) -> ApiResult<Json<Vec<SpaceOverview>>> {
    require_admin(&user)?;
    Ok(Json(
        sqlx::query_as::<_, SpaceOverview>(
            "SELECT s.id, s.key, s.name, s.icon, s.default_role, s.anonymous_access, s.archived_at,
                    (SELECT count(*) FROM pages p WHERE p.space_id = s.id AND p.deleted_at IS NULL) AS pages,
                    (SELECT count(*) FROM attachments a JOIN pages p ON p.id = a.page_id WHERE p.space_id = s.id AND a.deleted_at IS NULL) AS attachments,
                    (SELECT COALESCE(sum(av.size), 0)::bigint FROM attachment_versions av JOIN attachments a ON a.id = av.attachment_id JOIN pages p ON p.id = a.page_id
                      WHERE p.space_id = s.id AND a.deleted_at IS NULL) AS storage_bytes,
                    (SELECT max(p.updated_at) FROM pages p WHERE p.space_id = s.id) AS last_updated,
                    s.personal_owner_id IS NOT NULL AS personal,
                    s.owner_id, o.name AS owner_name, COALESCE(o.status = 'active', false) AS owner_active
             FROM spaces s LEFT JOIN users o ON o.id = s.owner_id WHERE s.workspace_id = $1 ORDER BY s.archived_at IS NOT NULL, lower(s.name)",
        )
        .bind(user.workspace_id)
        .fetch_all(&state.db)
        .await?,
    ))
}

async fn space_by_key(state: &AppState, ws: Uuid, key: &str) -> ApiResult<(Uuid, Option<chrono::DateTime<chrono::Utc>>)> {
    sqlx::query_as("SELECT id, archived_at FROM spaces WHERE workspace_id = $1 AND upper(key) = upper($2)")
        .bind(ws)
        .bind(key)
        .fetch_optional(&state.db)
        .await?
        .ok_or(ApiError::NotFound("space"))
}

/// `POST /api/admin/spaces/{key}/unarchive`
pub async fn unarchive(
    State(state): State<AppState>,
    CurrentUser(user): CurrentUser,
    axum::extract::Path(key): axum::extract::Path<String>,
) -> ApiResult<axum::http::StatusCode> {
    require_admin(&user)?;
    let (id, _) = space_by_key(&state, user.workspace_id, &key).await?;
    sqlx::query("UPDATE spaces SET archived_at = NULL WHERE id = $1").bind(id).execute(&state.db).await?;
    crate::audit::log(&state, user.workspace_id, Some(user.id), "space.unarchive", Some("space"), Some(id), json!({"key": key})).await;
    Ok(axum::http::StatusCode::NO_CONTENT)
}

#[derive(Deserialize)]
pub struct ConfirmDelete {
    confirm: String,
}

/// `DELETE /api/admin/spaces/{key} {confirm: <key>}` — permanently deletes an archived space
/// with all its pages, history, comments and attachments.
pub async fn delete_space(
    State(state): State<AppState>,
    CurrentUser(user): CurrentUser,
    axum::extract::Path(key): axum::extract::Path<String>,
    Json(req): Json<ConfirmDelete>,
) -> ApiResult<axum::http::StatusCode> {
    require_admin(&user)?;
    let (id, archived) = space_by_key(&state, user.workspace_id, &key).await?;
    if archived.is_none() {
        return Err(ApiError::bad("archive the space first; only archived spaces can be deleted"));
    }
    if !req.confirm.eq_ignore_ascii_case(&key) {
        return Err(ApiError::bad("type the space key to confirm"));
    }
    let pages: Vec<Uuid> = sqlx::query_scalar("SELECT id FROM pages WHERE space_id = $1").bind(id).fetch_all(&state.db).await?;
    for p in &pages {
        state.rooms.evict(*p);
    }
    sqlx::query("DELETE FROM spaces WHERE id = $1").bind(id).execute(&state.db).await?;
    for p in pages {
        state.search.enqueue(p);
    }
    crate::audit::log(&state, user.workspace_id, Some(user.id), "space.delete", Some("space"), Some(id), json!({"key": key})).await;
    Ok(axum::http::StatusCode::NO_CONTENT)
}
