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
#[serde(rename_all = "camelCase")]
pub struct WorkspaceUpdate {
    name: Option<String>,
    /// `#rrggbb`, or "" for the built-in colour
    accent_color: Option<String>,
    /// one of [`DATE_FORMATS`]
    date_format: Option<String>,
    /// IANA name used for people without their own time zone; "" = their browser's
    time_zone: Option<String>,
}

/// How dates are written: the reader's browser locale, or a fixed pattern.
pub const DATE_FORMATS: &[&str] = &["auto", "yyyy-mm-dd", "dd.mm.yyyy", "dd/mm/yyyy", "mm/dd/yyyy", "d mmm yyyy"];

/// `#rrggbb` (lower-cased), or None.
pub fn clean_color(s: &str) -> Option<String> {
    let s = s.trim();
    (s.len() == 7 && s.starts_with('#') && s[1..].chars().all(|c| c.is_ascii_hexdigit())).then(|| s.to_ascii_lowercase())
}

/// `PUT /api/admin/workspace` `{name?, accentColor?, dateFormat?, timeZone?}` — renames the
/// workspace and sets its look and date conventions (admins).
pub async fn rename(State(state): State<AppState>, CurrentUser(admin): CurrentUser, Json(req): Json<WorkspaceUpdate>) -> ApiResult<Json<Value>> {
    require_admin(&admin)?;
    let ws = admin.workspace_id;
    let mut changed = serde_json::Map::new();
    if let Some(name) = &req.name {
        let name = name.split_whitespace().collect::<Vec<_>>().join(" ");
        if name.is_empty() || name.chars().count() > 80 {
            return Err(ApiError::bad("the workspace name needs 1 to 80 characters"));
        }
        sqlx::query("UPDATE workspaces SET name = $2 WHERE id = $1").bind(ws).bind(&name).execute(&state.db).await?;
        audit::log(&state, ws, Some(admin.id), "workspace.renamed", None, None, json!({ "name": name })).await;
    }
    if let Some(c) = &req.accent_color {
        let c = if c.trim().is_empty() { String::new() } else { clean_color(c).ok_or_else(|| ApiError::bad("accentColor must look like #1f6feb"))? };
        changed.insert("accentColor".into(), json!(c));
    }
    if let Some(f) = &req.date_format {
        if !DATE_FORMATS.contains(&f.as_str()) {
            return Err(ApiError::bad(format!("dateFormat must be one of {}", DATE_FORMATS.join(", "))));
        }
        changed.insert("dateFormat".into(), json!(f));
    }
    if let Some(tz) = &req.time_zone {
        let tz = tz.trim();
        if !tz.is_empty() {
            let ok: bool = sqlx::query_scalar("SELECT EXISTS (SELECT 1 FROM pg_timezone_names WHERE name = $1)").bind(tz).fetch_one(&state.db).await?;
            if !ok || tz.len() > 64 {
                return Err(ApiError::bad("timeZone must be an IANA time zone name such as Europe/Zurich"));
            }
        }
        changed.insert("timeZone".into(), json!(tz));
    }
    if !changed.is_empty() {
        sqlx::query("UPDATE workspaces SET settings = settings || $2 WHERE id = $1").bind(ws).bind(Value::Object(changed.clone())).execute(&state.db).await?;
        audit::log(&state, ws, Some(admin.id), "workspace.settings", None, None, Value::Object(changed)).await;
    }
    let mut out = look(&state, ws).await?;
    let name: String = sqlx::query_scalar("SELECT name FROM workspaces WHERE id = $1").bind(ws).fetch_one(&state.db).await?;
    out.insert("name".into(), json!(name));
    Ok(Json(Value::Object(out)))
}

/// Writes a calendar day the way the workspace wants dates written (server-rendered tables and
/// exports; "auto" gives `5 Mar 2026`).
pub fn format_day(format: &str, d: chrono::NaiveDate) -> String {
    let pattern = match format {
        "yyyy-mm-dd" => "%Y-%m-%d",
        "dd.mm.yyyy" => "%d.%m.%Y",
        "dd/mm/yyyy" => "%d/%m/%Y",
        "mm/dd/yyyy" => "%m/%d/%Y",
        _ => "%-d %b %Y",
    };
    d.format(pattern).to_string()
}

/// The workspace's date format (see [`DATE_FORMATS`]).
pub async fn date_format(state: &AppState, ws: Uuid) -> ApiResult<String> {
    let f: Option<String> =
        sqlx::query_scalar("SELECT settings->>'dateFormat' FROM workspaces WHERE id = $1").bind(ws).fetch_optional(&state.db).await?.flatten();
    Ok(f.unwrap_or_else(|| "auto".into()))
}

/// The workspace's look and date conventions as the client applies them (`accentColor`,
/// `dateFormat`, `timeZone`; unset ones are null).
pub async fn look(state: &AppState, ws: Uuid) -> ApiResult<serde_json::Map<String, Value>> {
    let (color, format, tz): (Option<String>, Option<String>, Option<String>) =
        sqlx::query_as("SELECT NULLIF(settings->>'accentColor', ''), settings->>'dateFormat', NULLIF(settings->>'timeZone', '') FROM workspaces WHERE id = $1")
            .bind(ws)
            .fetch_optional(&state.db)
            .await?
            .unwrap_or_default();
    let mut out = serde_json::Map::new();
    out.insert("accentColor".into(), json!(color));
    out.insert("dateFormat".into(), json!(format.unwrap_or_else(|| "auto".into())));
    out.insert("timeZone".into(), json!(tz));
    Ok(out)
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn days_in_each_format() {
        let d = chrono::NaiveDate::from_ymd_opt(2026, 3, 5).unwrap();
        let shown: Vec<String> = DATE_FORMATS.iter().map(|f| format_day(f, d)).collect();
        assert_eq!(shown, ["5 Mar 2026", "2026-03-05", "05.03.2026", "05/03/2026", "03/05/2026", "5 Mar 2026"]);
        assert_eq!(clean_color("#AbCdEf").as_deref(), Some("#abcdef"));
        assert_eq!(clean_color("#abc"), None);
    }
}
