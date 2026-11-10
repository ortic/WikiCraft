//! Public share links: anyone with the link can read a page (and optionally its sub-pages)
//! without an account. Links can expire, be revoked, and be switched off workspace-wide.
//! Sub-pages with their own view restrictions — and everything below them — are never
//! included. Public pages are rendered to HTML on the server; images and attachments are
//! served through the link.

use std::collections::HashSet;

use axum::Json;
use axum::extract::{Path, State};
use axum::http::StatusCode;
use axum::response::Response;
use serde::Deserialize;
use serde_json::{Value, json};
use uuid::Uuid;
use wikicraft_doc_model::{Node, html, nodes};

use crate::app::{AppState, new_id};
use crate::audit;
use crate::auth::{CurrentUser, random_token, require_admin};
use crate::authz::{Need, Principal, require_page};
use crate::error::{ApiError, ApiResult};

/// Whether the page or one of its ancestors has a view restriction.
async fn restricted(state: &AppState, page: Uuid) -> ApiResult<bool> {
    Ok(sqlx::query_scalar(
        "WITH RECURSIVE anc AS (SELECT id, parent_id, 0 AS depth FROM pages WHERE id = $1
                                UNION ALL SELECT p.id, p.parent_id, anc.depth + 1 FROM pages p JOIN anc ON p.id = anc.parent_id WHERE anc.depth < 200)
         SELECT EXISTS (SELECT 1 FROM page_restrictions r JOIN anc ON r.page_id = anc.id WHERE r.kind = 'view')",
    )
    .bind(page)
    .fetch_one(&state.db)
    .await?)
}

/// Workspace-wide switches an admin can turn off (all on by default), stored in
/// `workspaces.settings`.
pub const SWITCHES: [&str; 4] = ["publicLinks", "exports", "ai", "personalSpaces"];

/// Whether the workspace switch `key` (one of [`SWITCHES`]) is on.
pub async fn switch_on(state: &AppState, ws: Uuid, key: &str) -> ApiResult<bool> {
    debug_assert!(SWITCHES.contains(&key));
    let v: Option<bool> =
        sqlx::query_scalar("SELECT (settings->>$2)::boolean FROM workspaces WHERE id = $1").bind(ws).bind(key).fetch_optional(&state.db).await?.flatten();
    Ok(v.unwrap_or(true))
}

/// Whether editors may see who viewed a page (off unless an admin turns it on: it's about
/// people's reading habits).
pub async fn viewer_list_on(state: &AppState, ws: Uuid) -> ApiResult<bool> {
    let v: Option<bool> =
        sqlx::query_scalar("SELECT (settings->>'viewerList')::boolean FROM workspaces WHERE id = $1").bind(ws).fetch_optional(&state.db).await?.flatten();
    Ok(v.unwrap_or(false))
}

async fn links_enabled(state: &AppState, ws: Uuid) -> ApiResult<bool> {
    switch_on(state, ws, "publicLinks").await
}

#[derive(serde::Serialize, sqlx::FromRow)]
#[serde(rename_all = "camelCase")]
pub struct LinkRow {
    id: Uuid,
    token: String,
    include_children: bool,
    created_by_name: Option<String>,
    created_at: chrono::DateTime<chrono::Utc>,
    expires_at: Option<chrono::DateTime<chrono::Utc>>,
    views: i64,
    last_viewed_at: Option<chrono::DateTime<chrono::Utc>>,
    /// visitors need a password
    has_password: bool,
}

/// `GET /api/pages/{id}/share-links` (editors of the page)
pub async fn list(State(state): State<AppState>, CurrentUser(user): CurrentUser, Path(page): Path<Uuid>) -> ApiResult<Json<Value>> {
    let p = Principal::load(&state, Some(user.clone())).await?;
    require_page(&state, &p, page, Need::Edit).await?;
    let links = sqlx::query_as::<_, LinkRow>(
        "SELECT l.id, l.token, l.include_children, u.name AS created_by_name, l.created_at, l.expires_at, l.views, l.last_viewed_at,
                l.password_hash IS NOT NULL AS has_password
         FROM share_links l LEFT JOIN users u ON u.id = l.created_by
         WHERE l.page_id = $1 AND l.revoked_at IS NULL AND (l.expires_at IS NULL OR l.expires_at > now()) ORDER BY l.created_at DESC",
    )
    .bind(page)
    .fetch_all(&state.db)
    .await?;
    Ok(Json(json!({ "enabled": links_enabled(&state, user.workspace_id).await?, "links": links })))
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CreateLink {
    #[serde(default)]
    include_children: bool,
    /// days until the link stops working (none = no expiry)
    expires_in_days: Option<i64>,
    /// visitors have to enter it first (6 to 200 characters)
    password: Option<String>,
}

/// `POST /api/pages/{id}/share-links`
pub async fn create(
    State(state): State<AppState>,
    CurrentUser(user): CurrentUser,
    Path(page): Path<Uuid>,
    Json(req): Json<CreateLink>,
) -> ApiResult<(StatusCode, Json<Value>)> {
    if user.is_guest() {
        return Err(ApiError::Forbidden);
    }
    let p = Principal::load(&state, Some(user.clone())).await?;
    require_page(&state, &p, page, Need::Edit).await?;
    if !links_enabled(&state, user.workspace_id).await? {
        return Err(ApiError::bad("public links are turned off for this workspace"));
    }
    if restricted(&state, page).await? {
        return Err(ApiError::bad("pages with view restrictions can't be shared publicly"));
    }
    if crate::authz::in_draft(&state, page).await {
        return Err(ApiError::bad("publish the draft before sharing it publicly"));
    }
    let password_hash = match req.password.as_deref().filter(|p| !p.is_empty()) {
        Some(p) if !(6..=200).contains(&p.chars().count()) => return Err(ApiError::bad("the password needs 6 to 200 characters")),
        Some(p) => Some(crate::auth::hash_password(p.to_string()).await.map_err(ApiError::Internal)?),
        None => None,
    };
    let id = new_id();
    let token = random_token();
    let days = req.expires_in_days.filter(|d| *d > 0).map(|d| d.min(3650));
    sqlx::query(
        "INSERT INTO share_links (id, token, workspace_id, page_id, include_children, created_by, expires_at, password_hash)
         VALUES ($1, $2, $3, $4, $5, $6, CASE WHEN $7::bigint IS NULL THEN NULL ELSE now() + make_interval(days => $7::int) END, $8)",
    )
    .bind(id)
    .bind(&token)
    .bind(user.workspace_id)
    .bind(page)
    .bind(req.include_children)
    .bind(user.id)
    .bind(days)
    .bind(&password_hash)
    .execute(&state.db)
    .await?;
    audit::log(
        &state,
        user.workspace_id,
        Some(user.id),
        "page.shared_publicly",
        Some("page"),
        Some(page),
        json!({"includeChildren": req.include_children, "expiresInDays": days, "password": password_hash.is_some()}),
    )
    .await;
    Ok((StatusCode::CREATED, Json(json!({ "id": id, "token": token, "url": format!("{}/s/{token}", state.config.base_url.trim_end_matches('/')) }))))
}

/// `DELETE /api/share-links/{id}`
pub async fn revoke(State(state): State<AppState>, CurrentUser(user): CurrentUser, Path(id): Path<Uuid>) -> ApiResult<StatusCode> {
    let page: Uuid = sqlx::query_scalar("SELECT page_id FROM share_links WHERE id = $1 AND workspace_id = $2")
        .bind(id)
        .bind(user.workspace_id)
        .fetch_optional(&state.db)
        .await?
        .ok_or(ApiError::NotFound("link"))?;
    let p = Principal::load(&state, Some(user.clone())).await?;
    require_page(&state, &p, page, Need::Edit).await?;
    sqlx::query("UPDATE share_links SET revoked_at = now() WHERE id = $1").bind(id).execute(&state.db).await?;
    audit::log(&state, user.workspace_id, Some(user.id), "page.share_revoked", Some("page"), Some(page), json!({})).await;
    Ok(StatusCode::NO_CONTENT)
}

async fn switches(state: &AppState, ws: Uuid) -> ApiResult<serde_json::Map<String, Value>> {
    let mut out = serde_json::Map::new();
    for key in SWITCHES {
        out.insert(key.to_string(), Value::Bool(switch_on(state, ws, key).await?));
    }
    Ok(out)
}

/// `GET|PUT /api/admin/settings` — workspace-wide switches (`{publicLinks, exports, ai,
/// personalSpaces}`; a PUT changes the ones it names).
pub async fn get_settings(State(state): State<AppState>, CurrentUser(user): CurrentUser) -> ApiResult<Json<Value>> {
    require_admin(&user)?;
    let mut out = switches(&state, user.workspace_id).await?;
    let days: Option<i32> =
        sqlx::query_scalar("SELECT (settings->>'trashRetentionDays')::int FROM workspaces WHERE id = $1").bind(user.workspace_id).fetch_one(&state.db).await?;
    out.insert("trashRetentionDays".into(), json!(days.unwrap_or(0)));
    out.insert("viewerList".into(), json!(viewer_list_on(&state, user.workspace_id).await?));
    let mfa: Option<String> =
        sqlx::query_scalar("SELECT settings->>'requireTwoFactor' FROM workspaces WHERE id = $1").bind(user.workspace_id).fetch_one(&state.db).await?;
    out.insert("requireTwoFactor".into(), json!(mfa.unwrap_or_else(|| "off".into())));
    out.insert("drawioUrl".into(), json!(drawio_url(&state, user.workspace_id).await?));
    out.insert("attachments".into(), json!(crate::routes::attachments::upload_policy(&state, user.workspace_id).await?));
    out.insert("serverMaxUploadMb".into(), json!(state.config.max_upload_mb));
    out.insert("attachmentStorageBytes".into(), json!(crate::routes::attachments::storage_used(&state, user.workspace_id, None).await?));
    Ok(Json(Value::Object(out)))
}

/// The draw.io editor this wiki uses for diagrams (embed mode): diagrams.net's unless an admin
/// set a self-hosted one; None when they turned diagrams off (empty setting).
pub const DEFAULT_DRAWIO_URL: &str = "https://embed.diagrams.net";

pub async fn drawio_url(state: &AppState, ws: Uuid) -> ApiResult<Option<String>> {
    let set: Option<String> =
        sqlx::query_scalar("SELECT settings->>'drawioUrl' FROM workspaces WHERE id = $1").bind(ws).fetch_optional(&state.db).await?.flatten();
    Ok(match set {
        None => Some(DEFAULT_DRAWIO_URL.into()),
        Some(s) if s.is_empty() => None,
        Some(s) => Some(s),
    })
}

pub async fn put_settings(
    State(state): State<AppState>,
    CurrentUser(user): CurrentUser,
    Json(req): Json<serde_json::Map<String, Value>>,
) -> ApiResult<Json<Value>> {
    require_admin(&user)?;
    for (key, v) in &req {
        if key == "requireTwoFactor" {
            // who must use two-factor authentication: nobody extra, admins, or everyone
            let v = v
                .as_str()
                .filter(|v| matches!(*v, "off" | "admins" | "everyone"))
                .ok_or_else(|| ApiError::bad("requireTwoFactor must be off, admins or everyone"))?;
            sqlx::query("UPDATE workspaces SET settings = settings || jsonb_build_object('requireTwoFactor', $2::text) WHERE id = $1")
                .bind(user.workspace_id)
                .bind(v)
                .execute(&state.db)
                .await?;
            audit::log(&state, user.workspace_id, Some(user.id), "workspace.settings", None, None, json!({ "requireTwoFactor": v })).await;
            continue;
        }
        if key == "drawioUrl" {
            // "" turns draw.io diagrams off; otherwise an https address of a draw.io (embed mode)
            let url = v.as_str().map(|s| s.trim().trim_end_matches('/').to_string()).ok_or_else(|| ApiError::bad("drawioUrl must be a string"))?;
            let ok = url.is_empty()
                || (url.starts_with("https://")
                    && url.len() <= 300
                    && !url.chars().any(|c| c.is_whitespace() || matches!(c, '"' | '\'' | '<' | '>' | '\\' | '?' | '#')));
            if !ok {
                return Err(ApiError::bad("drawioUrl must be an https address without query (or empty to turn diagrams off)"));
            }
            sqlx::query("UPDATE workspaces SET settings = settings || jsonb_build_object('drawioUrl', $2::text) WHERE id = $1")
                .bind(user.workspace_id)
                .bind(&url)
                .execute(&state.db)
                .await?;
            audit::log(&state, user.workspace_id, Some(user.id), "workspace.settings", None, None, json!({ "drawioUrl": url })).await;
            continue;
        }
        if key == "attachments" {
            // upload rules: {maxFileMb, blockedTypes, allowedTypes, spaceQuotaMb, workspaceQuotaMb}, each optional
            let o = v.as_object().ok_or_else(|| ApiError::bad("attachments must be an object"))?;
            let mut set = serde_json::Map::new();
            for (k, val) in o {
                match k.as_str() {
                    "maxFileMb" => {
                        let cap = state.config.max_upload_mb;
                        let mb = val
                            .as_u64()
                            .filter(|m| *m <= cap)
                            .ok_or_else(|| ApiError::bad(format!("maxFileMb must be 0 (the server's limit) to {cap} (WIKICRAFT_MAX_UPLOAD_MB)")))?;
                        set.insert("attachmentMaxMb".into(), json!(mb));
                    }
                    "blockedTypes" | "allowedTypes" => {
                        let types = crate::routes::attachments::clean_types(val).map_err(ApiError::bad)?;
                        set.insert(if k == "blockedTypes" { "attachmentBlockedTypes" } else { "attachmentAllowedTypes" }.into(), json!(types));
                    }
                    "spaceQuotaMb" | "workspaceQuotaMb" => {
                        let mb =
                            val.as_u64().filter(|m| *m <= 100_000_000).ok_or_else(|| ApiError::bad(format!("{k} must be 0 (no limit) or a number of MB")))?;
                        set.insert(k.clone(), json!(mb));
                    }
                    other => return Err(ApiError::bad(format!("unknown attachment setting {other}"))),
                }
            }
            sqlx::query("UPDATE workspaces SET settings = settings || $2 WHERE id = $1")
                .bind(user.workspace_id)
                .bind(Value::Object(set.clone()))
                .execute(&state.db)
                .await?;
            audit::log(&state, user.workspace_id, Some(user.id), "workspace.settings", None, None, json!({ "attachments": set })).await;
            continue;
        }
        if key == "viewerList" {
            let on = v.as_bool().ok_or_else(|| ApiError::bad("viewerList must be true or false"))?;
            sqlx::query("UPDATE workspaces SET settings = settings || jsonb_build_object('viewerList', $2::boolean) WHERE id = $1")
                .bind(user.workspace_id)
                .bind(on)
                .execute(&state.db)
                .await?;
            audit::log(&state, user.workspace_id, Some(user.id), "workspace.settings", None, None, json!({ "viewerList": on })).await;
            continue;
        }
        if key == "trashRetentionDays" {
            // days a page stays in the trash before it's deleted for good; 0 = forever
            let days = v.as_i64().filter(|d| (0..=3650).contains(d)).ok_or_else(|| ApiError::bad("trashRetentionDays must be 0 (keep forever) to 3650"))?;
            sqlx::query("UPDATE workspaces SET settings = settings || jsonb_build_object('trashRetentionDays', $2::int) WHERE id = $1")
                .bind(user.workspace_id)
                .bind(days as i32)
                .execute(&state.db)
                .await?;
            audit::log(&state, user.workspace_id, Some(user.id), "workspace.settings", None, None, json!({ "trashRetentionDays": days })).await;
            continue;
        }
        let (Some(key), Some(v)) = (SWITCHES.iter().find(|k| *k == key), v.as_bool()) else {
            return Err(ApiError::bad(format!("unknown setting {key} (expected true/false for one of {})", SWITCHES.join(", "))));
        };
        sqlx::query("UPDATE workspaces SET settings = settings || jsonb_build_object($2::text, $3::boolean) WHERE id = $1")
            .bind(user.workspace_id)
            .bind(key)
            .bind(v)
            .execute(&state.db)
            .await?;
        audit::log(&state, user.workspace_id, Some(user.id), "workspace.settings", None, None, json!({ *key: v })).await;
    }
    get_settings(State(state), CurrentUser(user)).await
}

/// `GET /api/features` — what the caller may use here (switches as they apply to them: admins
/// can always export; Ask the wiki also needs an API key).
pub async fn features(State(state): State<AppState>, CurrentUser(user): CurrentUser) -> ApiResult<Json<Value>> {
    let mut f = switches(&state, user.workspace_id).await?;
    if user.is_admin() {
        f.insert("exports".into(), Value::Bool(true));
    }
    let ai_configured = state.config.ai_api_key.as_deref().is_some_and(|k| !k.is_empty());
    let ai = f.get("ai").and_then(Value::as_bool).unwrap_or(false) && ai_configured;
    f.insert("ai".into(), Value::Bool(ai));
    if user.is_guest() {
        f.insert("personalSpaces".into(), Value::Bool(false));
    }
    f.insert("viewerList".into(), Value::Bool(viewer_list_on(&state, user.workspace_id).await?));
    f.insert("drawioUrl".into(), json!(drawio_url(&state, user.workspace_id).await?));
    let policy = crate::routes::attachments::upload_policy(&state, user.workspace_id).await?;
    f.insert("maxFileMb".into(), json!(policy.max_file_mb));
    Ok(Json(Value::Object(f)))
}

/// Refuses exports for non-admins when an admin turned them off.
pub async fn require_exports(state: &AppState, ws: Uuid, user: Option<&crate::auth::User>) -> ApiResult<()> {
    if user.is_some_and(|u| u.is_admin()) || switch_on(state, ws, "exports").await? {
        Ok(())
    } else {
        Err(ApiError::bad("exports are turned off on this wiki"))
    }
}

// ---- public side ---------------------------------------------------------------------------

struct Shared {
    link_id: Uuid,
    root: Uuid,
    /// root + visible sub-pages (when the link includes children)
    pages: HashSet<Uuid>,
}

/// The cookie that holds a browser's unlock of a password-protected link.
fn unlock_cookie(link_id: Uuid) -> String {
    format!("wc_share_{}", link_id.simple())
}

/// Resolves a token to the set of pages it opens. Unknown, revoked and expired tokens — and
/// all tokens while public links are off — look the same: not found. A link with a password
/// answers 401 until this browser unlocked it.
async fn resolve(state: &AppState, token: &str, headers: &axum::http::HeaderMap) -> ApiResult<Shared> {
    let row: Option<(Uuid, Uuid, Uuid, bool, bool)> = sqlx::query_as(
        "SELECT l.id, l.workspace_id, l.page_id, l.include_children, l.password_hash IS NOT NULL FROM share_links l JOIN pages p ON p.id = l.page_id JOIN spaces s ON s.id = p.space_id
         WHERE l.token = $1 AND l.revoked_at IS NULL AND (l.expires_at IS NULL OR l.expires_at > now()) AND p.deleted_at IS NULL AND s.archived_at IS NULL",
    )
    .bind(token)
    .fetch_optional(&state.db)
    .await?;
    let (link_id, ws, root, children, has_password) = row.ok_or(ApiError::NotFound("page"))?;
    if has_password {
        let grant = crate::auth::cookie_value(headers, &unlock_cookie(link_id)).unwrap_or("");
        let unlocked: bool = sqlx::query_scalar("SELECT EXISTS (SELECT 1 FROM share_unlocks WHERE token_hash = $1 AND link_id = $2 AND expires_at > now())")
            .bind(crate::auth::token_hash(grant))
            .bind(link_id)
            .fetch_one(&state.db)
            .await?;
        if !unlocked {
            return Err(ApiError::Unauthorized);
        }
    }
    // switched off, restricted or moved into a draft after the link was made: fail closed
    if !links_enabled(state, ws).await? || restricted(state, root).await? || crate::authz::in_draft(state, root).await {
        return Err(ApiError::NotFound("page"));
    }
    let mut pages = HashSet::from([root]);
    if children {
        // descend, but never into a page with its own view restriction or an unpublished draft
        let ids: Vec<Uuid> = sqlx::query_scalar(
            "WITH RECURSIVE sub AS (
                SELECT id FROM pages WHERE parent_id = $1 AND deleted_at IS NULL AND kind <> 'blog' AND NOT draft
                  AND NOT EXISTS (SELECT 1 FROM page_restrictions r WHERE r.page_id = pages.id AND r.kind = 'view')
                UNION
                SELECT c.id FROM pages c JOIN sub ON c.parent_id = sub.id
                WHERE c.deleted_at IS NULL AND NOT c.draft AND NOT EXISTS (SELECT 1 FROM page_restrictions r WHERE r.page_id = c.id AND r.kind = 'view'))
             SELECT id FROM sub LIMIT 5000",
        )
        .bind(root)
        .fetch_all(&state.db)
        .await?;
        pages.extend(ids);
    }
    Ok(Shared { link_id, root, pages })
}

/// Rewrites links and media so they work for anonymous readers of this share.
fn public_doc(doc: &mut Node, token: &str, shared: &HashSet<Uuid>) {
    doc.walk_mut(&mut |n| {
        for key in ["src", "href"] {
            let Some(url) = n.attr_str(key).map(String::from) else { continue };
            if let Some(att) = url.strip_prefix("/api/attachments/").and_then(|r| r.split('/').next()).and_then(|s| s.parse::<Uuid>().ok()) {
                n.attrs.insert(key.into(), Value::String(format!("/api/public/{token}/attachments/{att}")));
            }
        }
        // live blocks need an account
        if n.kind == nodes::MACRO || n.kind == nodes::CHILD_PAGES || n.kind == nodes::INCLUDE {
            n.kind = nodes::PARAGRAPH.into();
            n.attrs.clear();
            n.content.clear();
        }
        for m in &mut n.marks {
            if m.kind != "link" {
                continue;
            }
            let Some(href) = m.attrs.get("href").and_then(Value::as_str).map(String::from) else { continue };
            let target = href.strip_prefix("/pages/").map(|r| r.split(['#', '?', '/']).next().unwrap_or(""));
            if let Some(id) = target.and_then(|t| t.parse::<Uuid>().ok()) {
                let anchor = href.split_once('#').map(|(_, a)| format!("#{a}")).unwrap_or_default();
                let new = if shared.contains(&id) { format!("/s/{token}/{id}{anchor}") } else { href.clone() };
                m.attrs.insert("href".into(), Value::String(new));
            } else if let Some(att) = href.strip_prefix("/api/attachments/").and_then(|r| r.split('/').next()).and_then(|s| s.parse::<Uuid>().ok()) {
                m.attrs.insert("href".into(), Value::String(format!("/api/public/{token}/attachments/{att}")));
            }
        }
    });
}

async fn render_page(state: &AppState, token: &str, s: &Shared, page: Uuid) -> ApiResult<Json<Value>> {
    if !s.pages.contains(&page) {
        return Err(ApiError::NotFound("page"));
    }
    let (title, content, updated_at, space_name, updated_by): (String, Value, chrono::DateTime<chrono::Utc>, String, Option<String>) = sqlx::query_as(
        "SELECT p.title, p.content, p.updated_at, s.name, u.name FROM pages p JOIN spaces s ON s.id = p.space_id LEFT JOIN users u ON u.id = p.updated_by WHERE p.id = $1",
    )
    .bind(page)
    .fetch_one(&state.db)
    .await?;
    let mut doc = match state.rooms.get(page) {
        Some(room) => room.snapshot(),
        None => Node::from_json(&content)?,
    };
    public_doc(&mut doc, token, &s.pages);
    let body = html::render(&doc, &html::HtmlOptions { heading_ids: true });
    // the shared tree (titles only) for navigation
    let tree: Vec<(Uuid, Option<Uuid>, String)> = if s.pages.len() > 1 {
        let ids: Vec<Uuid> = s.pages.iter().copied().collect();
        sqlx::query_as("SELECT id, parent_id, title FROM pages WHERE id = ANY($1) ORDER BY position, lower(title)").bind(&ids).fetch_all(&state.db).await?
    } else {
        vec![]
    };
    let tree: Vec<Value> =
        tree.into_iter().map(|(id, parent, title)| json!({"id": id, "parentId": if id == s.root { None } else { parent }, "title": title})).collect();
    sqlx::query("UPDATE share_links SET views = views + 1, last_viewed_at = now() WHERE id = $1").bind(s.link_id).execute(&state.db).await?;
    Ok(Json(json!({
        "id": page,
        "rootId": s.root,
        "title": title,
        "html": body,
        "updatedAt": updated_at,
        "updatedByName": updated_by,
        "spaceName": space_name,
        "tree": tree,
    })))
}

/// `GET /api/public/{token}` — the shared page.
pub async fn view(State(state): State<AppState>, headers: axum::http::HeaderMap, Path(token): Path<String>) -> ApiResult<Json<Value>> {
    let s = resolve(&state, &token, &headers).await?;
    let root = s.root;
    render_page(&state, &token, &s, root).await
}

/// `GET /api/public/{token}/pages/{id}` — a sub-page of a shared tree.
pub async fn view_page(State(state): State<AppState>, headers: axum::http::HeaderMap, Path((token, page)): Path<(String, Uuid)>) -> ApiResult<Json<Value>> {
    let s = resolve(&state, &token, &headers).await?;
    render_page(&state, &token, &s, page).await
}

#[derive(Deserialize)]
pub struct Unlock {
    password: String,
}

/// `POST /api/public/{token}/unlock` — the password of a protected link: this browser may open
/// it for 12 hours (an HttpOnly cookie for this link's paths). Wrong guesses are throttled per
/// link (10 per 15 minutes).
pub async fn unlock(State(state): State<AppState>, Path(token): Path<String>, Json(req): Json<Unlock>) -> ApiResult<Response> {
    let row: Option<(Uuid, Option<String>)> =
        sqlx::query_as("SELECT id, password_hash FROM share_links WHERE token = $1 AND revoked_at IS NULL AND (expires_at IS NULL OR expires_at > now())")
            .bind(&token)
            .fetch_optional(&state.db)
            .await?;
    let (link_id, Some(hash)) = row.ok_or(ApiError::NotFound("page"))? else { return Err(ApiError::bad("this link has no password")) };
    let throttle = format!("share:{link_id}");
    if !crate::auth::login_allowed(&state, &throttle).await {
        return Err(ApiError::BadRequest("too many wrong passwords, try again in 15 minutes".into()));
    }
    if !crate::auth::verify_password(req.password, hash).await {
        crate::auth::record_login_failure(&state, &throttle).await;
        return Err(ApiError::Denied("wrong password".into()));
    }
    crate::auth::clear_login_failures(&state, &throttle).await;
    let grant = random_token();
    sqlx::query("INSERT INTO share_unlocks (token_hash, link_id, expires_at) VALUES ($1, $2, now() + interval '12 hours')")
        .bind(crate::auth::token_hash(&grant))
        .bind(link_id)
        .execute(&state.db)
        .await?;
    let _ = sqlx::query("DELETE FROM share_unlocks WHERE expires_at < now()").execute(&state.db).await;
    let secure = if state.config.base_url.starts_with("https://") { "; Secure" } else { "" };
    let cookie = format!("{}={grant}; Path=/api/public/{token}; HttpOnly; SameSite=Lax; Max-Age=43200{secure}", unlock_cookie(link_id));
    Ok(axum::response::IntoResponse::into_response((StatusCode::NO_CONTENT, [(axum::http::header::SET_COOKIE, cookie)])))
}

/// `GET /api/public/{token}/attachments/{id}` — files of shared pages only.
pub async fn attachment(State(state): State<AppState>, headers: axum::http::HeaderMap, Path((token, id)): Path<(String, Uuid)>) -> ApiResult<Response> {
    let s = resolve(&state, &token, &headers).await?;
    let row: Option<(Uuid, String, String, Option<String>, i64)> =
        sqlx::query_as("SELECT page_id, filename, media_type, blob_key, size FROM attachments WHERE id = $1 AND deleted_at IS NULL")
            .bind(id)
            .fetch_optional(&state.db)
            .await?;
    let (page, filename, media_type, key, size) = row.ok_or(ApiError::NotFound("attachment"))?;
    if !s.pages.contains(&page) {
        return Err(ApiError::NotFound("attachment"));
    }
    crate::routes::attachments::serve(&state, filename, media_type, key, size).await
}
