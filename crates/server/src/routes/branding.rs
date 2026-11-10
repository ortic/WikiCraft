//! Workspace branding: the workspace name and a logo shown in the header, on sign-in pages and
//! on shared pages. The logo lives in blob storage; `workspaces.settings.logo` points to it.

use axum::Json;
use axum::extract::{Multipart, State};
use axum::http::{StatusCode, header};
use axum::response::{IntoResponse, Response};
use serde::Deserialize;
use serde_json::{Value, json};
use uuid::Uuid;

use crate::app::AppState;
use crate::audit;
use crate::auth::{CurrentUser, MaybeUser, require_admin};
use crate::error::{ApiError, ApiResult};

const MAX_LOGO: usize = 1024 * 1024;

/// PNG, JPEG or WebP by their magic bytes (no SVG: it can carry scripts).
fn logo_type(data: &[u8]) -> Option<&'static str> {
    if data.starts_with(b"\x89PNG\r\n\x1a\n") {
        Some("image/png")
    } else if data.starts_with(b"\xFF\xD8\xFF") {
        Some("image/jpeg")
    } else if data.len() > 12 && &data[0..4] == b"RIFF" && &data[8..12] == b"WEBP" {
        Some("image/webp")
    } else {
        None
    }
}

#[derive(Deserialize)]
pub struct WorkspaceUpdate {
    name: String,
}

/// `PUT /api/admin/workspace` `{name}` — renames the workspace (admins).
pub async fn rename(State(state): State<AppState>, CurrentUser(admin): CurrentUser, Json(req): Json<WorkspaceUpdate>) -> ApiResult<Json<Value>> {
    require_admin(&admin)?;
    let name = req.name.split_whitespace().collect::<Vec<_>>().join(" ");
    if name.is_empty() || name.chars().count() > 80 {
        return Err(ApiError::bad("the workspace name needs 1 to 80 characters"));
    }
    sqlx::query("UPDATE workspaces SET name = $2 WHERE id = $1").bind(admin.workspace_id).bind(&name).execute(&state.db).await?;
    audit::log(&state, admin.workspace_id, Some(admin.id), "workspace.renamed", None, None, json!({ "name": name })).await;
    Ok(Json(json!({ "name": name })))
}

/// `POST /api/admin/workspace/logo` (multipart `file`): PNG, JPEG or WebP up to 1 MB.
pub async fn upload_logo(State(state): State<AppState>, CurrentUser(admin): CurrentUser, mut mp: Multipart) -> ApiResult<Json<Value>> {
    require_admin(&admin)?;
    let mut data = None;
    while let Some(field) = mp.next_field().await.map_err(|e| ApiError::bad(format!("multipart: {e}")))? {
        if field.file_name().is_some() {
            data = Some(field.bytes().await.map_err(|_| ApiError::bad("the logo is limited to 1 MB"))?);
            break;
        }
    }
    let data = data.ok_or_else(|| ApiError::bad("no file in request"))?;
    if data.len() > MAX_LOGO {
        return Err(ApiError::bad("the logo is limited to 1 MB"));
    }
    let Some(kind) = logo_type(&data) else {
        return Err(ApiError::bad("use a PNG, JPEG or WebP image"));
    };
    let key = state.blobs.put(&state.db, data).await.map_err(ApiError::Internal)?;
    let logo = json!({ "key": key, "type": kind, "v": chrono::Utc::now().timestamp_millis().to_string() });
    sqlx::query("UPDATE workspaces SET settings = jsonb_set(settings, '{logo}', $2) WHERE id = $1")
        .bind(admin.workspace_id)
        .bind(&logo)
        .execute(&state.db)
        .await?;
    audit::log(&state, admin.workspace_id, Some(admin.id), "workspace.logo", None, None, json!({ "type": kind })).await;
    Ok(Json(json!({ "logoVersion": logo["v"] })))
}

/// `DELETE /api/admin/workspace/logo` — back to the WikiCraft mark.
pub async fn delete_logo(State(state): State<AppState>, CurrentUser(admin): CurrentUser) -> ApiResult<StatusCode> {
    require_admin(&admin)?;
    sqlx::query("UPDATE workspaces SET settings = settings - 'logo' WHERE id = $1").bind(admin.workspace_id).execute(&state.db).await?;
    audit::log(&state, admin.workspace_id, Some(admin.id), "workspace.logo", None, None, json!({ "removed": true })).await;
    Ok(StatusCode::NO_CONTENT)
}

/// The logo version of a workspace (for cache-busting URLs), if it has one.
pub async fn logo_version(state: &AppState, ws: Uuid) -> ApiResult<Option<String>> {
    Ok(sqlx::query_scalar("SELECT settings->'logo'->>'v' FROM workspaces WHERE id = $1").bind(ws).fetch_optional(&state.db).await?.flatten())
}

/// `GET /api/workspace/logo` — the logo image (public: sign-in and shared pages show it too).
pub async fn logo(State(state): State<AppState>, MaybeUser(user): MaybeUser) -> ApiResult<Response> {
    let row: Option<(Option<String>, Option<String>)> = match &user {
        Some(u) => sqlx::query_as("SELECT settings->'logo'->>'key', settings->'logo'->>'type' FROM workspaces WHERE id = $1").bind(u.workspace_id),
        None => sqlx::query_as("SELECT settings->'logo'->>'key', settings->'logo'->>'type' FROM workspaces ORDER BY created_at LIMIT 1"),
    }
    .fetch_optional(&state.db)
    .await?;
    let Some((Some(key), Some(kind))) = row else { return Err(ApiError::NotFound("logo")) };
    // only types we accepted on upload are ever served
    let kind = ["image/png", "image/jpeg", "image/webp"].into_iter().find(|k| *k == kind).ok_or(ApiError::NotFound("logo"))?;
    let data = state.blobs.get(&key).await.map_err(ApiError::Internal)?;
    Ok(([(header::CONTENT_TYPE, kind), (header::CACHE_CONTROL, "public, max-age=300"), (header::X_CONTENT_TYPE_OPTIONS, "nosniff")], data).into_response())
}
