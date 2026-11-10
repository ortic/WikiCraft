//! Page verification: a "verified" badge with an expiry, set by people who may edit the page,
//! and review requests to them. A page edited after it was verified stays verified (until the
//! expiry) but says so.

use axum::Json;
use axum::extract::{Path, Query, State};
use axum::http::StatusCode;
use serde::Deserialize;
use serde_json::{Value, json};
use uuid::Uuid;

use crate::app::{AppState, new_id};
use crate::audit;
use crate::auth::{CurrentUser, MaybeUser};
use crate::authz::{Need, Principal, page_access, require_page};
use crate::error::{ApiError, ApiResult};
use crate::routes::search::{filter_visible, visible_spaces};
use crate::routes::spaces::workspace_of;

type Row = (Option<Uuid>, Option<String>, chrono::DateTime<chrono::Utc>, Option<chrono::DateTime<chrono::Utc>>, Option<String>, chrono::DateTime<chrono::Utc>);

/// The verification state of a page: `none`, `verified` or `expired`.
pub async fn summary(state: &AppState, page: Uuid) -> ApiResult<Value> {
    let row: Option<Row> = sqlx::query_as(
        "SELECT v.verified_by, u.name, v.verified_at, v.expires_at, v.note, p.updated_at
         FROM page_verifications v JOIN pages p ON p.id = v.page_id LEFT JOIN users u ON u.id = v.verified_by WHERE v.page_id = $1",
    )
    .bind(page)
    .fetch_optional(&state.db)
    .await?;
    let Some((by, by_name, at, expires, note, updated)) = row else { return Ok(json!({ "status": "none" })) };
    let expired = expires.is_some_and(|e| e <= chrono::Utc::now());
    Ok(json!({
        "status": if expired { "expired" } else { "verified" },
        "verifiedBy": by.map(|id| json!({ "id": id, "name": by_name })),
        "verifiedAt": at,
        "expiresAt": expires,
        "note": note,
        // a minute of slack: verifying itself doesn't count as an edit
        "editedSince": updated > at + chrono::Duration::minutes(1),
    }))
}

/// `GET /api/pages/{id}/verification`
pub async fn get(State(state): State<AppState>, MaybeUser(user): MaybeUser, Path(id): Path<Uuid>) -> ApiResult<Json<Value>> {
    let p = Principal::load(&state, user).await?;
    require_page(&state, &p, id, Need::View).await?;
    Ok(Json(summary(&state, id).await?))
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct VerifyReq {
    /// days until the verification expires; none = no expiry
    days: Option<i64>,
    note: Option<String>,
}

/// `PUT /api/pages/{id}/verification` (people who may edit the page)
pub async fn verify(State(state): State<AppState>, CurrentUser(user): CurrentUser, Path(id): Path<Uuid>, Json(req): Json<VerifyReq>) -> ApiResult<Json<Value>> {
    let p = Principal::load(&state, Some(user.clone())).await?;
    require_page(&state, &p, id, Need::Edit).await?;
    if req.days.is_some_and(|d| !(1..=3650).contains(&d)) {
        return Err(ApiError::bad("days must be between 1 and 3650"));
    }
    let note = req.note.map(|n| n.trim().chars().take(500).collect::<String>()).filter(|n| !n.is_empty());
    let expires = req.days.map(|d| chrono::Utc::now() + chrono::Duration::days(d));
    sqlx::query(
        "INSERT INTO page_verifications (page_id, verified_by, verified_at, expires_at, note) VALUES ($1, $2, now(), $3, $4)
         ON CONFLICT (page_id) DO UPDATE SET verified_by = $2, verified_at = now(), expires_at = $3, note = $4",
    )
    .bind(id)
    .bind(user.id)
    .bind(expires)
    .bind(&note)
    .execute(&state.db)
    .await?;
    audit::log(&state, user.workspace_id, Some(user.id), "page.verify", Some("page"), Some(id), json!({ "days": req.days })).await;
    Ok(Json(summary(&state, id).await?))
}

/// `DELETE /api/pages/{id}/verification`
pub async fn unverify(State(state): State<AppState>, CurrentUser(user): CurrentUser, Path(id): Path<Uuid>) -> ApiResult<StatusCode> {
    let p = Principal::load(&state, Some(user.clone())).await?;
    require_page(&state, &p, id, Need::Edit).await?;
    sqlx::query("DELETE FROM page_verifications WHERE page_id = $1").bind(id).execute(&state.db).await?;
    audit::log(&state, user.workspace_id, Some(user.id), "page.unverify", Some("page"), Some(id), json!({})).await;
    Ok(StatusCode::NO_CONTENT)
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ReviewReq {
    user_id: Uuid,
    note: Option<String>,
}

/// `POST /api/pages/{id}/verification/request`: asks someone who may edit the page to review it.
pub async fn request_review(
    State(state): State<AppState>,
    CurrentUser(user): CurrentUser,
    Path(id): Path<Uuid>,
    Json(req): Json<ReviewReq>,
) -> ApiResult<StatusCode> {
    let p = Principal::load(&state, Some(user.clone())).await?;
    require_page(&state, &p, id, Need::View).await?;
    let reviewer = sqlx::query_as::<_, crate::auth::User>(sqlx::AssertSqlSafe(format!(
        "SELECT {} FROM users WHERE id = $1 AND workspace_id = $2 AND status = 'active'",
        crate::auth::USER_COLUMNS
    )))
    .bind(req.user_id)
    .bind(user.workspace_id)
    .fetch_optional(&state.db)
    .await?
    .ok_or(ApiError::NotFound("user"))?;
    // only people who could verify the page can be asked (and this doesn't reveal more than
    // "can't review": both cases look the same)
    // twenty requests per person and hour
    let recent: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM notifications WHERE actor_id = $1 AND kind = 'verification_request' AND created_at > now() - interval '1 hour'",
    )
    .bind(user.id)
    .fetch_one(&state.db)
    .await?;
    if recent >= 20 {
        return Err(ApiError::bad("too many review requests; try again later"));
    }
    // only people who could verify the page are asked; the answer is the same either way, so
    // it doesn't tell who may edit the page
    let rp = Principal::load(&state, Some(reviewer.clone())).await?;
    if !page_access(&state, &rp, id).await.map(|(a, _)| a.edit).unwrap_or(false) {
        return Ok(StatusCode::NO_CONTENT);
    }
    // one open request per page and reviewer
    let open: bool = sqlx::query_scalar(
        "SELECT EXISTS (SELECT 1 FROM notifications WHERE user_id = $1 AND page_id = $2 AND kind = 'verification_request' AND read_at IS NULL)",
    )
    .bind(reviewer.id)
    .bind(id)
    .fetch_one(&state.db)
    .await?;
    if !open {
        let note = req.note.map(|n| n.trim().chars().take(500).collect::<String>()).filter(|n| !n.is_empty());
        sqlx::query("INSERT INTO notifications (id, user_id, kind, page_id, actor_id, data) VALUES ($1, $2, 'verification_request', $3, $4, $5)")
            .bind(new_id())
            .bind(reviewer.id)
            .bind(id)
            .bind(user.id)
            .bind(json!({ "excerpt": note }))
            .execute(&state.db)
            .await?;
    }
    Ok(StatusCode::NO_CONTENT)
}

#[derive(Deserialize)]
pub struct ReportQuery {
    /// `expired` (default) | `verified` | `edited` (verified, but changed since)
    status: Option<String>,
    space: Option<String>,
}

/// `GET /api/reports/verification?status=&space=`: verified pages by state, for owners who
/// keep documentation current. Only pages the caller can see.
pub async fn report(State(state): State<AppState>, MaybeUser(user): MaybeUser, Query(q): Query<ReportQuery>) -> ApiResult<Json<Vec<Value>>> {
    let p = Principal::load(&state, user).await?;
    let ws = workspace_of(&state, &p).await?;
    let visible_sp = visible_spaces(&state, &p).await?;
    if visible_sp.is_empty() {
        return Ok(Json(vec![]));
    }
    let space_ids: Vec<Uuid> = visible_sp.iter().map(|(s, _)| s.id).collect();
    let status = q.status.as_deref().unwrap_or("expired");
    let cond = match status {
        "verified" => "(v.expires_at IS NULL OR v.expires_at > now())",
        "edited" => "(v.expires_at IS NULL OR v.expires_at > now()) AND p.updated_at > v.verified_at + interval '1 minute'",
        _ => "v.expires_at <= now()",
    };
    #[allow(clippy::type_complexity)]
    let rows: Vec<(
        Uuid,
        String,
        Uuid,
        String,
        Option<String>,
        chrono::DateTime<chrono::Utc>,
        Option<chrono::DateTime<chrono::Utc>>,
        chrono::DateTime<chrono::Utc>,
    )> = sqlx::query_as(sqlx::AssertSqlSafe(format!(
        "SELECT p.id, p.title, p.space_id, s.key, u.name, v.verified_at, v.expires_at, p.updated_at
             FROM page_verifications v JOIN pages p ON p.id = v.page_id JOIN spaces s ON s.id = p.space_id LEFT JOIN users u ON u.id = v.verified_by
             WHERE p.workspace_id = $1 AND p.deleted_at IS NULL AND s.archived_at IS NULL AND p.space_id = ANY($2)
               AND ($3::text IS NULL OR upper(s.key) = upper($3)) AND {cond}
             ORDER BY v.expires_at NULLS LAST, p.title LIMIT 500"
    )))
    .bind(ws)
    .bind(&space_ids)
    .bind(&q.space)
    .fetch_all(&state.db)
    .await?;
    let pairs: Vec<(Uuid, Uuid)> = rows.iter().map(|r| (r.0, r.2)).collect();
    let visible = filter_visible(&state, &p, &visible_sp, &pairs).await?;
    Ok(Json(
        rows.into_iter()
            .filter(|r| visible.contains(&r.0))
            .map(|(id, title, _, key, by, at, expires, updated)| {
                json!({ "id": id, "title": title, "spaceKey": key, "verifiedBy": by, "verifiedAt": at, "expiresAt": expires, "updatedAt": updated })
            })
            .collect(),
    ))
}
