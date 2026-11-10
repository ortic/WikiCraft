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
    /// an unpublished draft only its creator sees
    pub draft: bool,
}

/// Inserts a page (and its first version). Returns the id.
pub async fn insert_page<'e, E: PgExecutor<'e> + Copy>(db: E, mut p: NewPage) -> ApiResult<Uuid> {
    wikicraft_doc_model::blocks::ensure_ids(&mut p.content);
    let plain = text::plain_text(&p.content);
    let content = p.content.to_json();
    sqlx::query(
        "INSERT INTO pages (id, workspace_id, space_id, parent_id, kind, title, content, text_content, position, created_by, updated_by, created_at, updated_at, source_ref, draft)
         VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10, $10, COALESCE($11, now()), COALESCE($11, now()), $12, $13)",
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
    .bind(p.draft)
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
    create_version_with(state, page_id, editor, message, false).await
}

/// [`create_version`]; a `minor` version doesn't notify the page's watchers.
pub async fn create_version_with(state: &AppState, page_id: Uuid, editor: Option<Uuid>, message: Option<String>, minor: bool) -> anyhow::Result<Option<i32>> {
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
    sqlx::query("INSERT INTO page_versions (id, page_id, version, title, content, created_by, message, minor) VALUES ($1, $2, $3, $4, $5, $6, $7, $8)")
        .bind(new_id())
        .bind(page_id)
        .bind(next)
        .bind(&title)
        .bind(&content)
        .bind(editor)
        .bind(message)
        .bind(minor)
        .execute(&mut *tx)
        .await?;
    sqlx::query("UPDATE pages SET version = $2 WHERE id = $1").bind(page_id).bind(next).execute(&mut *tx).await?;
    tx.commit().await?;
    if let Some(editor) = editor
        && let Err(e) = crate::routes::notifications::auto_watch(state, page_id, editor).await
    {
        tracing::warn!("auto-watch failed: {e}");
    }
    if !minor && let Err(e) = crate::routes::notifications::notify_page_updated(state, page_id, next, editor).await {
        tracing::warn!("watch notifications failed: {e:#}");
    }
    crate::webhooks::emit_page(state, "page.updated", page_id, editor, json!({"version": next, "message": message_for_hook, "minor": minor})).await;
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
    /// an empty page even when the space has a default template
    #[serde(default)]
    blank: bool,
    /// kind `link`: the external page the tree item points to (http/https)
    url: Option<String>,
    /// an unpublished draft only its creator sees until `POST /pages/{id}/publish`
    #[serde(default)]
    draft: bool,
    /// answers for the template's `{{variables}}`, by name
    #[serde(default)]
    variables: std::collections::HashMap<String, String>,
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
    let kind = req.kind.clone().unwrap_or_else(|| "page".into());
    if !matches!(kind.as_str(), "page" | "folder" | "blog" | "link") {
        return Err(ApiError::bad("kind must be page, folder, blog or link"));
    }
    // a link in the page tree: its content is a single link preview of the URL
    let mut link_title = None;
    let req = if kind == "link" {
        let url = req.url.as_deref().map(str::trim).unwrap_or_default();
        let parsed = openidconnect::url::Url::parse(url).ok().filter(|u| matches!(u.scheme(), "http" | "https") && u.host_str().is_some() && url.len() <= 2000);
        let Some(parsed) = parsed else { return Err(ApiError::bad("a link needs an http(s) URL")) };
        link_title = parsed.host_str().map(|h| h.trim_start_matches("www.").to_string());
        let doc = json!({"type": "doc", "content": [{"type": "embed", "attrs": {"url": parsed.as_str()}}]});
        CreatePage { content: Some(doc), markdown: None, template_id: None, blank: true, ..req }
    } else {
        req
    };
    let mut template_title = None;
    // blueprints file their pages under an index page (unless a parent was picked)
    let blueprint = req
        .template_id
        .as_deref()
        .filter(|_| req.parent_id.is_none() && kind == "page" && req.content.is_none() && req.markdown.is_none())
        .and_then(crate::routes::templates::blueprint);
    let parent_id = match &blueprint {
        Some(bp) => blueprint_index(&state, &p, &space, &user, bp).await?,
        None => req.parent_id,
    };
    // pages start from the space's default template unless asked for something else
    let template = req.template_id.clone().or_else(|| if req.blank || kind != "page" { None } else { space.default_template.clone() });
    let content = match (req.content, req.markdown, &template) {
        (Some(c), _, _) => {
            let mut n = Node::from_json(&c)?;
            wikicraft_doc_model::node::sanitize_tree(&mut n);
            n
        }
        (None, Some(md), _) => markdown::markdown_to_pm(&md).doc,
        (None, None, Some(t)) => {
            let (title, mut doc) = crate::routes::templates::resolve_for(&state, &p, user.workspace_id, t).await?;
            wikicraft_doc_model::template::fill(&mut doc, &req.variables);
            template_title = title.map(|t| wikicraft_doc_model::template::fill_title(&t, &req.variables));
            doc
        }
        _ => Node::empty_doc(),
    };
    let title = req
        .title
        .map(|t| t.trim().to_string())
        .filter(|t| !t.is_empty())
        .or(template_title.map(|t| t.trim().to_string()).filter(|t| !t.is_empty()))
        .or(link_title)
        .unwrap_or_else(|| "Untitled".into());
    let position = next_position(&state, space.id, parent_id).await?;
    let doc = content.clone();
    let draft = req.draft && matches!(kind.as_str(), "page" | "blog");
    let id = insert_page(
        &state.db,
        NewPage {
            id: new_id(),
            workspace_id: user.workspace_id,
            space_id: space.id,
            parent_id,
            kind,
            title,
            content,
            position,
            created_by: Some(user.id),
            created_at: None,
            source_ref: None,
            draft,
        },
    )
    .await?;
    for l in blueprint.iter().flat_map(|bp| bp.labels) {
        sqlx::query("INSERT INTO labels (page_id, name) VALUES ($1, $2) ON CONFLICT DO NOTHING").bind(id).bind(l).execute(&state.db).await?;
    }
    state.search.enqueue(id);
    sqlx::query("INSERT INTO watches (user_id, page_id) VALUES ($1, $2) ON CONFLICT DO NOTHING").bind(user.id).bind(id).execute(&state.db).await?;
    // content created through the API: index its tasks and notify mentioned people right away
    crate::routes::tasks::index_page(&mut *state.db.acquire().await?, id, &doc).await?;
    if !draft {
        announce_new_page(&state, id, &doc, user.id).await;
    }
    Ok((StatusCode::CREATED, Json(json!({ "id": id, "parentId": parent_id }))))
}

/// The index page a blueprint files its pages under: the space's top-level page with that title
/// if the person can see it, else a new one listing its children newest first. None (top level)
/// when the person may see the index but not edit below it.
async fn blueprint_index(
    state: &AppState,
    p: &Principal,
    space: &crate::routes::spaces::SpaceRow,
    user: &crate::auth::User,
    bp: &crate::routes::templates::Blueprint,
) -> ApiResult<Option<Uuid>> {
    let existing: Vec<Uuid> = sqlx::query_scalar(
        "SELECT id FROM pages WHERE space_id = $1 AND parent_id IS NULL AND lower(title) = lower($2) AND kind = 'page'
            AND deleted_at IS NULL AND archived_at IS NULL AND NOT draft ORDER BY created_at",
    )
    .bind(space.id)
    .bind(bp.index_title)
    .fetch_all(&state.db)
    .await?;
    for id in &existing {
        if require_page(state, p, *id, Need::Edit).await.is_ok() {
            return Ok(Some(*id));
        }
    }
    if !existing.is_empty() {
        return Ok(None);
    }
    let content = Node::doc(vec![Node::paragraph(vec![Node::text(bp.intro)]), Node::new(wikicraft_doc_model::nodes::CHILD_PAGES).attr("reverse", true)]);
    let id = insert_page(
        &state.db,
        NewPage {
            id: new_id(),
            workspace_id: user.workspace_id,
            space_id: space.id,
            parent_id: None,
            kind: "page".into(),
            title: bp.index_title.into(),
            content,
            position: next_position(state, space.id, None).await?,
            created_by: Some(user.id),
            created_at: None,
            source_ref: None,
            draft: false,
        },
    )
    .await?;
    state.search.enqueue(id);
    Ok(Some(id))
}

/// Mentions, watchers and webhooks learn about a page once others can see it: at creation, or
/// when its draft is published.
async fn announce_new_page(state: &AppState, id: Uuid, doc: &Node, actor: Uuid) {
    if let Err(e) = crate::routes::notifications::notify_page_mentions(state, id, doc, Some(actor)).await {
        tracing::warn!(page = %id, "mention notifications failed: {e:#}");
    }
    crate::webhooks::emit_page(state, "page.created", id, Some(actor), json!({})).await;
    if let Err(e) = crate::routes::notifications::notify_page_created(state, id, actor).await {
        tracing::warn!("space watch notifications: {e}");
    }
}

/// `POST /pages/{id}/publish` — makes an unpublished draft visible to everyone who can see its
/// place in the space (its creator only).
pub async fn publish(State(state): State<AppState>, CurrentUser(user): CurrentUser, Path(id): Path<Uuid>) -> ApiResult<StatusCode> {
    let p = Principal::load(&state, Some(user.clone())).await?;
    require_page(&state, &p, id, Need::Edit).await?;
    if !publish_draft(&state, id, &user).await? {
        return Err(ApiError::bad("this page isn't your unpublished draft"));
    }
    Ok(StatusCode::NO_CONTENT)
}

/// Publishes `user`'s draft: others see it from now on, mentioned people, watchers and webhooks
/// are told, and a "Published" version is made. False when it isn't their draft (any more).
async fn publish_draft(state: &AppState, id: Uuid, user: &crate::auth::User) -> ApiResult<bool> {
    let published =
        sqlx::query("UPDATE pages SET draft = false, publish_at = NULL, updated_at = now(), updated_by = $2 WHERE id = $1 AND draft AND created_by = $2")
            .bind(id)
            .bind(user.id)
            .execute(&state.db)
            .await?;
    if published.rows_affected() == 0 {
        return Ok(false);
    }
    // the content as it is now, from the live session if one is open
    if let Some(room) = state.rooms.get(id) {
        crate::collab::persist_with(state, &room, false).await.map_err(ApiError::Internal)?;
    }
    if let Err(e) = create_version(state, id, Some(user.id), Some("Published".into())).await {
        tracing::warn!(page = %id, "version on publish failed: {e:#}");
    }
    let content: Value = sqlx::query_scalar("SELECT content FROM pages WHERE id = $1").bind(id).fetch_one(&state.db).await?;
    let doc = Node::from_json(&content).unwrap_or_else(|_| Node::empty_doc());
    state.search.enqueue(id);
    announce_new_page(state, id, &doc, user.id).await;
    audit::log(state, user.workspace_id, Some(user.id), "page.publish", Some("page"), Some(id), json!({})).await;
    Ok(true)
}

#[derive(Deserialize)]
pub struct Schedule {
    at: chrono::DateTime<chrono::Utc>,
}

/// `PUT /pages/{id}/publish-schedule` — the draft publishes itself at `at` (its creator only).
pub async fn schedule_publish(
    State(state): State<AppState>,
    CurrentUser(user): CurrentUser,
    Path(id): Path<Uuid>,
    Json(req): Json<Schedule>,
) -> ApiResult<Json<Value>> {
    let p = Principal::load(&state, Some(user.clone())).await?;
    require_page(&state, &p, id, Need::Edit).await?;
    let now = chrono::Utc::now();
    if req.at <= now + chrono::Duration::seconds(30) {
        return Err(ApiError::bad("pick a time in the future, or publish now"));
    }
    if req.at > now + chrono::Duration::days(366) {
        return Err(ApiError::bad("schedules go at most a year ahead"));
    }
    let set = sqlx::query("UPDATE pages SET publish_at = $3 WHERE id = $1 AND draft AND created_by = $2 AND deleted_at IS NULL")
        .bind(id)
        .bind(user.id)
        .bind(req.at)
        .execute(&state.db)
        .await?;
    if set.rows_affected() == 0 {
        return Err(ApiError::bad("only your own unpublished drafts can be scheduled"));
    }
    audit::log(&state, user.workspace_id, Some(user.id), "page.schedule_publish", Some("page"), Some(id), json!({"at": req.at})).await;
    Ok(Json(json!({ "publishAt": req.at })))
}

/// `DELETE /pages/{id}/publish-schedule` — the draft stays a draft.
pub async fn unschedule_publish(State(state): State<AppState>, CurrentUser(user): CurrentUser, Path(id): Path<Uuid>) -> ApiResult<StatusCode> {
    sqlx::query("UPDATE pages SET publish_at = NULL WHERE id = $1 AND draft AND created_by = $2").bind(id).bind(user.id).execute(&state.db).await?;
    Ok(StatusCode::NO_CONTENT)
}

/// Publishes drafts whose time has come, on behalf of their creators (if they can still edit
/// them). Each due draft is claimed by one replica. Returns how many were published.
pub async fn publish_scheduled(state: &AppState) -> anyhow::Result<usize> {
    let due: Vec<(Uuid, Uuid)> = sqlx::query_as(
        "UPDATE pages SET publish_at = NULL WHERE id IN (
            SELECT id FROM pages WHERE draft AND publish_at <= now() AND deleted_at IS NULL ORDER BY publish_at LIMIT 50 FOR UPDATE SKIP LOCKED)
         RETURNING id, created_by",
    )
    .fetch_all(&state.db)
    .await?;
    let mut published = 0;
    for (id, creator) in due {
        let user: Option<crate::auth::User> =
            sqlx::query_as(sqlx::AssertSqlSafe(format!("SELECT {} FROM users WHERE id = $1 AND status = 'active'", crate::auth::USER_COLUMNS)))
                .bind(creator)
                .fetch_optional(&state.db)
                .await?;
        let Some(user) = user else {
            tracing::warn!(page = %id, "scheduled publishing skipped: the author's account isn't active");
            continue;
        };
        let p = Principal::load(state, Some(user.clone())).await.map_err(|e| anyhow::anyhow!("{e}"))?;
        if require_page(state, &p, id, Need::Edit).await.is_err() {
            tracing::warn!(page = %id, "scheduled publishing skipped: the author can't edit the page any more");
            continue;
        }
        match publish_draft(state, id, &user).await {
            Ok(true) => published += 1,
            Ok(false) => {}
            Err(e) => tracing::warn!(page = %id, "scheduled publishing failed: {e}"),
        }
    }
    Ok(published)
}

#[derive(Serialize, sqlx::FromRow)]
#[serde(rename_all = "camelCase")]
pub struct DraftRow {
    id: Uuid,
    title: String,
    space_key: String,
    space_name: String,
    updated_at: chrono::DateTime<chrono::Utc>,
    /// when it publishes itself
    publish_at: Option<chrono::DateTime<chrono::Utc>>,
}

/// `GET /me/drafts` — my unpublished drafts, last edited first.
pub async fn my_drafts(State(state): State<AppState>, CurrentUser(user): CurrentUser) -> ApiResult<Json<Vec<DraftRow>>> {
    Ok(Json(
        sqlx::query_as::<_, DraftRow>(
            "SELECT p.id, p.title, s.key AS space_key, s.name AS space_name, p.updated_at, p.publish_at FROM pages p JOIN spaces s ON s.id = p.space_id
              WHERE p.draft AND p.created_by = $1 AND p.deleted_at IS NULL ORDER BY p.updated_at DESC LIMIT 100",
        )
        .bind(user.id)
        .fetch_all(&state.db)
        .await?,
    ))
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
    pub cover: Option<String>,
    pub content: Value,
    pub version: i32,
    pub status: String,
    pub page_status: Option<String>,
    pub full_width: bool,
    /// an unpublished draft (only its creator sees it)
    #[sqlx(default)]
    pub draft: bool,
    /// when the draft publishes itself
    #[sqlx(default)]
    pub publish_at: Option<chrono::DateTime<chrono::Utc>>,
    pub created_by: Option<Uuid>,
    pub created_by_name: Option<String>,
    /// responsible person: the owner if set, else the creator
    #[sqlx(default)]
    pub owner_id: Option<Uuid>,
    #[sqlx(default)]
    pub owner_name: Option<String>,
    pub created_at: chrono::DateTime<chrono::Utc>,
    pub updated_by: Option<Uuid>,
    pub updated_by_name: Option<String>,
    pub updated_at: chrono::DateTime<chrono::Utc>,
    pub deleted_at: Option<chrono::DateTime<chrono::Utc>>,
}

const PAGE_SELECT: &str = "SELECT p.id, p.space_id, s.key AS space_key, s.name AS space_name, p.parent_id, p.kind, p.title, p.icon, p.cover, p.content, p.version, p.status, p.page_status, p.full_width, p.draft, p.publish_at,
    p.created_by, cu.name AS created_by_name, COALESCE(p.owner_id, p.created_by) AS owner_id, COALESCE(ou.name, cu.name) AS owner_name,
    p.created_at, p.updated_by, uu.name AS updated_by_name, p.updated_at, p.deleted_at
    FROM pages p JOIN spaces s ON s.id = p.space_id LEFT JOIN users cu ON cu.id = p.created_by LEFT JOIN users uu ON uu.id = p.updated_by
    LEFT JOIN users ou ON ou.id = p.owner_id";

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

/// Records that `user` saw `version` and returns what changed since their previous visit:
/// `{version, at, versions, editors}` when other people saved versions in between.
async fn changed_since(state: &AppState, user: Uuid, page: Uuid, version: i32) -> ApiResult<Value> {
    let prev: Option<(i32, chrono::DateTime<chrono::Utc>)> = sqlx::query_as(
        "WITH old AS (SELECT version, seen_at FROM page_seen WHERE user_id = $1 AND page_id = $2)
         , up AS (INSERT INTO page_seen (user_id, page_id, version, seen_at) VALUES ($1, $2, $3, now())
                  ON CONFLICT (user_id, page_id) DO UPDATE SET version = GREATEST(page_seen.version, $3), seen_at = now())
         SELECT version, seen_at FROM old",
    )
    .bind(user)
    .bind(page)
    .bind(version)
    .fetch_optional(&state.db)
    .await?;
    let Some((seen, at)) = prev.filter(|(v, _)| *v < version) else { return Ok(Value::Null) };
    let (count, editors): (i64, Vec<String>) = sqlx::query_as(
        "SELECT count(*), COALESCE(array_agg(DISTINCT u.name) FILTER (WHERE u.name IS NOT NULL), '{}')
         FROM page_versions v LEFT JOIN users u ON u.id = v.created_by
         WHERE v.page_id = $1 AND v.version > $2 AND v.version <= $3 AND v.created_by IS DISTINCT FROM $4",
    )
    .bind(page)
    .bind(seen)
    .bind(version)
    .bind(user)
    .fetch_one(&state.db)
    .await?;
    if count == 0 {
        return Ok(Value::Null);
    }
    Ok(json!({ "version": seen, "at": at, "versions": count, "editors": editors }))
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
    // what changed since this reader's last visit (versions by other people)
    let changed_since = match &user {
        Some(u) => changed_since(&state, u.id, id, page.version).await?,
        None => Value::Null,
    };
    // watching it directly (maybe with the pages below), or through a page above it
    let (watching, watching_children, watched_through): (bool, bool, Option<Value>) = match &user {
        Some(u) => {
            let direct: Option<bool> = sqlx::query_scalar("SELECT include_children FROM watches WHERE user_id = $1 AND page_id = $2")
                .bind(u.id)
                .bind(id)
                .fetch_optional(&state.db)
                .await?;
            let through: Option<(Uuid, String)> = sqlx::query_as(
                "WITH RECURSIVE up AS (
                    SELECT parent_id AS id, 1 AS depth FROM pages WHERE id = $2 AND parent_id IS NOT NULL
                    UNION ALL SELECT p.parent_id, up.depth + 1 FROM pages p JOIN up ON p.id = up.id WHERE p.parent_id IS NOT NULL AND up.depth < 200)
                 SELECT p.id, p.title FROM up JOIN watches w ON w.page_id = up.id AND w.user_id = $1 AND w.include_children
                 JOIN pages p ON p.id = up.id ORDER BY up.depth LIMIT 1",
            )
            .bind(u.id)
            .bind(id)
            .fetch_optional(&state.db)
            .await?;
            (direct.is_some(), direct.unwrap_or(false), through.map(|(id, title)| json!({"id": id, "title": title})))
        }
        None => (false, false, None),
    };
    let mut v = serde_json::to_value(&page)?;
    let o = v.as_object_mut().expect("object");
    o.insert("breadcrumbs".into(), serde_json::to_value(crumbs)?);
    o.insert("labels".into(), json!(labels));
    o.insert("changedSince".into(), changed_since);
    o.insert("access".into(), serde_json::to_value(access)?);
    o.insert("restricted".into(), json!(restricted));
    o.insert("favorite".into(), json!(favorite));
    o.insert("watching".into(), json!(watching));
    o.insert("watchingChildren".into(), json!(watching_children));
    o.insert("watchedThrough".into(), json!(watched_through));
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
    cover: Option<String>,
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
    // icons are symbols of our icon set ("name" or "name:color"); "" removes
    if let Some(i) = req.icon.as_deref().filter(|i| !i.is_empty()) {
        let ok = i.len() <= 40 && i.split(':').count() <= 2 && i.chars().all(|c| c.is_ascii_lowercase() || c == '-' || c == ':');
        if !ok {
            return Err(ApiError::bad("icon must be a symbol name like \"rocket\" or \"rocket:violet\""));
        }
    }
    if let Some(c) = req.cover.as_deref().filter(|c| !c.is_empty())
        && !valid_cover(c)
    {
        return Err(ApiError::bad("cover must be a gradient preset, an attachment or an https image URL"));
    }
    sqlx::query(
        "UPDATE pages SET title = COALESCE($2, title), icon = CASE WHEN $3::text IS NULL THEN icon WHEN $3 = '' THEN NULL ELSE $3 END,
         full_width = COALESCE($4, full_width), page_status = CASE WHEN $5::text IS NULL THEN page_status WHEN $5 = '' THEN NULL ELSE $5 END,
         cover = CASE WHEN $7::text IS NULL THEN cover WHEN $7 = '' THEN NULL ELSE $7 END,
         updated_at = now(), updated_by = $6 WHERE id = $1",
    )
    .bind(id)
    .bind(title)
    .bind(req.icon)
    .bind(req.full_width)
    .bind(req.page_status)
    .bind(user.id)
    .bind(req.cover)
    .execute(&state.db)
    .await?;
    state.search.enqueue(id);
    Ok(StatusCode::NO_CONTENT)
}

pub const COVER_GRADIENTS: &[&str] = &["sunrise", "ocean", "forest", "dusk", "slate", "mint"];

/// `gradient:<preset>`, `/api/attachments/<uuid>/content` or a plain https URL.
pub fn valid_cover(c: &str) -> bool {
    if let Some(g) = c.strip_prefix("gradient:") {
        return COVER_GRADIENTS.contains(&g);
    }
    if let Some(rest) = c.strip_prefix("/api/attachments/") {
        return rest.strip_suffix("/content").is_some_and(|id| id.parse::<Uuid>().is_ok());
    }
    c.len() <= 2000 && c.starts_with("https://") && !c.chars().any(|ch| ch.is_whitespace() || matches!(ch, '"' | '\'' | '<' | '>' | '\\' | '(' | ')'))
}

#[derive(Deserialize)]
pub struct PutContent {
    content: Option<Value>,
    markdown: Option<String>,
    message: Option<String>,
    /// a minor change: watchers aren't notified
    #[serde(default)]
    minor: bool,
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
    // this version takes the session's changes so far: minor if asked, or if everybody said so
    let minor = room.take_minor() || req.minor;
    let v = create_version_with(&state, id, Some(user.id), req.message, minor).await?;
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
                draft: false,
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

/// `POST /pages/{id}/purge` — deletes a page that is in the trash for good, with its trashed
/// sub-pages, history, comments and attachments (space admins).
pub async fn purge(State(state): State<AppState>, CurrentUser(user): CurrentUser, Path(id): Path<Uuid>) -> ApiResult<Json<Value>> {
    let p = Principal::load(&state, Some(user.clone())).await?;
    let row: Option<(Uuid, String, Option<chrono::DateTime<chrono::Utc>>)> =
        sqlx::query_as("SELECT space_id, title, deleted_at FROM pages WHERE id = $1 AND workspace_id = $2")
            .bind(id)
            .bind(user.workspace_id)
            .fetch_optional(&state.db)
            .await?;
    let Some((space_id, title, Some(_))) = row else { return Err(ApiError::NotFound("deleted page")) };
    let sa = crate::authz::load_space_access(&state, space_id).await?;
    let role = crate::authz::space_role(&state, &p, &sa).await?;
    if role < Role::Admin {
        return Err(ApiError::Forbidden);
    }
    let pairs: Vec<(Uuid, Option<Uuid>)> = sqlx::query_as("SELECT id, parent_id FROM pages WHERE space_id = $1").bind(space_id).fetch_all(&state.db).await?;
    if !crate::authz::visible_page_ids(&state, &p, space_id, role, &pairs).await?.contains(&id) {
        return Err(ApiError::NotFound("deleted page"));
    }
    let gone = crate::purge::purge_pages(&state, &[id]).await?;
    audit::log(&state, user.workspace_id, Some(user.id), "page.purge", Some("page"), Some(id), json!({"title": title, "count": gone.len()})).await;
    Ok(Json(json!({ "purged": gone.len() })))
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
    /// watchers weren't notified about it
    minor: bool,
}

pub async fn versions(State(state): State<AppState>, MaybeUser(user): MaybeUser, Path(id): Path<Uuid>) -> ApiResult<Json<Vec<VersionRow>>> {
    let p = Principal::load(&state, user).await?;
    require_page(&state, &p, id, Need::View).await?;
    let rows = sqlx::query_as::<_, VersionRow>(
        "SELECT v.version, v.title, v.created_by, u.name AS created_by_name, v.created_at, v.message, v.minor FROM page_versions v LEFT JOIN users u ON u.id = v.created_by WHERE v.page_id = $1 ORDER BY v.version DESC",
    )
    .bind(id)
    .fetch_all(&state.db)
    .await?;
    Ok(Json(rows))
}

#[derive(Deserialize)]
pub struct SaveVersion {
    message: Option<String>,
    /// don't notify watchers about this version
    #[serde(default)]
    minor: bool,
}

pub async fn save_version(
    State(state): State<AppState>,
    CurrentUser(user): CurrentUser,
    Path(id): Path<Uuid>,
    Json(req): Json<SaveVersion>,
) -> ApiResult<Json<Value>> {
    let p = Principal::load(&state, Some(user.clone())).await?;
    require_page(&state, &p, id, Need::Edit).await?;
    let mut minor = req.minor;
    if let Some(room) = state.rooms.get(id) {
        crate::collab::persist_with(&state, &room, false).await?;
        // the session's changes so far go into this version: minor if everybody said so
        minor |= room.take_minor();
    }
    let v = create_version_with(&state, id, Some(user.id), req.message.filter(|m| !m.trim().is_empty()), minor).await?;
    Ok(Json(json!({ "version": v })))
}

#[derive(Deserialize)]
pub struct MinorEdit {
    minor: bool,
}

/// `GET /pages/{id}/minor-edit` — whether my changes in the open editing session are minor.
pub async fn get_minor_edit(State(state): State<AppState>, CurrentUser(user): CurrentUser, Path(id): Path<Uuid>) -> ApiResult<Json<Value>> {
    let p = Principal::load(&state, Some(user.clone())).await?;
    require_page(&state, &p, id, Need::Edit).await?;
    let minor = state.rooms.get(id).is_some_and(|r| r.is_minor(user.id));
    Ok(Json(json!({ "minor": minor })))
}

/// `PUT /pages/{id}/minor-edit` — marks my changes in the open editing session as minor (or
/// not): when everybody who edited says so, the session's version doesn't notify watchers.
pub async fn set_minor_edit(
    State(state): State<AppState>,
    CurrentUser(user): CurrentUser,
    Path(id): Path<Uuid>,
    Json(req): Json<MinorEdit>,
) -> ApiResult<Json<Value>> {
    let p = Principal::load(&state, Some(user.clone())).await?;
    require_page(&state, &p, id, Need::Edit).await?;
    let room = state.rooms.get(id).ok_or_else(|| ApiError::Conflict("the page isn't open for editing".into()))?;
    room.set_minor(user.id, req.minor);
    Ok(Json(json!({ "minor": req.minor })))
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
    /// Word only: include every descendant page the reader can see
    tree: Option<bool>,
}

/// Exports a page as Markdown, standalone HTML, Word (`docx`) or JSON.
pub async fn export(State(state): State<AppState>, MaybeUser(user): MaybeUser, Path(id): Path<Uuid>, Query(q): Query<ExportQuery>) -> ApiResult<Response> {
    let p = Principal::load(&state, user).await?;
    require_page(&state, &p, id, Need::View).await?;
    let ws: Uuid =
        sqlx::query_scalar("SELECT s.workspace_id FROM pages p JOIN spaces s ON s.id = p.space_id WHERE p.id = $1").bind(id).fetch_one(&state.db).await?;
    crate::routes::share::require_exports(&state, ws, p.user.as_ref()).await?;
    let (title, content): (String, Value) = sqlx::query_as("SELECT title, content FROM pages WHERE id = $1").bind(id).fetch_one(&state.db).await?;
    let doc = match state.rooms.get(id) {
        Some(r) => r.snapshot(),
        None => Node::from_json(&content)?,
    };
    let safe_name: String = title.chars().map(|c| if c.is_alphanumeric() || c == '-' || c == '_' { c } else { '_' }).collect();
    if q.format.as_deref() == Some("docx") {
        let body = export_docx(&state, &p, id, title, doc, q.tree.unwrap_or(false)).await?;
        let mime = "application/vnd.openxmlformats-officedocument.wordprocessingml.document";
        return Ok(([(header::CONTENT_TYPE, mime.to_string()), (header::CONTENT_DISPOSITION, format!("attachment; filename=\"{safe_name}.docx\""))], body)
            .into_response());
    }
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

/// One page of a Word export, with everything the renderer needs.
struct DocxPage {
    id: Uuid,
    title: String,
    subtitle: String,
    doc: Node,
    children: Vec<(String, String)>,
    macros: Vec<Option<crate::routes::reports::Snapshot>>,
}

/// Word exports are memory-heavy: at most two at a time per replica.
static DOCX_EXPORTS: tokio::sync::Semaphore = tokio::sync::Semaphore::const_new(2);
/// report macros evaluated per export (a page tree shares the budget)
const DOCX_MACRO_BUDGET: usize = 40;

async fn docx_page(state: &AppState, p: &Principal, id: Uuid, title: String, doc: Node, macro_budget: &mut usize) -> ApiResult<DocxPage> {
    let (space, space_key, updated_at, updated_by): (String, String, chrono::DateTime<chrono::Utc>, Option<String>) = sqlx::query_as(
        "SELECT s.name, s.key, p.updated_at, u.name FROM pages p JOIN spaces s ON s.id = p.space_id LEFT JOIN users u ON u.id = p.updated_by WHERE p.id = $1",
    )
    .bind(id)
    .fetch_one(&state.db)
    .await?;
    let base = state.config.base_url.trim_end_matches('/').to_string();
    // children macro: the child pages the reader can see
    let mut children = Vec::new();
    let mut has_children_macro = false;
    doc.walk(&mut |n| has_children_macro |= n.kind == wikicraft_doc_model::nodes::CHILD_PAGES);
    if has_children_macro {
        let rows: Vec<(Uuid, String)> =
            sqlx::query_as("SELECT id, title FROM pages WHERE parent_id = $1 AND deleted_at IS NULL ORDER BY position, lower(title) LIMIT 200")
                .bind(id)
                .fetch_all(&state.db)
                .await?;
        for (cid, ctitle) in rows {
            if require_page(state, p, cid, Need::View).await.is_ok() {
                children.push((ctitle, format!("{base}/pages/{cid}")));
            }
        }
    }
    // live report macros are evaluated now, for this reader
    let mut found: Vec<(String, serde_json::Map<String, Value>)> = Vec::new();
    doc.walk(&mut |n| {
        if n.kind == wikicraft_doc_model::nodes::MACRO {
            let params = n.attrs.get("params").and_then(|v| v.as_object()).cloned().unwrap_or_default();
            found.push((n.attr_str("name").unwrap_or("").to_string(), params));
        }
    });
    let mut macros = Vec::with_capacity(found.len());
    for (name, params) in found.iter() {
        let snap = if *macro_budget > 0 {
            *macro_budget -= 1;
            crate::routes::reports::snapshot(state, p, name, params, id, &space_key, &base).await?
        } else {
            None
        };
        macros.push(snap);
    }
    let by = updated_by.map(|n| format!(" by {n}")).unwrap_or_default();
    let subtitle = format!("{space} \u{00B7} Last updated {}{by}", updated_at.format("%-d %b %Y"));
    Ok(DocxPage { id, title, subtitle, doc, children, macros })
}

/// The page, or with `tree` the page and every descendant the reader can see (depth first,
/// at most 200 pages), as one Word document.
async fn export_docx(state: &AppState, p: &Principal, id: Uuid, title: String, doc: Node, tree: bool) -> ApiResult<Vec<u8>> {
    if tree && p.user.is_none() {
        return Err(ApiError::Unauthorized);
    }
    let _slot = DOCX_EXPORTS.acquire().await.map_err(|e| ApiError::Internal(e.into()))?;
    let mut budget = DOCX_MACRO_BUDGET;
    let mut pages = vec![docx_page(state, p, id, title, doc, &mut budget).await?];
    if tree {
        let rows: Vec<(Uuid, String, Value)> = sqlx::query_as(
            "WITH RECURSIVE t AS (
                SELECT id, 0 AS depth, ARRAY[]::float8[] AS path, ARRAY[]::text[] AS names FROM pages WHERE id = $1
                UNION ALL
                SELECT c.id, t.depth + 1, t.path || c.position, t.names || lower(c.title)
                FROM pages c JOIN t ON c.parent_id = t.id WHERE c.deleted_at IS NULL AND t.depth < 30)
             SELECT p.id, p.title, p.content FROM t JOIN pages p ON p.id = t.id WHERE t.depth > 0 ORDER BY t.path, t.names LIMIT 400",
        )
        .bind(id)
        .fetch_all(&state.db)
        .await?;
        for (cid, ctitle, content) in rows {
            if pages.len() >= 200 {
                break;
            }
            if require_page(state, p, cid, Need::View).await.is_err() {
                continue;
            }
            let cdoc = match state.rooms.get(cid) {
                Some(r) => r.snapshot(),
                None => Node::from_json(&content).unwrap_or_else(|_| Node::empty_doc()),
            };
            pages.push(docx_page(state, p, cid, ctitle, cdoc, &mut budget).await?);
        }
    }
    // one image map for all pages (each image checked against the reader)
    let mut images = crate::docx::Images::new();
    for pg in &pages {
        crate::docx::load_images(state, p, &pg.doc, &mut images).await;
    }
    let base = state.config.base_url.trim_end_matches('/').to_string();
    let bytes = tokio::task::spawn_blocking(move || {
        let inputs: Vec<crate::docx::DocxInput> = pages
            .iter()
            .map(|pg| crate::docx::DocxInput {
                title: &pg.title,
                subtitle: Some(pg.subtitle.clone()),
                doc: &pg.doc,
                images: &images,
                base_url: &base,
                children: &pg.children,
                macros: &pg.macros,
                page_id: Some(pg.id),
            })
            .collect();
        crate::docx::render_many(&inputs)
    })
    .await
    .map_err(|e| ApiError::Internal(e.into()))?
    .map_err(ApiError::Internal)?;
    Ok(bytes)
}

#[derive(Deserialize)]
pub struct MarkdownReq {
    markdown: String,
}

/// `POST /api/markdown`: Markdown → document content (used when pasting Markdown text).
pub async fn markdown_to_doc(CurrentUser(_user): CurrentUser, Json(req): Json<MarkdownReq>) -> ApiResult<Json<Value>> {
    if req.markdown.len() > 1_000_000 {
        return Err(ApiError::bad("Markdown larger than 1 MB"));
    }
    let doc = tokio::task::spawn_blocking(move || markdown::markdown_to_pm(&req.markdown).doc).await.map_err(|e| ApiError::Internal(e.into()))?;
    Ok(Json(json!({ "content": doc })))
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct OwnerReq {
    user_id: Uuid,
}

/// `PUT /api/pages/{id}/owner {userId}`: hands the page over (page admins, or its owner).
/// The new owner must be able to edit the page.
pub async fn set_owner(
    State(state): State<AppState>,
    CurrentUser(user): CurrentUser,
    Path(id): Path<Uuid>,
    Json(req): Json<OwnerReq>,
) -> ApiResult<Json<Value>> {
    let p = Principal::load(&state, Some(user.clone())).await?;
    let (access, _) = require_page(&state, &p, id, Need::View).await?;
    let owner: Option<Uuid> = sqlx::query_scalar("SELECT COALESCE(owner_id, created_by) FROM pages WHERE id = $1").bind(id).fetch_one(&state.db).await?;
    if !access.admin && owner != Some(user.id) {
        return Err(ApiError::Forbidden);
    }
    let new_owner = sqlx::query_as::<_, crate::auth::User>(sqlx::AssertSqlSafe(format!(
        "SELECT {} FROM users WHERE id = $1 AND workspace_id = $2 AND status = 'active'",
        crate::auth::USER_COLUMNS
    )))
    .bind(req.user_id)
    .bind(user.workspace_id)
    .fetch_optional(&state.db)
    .await?
    .ok_or(ApiError::NotFound("user"))?;
    let np = Principal::load(&state, Some(new_owner.clone())).await?;
    if !crate::authz::page_access(&state, &np, id).await.map(|(a, _)| a.edit).unwrap_or(false) {
        return Err(ApiError::bad("the new owner must be able to edit the page"));
    }
    sqlx::query("UPDATE pages SET owner_id = $2 WHERE id = $1").bind(id).bind(new_owner.id).execute(&state.db).await?;
    audit::log(&state, user.workspace_id, Some(user.id), "page.owner", Some("page"), Some(id), json!({"owner": new_owner.id})).await;
    Ok(Json(json!({ "ownerId": new_owner.id, "ownerName": new_owner.name })))
}

#[derive(Serialize, sqlx::FromRow)]
#[serde(rename_all = "camelCase")]
pub struct ViewerRow {
    user_id: Uuid,
    name: String,
    last_viewed: chrono::NaiveDate,
    views: i64,
}

/// `GET /pages/{id}/viewers` — who read the page and when (people who can edit it, when an admin
/// turned the viewer list on).
pub async fn viewers(State(state): State<AppState>, CurrentUser(user): CurrentUser, Path(id): Path<Uuid>) -> ApiResult<Json<Vec<ViewerRow>>> {
    if !crate::routes::share::viewer_list_on(&state, user.workspace_id).await? {
        return Err(ApiError::Forbidden);
    }
    let p = Principal::load(&state, Some(user)).await?;
    require_page(&state, &p, id, Need::Edit).await?;
    Ok(Json(
        sqlx::query_as::<_, ViewerRow>(
            "SELECT v.user_id, u.name, max(v.day) AS last_viewed, sum(v.views)::bigint AS views
               FROM page_views v JOIN users u ON u.id = v.user_id
              WHERE v.page_id = $1 AND u.status <> 'placeholder'
              GROUP BY v.user_id, u.name ORDER BY max(v.day) DESC, lower(u.name) LIMIT 200",
        )
        .bind(id)
        .fetch_all(&state.db)
        .await?,
    ))
}
