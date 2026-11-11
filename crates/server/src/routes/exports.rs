//! Space export (HTML site, Markdown, backup) and backup restore — both run as jobs.

use axum::Json;
use axum::body::Body;
use axum::extract::{Multipart, Path, State};
use axum::http::{StatusCode, header};
use axum::response::{IntoResponse, Response};
use serde::Deserialize;
use serde_json::{Value, json};
use uuid::Uuid;

use crate::app::{AppState, new_id};
use crate::auth::{CurrentUser, require_admin};
use crate::authz::{Perm, Principal, Role};
use crate::error::{ApiError, ApiResult};
use crate::export::Format;
use crate::routes::spaces::load_space_with_access;

#[derive(Deserialize)]
pub struct ExportRequest {
    format: Format,
}

/// `POST /api/spaces/{key}/export` `{"format": "html" | "markdown" | "backup"}` → `{jobId}`.
pub async fn start(
    State(state): State<AppState>,
    CurrentUser(user): CurrentUser,
    Path(key): Path<String>,
    Json(req): Json<ExportRequest>,
) -> ApiResult<(StatusCode, Json<Value>)> {
    let p = Principal::load(&state, Some(user.clone())).await?;
    let (space, a) = load_space_with_access(&state, &p, &key).await?;
    if (req.format == Format::Backup && a.role < Role::Admin) || !a.perms.has(Perm::Export) {
        return Err(ApiError::Forbidden);
    }
    crate::routes::share::require_exports(&state, user.workspace_id, Some(&user)).await?;
    let job = new_id();
    sqlx::query("INSERT INTO jobs (id, workspace_id, kind, created_by, progress, input) VALUES ($1, $2, 'export.space', $3, $4, $5)")
        .bind(job)
        .bind(user.workspace_id)
        .bind(user.id)
        .bind(json!({"phase": "queued"}))
        .bind(json!({"spaceId": space.id, "format": req.format}))
        .execute(&state.db)
        .await?;
    crate::audit::log(&state, user.workspace_id, Some(user.id), "space.exported", Some("space"), Some(space.id), json!({"format": req.format})).await;
    Ok((StatusCode::ACCEPTED, Json(json!({ "jobId": job }))))
}

/// `GET /api/jobs/{id}/download` — the export archive (only for the user who requested it).
pub async fn download(State(state): State<AppState>, CurrentUser(user): CurrentUser, Path(id): Path<Uuid>) -> ApiResult<Response> {
    // exports started before an admin turned them off can't be fetched afterwards
    crate::routes::share::require_exports(&state, user.workspace_id, Some(&user)).await?;
    let result: Option<Value> = sqlx::query_scalar(
        "SELECT result FROM jobs WHERE id = $1 AND workspace_id = $2 AND created_by = $3 AND kind = 'export.space' AND status = 'succeeded'",
    )
    .bind(id)
    .bind(user.workspace_id)
    .bind(user.id)
    .fetch_optional(&state.db)
    .await?
    .flatten();
    let result = result.ok_or(ApiError::NotFound("export"))?;
    let file = result.get("file").and_then(Value::as_str).ok_or_else(|| ApiError::bad("this export has expired; start a new one"))?;
    let name: String = result
        .get("filename")
        .and_then(Value::as_str)
        .unwrap_or("export.zip")
        .chars()
        .filter(|c| c.is_ascii_alphanumeric() || matches!(c, '-' | '_' | '.'))
        .collect();
    let stream = state.blobs.stream_key(file).await.map_err(ApiError::Internal)?;
    let mut headers = vec![(header::CONTENT_TYPE, "application/zip".to_string()), (header::CONTENT_DISPOSITION, format!("attachment; filename=\"{name}\""))];
    if let Some(size) = result.get("size").and_then(Value::as_u64) {
        headers.push((header::CONTENT_LENGTH, size.to_string()));
    }
    let mut resp = Body::from_stream(stream).into_response();
    for (k, v) in headers {
        resp.headers_mut().insert(k, v.parse().map_err(|_| ApiError::Internal(anyhow::anyhow!("bad header")))?);
    }
    Ok(resp)
}

/// `POST /api/imports/backup` (multipart: `file`, optional `spaceKey`, `spaceName`) — restores a
/// space backup as a new space (workspace admins).
pub async fn restore(State(state): State<AppState>, CurrentUser(user): CurrentUser, mut mp: Multipart) -> ApiResult<(StatusCode, Json<Value>)> {
    require_admin(&user)?;
    let (path, fields, _) = crate::routes::imports::receive_upload(&state, &mut mp).await?;
    let p = path.clone();
    let inspected = tokio::task::spawn_blocking(move || crate::export::inspect_backup(&p)).await.map_err(|e| ApiError::Internal(e.into()));
    let (key, name, pages) = match inspected {
        Ok(Ok(v)) => v,
        Ok(Err(e)) => {
            let _ = tokio::fs::remove_file(&path).await;
            return Err(ApiError::bad(format!("{e:#}")));
        }
        Err(e) => {
            let _ = tokio::fs::remove_file(&path).await;
            return Err(e);
        }
    };
    let space_key = fields.get("spaceKey").filter(|k| !k.trim().is_empty()).cloned().unwrap_or(key);
    let space_key = crate::routes::spaces::validate_key(&space_key)?;
    let taken: bool = sqlx::query_scalar("SELECT EXISTS (SELECT 1 FROM spaces WHERE workspace_id = $1 AND lower(key) = lower($2))")
        .bind(user.workspace_id)
        .bind(&space_key)
        .fetch_one(&state.db)
        .await?;
    if taken {
        let _ = tokio::fs::remove_file(&path).await;
        return Err(ApiError::Conflict(format!("a space with key {space_key} already exists; choose another key")));
    }
    let job = new_id();
    let blob = format!("uploads/{job}.zip");
    let stored = state.blobs.put_file(&blob, &path).await;
    let _ = tokio::fs::remove_file(&path).await;
    stored.map_err(ApiError::Internal)?;
    sqlx::query("INSERT INTO jobs (id, workspace_id, kind, created_by, progress, input) VALUES ($1, $2, 'import.backup', $3, $4, $5)")
        .bind(job)
        .bind(user.workspace_id)
        .bind(user.id)
        .bind(json!({"phase": "queued"}))
        .bind(json!({"upload": blob, "options": {"spaceKey": space_key, "spaceName": fields.get("spaceName")}}))
        .execute(&state.db)
        .await?;
    Ok((StatusCode::ACCEPTED, Json(json!({ "jobId": job, "spaceKey": space_key, "spaceName": name, "pages": pages }))))
}
