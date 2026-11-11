//! Uploads in parts (`/api/uploads`), for files too large for one request: a multi-GB Confluence
//! export goes up in [`PART_SIZE`] pieces, in order; after a network error or a reload the
//! client asks how far it got and carries on. Parts are kept in file storage, so any replica can
//! take the next one. A finished upload is used by id: the dry run and the import read the same
//! upload instead of the file being sent twice. Unfinished or unused uploads go after a day.

use std::path::PathBuf;

use axum::Json;
use axum::body::Bytes;
use axum::extract::{Path, State};
use axum::http::StatusCode;
use futures::StreamExt;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use tokio::io::AsyncWriteExt;
use uuid::Uuid;

use crate::app::{AppState, new_id};
use crate::auth::{CurrentUser, User, require_admin};
use crate::error::{ApiError, ApiResult};
use crate::importer::ImportOptions;

/// Size of every part but the last.
pub const PART_SIZE: usize = 8 << 20;
/// unfinished or unused uploads are removed after this long without activity
const ABANDONED_HOURS: i32 = 24;

#[derive(Serialize, sqlx::FromRow)]
#[serde(rename_all = "camelCase")]
pub struct UploadState {
    id: Uuid,
    filename: String,
    size: i64,
    part_size: i32,
    /// parts received so far: the next one to send is this number
    parts: i32,
    received: i64,
    /// every byte is there
    complete: bool,
}

const STATE_COLUMNS: &str = "id, filename, size, part_size, parts, received, received = size AS complete";

#[derive(Deserialize)]
pub struct NewUpload {
    filename: String,
    size: i64,
}

fn part_key(id: Uuid, n: i32) -> String {
    format!("uploads/{id}/{n:06}")
}

/// `POST /api/uploads {filename, size}` — starts an upload (admins: imports).
pub async fn create(State(state): State<AppState>, CurrentUser(user): CurrentUser, Json(req): Json<NewUpload>) -> ApiResult<(StatusCode, Json<UploadState>)> {
    require_admin(&user)?;
    let max = (state.config.max_upload_mb * 1024 * 1024) as i64;
    if req.size < 0 || req.size > max {
        return Err(ApiError::TooLarge);
    }
    let filename: String = req.filename.trim().chars().take(255).collect();
    let row = sqlx::query_as::<_, UploadState>(sqlx::AssertSqlSafe(format!(
        "INSERT INTO uploads (id, workspace_id, created_by, filename, size, part_size) VALUES ($1, $2, $3, $4, $5, $6) RETURNING {STATE_COLUMNS}"
    )))
    .bind(new_id())
    .bind(user.workspace_id)
    .bind(user.id)
    .bind(if filename.is_empty() { "upload".to_string() } else { filename })
    .bind(req.size)
    .bind(PART_SIZE as i32)
    .fetch_one(&state.db)
    .await?;
    Ok((StatusCode::CREATED, Json(row)))
}

async fn load(state: &AppState, user: &User, id: Uuid) -> ApiResult<UploadState> {
    sqlx::query_as::<_, UploadState>(sqlx::AssertSqlSafe(format!("SELECT {STATE_COLUMNS} FROM uploads WHERE id = $1 AND created_by = $2")))
        .bind(id)
        .bind(user.id)
        .fetch_optional(&state.db)
        .await?
        .ok_or(ApiError::NotFound("upload"))
}

/// `GET /api/uploads/{id}` — how far it got.
pub async fn get(State(state): State<AppState>, CurrentUser(user): CurrentUser, Path(id): Path<Uuid>) -> ApiResult<Json<UploadState>> {
    Ok(Json(load(&state, &user, id).await?))
}

/// `PUT /api/uploads/{id}/parts/{n}` (raw bytes) — part `n` (from 0). Parts come in order; a
/// part sent again (the answer got lost) is acknowledged without storing it twice.
pub async fn put_part(
    State(state): State<AppState>,
    CurrentUser(user): CurrentUser,
    Path((id, n)): Path<(Uuid, i32)>,
    body: Bytes,
) -> ApiResult<Json<UploadState>> {
    let mut tx = state.db.begin().await?;
    // one part at a time per upload
    let up = sqlx::query_as::<_, UploadState>(sqlx::AssertSqlSafe(format!("SELECT {STATE_COLUMNS} FROM uploads WHERE id = $1 AND created_by = $2 FOR UPDATE")))
        .bind(id)
        .bind(user.id)
        .fetch_optional(&mut *tx)
        .await?
        .ok_or(ApiError::NotFound("upload"))?;
    if n < up.parts {
        return Ok(Json(up));
    }
    if n > up.parts {
        return Err(ApiError::Conflict(format!("part {} comes next", up.parts)));
    }
    let len = body.len() as i64;
    let rest = up.size - up.received;
    // every part is full-sized except the last
    if len == 0 || len > rest || (len < up.part_size as i64 && len != rest) {
        return Err(ApiError::bad(format!("part {n} must have {} bytes", rest.min(up.part_size as i64))));
    }
    state.blobs.put_key(&part_key(id, n), body).await.map_err(ApiError::Internal)?;
    let row = sqlx::query_as::<_, UploadState>(sqlx::AssertSqlSafe(format!(
        "UPDATE uploads SET parts = parts + 1, received = received + $2, updated_at = now() WHERE id = $1 RETURNING {STATE_COLUMNS}"
    )))
    .bind(id)
    .bind(len)
    .fetch_one(&mut *tx)
    .await?;
    tx.commit().await?;
    Ok(Json(row))
}

/// `DELETE /api/uploads/{id}`
pub async fn delete(State(state): State<AppState>, CurrentUser(user): CurrentUser, Path(id): Path<Uuid>) -> ApiResult<StatusCode> {
    let up = load(&state, &user, id).await?;
    discard(&state, up.id, up.parts).await;
    Ok(StatusCode::NO_CONTENT)
}

/// Removes an upload and its parts.
async fn discard(state: &AppState, id: Uuid, parts: i32) {
    for n in 0..parts {
        let _ = state.blobs.delete_key(&part_key(id, n)).await;
    }
    let _ = sqlx::query("DELETE FROM uploads WHERE id = $1").bind(id).execute(&state.db).await;
}

/// The finished upload as one local file (the caller removes it).
async fn assemble(state: &AppState, up: &UploadState) -> ApiResult<PathBuf> {
    if !up.complete {
        return Err(ApiError::Conflict(format!("the upload isn't finished: {} of {} bytes", up.received, up.size)));
    }
    let path = state.config.upload_dir().join(format!("{}.upload", new_id()));
    let mut f = tokio::fs::File::create(&path).await.map_err(|e| ApiError::Internal(e.into()))?;
    let mut total = 0i64;
    for n in 0..up.parts {
        let mut stream = state.blobs.stream_key(&part_key(up.id, n)).await.map_err(ApiError::Internal)?;
        while let Some(chunk) = stream.next().await {
            let chunk = chunk.map_err(|e| ApiError::Internal(e.into()))?;
            total += chunk.len() as i64;
            f.write_all(&chunk).await.map_err(|e| ApiError::Internal(e.into()))?;
        }
    }
    f.flush().await.map_err(|e| ApiError::Internal(e.into()))?;
    if total != up.size {
        let _ = tokio::fs::remove_file(&path).await;
        return Err(ApiError::Internal(anyhow::anyhow!("upload {} has {total} of {} bytes in storage", up.id, up.size)));
    }
    Ok(path)
}

/// `POST /api/uploads/{id}/analyze-confluence` — dry run of an uploaded Confluence export (the
/// upload stays for the import).
pub async fn analyze_confluence(State(state): State<AppState>, CurrentUser(user): CurrentUser, Path(id): Path<Uuid>) -> ApiResult<Json<Value>> {
    require_admin(&user)?;
    let up = load(&state, &user, id).await?;
    let path = assemble(&state, &up).await?;
    sqlx::query("UPDATE uploads SET updated_at = now() WHERE id = $1").bind(id).execute(&state.db).await?;
    Ok(Json(crate::routes::imports::analyze_file(&state, &user, path).await?))
}

#[derive(Deserialize, Default)]
pub struct ImportRequest {
    #[serde(default)]
    options: Option<ImportOptions>,
}

/// `POST /api/uploads/{id}/import-confluence {options}` — imports an uploaded Confluence export
/// (the upload is used up).
pub async fn import_confluence(
    State(state): State<AppState>,
    CurrentUser(user): CurrentUser,
    Path(id): Path<Uuid>,
    Json(req): Json<ImportRequest>,
) -> ApiResult<(StatusCode, Json<Value>)> {
    require_admin(&user)?;
    let up = load(&state, &user, id).await?;
    let path = assemble(&state, &up).await?;
    let job = crate::routes::imports::start_confluence_import(&state, &user, path, req.options.unwrap_or_default()).await?;
    discard(&state, up.id, up.parts).await;
    Ok((StatusCode::ACCEPTED, Json(json!({ "jobId": job }))))
}

/// Removes uploads nobody touched for a day (housekeeping); returns how many.
pub async fn remove_abandoned(state: &AppState) -> anyhow::Result<usize> {
    let old: Vec<(Uuid, i32)> = sqlx::query_as("SELECT id, parts FROM uploads WHERE updated_at < now() - make_interval(hours => $1)")
        .bind(ABANDONED_HOURS)
        .fetch_all(&state.db)
        .await?;
    for (id, parts) in &old {
        discard(state, *id, *parts).await;
    }
    Ok(old.len())
}
