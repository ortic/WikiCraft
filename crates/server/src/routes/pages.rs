use axum::Json;
use axum::extract::{Path, Query, State};
use axum::http::{StatusCode, header};
use axum::response::{IntoResponse, Response};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use sqlx::PgExecutor;
use uuid::Uuid;
use wikicraft_doc_model::{Node, html, markdown, text};

use crate::app::{AppState, new_id};
use crate::audit;
use crate::auth::{CurrentUser, MaybeUser};
use crate::authz::{Need, Principal, Role, require_page};
use crate::error::{ApiError, ApiResult};
use crate::routes::spaces::load_space;

pub struct NewPage {
    pub id: Uuid,
    pub workspace_id: Uuid,
    pub space_id: Uuid,
    pub parent_id: Option<Uuid>,
    pub kind: String,
    pub title: String,
    pub content: Node,
    pub position: f64,
    pub created_by: Option<Uuid>,
    pub created_at: Option<chrono::DateTime<chrono::Utc>>,
    pub source_ref: Option<String>,
}

/// Inserts a page (and its first version). Returns the id.
pub async fn insert_page<'e, E: PgExecutor<'e> + Copy>(db: E, p: NewPage) -> ApiResult<Uuid> {
    let plain = text::plain_text(&p.content);
    let content = p.content.to_json();
    sqlx::query(
        "INSERT INTO pages (id, workspace_id, space_id, parent_id, kind, title, content, text_content, position, created_by, updated_by, created_at, updated_at, source_ref)
         VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10, $10, COALESCE($11, now()), COALESCE($11, now()), $12)",
    )
    .bind(p.id)
    .bind(p.workspace_id)
    .bind(p.space_id)
    .bind(p.parent_id)
    .bind(&p.kind)
    .bind(&p.title)
    .bind(&content)
    .bind(&plain)
    .bind(p.position)
    .bind(p.created_by)
    .bind(p.created_at)
    .bind(&p.source_ref)
    .execute(db)
    .await?;
    sqlx::query("INSERT INTO page_versions (id, page_id, version, title, content, created_by, created_at) VALUES ($1, $2, 1, $3, $4, $5, COALESCE($6, now()))")
        .bind(new_id())
        .bind(p.id)
        .bind(&p.title)
        .bind(&content)
        .bind(p.created_by)
        .bind(p.created_at)
        .execute(db)
        .await?;
    Ok(p.id)
}

/// Snapshot the current content as a new version if it differs from the latest version.
pub async fn create_version(state: &AppState, page_id: Uuid, editor: Option<Uuid>, message: Option<String>) -> anyhow::Result<Option<i32>> {
    let message_for_hook = message.clone();
    let mut tx = state.db.begin().await?;
    let row: Option<(String, Value, i32)> =
        sqlx::query_as("SELECT title, content, version FROM pages WHERE id = $1 FOR UPDATE").bind(page_id).fetch_optional(&mut *tx).await?;
    let Some((title, content, _)) = row else { return Ok(None) };
    let latest: Option<(i32, Value, String)> =
        sqlx::query_as("SELECT version, content, title FROM page_versions WHERE page_id = $1 ORDER BY version DESC LIMIT 1")
            .bind(page_id)
            .fetch_optional(&mut *tx)
            .await?;
    if let Some((_, c, t)) = &latest
        && *c == content
        && *t == title
        && message.is_none()
    {
        return Ok(None);
    }
    let next = latest.map(|l| l.0 + 1).unwrap_or(1);
    sqlx::query("INSERT INTO page_versions (id, page_id, version, title, content, created_by, message) VALUES ($1, $2, $3, $4, $5, $6, $7)")
        .bind(new_id())
        .bind(page_id)
        .bind(next)
        .bind(&title)
        .bind(&content)
        .bind(editor)
        .bind(message)
        .execute(&mut *tx)
        .await?;
    sqlx::query("UPDATE pages SET version = $2 WHERE id = $1").bind(page_id).bind(next).execute(&mut *tx).await?;
    tx.commit().await?;
    if let Err(e) = crate::routes::notifications::notify_page_updated(state, page_id, next, editor).await {
        tracing::warn!("watch notifications failed: {e:#}");
    }
    crate::webhooks::emit_page(state, "page.updated", page_id, editor, json!({"version": next, "message": message_for_hook})).await;
    Ok(Some(next))
}

async fn next_position(state: &AppState, space_id: Uuid, parent: Option<Uuid>) -> ApiResult<f64> {
    let max: Option<f64> =
        sqlx::query_scalar("SELECT max(position) FROM pages WHERE space_id = $1 AND parent_id IS NOT DISTINCT FROM $2 AND deleted_at IS NULL")
            .bind(space_id)
            .bind(parent)
            .fetch_one(&state.db)
            .await?;
    Ok(max.map(|m| m + 1.0).unwrap_or(0.0))
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CreatePage {
    space_key: String,
    parent_id: Option<Uuid>,
    title: Option<String>,
    content: Option<Value>,
    markdown: Option<String>,
    kind: Option<String>,
    /// Create from a template (`builtin:*` or a template id).
    template_id: Option<String>,
}

pub async fn create(State(state): State<AppState>, CurrentUser(user): CurrentUser, Json(req): Json<CreatePage>) -> ApiResult<(StatusCode, Json<Value>)> {
    let p = Principal::load(&state, Some(user.clone())).await?;
    let (space, role) = load_space(&state, &p, &req.space_key).await?;
    if role < Role::Editor {
        return Err(ApiError::Forbidden);
    }
    if let Some(parent) = req.parent_id {
        let (_, parent_space) = require_page(&state, &p, parent, Need::View).await?;
        if parent_space != space.id {
            return Err(ApiError::bad("parent page belongs to another space"));
        }
    }
    let kind = req.kind.unwrap_or_else(|| "page".into());
    if !matches!(kind.as_str(), "page" | "folder" | "blog") {
        return Err(ApiError::bad("kind must be page, folder or blog"));
    }
    let mut template_title = None;
    let content = match (req.content, req.markdown, &req.template_id) {
        (Some(c), _, _) => {
            let mut n = Node::from_json(&c)?;
            wikicraft_doc_model::node::sanitize_tree(&mut n);
            n
        }
        (None, Some(md), _) => markdown::markdown_to_pm(&md).doc,
        (None, None, Some(t)) => {
            let (title, doc) = crate::routes::templates::resolve(&state, user.workspace_id, t).await?;
            template_title = title;
            doc
        }
        _ => Node::empty_doc(),
    };
    let title = req
        .title
        .map(|t| t.trim().to_string())
        .filter(|t| !t.is_empty())
        .or(template_title.map(|t| t.trim().to_string()).filter(|t| !t.is_empty()))
        .unwrap_or_else(|| "Untitled".into());
    let position = next_position(&state, space.id, req.parent_id).await?;
    let doc = content.clone();
    let id = insert_page(
        &state.db,
        NewPage {
            id: new_id(),
            workspace_id: user.workspace_id,
            space_id: space.id,
            parent_id: req.parent_id,
            kind,
            title,
            content,
            position,
            created_by: Some(user.id),
            created_at: None,
            source_ref: None,
        },
    )
    .await?;
    state.search.enqueue(id);
    sqlx::query("INSERT INTO watches (user_id, page_id) VALUES ($1, $2) ON CONFLICT DO NOTHING").bind(user.id).bind(id).execute(&state.db).await?;
    // content created through the API: index its tasks and notify mentioned people right away
    crate::routes::tasks::index_page(&mut *state.db.acquire().await?, id, &doc).await?;
    if let Err(e) = crate::routes::notifications::notify_page_mentions(&state, id, &doc, Some(user.id)).await {
        tracing::warn!(page = %id, "mention notifications failed: {e:#}");
    }
    crate::webhooks::emit_page(&state, "page.created", id, Some(user.id), json!({})).await;
    Ok((StatusCode::CREATED, Json(json!({ "id": id }))))
}

#[derive(Serialize, sqlx::FromRow)]
#[serde(rename_all = "camelCase")]
pub struct PageRow {
    pub id: Uuid,
    pub space_id: Uuid,
    pub space_key: String,
    pub space_name: String,
    pub parent_id: Option<Uuid>,
    pub kind: String,
    pub title: String,
    pub icon: Option<String>,
    pub content: Value,
    pub version: i32,
    pub status: String,
    pub page_status: Option<String>,
    pub full_width: bool,
    pub created_by: Option<Uuid>,
    pub created_by_name: Option<String>,
    pub created_at: chrono::DateTime<chrono::Utc>,
    pub updated_by: Option<Uuid>,
    pub updated_by_name: Option<String>,
    pub updated_at: chrono::DateTime<chrono::Utc>,
    pub deleted_at: Option<chrono::DateTime<chrono::Utc>>,
}

const PAGE_SELECT: &str = "SELECT p.id, p.space_id, s.key AS space_key, s.name AS space_name, p.parent_id, p.kind, p.title, p.icon, p.content, p.version, p.status, p.page_status, p.full_width,
    p.created_by, cu.name AS created_by_name, p.created_at, p.updated_by, uu.name AS updated_by_name, p.updated_at, p.deleted_at
    FROM pages p JOIN spaces s ON s.id = p.space_id LEFT JOIN users cu ON cu.id = p.created_by LEFT JOIN users uu ON uu.id = p.updated_by";

#[derive(Serialize, sqlx::FromRow)]
pub struct Crumb {
    id: Uuid,
    title: String,
}

pub async fn breadcrumbs(state: &AppState, page_id: Uuid) -> ApiResult<Vec<Crumb>> {
    Ok(sqlx::query_as::<_, Crumb>(
        "WITH RECURSIVE chain AS (
            SELECT id, parent_id, title, 0 AS depth FROM pages WHERE id = $1
            UNION ALL SELECT p.id, p.parent_id, p.title, c.depth + 1 FROM pages p JOIN chain c ON p.id = c.parent_id WHERE c.depth < 200
         ) SELECT id, title FROM chain WHERE depth > 0 ORDER BY depth DESC",
    )
    .bind(page_id)
    .fetch_all(&state.db)
    .await?)
}

pub async fn get(State(state): State<AppState>, MaybeUser(user): MaybeUser, Path(id): Path<Uuid>) -> ApiResult<Json<Value>> {
    let p = Principal::load(&state, user.clone()).await?;
    let (access, _) = require_page(&state, &p, id, Need::View).await?;
    let mut page = sqlx::query_as::<_, PageRow>(sqlx::AssertSqlSafe(format!("{PAGE_SELECT} WHERE p.id = $1")))
        .bind(id)
        .fetch_optional(&state.db)
        .await?
        .ok_or(ApiError::NotFound("page"))?;
    // live content if a collaboration session is active (DB may lag by ~2s)
    if let Some(room) = state.rooms.get(id) {
        page.content = room.snapshot().to_json();
    }
    let crumbs = breadcrumbs(&state, id).await?;
    let labels: Vec<String> = sqlx::query_scalar("SELECT name FROM labels WHERE page_id = $1 ORDER BY name").bind(id).fetch_all(&state.db).await?;
    let restricted: bool = sqlx::query_scalar("SELECT EXISTS (SELECT 1 FROM page_restrictions WHERE page_id = $1)").bind(id).fetch_one(&state.db).await?;
    let (favorite, comment_count): (bool, i64) = match &user {
        Some(u) => {
            sqlx::query(
                "INSERT INTO recent_views (user_id, page_id, viewed_at) VALUES ($1, $2, now()) ON CONFLICT (user_id, page_id) DO UPDATE SET viewed_at = now()",
            )
            .bind(u.id)
            .bind(id)
            .execute(&state.db)
            .await?;
            let fav: bool = sqlx::query_scalar("SELECT EXISTS (SELECT 1 FROM favorites WHERE user_id = $1 AND page_id = $2)")
                .bind(u.id)
                .bind(id)
                .fetch_one(&state.db)
                .await?;
            let cc: i64 = sqlx::query_scalar("SELECT count(*) FROM comments WHERE page_id = $1 AND deleted_at IS NULL").bind(id).fetch_one(&state.db).await?;
            (fav, cc)
        }
        None => (false, 0),
    };
    // view statistics (anonymous views aggregate under NULL user)
    sqlx::query(
        "INSERT INTO page_views (page_id, user_id, day) VALUES ($1, $2, current_date)
         ON CONFLICT (page_id, COALESCE(user_id, '00000000-0000-0000-0000-000000000000'::uuid), day) DO UPDATE SET views = page_views.views + 1",
    )
    .bind(id)
    .bind(user.as_ref().map(|u| u.id))
    .execute(&state.db)
    .await?;
    let (views, viewers): (i64, i64) = sqlx::query_as("SELECT COALESCE(sum(views), 0)::bigint, count(DISTINCT user_id) FROM page_views WHERE page_id = $1")
        .bind(id)
        .fetch_one(&state.db)
        .await?;
    let watching: bool = match &user {
        Some(u) => {
            sqlx::query_scalar("SELECT EXISTS (SELECT 1 FROM watches WHERE user_id = $1 AND page_id = $2)").bind(u.id).bind(id).fetch_one(&state.db).await?
        }
        None => false,
    };
    let mut v = serde_json::to_value(&page)?;
    let o = v.as_object_mut().expect("object");
    o.insert("breadcrumbs".into(), serde_json::to_value(crumbs)?);
    o.insert("labels".into(), json!(labels));
    o.insert("access".into(), serde_json::to_value(access)?);
    o.insert("restricted".into(), json!(restricted));
    o.insert("favorite".into(), json!(favorite));
    o.insert("watching".into(), json!(watching));
    o.insert("views".into(), json!(views));
    o.insert("viewers".into(), json!(viewers));
    o.insert("commentCount".into(), json!(comment_count));
    o.insert("likes".into(), crate::routes::likes::page_summary(&state, id, user.as_ref().map(|u| u.id)).await?);
    Ok(Json(v))
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct UpdatePage {
    title: Option<String>,
    icon: Option<String>,
    full_width: Option<bool>,
    page_status: Option<String>,
}

pub async fn update(State(state): State<AppState>, CurrentUser(user): CurrentUser, Path(id): Path<Uuid>, Json(req): Json<UpdatePage>) -> ApiResult<StatusCode> {
    let p = Principal::load(&state, Some(user.clone())).await?;
    require_page(&state, &p, id, Need::Edit).await?;
    let title = req.title.as_deref().map(str::trim).filter(|t| !t.is_empty());
    if title.is_some_and(|t| t.chars().count() > 255) {
        return Err(ApiError::bad("title too long"));
    }
    sqlx::query(
        "UPDATE pages SET title = COALESCE($2, title), icon = CASE WHEN $3::text IS NULL THEN icon WHEN $3 = '' THEN NULL ELSE $3 END,
         full_width = COALESCE($4, full_width), page_status = CASE WHEN $5::text IS NULL THEN page_status WHEN $5 = '' THEN NULL ELSE $5 END,
         updated_at = now(), updated_by = $6 WHERE id = $1",
    )
    .bind(id)
    .bind(title)
    .bind(req.icon)
    .bind(req.full_width)
    .bind(req.page_status)
    .bind(user.id)
    .execute(&state.db)
    .await?;
    state.search.enqueue(id);
    Ok(StatusCode::NO_CONTENT)
}

#[derive(Deserialize)]
pub struct PutContent {
    content: Option<Value>,
    markdown: Option<String>,
    message: Option<String>,
}

/// Replaces the page content (API clients). Live editors receive the change immediately.
pub async fn put_content(
    State(state): State<AppState>,
    CurrentUser(user): CurrentUser,
    Path(id): Path<Uuid>,
    Json(req): Json<PutContent>,
) -> ApiResult<Json<Value>> {
    let p = Principal::load(&state, Some(user.clone())).await?;
    require_page(&state, &p, id, Need::Edit).await?;
    let doc = match (req.content, req.markdown) {
        (Some(c), _) => {
            let mut n = Node::from_json(&c)?;
            if n.kind != "doc" {
                return Err(ApiError::bad("content must be a ProseMirror doc node"));
            }
            wikicraft_doc_model::node::sanitize_tree(&mut n);
            n
        }
        (None, Some(md)) => markdown::markdown_to_pm(&md).doc,
        _ => return Err(ApiError::bad("content or markdown required")),
    };
    let room = state.rooms.get_or_load(&state, id).await?;
    room.replace_content(&doc, Some(user.id));
    crate::collab::persist_with(&state, &room, false).await?;
    let v = create_version(&state, id, Some(user.id), req.message).await?;
    Ok(Json(json!({ "version": v })))
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct MovePage {
    /// New parent (`null` = space root).
    parent_id: Option<Uuid>,
    /// Target space (defaults to the current one).
    space_key: Option<String>,
    /// Place directly before/after this sibling (otherwise appended).
    before_id: Option<Uuid>,
    after_id: Option<Uuid>,
}

pub async fn move_page(
    State(state): State<AppState>,
    CurrentUser(user): CurrentUser,
    Path(id): Path<Uuid>,
    Json(req): Json<MovePage>,
) -> ApiResult<StatusCode> {
    let p = Principal::load(&state, Some(user.clone())).await?;
    let (_, from_space) = require_page(&state, &p, id, Need::Admin).await?;
    let target_space = match &req.space_key {
        Some(k) => {
            let (s, role) = load_space(&state, &p, k).await?;
            if role < Role::Editor {
                return Err(ApiError::Forbidden);
            }
            s.id
        }
        None => from_space,
    };
    if target_space != from_space {
        // moving a subtree to another space hands it to that space's admins (who bypass
        // restrictions): only admins of the source space may do that
        let sa = crate::authz::load_space_access(&state, from_space).await?;
        if crate::authz::space_role(&state, &p, &sa).await? < Role::Admin {
            return Err(ApiError::Forbidden);
        }
    }
    if let Some(parent) = req.parent_id {
        if parent == id {
            return Err(ApiError::bad("a page can't be its own parent"));
        }
        let (_, ps) = require_page(&state, &p, parent, Need::View).await?;
        if ps != target_space {
            return Err(ApiError::bad("parent is in another space"));
        }
        // no cycles: the new parent must not be a descendant of the moved page
        let is_desc: bool = sqlx::query_scalar(
            "WITH RECURSIVE sub AS (SELECT id FROM pages WHERE id = $1 UNION ALL SELECT p.id FROM pages p JOIN sub ON p.parent_id = sub.id) SELECT EXISTS (SELECT 1 FROM sub WHERE id = $2)",
        )
        .bind(id)
        .bind(parent)
        .fetch_one(&state.db)
        .await?;
        if is_desc {
            return Err(ApiError::bad("can't move a page below one of its descendants"));
        }
    }
    let sibling_pos = |sid: Uuid| {
        let db = state.db.clone();
        async move { sqlx::query_scalar::<_, f64>("SELECT position FROM pages WHERE id = $1").bind(sid).fetch_optional(&db).await }
    };
    let siblings: Vec<(Uuid, f64)> = sqlx::query_as(
        "SELECT id, position FROM pages WHERE space_id = $1 AND parent_id IS NOT DISTINCT FROM $2 AND deleted_at IS NULL AND id <> $3 ORDER BY position",
    )
    .bind(target_space)
    .bind(req.parent_id)
    .bind(id)
    .fetch_all(&state.db)
    .await?;
    let position = if let Some(b) = req.before_id {
        let bp = sibling_pos(b).await?.ok_or(ApiError::NotFound("sibling"))?;
        let prev = siblings.iter().take_while(|(sid, _)| *sid != b).last().map(|s| s.1);
        match prev {
            Some(pp) => (pp + bp) / 2.0,
            None => bp - 1.0,
        }
    } else if let Some(a) = req.after_id {
        let ap = sibling_pos(a).await?.ok_or(ApiError::NotFound("sibling"))?;
        let next = siblings.iter().skip_while(|(sid, _)| *sid != a).nth(1).map(|s| s.1);
        match next {
            Some(np) => (ap + np) / 2.0,
            None => ap + 1.0,
        }
    } else {
        siblings.last().map(|s| s.1 + 1.0).unwrap_or(0.0)
    };
    let mut tx = state.db.begin().await?;
    sqlx::query("UPDATE pages SET parent_id = $2, position = $3, space_id = $4, updated_at = now() WHERE id = $1")
        .bind(id)
        .bind(req.parent_id)
        .bind(position)
        .bind(target_space)
        .execute(&mut *tx)
        .await?;
    if target_space != from_space {
        sqlx::query("WITH RECURSIVE sub AS (SELECT id FROM pages WHERE parent_id = $1 UNION ALL SELECT p.id FROM pages p JOIN sub ON p.parent_id = sub.id) UPDATE pages SET space_id = $2 WHERE id IN (SELECT id FROM sub)")
            .bind(id)
            .bind(target_space)
            .execute(&mut *tx)
            .await?;
    }
    // renumber siblings when floating point gaps get too small
    let gap_too_small = siblings.iter().any(|(_, sp)| (sp - position).abs() < 1e-9);
    if gap_too_small {
        sqlx::query(
            "UPDATE pages p SET position = r.rn FROM (SELECT id, row_number() OVER (ORDER BY position, id) AS rn FROM pages WHERE space_id = $1 AND parent_id IS NOT DISTINCT FROM $2 AND deleted_at IS NULL) r WHERE p.id = r.id",
        )
        .bind(target_space)
        .bind(req.parent_id)
        .execute(&mut *tx)
        .await?;
    }
    tx.commit().await?;
    // the search index stores the space: reindex the moved subtree
    let subtree: Vec<Uuid> = sqlx::query_scalar(
        "WITH RECURSIVE sub AS (SELECT id FROM pages WHERE id = $1 UNION ALL SELECT p.id FROM pages p JOIN sub ON p.parent_id = sub.id) SELECT id FROM sub",
    )
    .bind(id)
    .fetch_all(&state.db)
    .await?;
    for pid in subtree {
        state.search.enqueue(pid);
    }
    audit::log(
        &state,
        user.workspace_id,
        Some(user.id),
        "page.move",
        Some("page"),
        Some(id),
        json!({"parentId": req.parent_id, "spaceChanged": target_space != from_space}),
    )
    .await;
    crate::webhooks::emit_page(&state, "page.moved", id, Some(user.id), json!({"spaceChanged": target_space != from_space})).await;
    Ok(StatusCode::NO_CONTENT)
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CopyPage {
    parent_id: Option<Uuid>,
    title: Option<String>,
    #[serde(default)]
    include_children: bool,
}

pub async fn copy(
    State(state): State<AppState>,
    CurrentUser(user): CurrentUser,
    Path(id): Path<Uuid>,
    Json(req): Json<CopyPage>,
) -> ApiResult<(StatusCode, Json<Value>)> {
    let p = Principal::load(&state, Some(user.clone())).await?;
    let (access, space) = require_page(&state, &p, id, Need::View).await?;
    let role = access.space_role.unwrap_or(Role::None);
    if role < Role::Editor {
        return Err(ApiError::Forbidden);
    }
    let source_parent: Option<Uuid> = sqlx::query_scalar("SELECT parent_id FROM pages WHERE id = $1").bind(id).fetch_one(&state.db).await?;
    let target_parent = match req.parent_id {
        Some(par) => {
            let (_, ps) = require_page(&state, &p, par, Need::Edit).await?;
            if ps != space {
                return Err(ApiError::bad("target parent is in another space"));
            }
            Some(par)
        }
        None => source_parent,
    };
    if let Some(room) = state.rooms.get(id) {
        crate::collab::persist_with(&state, &room, false).await?;
    }
    // only descendants the caller can see are copied (restricted children stay private)
    let pairs: Vec<(Uuid, Option<Uuid>)> =
        sqlx::query_as("SELECT id, parent_id FROM pages WHERE space_id = $1 AND deleted_at IS NULL").bind(space).fetch_all(&state.db).await?;
    let visible = crate::authz::visible_page_ids(&state, &p, space, role, &pairs).await?;
    let mut queue = vec![(id, target_parent)];
    let mut map = std::collections::HashMap::new();
    let mut first_new = None;
    let mut i = 0;
    while i < queue.len() && i < 5000 {
        let (src, parent) = queue[i];
        let (title, kind, content): (String, String, Value) =
            sqlx::query_as("SELECT title, kind, content FROM pages WHERE id = $1").bind(src).fetch_one(&state.db).await?;
        let title = if i == 0 { req.title.clone().unwrap_or_else(|| format!("Copy of {title}")) } else { title };
        let new = insert_page(
            &state.db,
            NewPage {
                id: new_id(),
                workspace_id: user.workspace_id,
                space_id: space,
                parent_id: parent,
                kind,
                title,
                content: Node::from_json(&content).unwrap_or_else(|_| Node::empty_doc()),
                position: next_position(&state, space, parent).await?,
                created_by: Some(user.id),
                created_at: None,
                source_ref: None,
            },
        )
        .await?;
        sqlx::query("INSERT INTO labels (page_id, name) SELECT $2, name FROM labels WHERE page_id = $1").bind(src).bind(new).execute(&state.db).await?;
        // restrictions travel with the copy (copying must never widen access)
        sqlx::query("INSERT INTO page_restrictions (page_id, kind, subject_type, subject_id) SELECT $2, kind, subject_type, subject_id FROM page_restrictions WHERE page_id = $1")
            .bind(src)
            .bind(new)
            .execute(&state.db)
            .await?;
        state.search.enqueue(new);
        map.insert(src, new);
        first_new.get_or_insert(new);
        if req.include_children {
            let kids: Vec<Uuid> =
                sqlx::query_scalar("SELECT id FROM pages WHERE parent_id = $1 AND deleted_at IS NULL ORDER BY position").bind(src).fetch_all(&state.db).await?;
            queue.extend(kids.into_iter().filter(|k| visible.contains(k)).map(|k| (k, Some(new))));
        }
        i += 1;
    }
    audit::log(&state, user.workspace_id, Some(user.id), "page.copy", Some("page"), Some(id), json!({"copied": map.len()})).await;
    Ok((StatusCode::CREATED, Json(json!({ "id": first_new, "copied": map.len() }))))
}

/// Moves a page and its descendants to the trash.
pub async fn trash(State(state): State<AppState>, CurrentUser(user): CurrentUser, Path(id): Path<Uuid>) -> ApiResult<StatusCode> {
    let p = Principal::load(&state, Some(user.clone())).await?;
    let (access, space) = require_page(&state, &p, id, Need::Admin).await?;
    let subtree: Vec<Uuid> = sqlx::query_scalar("WITH RECURSIVE sub AS (SELECT id FROM pages WHERE id = $1 UNION ALL SELECT p.id FROM pages p JOIN sub ON p.parent_id = sub.id WHERE p.deleted_at IS NULL) SELECT id FROM sub")
        .bind(id)
        .fetch_all(&state.db)
        .await?;
    let pairs: Vec<(Uuid, Option<Uuid>)> =
        sqlx::query_as("SELECT id, parent_id FROM pages WHERE space_id = $1 AND deleted_at IS NULL").bind(space).fetch_all(&state.db).await?;
    let visible = crate::authz::visible_page_ids(&state, &p, space, access.space_role.unwrap_or(Role::None), &pairs).await?;
    if subtree.iter().any(|s| !visible.contains(s)) {
        return Err(ApiError::Conflict("this page has child pages you can't see; ask a space admin to delete it".into()));
    }
    let ids: Vec<Uuid> = sqlx::query_scalar(
        "WITH RECURSIVE sub AS (SELECT id FROM pages WHERE id = $1 UNION ALL SELECT p.id FROM pages p JOIN sub ON p.parent_id = sub.id WHERE p.deleted_at IS NULL)
         UPDATE pages SET deleted_at = now(), deleted_by = $2 WHERE id IN (SELECT id FROM sub) RETURNING id",
    )
    .bind(id)
    .bind(user.id)
    .fetch_all(&state.db)
    .await?;
    for i in &ids {
        state.search.enqueue(*i);
        // drop live sessions: editors of a trashed page must not keep writing to it
        state.rooms.evict(*i);
    }
    audit::log(&state, user.workspace_id, Some(user.id), "page.trash", Some("page"), Some(id), json!({"count": ids.len()})).await;
    crate::webhooks::emit_page(&state, "page.trashed", id, Some(user.id), json!({"count": ids.len()})).await;
    Ok(StatusCode::NO_CONTENT)
}

pub async fn restore(State(state): State<AppState>, CurrentUser(user): CurrentUser, Path(id): Path<Uuid>) -> ApiResult<StatusCode> {
    let p = Principal::load(&state, Some(user.clone())).await?;
    let row: Option<(Uuid, Option<chrono::DateTime<chrono::Utc>>, Option<Uuid>)> =
        sqlx::query_as("SELECT space_id, deleted_at, parent_id FROM pages WHERE id = $1").bind(id).fetch_optional(&state.db).await?;
    let Some((space_id, Some(deleted_at), parent)) = row else { return Err(ApiError::NotFound("deleted page")) };
    let sa = crate::authz::load_space_access(&state, space_id).await?;
    let role = crate::authz::space_role(&state, &p, &sa).await?;
    if role < Role::Editor {
        return Err(ApiError::Forbidden);
    }
    let pairs: Vec<(Uuid, Option<Uuid>)> = sqlx::query_as("SELECT id, parent_id FROM pages WHERE space_id = $1").bind(space_id).fetch_all(&state.db).await?;
    if !crate::authz::visible_page_ids(&state, &p, space_id, role, &pairs).await?.contains(&id) {
        return Err(ApiError::NotFound("deleted page"));
    }
    if let Some(par) = parent {
        let parent_deleted: bool =
            sqlx::query_scalar("SELECT deleted_at IS NOT NULL FROM pages WHERE id = $1").bind(par).fetch_optional(&state.db).await?.unwrap_or(false);
        if parent_deleted {
            // restoring to the root would drop restrictions inherited from the parent
            return Err(ApiError::Conflict("restore the parent page first".into()));
        }
    }
    let mut tx = state.db.begin().await?;
    let ids: Vec<Uuid> = sqlx::query_scalar(
        "WITH RECURSIVE sub AS (SELECT id FROM pages WHERE id = $1 UNION ALL SELECT p.id FROM pages p JOIN sub ON p.parent_id = sub.id WHERE p.deleted_at = $2)
         UPDATE pages SET deleted_at = NULL, deleted_by = NULL WHERE id IN (SELECT id FROM sub) RETURNING id",
    )
    .bind(id)
    .bind(deleted_at)
    .fetch_all(&mut *tx)
    .await?;
    tx.commit().await?;
    for i in &ids {
        state.search.enqueue(*i);
    }
    audit::log(&state, user.workspace_id, Some(user.id), "page.restore", Some("page"), Some(id), json!({"count": ids.len()})).await;
    crate::webhooks::emit_page(&state, "page.restored", id, Some(user.id), json!({})).await;
    Ok(StatusCode::NO_CONTENT)
}

#[derive(Serialize, sqlx::FromRow)]
#[serde(rename_all = "camelCase")]
pub struct ChildRow {
    id: Uuid,
    title: String,
    kind: String,
    icon: Option<String>,
    parent_id: Option<Uuid>,
    updated_at: chrono::DateTime<chrono::Utc>,
    excerpt: String,
}

#[derive(Deserialize)]
pub struct ChildrenQuery {
    depth: Option<i32>,
}

/// Visible descendants (for the child-pages block). `depth` 1 = direct children, 0 = all.
pub async fn children(
    State(state): State<AppState>,
    MaybeUser(user): MaybeUser,
    Path(id): Path<Uuid>,
    Query(q): Query<ChildrenQuery>,
) -> ApiResult<Json<Vec<ChildRow>>> {
    let p = Principal::load(&state, user).await?;
    let (access, space_id) = require_page(&state, &p, id, Need::View).await?;
    let depth = match q.depth.unwrap_or(1) {
        0 => 50,
        d => d.clamp(1, 50),
    };
    let rows = sqlx::query_as::<_, ChildRow>(
        "WITH RECURSIVE sub AS (
            SELECT id, 1 AS depth FROM pages WHERE parent_id = $1 AND deleted_at IS NULL
            UNION ALL SELECT p.id, sub.depth + 1 FROM pages p JOIN sub ON p.parent_id = sub.id WHERE p.deleted_at IS NULL AND sub.depth < $2
         ) SELECT p.id, p.title, p.kind, p.icon, p.parent_id, p.updated_at, left(p.text_content, 200) AS excerpt FROM pages p JOIN sub ON sub.id = p.id ORDER BY sub.depth, p.position, lower(p.title)",
    )
    .bind(id)
    .bind(depth)
    .fetch_all(&state.db)
    .await?;
    let pairs: Vec<(Uuid, Option<Uuid>)> =
        sqlx::query_as("SELECT id, parent_id FROM pages WHERE space_id = $1 AND deleted_at IS NULL").bind(space_id).fetch_all(&state.db).await?;
    let visible = crate::authz::visible_page_ids(&state, &p, space_id, access.space_role.unwrap_or(Role::None), &pairs).await?;
    Ok(Json(rows.into_iter().filter(|r| visible.contains(&r.id)).collect()))
}

#[derive(Serialize, sqlx::FromRow)]
#[serde(rename_all = "camelCase")]
pub struct VersionRow {
    version: i32,
    title: String,
    created_by: Option<Uuid>,
    created_by_name: Option<String>,
    created_at: chrono::DateTime<chrono::Utc>,
    message: Option<String>,
}

pub async fn versions(State(state): State<AppState>, MaybeUser(user): MaybeUser, Path(id): Path<Uuid>) -> ApiResult<Json<Vec<VersionRow>>> {
    let p = Principal::load(&state, user).await?;
    require_page(&state, &p, id, Need::View).await?;
    let rows = sqlx::query_as::<_, VersionRow>(
        "SELECT v.version, v.title, v.created_by, u.name AS created_by_name, v.created_at, v.message FROM page_versions v LEFT JOIN users u ON u.id = v.created_by WHERE v.page_id = $1 ORDER BY v.version DESC",
    )
    .bind(id)
    .fetch_all(&state.db)
    .await?;
    Ok(Json(rows))
}

#[derive(Deserialize)]
pub struct SaveVersion {
    message: Option<String>,
}

pub async fn save_version(
    State(state): State<AppState>,
    CurrentUser(user): CurrentUser,
    Path(id): Path<Uuid>,
    Json(req): Json<SaveVersion>,
) -> ApiResult<Json<Value>> {
    let p = Principal::load(&state, Some(user.clone())).await?;
    require_page(&state, &p, id, Need::Edit).await?;
    if let Some(room) = state.rooms.get(id) {
        crate::collab::persist_with(&state, &room, false).await?;
    }
    let v = create_version(&state, id, Some(user.id), req.message.filter(|m| !m.trim().is_empty())).await?;
    Ok(Json(json!({ "version": v })))
}

pub async fn get_version(State(state): State<AppState>, MaybeUser(user): MaybeUser, Path((id, version)): Path<(Uuid, i32)>) -> ApiResult<Json<Value>> {
    let p = Principal::load(&state, user).await?;
    require_page(&state, &p, id, Need::View).await?;
    let row: Option<(String, Value, chrono::DateTime<chrono::Utc>, Option<String>, Option<String>)> = sqlx::query_as(
        "SELECT v.title, v.content, v.created_at, u.name, v.message FROM page_versions v LEFT JOIN users u ON u.id = v.created_by WHERE v.page_id = $1 AND v.version = $2",
    )
    .bind(id)
    .bind(version)
    .fetch_optional(&state.db)
    .await?;
    let (title, content, created_at, author, message) = row.ok_or(ApiError::NotFound("version"))?;
    Ok(Json(json!({ "version": version, "title": title, "content": content, "createdAt": created_at, "createdByName": author, "message": message })))
}

pub async fn restore_version(State(state): State<AppState>, CurrentUser(user): CurrentUser, Path((id, version)): Path<(Uuid, i32)>) -> ApiResult<Json<Value>> {
    let p = Principal::load(&state, Some(user.clone())).await?;
    require_page(&state, &p, id, Need::Edit).await?;
    let row: Option<(String, Value)> =
        sqlx::query_as("SELECT title, content FROM page_versions WHERE page_id = $1 AND version = $2").bind(id).bind(version).fetch_optional(&state.db).await?;
    let (title, content) = row.ok_or(ApiError::NotFound("version"))?;
    let doc = Node::from_json(&content)?;
    let room = state.rooms.get_or_load(&state, id).await?;
    room.replace_content(&doc, Some(user.id));
    sqlx::query("UPDATE pages SET title = $2 WHERE id = $1").bind(id).bind(&title).execute(&state.db).await?;
    crate::collab::persist_with(&state, &room, false).await?;
    let v = create_version(&state, id, Some(user.id), Some(format!("Restored version {version}"))).await?;
    audit::log(&state, user.workspace_id, Some(user.id), "page.restore_version", Some("page"), Some(id), json!({"version": version})).await;
    Ok(Json(json!({ "version": v })))
}

#[derive(Serialize, Deserialize, sqlx::FromRow, Clone)]
#[serde(rename_all = "camelCase")]
pub struct RestrictionEntry {
    kind: String,
    subject_type: String,
    subject_id: Uuid,
    #[sqlx(default)]
    #[serde(default)]
    subject_name: Option<String>,
}

pub async fn get_restrictions(State(state): State<AppState>, CurrentUser(user): CurrentUser, Path(id): Path<Uuid>) -> ApiResult<Json<Value>> {
    let p = Principal::load(&state, Some(user)).await?;
    require_page(&state, &p, id, Need::View).await?;
    let rows = sqlx::query_as::<_, RestrictionEntry>(
        "SELECT r.kind, r.subject_type, r.subject_id, COALESCE(u.name, g.name) AS subject_name FROM page_restrictions r
         LEFT JOIN users u ON r.subject_type = 'user' AND u.id = r.subject_id LEFT JOIN groups g ON r.subject_type = 'group' AND g.id = r.subject_id WHERE r.page_id = $1",
    )
    .bind(id)
    .fetch_all(&state.db)
    .await?;
    // inherited view restrictions from ancestors
    let inherited: Vec<(Uuid, String)> = sqlx::query_as(
        "WITH RECURSIVE chain AS (SELECT id, parent_id, 0 AS depth FROM pages WHERE id = $1 UNION ALL SELECT p.id, p.parent_id, c.depth + 1 FROM pages p JOIN chain c ON p.id = c.parent_id WHERE c.depth < 200)
         SELECT DISTINCT c.id, p.title FROM chain c JOIN pages p ON p.id = c.id JOIN page_restrictions r ON r.page_id = c.id AND r.kind = 'view' WHERE c.depth > 0",
    )
    .bind(id)
    .fetch_all(&state.db)
    .await?;
    Ok(Json(json!({ "restrictions": rows, "inheritedFrom": inherited.into_iter().map(|(id, t)| json!({"id": id, "title": t})).collect::<Vec<_>>() })))
}

#[derive(Deserialize)]
pub struct PutRestrictions {
    restrictions: Vec<RestrictionEntry>,
}

pub async fn put_restrictions(
    State(state): State<AppState>,
    CurrentUser(user): CurrentUser,
    Path(id): Path<Uuid>,
    Json(req): Json<PutRestrictions>,
) -> ApiResult<StatusCode> {
    let p = Principal::load(&state, Some(user.clone())).await?;
    let (access, _) = require_page(&state, &p, id, Need::Admin).await?;
    // don't let users lock themselves out (unless they're space admins, who bypass restrictions)
    if access.space_role != Some(Role::Admin) {
        for kind in ["view", "edit"] {
            let entries: Vec<&RestrictionEntry> = req.restrictions.iter().filter(|r| r.kind == kind).collect();
            if !entries.is_empty() && !entries.iter().any(|r| p.matches(&r.subject_type, r.subject_id)) {
                return Err(ApiError::bad(format!("you must keep {kind} access for yourself")));
            }
        }
    }
    let mut tx = state.db.begin().await?;
    sqlx::query("DELETE FROM page_restrictions WHERE page_id = $1").bind(id).execute(&mut *tx).await?;
    for r in &req.restrictions {
        if !matches!(r.kind.as_str(), "view" | "edit") || !matches!(r.subject_type.as_str(), "user" | "group") {
            return Err(ApiError::bad("invalid restriction"));
        }
        sqlx::query("INSERT INTO page_restrictions (page_id, kind, subject_type, subject_id) VALUES ($1, $2, $3, $4) ON CONFLICT DO NOTHING")
            .bind(id)
            .bind(&r.kind)
            .bind(&r.subject_type)
            .bind(r.subject_id)
            .execute(&mut *tx)
            .await?;
    }
    tx.commit().await?;
    audit::log(&state, user.workspace_id, Some(user.id), "page.restrictions", Some("page"), Some(id), json!({"count": req.restrictions.len()})).await;
    Ok(StatusCode::NO_CONTENT)
}

pub async fn get_labels(State(state): State<AppState>, MaybeUser(user): MaybeUser, Path(id): Path<Uuid>) -> ApiResult<Json<Vec<String>>> {
    let p = Principal::load(&state, user).await?;
    require_page(&state, &p, id, Need::View).await?;
    Ok(Json(sqlx::query_scalar("SELECT name FROM labels WHERE page_id = $1 ORDER BY name").bind(id).fetch_all(&state.db).await?))
}

#[derive(Deserialize)]
pub struct PutLabels {
    labels: Vec<String>,
}

pub fn normalize_label(l: &str) -> Option<String> {
    let s: String =
        l.trim().to_lowercase().chars().map(|c| if c.is_whitespace() { '-' } else { c }).filter(|c| !matches!(c, ',' | ';' | '#' | '"' | '\'')).collect();
    (!s.is_empty() && s.len() <= 100).then_some(s)
}

pub async fn put_labels(
    State(state): State<AppState>,
    CurrentUser(user): CurrentUser,
    Path(id): Path<Uuid>,
    Json(req): Json<PutLabels>,
) -> ApiResult<Json<Vec<String>>> {
    let p = Principal::load(&state, Some(user)).await?;
    require_page(&state, &p, id, Need::Edit).await?;
    let mut labels: Vec<String> = req.labels.iter().filter_map(|l| normalize_label(l)).collect();
    labels.sort();
    labels.dedup();
    let mut tx = state.db.begin().await?;
    sqlx::query("DELETE FROM labels WHERE page_id = $1").bind(id).execute(&mut *tx).await?;
    for l in &labels {
        sqlx::query("INSERT INTO labels (page_id, name) VALUES ($1, $2)").bind(id).bind(l).execute(&mut *tx).await?;
    }
    tx.commit().await?;
    state.search.enqueue(id);
    Ok(Json(labels))
}

pub async fn favorite(State(state): State<AppState>, CurrentUser(user): CurrentUser, Path(id): Path<Uuid>) -> ApiResult<StatusCode> {
    let p = Principal::load(&state, Some(user.clone())).await?;
    require_page(&state, &p, id, Need::View).await?;
    sqlx::query("INSERT INTO favorites (user_id, page_id) VALUES ($1, $2) ON CONFLICT DO NOTHING").bind(user.id).bind(id).execute(&state.db).await?;
    Ok(StatusCode::NO_CONTENT)
}

pub async fn unfavorite(State(state): State<AppState>, CurrentUser(user): CurrentUser, Path(id): Path<Uuid>) -> ApiResult<StatusCode> {
    sqlx::query("DELETE FROM favorites WHERE user_id = $1 AND page_id = $2").bind(user.id).bind(id).execute(&state.db).await?;
    Ok(StatusCode::NO_CONTENT)
}

#[derive(Deserialize)]
pub struct ExportQuery {
    format: Option<String>,
}

/// Exports a page as Markdown or standalone HTML.
pub async fn export(State(state): State<AppState>, MaybeUser(user): MaybeUser, Path(id): Path<Uuid>, Query(q): Query<ExportQuery>) -> ApiResult<Response> {
    let p = Principal::load(&state, user).await?;
    require_page(&state, &p, id, Need::View).await?;
    let (title, content): (String, Value) = sqlx::query_as("SELECT title, content FROM pages WHERE id = $1").bind(id).fetch_one(&state.db).await?;
    let doc = match state.rooms.get(id) {
        Some(r) => r.snapshot(),
        None => Node::from_json(&content)?,
    };
    let safe_name: String = title.chars().map(|c| if c.is_alphanumeric() || c == '-' || c == '_' { c } else { '_' }).collect();
    let (body, mime, ext) = match q.format.as_deref().unwrap_or("md") {
        "html" => {
            let inner = html::render(&doc, &html::HtmlOptions { heading_ids: true });
            let page = format!(
                "<!doctype html><html><head><meta charset=\"utf-8\"><title>{t}</title><style>body{{font-family:system-ui,sans-serif;max-width:860px;margin:2rem auto;padding:0 1rem;line-height:1.6}}pre{{background:#f4f4f5;padding:1rem;overflow:auto}}table{{border-collapse:collapse}}td,th{{border:1px solid #ddd;padding:.4rem .6rem}}.callout{{border-left:4px solid #3b82f6;background:#eff6ff;padding:.6rem 1rem;margin:1rem 0}}.callout-warning{{border-color:#f59e0b;background:#fffbeb}}.callout-error{{border-color:#ef4444;background:#fef2f2}}.callout-success{{border-color:#22c55e;background:#f0fdf4}}</style></head><body><h1>{t}</h1>{inner}</body></html>",
                t = html::escape(&title)
            );
            (page, "text/html; charset=utf-8", "html")
        }
        "json" => (serde_json::to_string_pretty(&json!({"title": title, "content": doc}))?, "application/json", "json"),
        _ => (format!("# {title}\n\n{}", markdown::pm_to_markdown(&doc)), "text/markdown; charset=utf-8", "md"),
    };
    Ok(([(header::CONTENT_TYPE, mime.to_string()), (header::CONTENT_DISPOSITION, format!("attachment; filename=\"{safe_name}.{ext}\""))], body).into_response())
}
