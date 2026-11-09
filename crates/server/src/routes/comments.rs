use axum::Json;
use axum::extract::{Path, State};
use axum::http::StatusCode;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use uuid::Uuid;
use wikicraft_doc_model::{Node, markdown, text};

use crate::app::{AppState, new_id};
use crate::auth::{CurrentUser, MaybeUser};
use crate::authz::{Need, Principal, require_page};
use crate::error::{ApiError, ApiResult};

#[derive(Serialize, sqlx::FromRow)]
#[serde(rename_all = "camelCase")]
pub struct CommentRow {
    id: Uuid,
    page_id: Uuid,
    parent_id: Option<Uuid>,
    author_id: Option<Uuid>,
    author_name: Option<String>,
    content: Value,
    inline: Option<Value>,
    resolved_at: Option<chrono::DateTime<chrono::Utc>>,
    resolved_by_name: Option<String>,
    created_at: chrono::DateTime<chrono::Utc>,
    updated_at: chrono::DateTime<chrono::Utc>,
}

const SELECT: &str = "SELECT c.id, c.page_id, c.parent_id, c.author_id, u.name AS author_name, c.content, c.inline, c.resolved_at, r.name AS resolved_by_name, c.created_at, c.updated_at
    FROM comments c LEFT JOIN users u ON u.id = c.author_id LEFT JOIN users r ON r.id = c.resolved_by";

pub async fn list(State(state): State<AppState>, MaybeUser(user): MaybeUser, Path(page_id): Path<Uuid>) -> ApiResult<Json<Vec<CommentRow>>> {
    let p = Principal::load(&state, user).await?;
    require_page(&state, &p, page_id, Need::View).await?;
    let rows = sqlx::query_as::<_, CommentRow>(sqlx::AssertSqlSafe(format!("{SELECT} WHERE c.page_id = $1 AND c.deleted_at IS NULL ORDER BY c.created_at")))
        .bind(page_id)
        .fetch_all(&state.db)
        .await?;
    Ok(Json(rows))
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CreateComment {
    content: Option<Value>,
    text: Option<String>,
    parent_id: Option<Uuid>,
    /// Inline comment: the selected text. The client adds a `comment` mark with the returned id.
    inline: Option<Value>,
}

fn comment_doc(content: Option<Value>, text: Option<String>) -> ApiResult<Node> {
    let mut doc = match (content, text) {
        (Some(c), _) => Node::from_json(&c)?,
        (None, Some(t)) => markdown::markdown_to_pm(&t).doc,
        _ => return Err(ApiError::bad("content or text required")),
    };
    wikicraft_doc_model::node::sanitize_tree(&mut doc);
    if text::plain_text(&doc).trim().is_empty() && !doc.content.iter().any(|n| n.kind == "image") {
        return Err(ApiError::bad("comment is empty"));
    }
    Ok(doc)
}

pub async fn create(
    State(state): State<AppState>,
    CurrentUser(user): CurrentUser,
    Path(page_id): Path<Uuid>,
    Json(req): Json<CreateComment>,
) -> ApiResult<(StatusCode, Json<CommentRow>)> {
    let p = Principal::load(&state, Some(user.clone())).await?;
    require_page(&state, &p, page_id, Need::Comment).await?;
    let doc = comment_doc(req.content, req.text)?;
    if let Some(parent) = req.parent_id {
        let ok: bool =
            sqlx::query_scalar("SELECT EXISTS (SELECT 1 FROM comments WHERE id = $1 AND page_id = $2)").bind(parent).bind(page_id).fetch_one(&state.db).await?;
        if !ok {
            return Err(ApiError::bad("parent comment not on this page"));
        }
    }
    let id = new_id();
    sqlx::query("INSERT INTO comments (id, page_id, parent_id, author_id, content, text_content, inline) VALUES ($1, $2, $3, $4, $5, $6, $7)")
        .bind(id)
        .bind(page_id)
        .bind(req.parent_id)
        .bind(user.id)
        .bind(doc.to_json())
        .bind(text::plain_text(&doc))
        .bind(req.inline)
        .execute(&state.db)
        .await?;
    notify_comment(&state, page_id, user.id, id, &doc).await?;
    let row = sqlx::query_as::<_, CommentRow>(sqlx::AssertSqlSafe(format!("{SELECT} WHERE c.id = $1"))).bind(id).fetch_one(&state.db).await?;
    crate::webhooks::emit_page(
        &state,
        "comment.created",
        page_id,
        Some(user.id),
        json!({"comment": {"id": id, "parentId": req.parent_id, "text": text::excerpt(&doc, 280)}}),
    )
    .await;
    Ok((StatusCode::CREATED, Json(row)))
}

/// Notifies page watchers and mentioned users.
async fn notify_comment(state: &AppState, page_id: Uuid, actor: Uuid, comment_id: Uuid, doc: &Node) -> ApiResult<()> {
    let mut mentioned: Vec<Uuid> = Vec::new();
    doc.walk(&mut |n| {
        if n.kind == "mention"
            && let Some(id) = n.attr_str("id").and_then(|s| s.parse().ok())
        {
            mentioned.push(id);
        }
    });
    let excerpt = text::excerpt(doc, 140);
    sqlx::query(
        "INSERT INTO notifications (id, user_id, kind, page_id, actor_id, data)
         SELECT gen_random_uuid(), w.user_id, 'comment', $1, $2, $3 FROM watches w WHERE w.page_id = $1 AND w.user_id <> $2 AND NOT (w.user_id = ANY($4))",
    )
    .bind(page_id)
    .bind(actor)
    .bind(json!({"commentId": comment_id, "excerpt": excerpt}))
    .bind(&mentioned)
    .execute(&state.db)
    .await?;
    for m in mentioned.iter().filter(|m| **m != actor) {
        sqlx::query("INSERT INTO notifications (id, user_id, kind, page_id, actor_id, data) VALUES ($1, $2, 'mention', $3, $4, $5)")
            .bind(new_id())
            .bind(m)
            .bind(page_id)
            .bind(actor)
            .bind(json!({"commentId": comment_id, "excerpt": excerpt}))
            .execute(&state.db)
            .await?;
    }
    Ok(())
}

#[derive(Deserialize)]
pub struct UpdateComment {
    content: Option<Value>,
    text: Option<String>,
    resolved: Option<bool>,
}

pub async fn update(
    State(state): State<AppState>,
    CurrentUser(user): CurrentUser,
    Path(id): Path<Uuid>,
    Json(req): Json<UpdateComment>,
) -> ApiResult<Json<CommentRow>> {
    let (page_id, author): (Uuid, Option<Uuid>) = sqlx::query_as("SELECT page_id, author_id FROM comments WHERE id = $1 AND deleted_at IS NULL")
        .bind(id)
        .fetch_optional(&state.db)
        .await?
        .ok_or(ApiError::NotFound("comment"))?;
    let p = Principal::load(&state, Some(user.clone())).await?;
    let (access, _) = require_page(&state, &p, page_id, Need::Comment).await?;
    if req.content.is_some() || req.text.is_some() {
        if author != Some(user.id) {
            return Err(ApiError::Forbidden);
        }
        let doc = comment_doc(req.content, req.text)?;
        sqlx::query("UPDATE comments SET content = $2, text_content = $3, updated_at = now() WHERE id = $1")
            .bind(id)
            .bind(doc.to_json())
            .bind(text::plain_text(&doc))
            .execute(&state.db)
            .await?;
    }
    if let Some(r) = req.resolved {
        if author != Some(user.id) && !access.edit {
            return Err(ApiError::Forbidden);
        }
        sqlx::query("UPDATE comments SET resolved_at = CASE WHEN $2 THEN now() ELSE NULL END, resolved_by = CASE WHEN $2 THEN $3 ELSE NULL END WHERE id = $1")
            .bind(id)
            .bind(r)
            .bind(user.id)
            .execute(&state.db)
            .await?;
    }
    Ok(Json(sqlx::query_as::<_, CommentRow>(sqlx::AssertSqlSafe(format!("{SELECT} WHERE c.id = $1"))).bind(id).fetch_one(&state.db).await?))
}

pub async fn delete(State(state): State<AppState>, CurrentUser(user): CurrentUser, Path(id): Path<Uuid>) -> ApiResult<StatusCode> {
    let (page_id, author): (Uuid, Option<Uuid>) = sqlx::query_as("SELECT page_id, author_id FROM comments WHERE id = $1 AND deleted_at IS NULL")
        .bind(id)
        .fetch_optional(&state.db)
        .await?
        .ok_or(ApiError::NotFound("comment"))?;
    let p = Principal::load(&state, Some(user.clone())).await?;
    let (access, _) = require_page(&state, &p, page_id, Need::View).await?;
    if author != Some(user.id) && !access.admin {
        return Err(ApiError::Forbidden);
    }
    sqlx::query("UPDATE comments SET deleted_at = now() WHERE id = $1 OR parent_id = $1").bind(id).execute(&state.db).await?;
    Ok(StatusCode::NO_CONTENT)
}
