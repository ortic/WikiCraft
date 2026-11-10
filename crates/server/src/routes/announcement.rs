//! Workspace announcement: a banner admins show to everyone (maintenance windows, migrations…).
//! Stored in `workspaces.settings.announcement`.

use axum::Json;
use axum::extract::State;
use serde::Deserialize;
use serde_json::{Value, json};

use crate::app::AppState;
use crate::audit;
use crate::auth::{CurrentUser, require_admin};
use crate::error::{ApiError, ApiResult};

const LEVELS: [&str; 3] = ["info", "warning", "success"];

/// `GET /api/announcement` → `{text, level, id}` or `null`.
pub async fn get(State(state): State<AppState>, CurrentUser(user): CurrentUser) -> ApiResult<Json<Value>> {
    let a: Option<Value> = sqlx::query_scalar("SELECT settings->'announcement' FROM workspaces WHERE id = $1")
        .bind(user.workspace_id)
        .fetch_optional(&state.db)
        .await?
        .flatten();
    Ok(Json(a.unwrap_or(Value::Null)))
}

#[derive(Deserialize)]
pub struct Announcement {
    text: String,
    #[serde(default)]
    level: Option<String>,
}

/// `PUT /api/admin/announcement` `{text, level}` (admins); an empty text removes the banner.
pub async fn put(State(state): State<AppState>, CurrentUser(admin): CurrentUser, Json(req): Json<Announcement>) -> ApiResult<Json<Value>> {
    require_admin(&admin)?;
    let text = req.text.split_whitespace().collect::<Vec<_>>().join(" ");
    if text.chars().count() > 500 {
        return Err(ApiError::bad("the announcement is limited to 500 characters"));
    }
    let level = req.level.as_deref().unwrap_or("info");
    if !LEVELS.contains(&level) {
        return Err(ApiError::bad("level must be info, warning or success"));
    }
    let value = if text.is_empty() {
        Value::Null
    } else {
        // a new id per change, so people who dismissed the previous one see it again
        json!({ "text": text, "level": level, "id": chrono::Utc::now().timestamp_millis().to_string() })
    };
    if value.is_null() {
        sqlx::query("UPDATE workspaces SET settings = settings - 'announcement' WHERE id = $1").bind(admin.workspace_id).execute(&state.db).await?;
    } else {
        sqlx::query("UPDATE workspaces SET settings = jsonb_set(settings, '{announcement}', $2) WHERE id = $1")
            .bind(admin.workspace_id)
            .bind(&value)
            .execute(&state.db)
            .await?;
    }
    audit::log(&state, admin.workspace_id, Some(admin.id), "workspace.announcement", None, None, json!({ "text": text, "level": level })).await;
    Ok(Json(value))
}
