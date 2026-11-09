//! Likes on pages and comments.

use axum::extract::{Path, State};
use axum::http::StatusCode;
use serde_json::{Value, json};
use uuid::Uuid;

use crate::app::{AppState, new_id};
use crate::auth::CurrentUser;
use crate::authz::{Need, Principal, page_access, require_page};
use crate::error::{ApiError, ApiResult};

/// `{count, liked, names}` for a page (names of the first likers, for the tooltip).
pub async fn page_summary(state: &AppState, page: Uuid, user: Option<Uuid>) -> ApiResult<Value> {
    let (count, liked): (i64, bool) = sqlx::query_as("SELECT count(*), COALESCE(bool_or(user_id = $2), false) FROM page_likes WHERE page_id = $1")
        .bind(page)
        .bind(user)
        .fetch_one(&state.db)
        .await?;
    let names: Vec<String> =
        sqlx::query_scalar("SELECT u.name FROM page_likes l JOIN users u ON u.id = l.user_id WHERE l.page_id = $1 ORDER BY l.created_at LIMIT 10")
            .bind(page)
            .fetch_all(&state.db)
            .await?;
    Ok(json!({ "count": count, "liked": liked, "names": names }))
}

/// `PUT /api/pages/{id}/like`
pub async fn like_page(State(state): State<AppState>, CurrentUser(user): CurrentUser, Path(page): Path<Uuid>) -> ApiResult<axum::Json<Value>> {
    let p = Principal::load(&state, Some(user.clone())).await?;
    require_page(&state, &p, page, Need::View).await?;
    let new = sqlx::query("INSERT INTO page_likes (page_id, user_id) VALUES ($1, $2) ON CONFLICT DO NOTHING")
        .bind(page)
        .bind(user.id)
        .execute(&state.db)
        .await?
        .rows_affected()
        == 1;
    if new {
        // tell the author (if they can still see the page)
        let author: Option<Uuid> = sqlx::query_scalar("SELECT created_by FROM pages WHERE id = $1").bind(page).fetch_one(&state.db).await?;
        if let Some(author) = author.filter(|a| *a != user.id) {
            let target = sqlx::query_as::<_, crate::auth::User>(sqlx::AssertSqlSafe(format!(
                "SELECT {} FROM users WHERE id = $1 AND status = 'active'",
                crate::auth::USER_COLUMNS
            )))
            .bind(author)
            .fetch_optional(&state.db)
            .await?;
            if let Some(target) = target {
                let tp = Principal::load(&state, Some(target)).await?;
                if page_access(&state, &tp, page).await.map(|(a, _)| a.view).unwrap_or(false) {
                    sqlx::query("INSERT INTO notifications (id, user_id, kind, page_id, actor_id, data) VALUES ($1, $2, 'like', $3, $4, '{}')")
                        .bind(new_id())
                        .bind(author)
                        .bind(page)
                        .bind(user.id)
                        .execute(&state.db)
                        .await?;
                }
            }
        }
    }
    Ok(axum::Json(page_summary(&state, page, Some(user.id)).await?))
}

/// `DELETE /api/pages/{id}/like`
pub async fn unlike_page(State(state): State<AppState>, CurrentUser(user): CurrentUser, Path(page): Path<Uuid>) -> ApiResult<axum::Json<Value>> {
    sqlx::query("DELETE FROM page_likes WHERE page_id = $1 AND user_id = $2").bind(page).bind(user.id).execute(&state.db).await?;
    let p = Principal::load(&state, Some(user.clone())).await?;
    require_page(&state, &p, page, Need::View).await?;
    Ok(axum::Json(page_summary(&state, page, Some(user.id)).await?))
}

async fn comment_page(state: &AppState, comment: Uuid) -> ApiResult<Uuid> {
    sqlx::query_scalar("SELECT page_id FROM comments WHERE id = $1 AND deleted_at IS NULL")
        .bind(comment)
        .fetch_optional(&state.db)
        .await?
        .ok_or(ApiError::NotFound("comment"))
}

/// `PUT /api/comments/{id}/like`
pub async fn like_comment(State(state): State<AppState>, CurrentUser(user): CurrentUser, Path(comment): Path<Uuid>) -> ApiResult<StatusCode> {
    let page = comment_page(&state, comment).await?;
    let p = Principal::load(&state, Some(user.clone())).await?;
    require_page(&state, &p, page, Need::View).await?;
    sqlx::query("INSERT INTO comment_likes (comment_id, user_id) VALUES ($1, $2) ON CONFLICT DO NOTHING")
        .bind(comment)
        .bind(user.id)
        .execute(&state.db)
        .await?;
    Ok(StatusCode::NO_CONTENT)
}

/// `DELETE /api/comments/{id}/like`
pub async fn unlike_comment(State(state): State<AppState>, CurrentUser(user): CurrentUser, Path(comment): Path<Uuid>) -> ApiResult<StatusCode> {
    sqlx::query("DELETE FROM comment_likes WHERE comment_id = $1 AND user_id = $2").bind(comment).bind(user.id).execute(&state.db).await?;
    Ok(StatusCode::NO_CONTENT)
}
