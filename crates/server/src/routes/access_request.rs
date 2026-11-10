//! "Request access": someone who opens a page they can't see asks the people who administer it.
//! The answer never reveals whether the page exists or what it is called; the admins get a
//! notification and grant access in one step (or with the usual restrictions / permissions).

use axum::Json;
use axum::extract::{Path, State};
use axum::http::StatusCode;
use serde::Deserialize;
use serde_json::{Value, json};
use uuid::Uuid;

use crate::app::{AppState, new_id};
use crate::auth::CurrentUser;
use crate::authz::{Need, Principal, Role, page_access, require_page};
use crate::error::{ApiError, ApiResult};

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

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Decision {
    user_id: Uuid,
}

/// Marks everyone's open requests of `requester` for `page` as handled.
async fn close_requests(state: &AppState, page: Uuid, requester: Uuid) -> ApiResult<()> {
    sqlx::query("UPDATE notifications SET read_at = now() WHERE kind = 'access_request' AND page_id = $1 AND actor_id = $2 AND read_at IS NULL")
        .bind(page)
        .bind(requester)
        .execute(&state.db)
        .await?;
    Ok(())
}

/// `POST /api/pages/{id}/access-grant {userId}` (people who may change the page's
/// restrictions): makes the page readable for the person. They become a space viewer if they
/// can't open the space (space admins only), and get a view entry on the page and on every page
/// above it whose view restrictions leave them out. They are notified; the request is closed.
pub async fn grant(State(state): State<AppState>, CurrentUser(user): CurrentUser, Path(id): Path<Uuid>, Json(req): Json<Decision>) -> ApiResult<Json<Value>> {
    let p = Principal::load(&state, Some(user.clone())).await?;
    let (access, space) = require_page(&state, &p, id, Need::Admin).await?;
    let target = sqlx::query_as::<_, crate::auth::User>(sqlx::AssertSqlSafe(format!(
        "SELECT {} FROM users WHERE id = $1 AND workspace_id = $2 AND status = 'active'",
        crate::auth::USER_COLUMNS
    )))
    .bind(req.user_id)
    .bind(user.workspace_id)
    .fetch_optional(&state.db)
    .await?
    .ok_or_else(|| ApiError::bad("unknown person"))?;
    let tp = Principal::load(&state, Some(target.clone())).await?;
    let (before, _) = page_access(&state, &tp, id).await?;
    if before.view {
        close_requests(&state, id, target.id).await?;
        return Ok(Json(json!({ "granted": false, "hadAccess": true })));
    }
    // a draft can't be shared before it's published
    let draft: bool = sqlx::query_scalar(
        "WITH RECURSIVE up AS (SELECT id, parent_id, draft, 0 AS depth FROM pages WHERE id = $1
            UNION ALL SELECT p.id, p.parent_id, p.draft, up.depth + 1 FROM pages p JOIN up ON p.id = up.parent_id WHERE up.depth < 200)
         SELECT bool_or(draft) FROM up",
    )
    .bind(id)
    .fetch_one(&state.db)
    .await?;
    if draft {
        return Err(ApiError::bad("this page is an unpublished draft; publish it first"));
    }
    let mut space_access = false;
    if before.space_role.unwrap_or(Role::None) == Role::None {
        if access.space_role.unwrap_or(Role::None) < Role::Admin {
            return Err(ApiError::Denied(format!("{} can't open this space; a space admin can add them", target.name)));
        }
        sqlx::query("INSERT INTO space_permissions (space_id, subject_type, subject_id, role) VALUES ($1, 'user', $2, 'viewer') ON CONFLICT DO NOTHING")
            .bind(space)
            .bind(target.id)
            .execute(&state.db)
            .await?;
        space_access = true;
    }
    // the page and the restricted pages above it
    let restricted: Vec<Uuid> = sqlx::query_scalar(
        "WITH RECURSIVE up AS (SELECT id, parent_id, 0 AS depth FROM pages WHERE id = $1
            UNION ALL SELECT p.id, p.parent_id, up.depth + 1 FROM pages p JOIN up ON p.id = up.parent_id WHERE up.depth < 200)
         SELECT up.id FROM up WHERE EXISTS (SELECT 1 FROM page_restrictions r WHERE r.page_id = up.id AND r.kind = 'view') ORDER BY up.depth",
    )
    .bind(id)
    .fetch_all(&state.db)
    .await?;
    for page in &restricted {
        sqlx::query("INSERT INTO page_restrictions (page_id, kind, subject_type, subject_id) VALUES ($1, 'view', 'user', $2) ON CONFLICT DO NOTHING")
            .bind(page)
            .bind(target.id)
            .execute(&state.db)
            .await?;
    }
    let (after, _) = page_access(&state, &tp, id).await?;
    if !after.view {
        return Err(ApiError::Conflict("access couldn't be granted this way; check the space permissions and restrictions".into()));
    }
    close_requests(&state, id, target.id).await?;
    sqlx::query("INSERT INTO notifications (id, user_id, kind, page_id, actor_id, data) VALUES ($1, $2, 'access_granted', $3, $4, '{}')")
        .bind(new_id())
        .bind(target.id)
        .bind(id)
        .bind(user.id)
        .execute(&state.db)
        .await?;
    let pages_above = restricted.iter().filter(|p| **p != id).count();
    crate::audit::log(
        &state,
        user.workspace_id,
        Some(user.id),
        "page.access_granted",
        Some("page"),
        Some(id),
        json!({"user": target.id, "spaceAccess": space_access, "pagesAbove": pages_above}),
    )
    .await;
    Ok(Json(json!({ "granted": true, "spaceAccess": space_access, "pagesAbove": pages_above })))
}

/// `POST /api/pages/{id}/access-dismiss {userId}`: closes a request without granting it.
pub async fn dismiss(State(state): State<AppState>, CurrentUser(user): CurrentUser, Path(id): Path<Uuid>, Json(req): Json<Decision>) -> ApiResult<StatusCode> {
    let p = Principal::load(&state, Some(user)).await?;
    require_page(&state, &p, id, Need::Admin).await?;
    close_requests(&state, id, req.user_id).await?;
    Ok(StatusCode::NO_CONTENT)
}
