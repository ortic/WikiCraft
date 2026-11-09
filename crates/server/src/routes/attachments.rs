use axum::Json;
use axum::body::Body;
use axum::extract::{Multipart, Path, State};
use axum::http::{StatusCode, header};
use axum::response::{IntoResponse, Response};
use bytes::Bytes;
use serde::Serialize;
use uuid::Uuid;

use crate::app::{AppState, new_id};
use crate::auth::{CurrentUser, MaybeUser};
use crate::authz::{Need, Principal, require_page};
use crate::error::{ApiError, ApiResult};

#[derive(Serialize, sqlx::FromRow)]
#[serde(rename_all = "camelCase")]
pub struct AttachmentRow {
    pub id: Uuid,
    pub page_id: Uuid,
    pub filename: String,
    pub media_type: String,
    pub size: i64,
    pub version: i32,
    pub created_by_name: Option<String>,
    pub created_at: chrono::DateTime<chrono::Utc>,
    pub updated_at: chrono::DateTime<chrono::Utc>,
}

const SELECT: &str = "SELECT a.id, a.page_id, a.filename, a.media_type, a.size, a.version, u.name AS created_by_name, a.created_at, a.updated_at FROM attachments a LEFT JOIN users u ON u.id = a.created_by";

pub async fn list(State(state): State<AppState>, MaybeUser(user): MaybeUser, Path(page_id): Path<Uuid>) -> ApiResult<Json<Vec<AttachmentRow>>> {
    let p = Principal::load(&state, user).await?;
    require_page(&state, &p, page_id, Need::View).await?;
    Ok(Json(
        sqlx::query_as::<_, AttachmentRow>(sqlx::AssertSqlSafe(format!("{SELECT} WHERE a.page_id = $1 AND a.deleted_at IS NULL ORDER BY lower(a.filename)")))
            .bind(page_id)
            .fetch_all(&state.db)
            .await?,
    ))
}

/// Sanitizes a client-supplied file name (no paths, control chars or leading dots).
pub fn clean_filename(name: &str) -> String {
    let base = name.rsplit(['/', '\\']).next().unwrap_or(name);
    let s: String = base.chars().filter(|c| !c.is_control() && !matches!(c, '"' | '<' | '>' | '|' | '*' | '?' | ':')).collect();
    let s = s.trim().trim_start_matches('.').to_string();
    if s.is_empty() { "file".into() } else { s.chars().take(200).collect() }
}

pub fn guess_media_type(filename: &str, data: &[u8]) -> String {
    // sniff a few common types, fall back to the extension
    let sniff = if data.starts_with(b"\x89PNG") {
        Some("image/png")
    } else if data.starts_with(b"\xFF\xD8\xFF") {
        Some("image/jpeg")
    } else if data.starts_with(b"GIF8") {
        Some("image/gif")
    } else if data.starts_with(b"%PDF") {
        Some("application/pdf")
    } else if data.len() > 12 && &data[0..4] == b"RIFF" && &data[8..12] == b"WEBP" {
        Some("image/webp")
    } else {
        None
    };
    sniff.map(String::from).unwrap_or_else(|| mime_guess::from_path(filename).first_or_octet_stream().essence_str().to_string())
}

/// Stores a file as a new attachment, or as a new version if the page already has one with that name.
pub async fn store_attachment(
    state: &AppState,
    workspace: Uuid,
    page_id: Uuid,
    user: Option<Uuid>,
    filename: &str,
    data: Bytes,
    created_at: Option<chrono::DateTime<chrono::Utc>>,
    source_ref: Option<&str>,
) -> ApiResult<Uuid> {
    let filename = clean_filename(filename);
    let media_type = guess_media_type(&filename, &data);
    let size = data.len() as i64;
    let mut tx = state.db.begin().await?;
    let key = state.blobs.put(&mut *tx, data).await?;
    let existing: Option<(Uuid, i32)> =
        sqlx::query_as("SELECT id, version FROM attachments WHERE page_id = $1 AND filename = $2 AND deleted_at IS NULL FOR UPDATE")
            .bind(page_id)
            .bind(&filename)
            .fetch_optional(&mut *tx)
            .await?;
    let (id, version) = match existing {
        Some((id, v)) => {
            sqlx::query("UPDATE attachments SET blob_key = $2, size = $3, media_type = $4, version = $5, text_content = NULL, updated_at = COALESCE($6, now()) WHERE id = $1")
                .bind(id)
                .bind(&key)
                .bind(size)
                .bind(&media_type)
                .bind(v + 1)
                .bind(created_at)
                .execute(&mut *tx)
                .await?;
            (id, v + 1)
        }
        None => {
            let id = new_id();
            sqlx::query("INSERT INTO attachments (id, workspace_id, page_id, filename, media_type, size, blob_key, created_by, created_at, updated_at, source_ref) VALUES ($1, $2, $3, $4, $5, $6, $7, $8, COALESCE($9, now()), COALESCE($9, now()), $10)")
                .bind(id)
                .bind(workspace)
                .bind(page_id)
                .bind(&filename)
                .bind(&media_type)
                .bind(size)
                .bind(&key)
                .bind(user)
                .bind(created_at)
                .bind(source_ref)
                .execute(&mut *tx)
                .await?;
            (id, 1)
        }
    };
    sqlx::query("INSERT INTO attachment_versions (id, attachment_id, version, blob_key, size, media_type, created_by, created_at) VALUES ($1, $2, $3, $4, $5, $6, $7, COALESCE($8, now()))")
        .bind(new_id())
        .bind(id)
        .bind(version)
        .bind(&key)
        .bind(size)
        .bind(&media_type)
        .bind(user)
        .bind(created_at)
        .execute(&mut *tx)
        .await?;
    tx.commit().await?;
    state.search.enqueue(page_id);
    crate::extract::spawn(state, id);
    Ok(id)
}

pub async fn upload(
    State(state): State<AppState>,
    CurrentUser(user): CurrentUser,
    Path(page_id): Path<Uuid>,
    mut mp: Multipart,
) -> ApiResult<(StatusCode, Json<Vec<AttachmentRow>>)> {
    let p = Principal::load(&state, Some(user.clone())).await?;
    require_page(&state, &p, page_id, Need::Edit).await?;
    let mut ids = Vec::new();
    while let Some(field) = mp.next_field().await.map_err(|e| ApiError::bad(format!("multipart: {e}")))? {
        let Some(name) = field.file_name().map(String::from) else { continue };
        let data = field.bytes().await.map_err(|e| ApiError::bad(format!("upload failed: {e}")))?;
        ids.push(store_attachment(&state, user.workspace_id, page_id, Some(user.id), &name, data, None, None).await?);
    }
    if ids.is_empty() {
        return Err(ApiError::bad("no file in request"));
    }
    let rows = sqlx::query_as::<_, AttachmentRow>(sqlx::AssertSqlSafe(format!("{SELECT} WHERE a.id = ANY($1)"))).bind(&ids).fetch_all(&state.db).await?;
    crate::webhooks::emit_page(
        &state,
        "attachment.created",
        page_id,
        Some(user.id),
        serde_json::json!({"attachments": serde_json::to_value(&rows).unwrap_or_default()}),
    )
    .await;
    Ok((StatusCode::CREATED, Json(rows)))
}

pub async fn download(State(state): State<AppState>, MaybeUser(user): MaybeUser, Path(id): Path<Uuid>) -> ApiResult<Response> {
    let row: Option<(Uuid, String, String, Option<String>, i64)> =
        sqlx::query_as("SELECT page_id, filename, media_type, blob_key, size FROM attachments WHERE id = $1 AND deleted_at IS NULL")
            .bind(id)
            .fetch_optional(&state.db)
            .await?;
    let (page_id, filename, media_type, key, size) = row.ok_or(ApiError::NotFound("attachment"))?;
    let p = Principal::load(&state, user).await?;
    require_page(&state, &p, page_id, Need::View).await?;
    serve(&state, filename, media_type, key, size).await
}

/// Streams an attachment whose access was already checked (also used by public share links).
pub async fn serve(state: &AppState, filename: String, media_type: String, key: Option<String>, size: i64) -> ApiResult<Response> {
    let key = key.ok_or(ApiError::NotFound("attachment content"))?;
    let stream = state.blobs.stream(&key).await.map_err(ApiError::Internal)?;
    // only render a safe allowlist inline; everything else (incl. HTML and SVG) downloads
    let inline =
        matches!(media_type.as_str(), "image/png" | "image/jpeg" | "image/gif" | "image/webp" | "application/pdf" | "video/mp4" | "audio/mpeg" | "text/plain");
    let disposition = format!("{}; filename*=UTF-8''{}", if inline { "inline" } else { "attachment" }, urlencode(&filename));
    Ok((
        [
            (header::CONTENT_TYPE, if inline { media_type } else { "application/octet-stream".into() }),
            (header::CONTENT_DISPOSITION, disposition),
            (header::CONTENT_LENGTH, size.to_string()),
            (header::CACHE_CONTROL, "private, max-age=3600".into()),
            (header::CONTENT_SECURITY_POLICY, "default-src 'none'; sandbox".into()),
        ],
        Body::from_stream(stream),
    )
        .into_response())
}

fn urlencode(s: &str) -> String {
    s.bytes().map(|b| if b.is_ascii_alphanumeric() || b"-_.~".contains(&b) { (b as char).to_string() } else { format!("%{b:02X}") }).collect()
}

pub async fn delete(State(state): State<AppState>, CurrentUser(user): CurrentUser, Path(id): Path<Uuid>) -> ApiResult<StatusCode> {
    let page_id: Uuid = sqlx::query_scalar("SELECT page_id FROM attachments WHERE id = $1 AND deleted_at IS NULL")
        .bind(id)
        .fetch_optional(&state.db)
        .await?
        .ok_or(ApiError::NotFound("attachment"))?;
    let p = Principal::load(&state, Some(user)).await?;
    require_page(&state, &p, page_id, Need::Edit).await?;
    sqlx::query("UPDATE attachments SET deleted_at = now() WHERE id = $1").bind(id).execute(&state.db).await?;
    state.search.enqueue(page_id);
    Ok(StatusCode::NO_CONTENT)
}
