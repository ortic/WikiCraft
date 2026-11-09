//! "Request access": someone who opens a page they can't see asks the people who administer it.
//! The answer never reveals whether the page exists or what it is called; the admins get a
//! notification and grant access with the usual restrictions / space permissions.

use axum::Json;
use axum::extract::{Path, State};
use axum::http::StatusCode;
use serde::Deserialize;
use serde_json::json;
use uuid::Uuid;

use crate::app::{AppState, new_id};
use crate::auth::CurrentUser;
use crate::authz::{Principal, page_access};
use crate::error::ApiResult;

#[derive(Deserialize, Default)]
pub struct AccessReq {
    message: Option<String>,
}

/// `POST /api/pages/{id}/access-request` — always 204 for pages the caller can't see.
pub async fn request(
    State(state): State<AppState>,
    CurrentUser(user): CurrentUser,
    Path(id): Path<Uuid>,
    body: Option<Json<AccessReq>>,
) -> ApiResult<StatusCode> {
    let req = body.map(|Json(b)| b).unwrap_or_default();
    let row: Option<(Uuid, Option<Uuid>)> = sqlx::query_as(
        "SELECT p.space_id, s.personal_owner_id FROM pages p JOIN spaces s ON s.id = p.space_id WHERE p.id = $1 AND p.workspace_id = $2 AND p.deleted_at IS NULL",
    )
    .bind(id)
    .bind(user.workspace_id)
    .fetch_optional(&state.db)
    .await?;
    let Some((space, personal_owner)) = row else { return Ok(StatusCode::NO_CONTENT) };
    let p = Principal::load(&state, Some(user.clone())).await?;
    if page_access(&state, &p, id).await.map(|(a, _)| a.view).unwrap_or(false) {
        return Ok(StatusCode::NO_CONTENT);
    }
    // once a day per page and person
    let recent: bool = sqlx::query_scalar(
        "SELECT EXISTS (SELECT 1 FROM notifications WHERE kind = 'access_request' AND page_id = $1 AND actor_id = $2 AND created_at > now() - interval '1 day')",
    )
    .bind(id)
    .bind(user.id)
    .fetch_one(&state.db)
    .await?;
    if recent {
        return Ok(StatusCode::NO_CONTENT);
    }
    // the space's admins (directly, through groups, the personal space owner); workspace
    // admins only when the space has none
    let mut candidates: Vec<Uuid> = sqlx::query_scalar(
        "SELECT DISTINCT u.id FROM users u WHERE u.workspace_id = $1 AND u.status = 'active' AND u.id <> $4 AND (
            u.id IN (SELECT subject_id FROM space_permissions WHERE space_id = $2 AND subject_type = 'user' AND role = 'admin')
            OR u.id IN (SELECT gm.user_id FROM group_members gm JOIN space_permissions sp ON sp.subject_type = 'group' AND sp.subject_id = gm.group_id
                        WHERE sp.space_id = $2 AND sp.role = 'admin')
            OR u.id = $3)
         LIMIT 50",
    )
    .bind(user.workspace_id)
    .bind(space)
    .bind(personal_owner)
    .bind(user.id)
    .fetch_all(&state.db)
    .await?;
    if candidates.is_empty() {
        candidates =
            sqlx::query_scalar("SELECT id FROM users WHERE workspace_id = $1 AND status = 'active' AND role IN ('owner', 'admin') AND id <> $2 LIMIT 50")
                .bind(user.workspace_id)
                .bind(user.id)
                .fetch_all(&state.db)
                .await?;
    }
    let message = req.message.map(|m| m.trim().chars().take(500).collect::<String>()).filter(|m| !m.is_empty());
    let mut sent = 0;
    for uid in candidates {
        let admin = sqlx::query_as::<_, crate::auth::User>(sqlx::AssertSqlSafe(format!("SELECT {} FROM users WHERE id = $1", crate::auth::USER_COLUMNS)))
            .bind(uid)
            .fetch_optional(&state.db)
            .await?;
        let Some(admin) = admin else { continue };
        let ap = Principal::load(&state, Some(admin)).await?;
        if !page_access(&state, &ap, id).await.map(|(a, _)| a.admin).unwrap_or(false) {
            continue;
        }
        sqlx::query("INSERT INTO notifications (id, user_id, kind, page_id, actor_id, data) VALUES ($1, $2, 'access_request', $3, $4, $5)")
            .bind(new_id())
            .bind(uid)
            .bind(id)
            .bind(user.id)
            .bind(json!({ "excerpt": message, "requesterId": user.id }))
            .execute(&state.db)
            .await?;
        sent += 1;
        if sent >= 10 {
            break;
        }
    }
    Ok(StatusCode::NO_CONTENT)
}
