//! Admin API for outgoing webhooks (see `crate::webhooks`).

use axum::Json;
use axum::extract::{Path, State};
use axum::http::StatusCode;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use uuid::Uuid;

use crate::app::{AppState, new_id};
use crate::audit;
use crate::auth::{CurrentUser, require_admin};
use crate::error::{ApiError, ApiResult};
use crate::webhooks::{EVENTS, new_secret, validate_url};

#[derive(Serialize, sqlx::FromRow)]
#[serde(rename_all = "camelCase")]
pub struct HookRow {
    id: Uuid,
    name: String,
    url: String,
    events: Vec<String>,
    space_key: Option<String>,
    active: bool,
    failure_count: i32,
    created_at: chrono::DateTime<chrono::Utc>,
    last_delivery_at: Option<chrono::DateTime<chrono::Utc>>,
    last_status: Option<i32>,
    last_error: Option<String>,
}

const HOOK_SELECT: &str = "SELECT w.id, w.name, w.url, w.events, s.key AS space_key, w.active, w.failure_count, w.created_at,
        d.created_at AS last_delivery_at, d.response_status AS last_status, d.last_error
     FROM webhooks w LEFT JOIN spaces s ON s.id = w.space_id
     LEFT JOIN LATERAL (SELECT created_at, response_status, last_error FROM webhook_deliveries WHERE webhook_id = w.id ORDER BY created_at DESC LIMIT 1) d ON true";

/// `GET /api/admin/webhooks`
pub async fn list(State(state): State<AppState>, CurrentUser(user): CurrentUser) -> ApiResult<Json<Value>> {
    require_admin(&user)?;
    let hooks = sqlx::query_as::<_, HookRow>(sqlx::AssertSqlSafe(format!("{HOOK_SELECT} WHERE w.workspace_id = $1 ORDER BY w.created_at")))
        .bind(user.workspace_id)
        .fetch_all(&state.db)
        .await?;
    Ok(Json(json!({ "hooks": hooks, "events": EVENTS.iter().filter(|e| **e != "ping").collect::<Vec<_>>() })))
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct HookInput {
    name: Option<String>,
    url: Option<String>,
    events: Option<Vec<String>>,
    /// space key; empty = all spaces
    space_key: Option<String>,
    active: Option<bool>,
}

async fn space_id(state: &AppState, ws: Uuid, key: Option<&str>) -> ApiResult<Option<Option<Uuid>>> {
    match key {
        None => Ok(None),
        Some(k) if k.trim().is_empty() => Ok(Some(None)),
        Some(k) => {
            let id: Option<Uuid> = sqlx::query_scalar("SELECT id FROM spaces WHERE workspace_id = $1 AND upper(key) = upper($2)")
                .bind(ws)
                .bind(k.trim())
                .fetch_optional(&state.db)
                .await?;
            id.map(|i| Some(Some(i))).ok_or_else(|| ApiError::bad(format!("unknown space {k}")))
        }
    }
}

fn check_events(events: &[String]) -> ApiResult<()> {
    match events.iter().find(|e| !EVENTS.contains(&e.as_str())) {
        Some(e) => Err(ApiError::bad(format!("unknown event {e}; known: {}", EVENTS.join(", ")))),
        None => Ok(()),
    }
}

/// `POST /api/admin/webhooks` — returns the signing secret (only now).
pub async fn create(State(state): State<AppState>, CurrentUser(user): CurrentUser, Json(req): Json<HookInput>) -> ApiResult<(StatusCode, Json<Value>)> {
    require_admin(&user)?;
    let url = validate_url(req.url.as_deref().unwrap_or("")).map_err(|e| ApiError::bad(format!("{e}")))?;
    let events = req.events.unwrap_or_default();
    check_events(&events)?;
    let space = space_id(&state, user.workspace_id, req.space_key.as_deref()).await?.flatten();
    let id = new_id();
    let secret = new_secret();
    sqlx::query("INSERT INTO webhooks (id, workspace_id, name, url, secret, events, space_id, active, created_by) VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9)")
        .bind(id)
        .bind(user.workspace_id)
        .bind(req.name.unwrap_or_default().trim())
        .bind(url.as_str())
        .bind(&secret)
        .bind(&events)
        .bind(space)
        .bind(req.active.unwrap_or(true))
        .bind(user.id)
        .execute(&state.db)
        .await?;
    audit::log(&state, user.workspace_id, Some(user.id), "webhook.create", Some("webhook"), Some(id), json!({"url": url.as_str(), "events": events})).await;
    Ok((StatusCode::CREATED, Json(json!({ "id": id, "secret": secret }))))
}

/// `PATCH /api/admin/webhooks/{id}`
pub async fn update(State(state): State<AppState>, CurrentUser(user): CurrentUser, Path(id): Path<Uuid>, Json(req): Json<HookInput>) -> ApiResult<StatusCode> {
    require_admin(&user)?;
    let url = match &req.url {
        Some(u) => Some(validate_url(u).map_err(|e| ApiError::bad(format!("{e}")))?.to_string()),
        None => None,
    };
    if let Some(e) = &req.events {
        check_events(e)?;
    }
    let space = space_id(&state, user.workspace_id, req.space_key.as_deref()).await?;
    let n = sqlx::query(
        "UPDATE webhooks SET name = COALESCE($3, name), url = COALESCE($4, url), events = COALESCE($5, events), active = COALESCE($6, active),
            space_id = CASE WHEN $7 THEN $8 ELSE space_id END, failure_count = CASE WHEN $6 THEN 0 ELSE failure_count END
         WHERE id = $1 AND workspace_id = $2",
    )
    .bind(id)
    .bind(user.workspace_id)
    .bind(req.name.as_deref().map(str::trim))
    .bind(url)
    .bind(req.events.as_ref())
    .bind(req.active)
    .bind(space.is_some())
    .bind(space.flatten())
    .execute(&state.db)
    .await?
    .rows_affected();
    if n == 0 {
        return Err(ApiError::NotFound("webhook"));
    }
    audit::log(&state, user.workspace_id, Some(user.id), "webhook.update", Some("webhook"), Some(id), json!({})).await;
    Ok(StatusCode::NO_CONTENT)
}

/// `DELETE /api/admin/webhooks/{id}`
pub async fn delete(State(state): State<AppState>, CurrentUser(user): CurrentUser, Path(id): Path<Uuid>) -> ApiResult<StatusCode> {
    require_admin(&user)?;
    let n = sqlx::query("DELETE FROM webhooks WHERE id = $1 AND workspace_id = $2").bind(id).bind(user.workspace_id).execute(&state.db).await?.rows_affected();
    if n == 0 {
        return Err(ApiError::NotFound("webhook"));
    }
    audit::log(&state, user.workspace_id, Some(user.id), "webhook.delete", Some("webhook"), Some(id), json!({})).await;
    Ok(StatusCode::NO_CONTENT)
}

/// `POST /api/admin/webhooks/{id}/test` — queues a `ping` for this hook only.
pub async fn test(State(state): State<AppState>, CurrentUser(user): CurrentUser, Path(id): Path<Uuid>) -> ApiResult<StatusCode> {
    require_admin(&user)?;
    let exists: bool = sqlx::query_scalar("SELECT EXISTS (SELECT 1 FROM webhooks WHERE id = $1 AND workspace_id = $2)")
        .bind(id)
        .bind(user.workspace_id)
        .fetch_one(&state.db)
        .await?;
    if !exists {
        return Err(ApiError::NotFound("webhook"));
    }
    sqlx::query("INSERT INTO webhook_deliveries (id, webhook_id, event, payload) VALUES ($1, $2, 'ping', $3)")
        .bind(new_id())
        .bind(id)
        .bind(json!({"event": "ping", "timestamp": chrono::Utc::now(), "webhookId": id, "data": {"message": "Hello from WikiCraft"}}))
        .execute(&state.db)
        .await?;
    Ok(StatusCode::ACCEPTED)
}

#[derive(Serialize, sqlx::FromRow)]
#[serde(rename_all = "camelCase")]
pub struct DeliveryRow {
    id: Uuid,
    event: String,
    created_at: chrono::DateTime<chrono::Utc>,
    attempts: i32,
    delivered_at: Option<chrono::DateTime<chrono::Utc>>,
    failed_at: Option<chrono::DateTime<chrono::Utc>>,
    next_attempt_at: chrono::DateTime<chrono::Utc>,
    response_status: Option<i32>,
    last_error: Option<String>,
    duration_ms: Option<i32>,
}

/// `GET /api/admin/webhooks/{id}/deliveries` — the latest 50.
pub async fn deliveries(State(state): State<AppState>, CurrentUser(user): CurrentUser, Path(id): Path<Uuid>) -> ApiResult<Json<Vec<DeliveryRow>>> {
    require_admin(&user)?;
    Ok(Json(
        sqlx::query_as::<_, DeliveryRow>(
            "SELECT d.id, d.event, d.created_at, d.attempts, d.delivered_at, d.failed_at, d.next_attempt_at, d.response_status, d.last_error, d.duration_ms
             FROM webhook_deliveries d JOIN webhooks w ON w.id = d.webhook_id WHERE d.webhook_id = $1 AND w.workspace_id = $2 ORDER BY d.created_at DESC LIMIT 50",
        )
        .bind(id)
        .bind(user.workspace_id)
        .fetch_all(&state.db)
        .await?,
    ))
}
