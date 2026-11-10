use axum::Json;
use axum::extract::{Path, State};
use axum::http::StatusCode;
use serde::{Deserialize, Serialize};
use serde_json::json;
use std::collections::{HashMap, HashSet};
use uuid::Uuid;

use crate::app::{AppState, new_id};
use crate::audit;
use crate::auth::{CurrentUser, MaybeUser};
use crate::authz::{Principal, Role, SpaceAccessRow, space_role, visible_page_ids};
use crate::error::{ApiError, ApiResult};

#[derive(Debug, Clone, Serialize, sqlx::FromRow)]
#[serde(rename_all = "camelCase")]
pub struct SpaceRow {
    pub id: Uuid,
    pub workspace_id: Uuid,
    pub key: String,
    pub name: String,
    pub description: Option<String>,
    pub icon: Option<String>,
    pub home_page_id: Option<Uuid>,
    pub default_role: String,
    pub anonymous_access: bool,
    pub personal_owner_id: Option<Uuid>,
    pub created_at: chrono::DateTime<chrono::Utc>,
    pub updated_at: chrono::DateTime<chrono::Utc>,
    pub archived_at: Option<chrono::DateTime<chrono::Utc>>,
    /// the template new pages start from
    #[sqlx(default)]
    pub default_template: Option<String>,
    /// directory grouping ("Teams", "Projects", …)
    #[sqlx(default)]
    pub category: Option<String>,
}

pub const SPACE_COLUMNS: &str = "id, workspace_id, key, name, description, icon, home_page_id, default_role, anonymous_access, personal_owner_id, created_at, updated_at, archived_at, default_template, category";

impl SpaceRow {
    pub fn access_row(&self) -> SpaceAccessRow {
        SpaceAccessRow {
            id: self.id,
            workspace_id: self.workspace_id,
            default_role: self.default_role.clone(),
            anonymous_access: self.anonymous_access,
            personal_owner_id: self.personal_owner_id,
            archived: self.archived_at.is_some(),
        }
    }
}

/// The workspace a request operates in (the user's, or the first one for anonymous access).
pub async fn workspace_of(state: &AppState, p: &Principal) -> ApiResult<Uuid> {
    match &p.user {
        Some(u) => Ok(u.workspace_id),
        None => {
            sqlx::query_scalar("SELECT id FROM workspaces ORDER BY created_at LIMIT 1").fetch_optional(&state.db).await?.ok_or(ApiError::NotFound("workspace"))
        }
    }
}

/// Loads a space by key and the principal's role in it (404 if no access).
pub async fn load_space(state: &AppState, p: &Principal, key: &str) -> ApiResult<(SpaceRow, Role)> {
    let ws = workspace_of(state, p).await?;
    // the current key, else a former one (renamed spaces keep their old links)
    let space = sqlx::query_as::<_, SpaceRow>(sqlx::AssertSqlSafe(format!(
        "SELECT {SPACE_COLUMNS} FROM spaces WHERE workspace_id = $1 AND (lower(key) = lower($2)
            OR (NOT EXISTS (SELECT 1 FROM spaces WHERE workspace_id = $1 AND lower(key) = lower($2))
                AND id = (SELECT space_id FROM space_key_aliases WHERE workspace_id = $1 AND lower(key) = lower($2))))"
    )))
    .bind(ws)
    .bind(key)
    .fetch_optional(&state.db)
    .await?
    .ok_or(ApiError::NotFound("space"))?;
    let role = space_role(state, p, &space.access_row()).await?;
    if role == Role::None {
        return Err(if p.user.is_none() { ApiError::Unauthorized } else { ApiError::NotFound("space") });
    }
    Ok((space, role))
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SpaceView {
    #[serde(flatten)]
    space: SpaceRow,
    role: Role,
    page_count: i64,
    /// the caller watches the whole space
    watching: bool,
}

/// Lists the spaces visible to the caller.
pub async fn list(State(state): State<AppState>, MaybeUser(user): MaybeUser) -> ApiResult<Json<Vec<SpaceView>>> {
    let p = Principal::load(&state, user).await?;
    let ws = workspace_of(&state, &p).await?;
    let spaces = sqlx::query_as::<_, SpaceRow>(sqlx::AssertSqlSafe(format!(
        "SELECT {SPACE_COLUMNS} FROM spaces WHERE workspace_id = $1 AND archived_at IS NULL ORDER BY lower(name)"
    )))
    .bind(ws)
    .fetch_all(&state.db)
    .await?;
    let counts: Vec<(Uuid, i64)> = sqlx::query_as("SELECT space_id, count(*) FROM pages WHERE workspace_id = $1 AND deleted_at IS NULL GROUP BY space_id")
        .bind(ws)
        .fetch_all(&state.db)
        .await?;
    let counts: std::collections::HashMap<Uuid, i64> = counts.into_iter().collect();
    let mut out = Vec::new();
    for s in spaces {
        let role = space_role(&state, &p, &s.access_row()).await?;
        if role > Role::None {
            let page_count = counts.get(&s.id).copied().unwrap_or(0);
            out.push(SpaceView { space: s, role, page_count, watching: false });
        }
    }
    Ok(Json(out))
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CreateSpace {
    /// a space template (`GET /api/space-templates`) to prefill the space with
    pub template: Option<String>,
    pub key: String,
    pub name: String,
    pub description: Option<String>,
    pub icon: Option<String>,
    pub default_role: Option<String>,
}

pub fn validate_key(key: &str) -> ApiResult<String> {
    let k = key.trim().to_uppercase();
    if k.is_empty() || k.len() > 32 || !k.chars().all(|c| c.is_ascii_alphanumeric() || c == '_' || c == '-') {
        return Err(ApiError::bad("space key must be 1-32 characters: letters, digits, '_' or '-'"));
    }
    Ok(k)
}

fn validate_role(r: &str) -> ApiResult<()> {
    if matches!(r, "none" | "viewer" | "commenter" | "editor" | "admin") { Ok(()) } else { Err(ApiError::bad("invalid role")) }
}

/// Creates a space with an empty home page.
pub async fn create(State(state): State<AppState>, CurrentUser(user): CurrentUser, Json(req): Json<CreateSpace>) -> ApiResult<(StatusCode, Json<SpaceRow>)> {
    if user.is_guest() {
        return Err(ApiError::Forbidden);
    }
    let key = validate_key(&req.key)?;
    let name = req.name.trim();
    if name.is_empty() {
        return Err(ApiError::bad("name is required"));
    }
    // a former key of another space still points there
    let former: Option<String> =
        sqlx::query_scalar("SELECT s.name FROM space_key_aliases a JOIN spaces s ON s.id = a.space_id WHERE a.workspace_id = $1 AND lower(a.key) = lower($2)")
            .bind(user.workspace_id)
            .bind(&key)
            .fetch_optional(&state.db)
            .await?;
    if let Some(n) = former {
        return Err(ApiError::Conflict(format!("the key {key} was used by the space “{n}” and still links there")));
    }
    if let Some(t) = req.template.as_deref().filter(|t| !t.is_empty())
        && !crate::routes::space_templates::exists(t)
    {
        return Err(ApiError::bad(format!("unknown space template {t}")));
    }
    let default_role = req.default_role.unwrap_or_else(|| "editor".into());
    validate_role(&default_role)?;
    let icon = req.icon.clone().or_else(|| req.template.as_deref().and_then(crate::routes::space_templates::icon));
    let space = create_space(&state, user.workspace_id, Some(user.id), &key, name, req.description.as_deref(), icon.as_deref(), &default_role, None).await?;
    // creator becomes space admin
    sqlx::query("INSERT INTO space_permissions (space_id, subject_type, subject_id, role) VALUES ($1, 'user', $2, 'admin') ON CONFLICT DO NOTHING")
        .bind(space.id)
        .bind(user.id)
        .execute(&state.db)
        .await?;
    // home page
    let home = crate::routes::pages::insert_page(
        &state.db,
        crate::routes::pages::NewPage {
            id: new_id(),
            workspace_id: user.workspace_id,
            space_id: space.id,
            parent_id: None,
            kind: "page".into(),
            title: name.to_string(),
            content: welcome_doc(name),
            position: 0.0,
            created_by: Some(user.id),
            created_at: None,
            source_ref: None,
        },
    )
    .await?;
    sqlx::query("UPDATE spaces SET home_page_id = $1 WHERE id = $2").bind(home).bind(space.id).execute(&state.db).await?;
    state.search.enqueue(home);
    if let Some(t) = req.template.as_deref().filter(|t| !t.is_empty()) {
        crate::routes::space_templates::apply(&state, t, user.workspace_id, space.id, home, user.id).await?;
    }
    audit::log(&state, user.workspace_id, Some(user.id), "space.create", Some("space"), Some(space.id), json!({"key": key})).await;
    let space = sqlx::query_as::<_, SpaceRow>(sqlx::AssertSqlSafe(format!("SELECT {SPACE_COLUMNS} FROM spaces WHERE id = $1")))
        .bind(space.id)
        .fetch_one(&state.db)
        .await?;
    crate::webhooks::emit(&state, user.workspace_id, "space.created", Some(space.id), json!({"space": {"id": space.id, "key": space.key, "name": space.name}}))
        .await;
    Ok((StatusCode::CREATED, Json(space)))
}

fn welcome_doc(name: &str) -> wikicraft_doc_model::Node {
    use wikicraft_doc_model::{Node, nodes};
    Node::doc(vec![
        Node::paragraph(vec![Node::text(format!("Welcome to the {name} space. Edit this page to tell people what this space is about."))]),
        Node::new(nodes::CHILD_PAGES),
    ])
}

#[allow(clippy::too_many_arguments)]
pub async fn create_space(
    state: &AppState,
    ws: Uuid,
    creator: Option<Uuid>,
    key: &str,
    name: &str,
    description: Option<&str>,
    icon: Option<&str>,
    default_role: &str,
    source_ref: Option<&str>,
) -> ApiResult<SpaceRow> {
    let id = new_id();
    let row = sqlx::query_as::<_, SpaceRow>(sqlx::AssertSqlSafe(format!(
        "INSERT INTO spaces (id, workspace_id, key, name, description, icon, default_role, created_by, source_ref) VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9) RETURNING {SPACE_COLUMNS}"
    )))
    .bind(id)
    .bind(ws)
    .bind(key)
    .bind(name)
    .bind(description)
    .bind(icon)
    .bind(default_role)
    .bind(creator)
    .bind(source_ref)
    .fetch_one(&state.db)
    .await
    .map_err(|e| match ApiError::from(e) {
        ApiError::Conflict(_) => ApiError::Conflict(format!("a space with key {key} already exists")),
        other => other,
    })?;
    Ok(row)
}

/// `GET /api/me/space`: the caller's personal space, if they have one.
pub async fn my_space(State(state): State<AppState>, CurrentUser(user): CurrentUser) -> ApiResult<Json<serde_json::Value>> {
    let key: Option<String> =
        sqlx::query_scalar("SELECT key FROM spaces WHERE personal_owner_id = $1 AND archived_at IS NULL").bind(user.id).fetch_optional(&state.db).await?;
    Ok(Json(json!({ "key": key })))
}

/// `POST /api/me/space`: creates the caller's personal space (private: only the owner, unless
/// they share it) or returns the existing one. Key `~NAME`.
pub async fn create_my_space(State(state): State<AppState>, CurrentUser(user): CurrentUser) -> ApiResult<(StatusCode, Json<SpaceRow>)> {
    if user.is_guest() {
        return Err(ApiError::Forbidden);
    }
    let allowed = crate::routes::share::switch_on(&state, user.workspace_id, "personalSpaces").await?;
    if let Some(row) = sqlx::query_as::<_, SpaceRow>(sqlx::AssertSqlSafe(format!("SELECT {SPACE_COLUMNS} FROM spaces WHERE personal_owner_id = $1")))
        .bind(user.id)
        .fetch_optional(&state.db)
        .await?
    {
        return Ok((StatusCode::OK, Json(row)));
    }
    if !allowed {
        return Err(ApiError::bad("personal spaces are turned off on this wiki"));
    }
    // ~ + the e-mail's local part (or name), made unique
    let base: String = user
        .email
        .as_deref()
        .and_then(|e| e.split('@').next())
        .unwrap_or(&user.name)
        .chars()
        .filter(|c| c.is_ascii_alphanumeric())
        .take(24)
        .collect::<String>()
        .to_uppercase();
    let base = if base.is_empty() { "ME".to_string() } else { base };
    let mut key = format!("~{base}");
    for i in 2..100 {
        let taken: bool = sqlx::query_scalar(
            "SELECT EXISTS (SELECT 1 FROM spaces WHERE workspace_id = $1 AND lower(key) = lower($2)) OR EXISTS (SELECT 1 FROM space_key_aliases WHERE workspace_id = $1 AND lower(key) = lower($2))",
        )
        .bind(user.workspace_id)
        .bind(&key)
        .fetch_one(&state.db)
        .await?;
        if !taken {
            break;
        }
        key = format!("~{base}{i}");
    }
    let space = create_space(&state, user.workspace_id, Some(user.id), &key, &user.name, Some("Personal space"), Some("star:amber"), "none", None).await?;
    sqlx::query("UPDATE spaces SET personal_owner_id = $2 WHERE id = $1").bind(space.id).bind(user.id).execute(&state.db).await?;
    let home = crate::routes::pages::insert_page(
        &state.db,
        crate::routes::pages::NewPage {
            id: new_id(),
            workspace_id: user.workspace_id,
            space_id: space.id,
            parent_id: None,
            kind: "page".into(),
            title: user.name.clone(),
            content: wikicraft_doc_model::Node::doc(vec![
                wikicraft_doc_model::Node::paragraph(vec![wikicraft_doc_model::Node::text(
                    "Your personal space: drafts, notes and anything you're not ready to share. Only you can see it until you share pages or the space.",
                )]),
                wikicraft_doc_model::Node::new(wikicraft_doc_model::nodes::CHILD_PAGES),
            ]),
            position: 0.0,
            created_by: Some(user.id),
            created_at: None,
            source_ref: None,
        },
    )
    .await?;
    sqlx::query("UPDATE spaces SET home_page_id = $1 WHERE id = $2").bind(home).bind(space.id).execute(&state.db).await?;
    state.search.enqueue(home);
    audit::log(&state, user.workspace_id, Some(user.id), "space.create_personal", Some("space"), Some(space.id), json!({"key": key})).await;
    let row = sqlx::query_as::<_, SpaceRow>(sqlx::AssertSqlSafe(format!("SELECT {SPACE_COLUMNS} FROM spaces WHERE id = $1")))
        .bind(space.id)
        .fetch_one(&state.db)
        .await?;
    Ok((StatusCode::CREATED, Json(row)))
}

pub async fn get(State(state): State<AppState>, MaybeUser(user): MaybeUser, Path(key): Path<String>) -> ApiResult<Json<SpaceView>> {
    let p = Principal::load(&state, user).await?;
    let (space, role) = load_space(&state, &p, &key).await?;
    let page_count: i64 =
        sqlx::query_scalar("SELECT count(*) FROM pages WHERE space_id = $1 AND deleted_at IS NULL").bind(space.id).fetch_one(&state.db).await?;
    let watching: bool = match &p.user {
        Some(u) => {
            sqlx::query_scalar("SELECT EXISTS (SELECT 1 FROM space_watches WHERE user_id = $1 AND space_id = $2)")
                .bind(u.id)
                .bind(space.id)
                .fetch_one(&state.db)
                .await?
        }
        None => false,
    };
    Ok(Json(SpaceView { space, role, page_count, watching }))
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct UpdateSpace {
    /// a new key; the old one keeps working as an alias
    key: Option<String>,
    name: Option<String>,
    description: Option<String>,
    icon: Option<String>,
    default_role: Option<String>,
    anonymous_access: Option<bool>,
    home_page_id: Option<Uuid>,
    /// template for new pages: a template id, or "" for none
    default_template: Option<String>,
    /// directory category, "" for none
    category: Option<String>,
}

/// Gives a space a new key and keeps the old one as an alias.
async fn rename_key(state: &AppState, ws: Uuid, space: Uuid, old: &str, new: &str) -> ApiResult<()> {
    let mut tx = state.db.begin().await?;
    let taken: Option<String> = sqlx::query_scalar(
        "SELECT name FROM spaces WHERE workspace_id = $1 AND lower(key) = lower($2) AND id <> $3
         UNION ALL SELECT s.name FROM space_key_aliases a JOIN spaces s ON s.id = a.space_id WHERE a.workspace_id = $1 AND lower(a.key) = lower($2) AND a.space_id <> $3 LIMIT 1",
    )
    .bind(ws)
    .bind(new)
    .bind(space)
    .fetch_optional(&mut *tx)
    .await?;
    if let Some(name) = taken {
        return Err(ApiError::Conflict(format!("the key {new} is (or was) used by the space “{name}”")));
    }
    // renaming back to a former key drops that alias
    sqlx::query("DELETE FROM space_key_aliases WHERE workspace_id = $1 AND lower(key) = lower($2)").bind(ws).bind(new).execute(&mut *tx).await?;
    sqlx::query("UPDATE spaces SET key = $2, updated_at = now() WHERE id = $1").bind(space).bind(new).execute(&mut *tx).await?;
    sqlx::query("INSERT INTO space_key_aliases (workspace_id, key, space_id) VALUES ($1, $2, $3) ON CONFLICT DO NOTHING")
        .bind(ws)
        .bind(old)
        .bind(space)
        .execute(&mut *tx)
        .await?;
    tx.commit().await?;
    Ok(())
}

pub async fn update(
    State(state): State<AppState>,
    CurrentUser(user): CurrentUser,
    Path(key): Path<String>,
    Json(req): Json<UpdateSpace>,
) -> ApiResult<Json<SpaceRow>> {
    let p = Principal::load(&state, Some(user.clone())).await?;
    let (space, role) = load_space(&state, &p, &key).await?;
    if role < Role::Admin {
        return Err(ApiError::Forbidden);
    }
    if let Some(r) = &req.default_role {
        validate_role(r)?;
    }
    if req.anonymous_access == Some(true) && !space.anonymous_access && !user.is_admin() {
        return Err(ApiError::bad("only workspace admins can make a space public"));
    }
    if let Some(t) = req.default_template.as_deref().filter(|t| !t.is_empty()) {
        // must resolve (built-in or a template of this workspace)
        crate::routes::templates::resolve(&state, user.workspace_id, t).await.map_err(|_| ApiError::bad("unknown template"))?;
    }
    if let Some(t) = &req.default_template {
        sqlx::query("UPDATE spaces SET default_template = $2 WHERE id = $1")
            .bind(space.id)
            .bind(Some(t.as_str()).filter(|t| !t.is_empty()))
            .execute(&state.db)
            .await?;
    }
    if let Some(c) = &req.category {
        let c = c.split_whitespace().collect::<Vec<_>>().join(" ");
        if c.chars().count() > 60 {
            return Err(ApiError::bad("categories are limited to 60 characters"));
        }
        sqlx::query("UPDATE spaces SET category = $2 WHERE id = $1").bind(space.id).bind(Some(c).filter(|c| !c.is_empty())).execute(&state.db).await?;
    }
    let new_key = match req.key.as_deref() {
        Some(k) => Some(validate_key(k)?).filter(|k| *k != space.key),
        None => None,
    };
    if let Some(k) = &new_key {
        rename_key(&state, user.workspace_id, space.id, &space.key, k).await?;
        audit::log(&state, user.workspace_id, Some(user.id), "space.rename_key", Some("space"), Some(space.id), json!({"from": space.key, "to": k})).await;
    }
    let row = sqlx::query_as::<_, SpaceRow>(sqlx::AssertSqlSafe(format!(
        "UPDATE spaces SET name = COALESCE($2, name), description = COALESCE($3, description), icon = COALESCE($4, icon),
            default_role = COALESCE($5, default_role), anonymous_access = COALESCE($6, anonymous_access), home_page_id = COALESCE($7, home_page_id), updated_at = now()
         WHERE id = $1 RETURNING {SPACE_COLUMNS}"
    )))
    .bind(space.id)
    .bind(req.name.as_deref().map(str::trim).filter(|s| !s.is_empty()))
    .bind(req.description)
    .bind(req.icon)
    .bind(req.default_role)
    .bind(req.anonymous_access)
    .bind(req.home_page_id)
    .fetch_one(&state.db)
    .await?;
    audit::log(&state, user.workspace_id, Some(user.id), "space.update", Some("space"), Some(space.id), json!({})).await;
    Ok(Json(row))
}

pub async fn archive(State(state): State<AppState>, CurrentUser(user): CurrentUser, Path(key): Path<String>) -> ApiResult<StatusCode> {
    let p = Principal::load(&state, Some(user.clone())).await?;
    let (space, role) = load_space(&state, &p, &key).await?;
    if role < Role::Admin {
        return Err(ApiError::Forbidden);
    }
    sqlx::query("UPDATE spaces SET archived_at = now() WHERE id = $1").bind(space.id).execute(&state.db).await?;
    audit::log(&state, user.workspace_id, Some(user.id), "space.archive", Some("space"), Some(space.id), json!({})).await;
    Ok(StatusCode::NO_CONTENT)
}

#[derive(Serialize, sqlx::FromRow, Clone)]
#[serde(rename_all = "camelCase")]
pub struct TreeNode {
    pub id: Uuid,
    pub parent_id: Option<Uuid>,
    pub title: String,
    pub kind: String,
    pub icon: Option<String>,
    pub position: f64,
    pub restricted: bool,
    /// kind `link`: where the tree item leads
    #[sqlx(default)]
    #[serde(skip_serializing_if = "Option::is_none")]
    pub url: Option<String>,
    #[serde(skip)]
    #[sqlx(default)]
    pub archived: bool,
}

/// Flat list of all pages in a space the caller can see (client builds the tree).
pub async fn tree(State(state): State<AppState>, MaybeUser(user): MaybeUser, Path(key): Path<String>) -> ApiResult<Json<serde_json::Value>> {
    let p = Principal::load(&state, user).await?;
    let (space, role) = load_space(&state, &p, &key).await?;
    let rows = sqlx::query_as::<_, TreeNode>(
        "SELECT p.id, p.parent_id, p.title, p.kind, p.icon, p.position,
                EXISTS (SELECT 1 FROM page_restrictions r WHERE r.page_id = p.id) AS restricted, p.archived_at IS NOT NULL AS archived,
                CASE WHEN p.kind = 'link' THEN p.content->'content'->0->'attrs'->>'url' END AS url
         FROM pages p WHERE p.space_id = $1 AND p.deleted_at IS NULL AND p.kind <> 'blog' ORDER BY p.position, lower(p.title)",
    )
    .bind(space.id)
    .fetch_all(&state.db)
    .await?;
    let pairs: Vec<(Uuid, Option<Uuid>)> = rows.iter().map(|r| (r.id, r.parent_id)).collect();
    let visible = visible_page_ids(&state, &p, space.id, role, &pairs).await?;
    // archived pages and everything under them leave the tree (Space settings → Archived)
    let parent: HashMap<Uuid, Option<Uuid>> = rows.iter().map(|r| (r.id, r.parent_id)).collect();
    let archived_roots: HashSet<Uuid> = rows.iter().filter(|r| r.archived).map(|r| r.id).collect();
    let under_archive = |mut id: Uuid| {
        for _ in 0..200 {
            if archived_roots.contains(&id) {
                return true;
            }
            match parent.get(&id).copied().flatten() {
                Some(p) => id = p,
                None => return false,
            }
        }
        false
    };
    let nodes: Vec<TreeNode> = rows.iter().filter(|r| visible.contains(&r.id) && !under_archive(r.id)).cloned().collect();
    Ok(Json(json!({ "spaceId": space.id, "homePageId": space.home_page_id, "nodes": nodes })))
}

#[derive(Serialize, Deserialize, sqlx::FromRow)]
#[serde(rename_all = "camelCase")]
pub struct Grant {
    pub subject_type: String,
    pub subject_id: Uuid,
    pub role: String,
    #[sqlx(default)]
    #[serde(default)]
    pub subject_name: Option<String>,
}

pub async fn get_permissions(State(state): State<AppState>, CurrentUser(user): CurrentUser, Path(key): Path<String>) -> ApiResult<Json<serde_json::Value>> {
    let p = Principal::load(&state, Some(user)).await?;
    let (space, role) = load_space(&state, &p, &key).await?;
    if role < Role::Admin {
        return Err(ApiError::Forbidden);
    }
    let grants = sqlx::query_as::<_, Grant>(
        "SELECT sp.subject_type, sp.subject_id, sp.role, COALESCE(u.name, g.name) AS subject_name FROM space_permissions sp
         LEFT JOIN users u ON sp.subject_type = 'user' AND u.id = sp.subject_id LEFT JOIN groups g ON sp.subject_type = 'group' AND g.id = sp.subject_id
         WHERE sp.space_id = $1 ORDER BY sp.subject_type, subject_name",
    )
    .bind(space.id)
    .fetch_all(&state.db)
    .await?;
    Ok(Json(json!({ "defaultRole": space.default_role, "anonymousAccess": space.anonymous_access, "grants": grants })))
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PutPermissions {
    default_role: Option<String>,
    anonymous_access: Option<bool>,
    grants: Vec<Grant>,
}

pub async fn put_permissions(
    State(state): State<AppState>,
    CurrentUser(user): CurrentUser,
    Path(key): Path<String>,
    Json(req): Json<PutPermissions>,
) -> ApiResult<StatusCode> {
    let p = Principal::load(&state, Some(user.clone())).await?;
    let (space, role) = load_space(&state, &p, &key).await?;
    if role < Role::Admin {
        return Err(ApiError::Forbidden);
    }
    if !req.grants.iter().any(|g| g.role == "admin") && !user.is_admin() {
        return Err(ApiError::bad("a space needs at least one admin"));
    }
    let mut tx = state.db.begin().await?;
    if let Some(r) = &req.default_role {
        validate_role(r)?;
        sqlx::query("UPDATE spaces SET default_role = $2 WHERE id = $1").bind(space.id).bind(r).execute(&mut *tx).await?;
    }
    if let Some(a) = req.anonymous_access {
        if a && !space.anonymous_access && !user.is_admin() {
            return Err(ApiError::bad("only workspace admins can make a space public"));
        }
        sqlx::query("UPDATE spaces SET anonymous_access = $2 WHERE id = $1").bind(space.id).bind(a).execute(&mut *tx).await?;
    }
    sqlx::query("DELETE FROM space_permissions WHERE space_id = $1").bind(space.id).execute(&mut *tx).await?;
    for g in &req.grants {
        validate_role(&g.role)?;
        if !matches!(g.subject_type.as_str(), "user" | "group") {
            return Err(ApiError::bad("subjectType must be user or group"));
        }
        sqlx::query("INSERT INTO space_permissions (space_id, subject_type, subject_id, role) VALUES ($1, $2, $3, $4) ON CONFLICT DO NOTHING")
            .bind(space.id)
            .bind(&g.subject_type)
            .bind(g.subject_id)
            .bind(&g.role)
            .execute(&mut *tx)
            .await?;
    }
    tx.commit().await?;
    audit::log(&state, user.workspace_id, Some(user.id), "space.permissions", Some("space"), Some(space.id), json!({"grants": req.grants.len()})).await;
    Ok(StatusCode::NO_CONTENT)
}

#[derive(Serialize, sqlx::FromRow)]
#[serde(rename_all = "camelCase")]
pub struct TrashItem {
    id: Uuid,
    title: String,
    deleted_at: chrono::DateTime<chrono::Utc>,
    deleted_by_name: Option<String>,
}

pub async fn trash(State(state): State<AppState>, CurrentUser(user): CurrentUser, Path(key): Path<String>) -> ApiResult<Json<Vec<TrashItem>>> {
    let p = Principal::load(&state, Some(user)).await?;
    let (space, role) = load_space(&state, &p, &key).await?;
    if role < Role::Editor {
        return Err(ApiError::Forbidden);
    }
    // only top-most deleted pages (children deleted with them are restored together)
    let rows = sqlx::query_as::<_, TrashItem>(
        "SELECT p.id, p.title, p.deleted_at, u.name AS deleted_by_name FROM pages p LEFT JOIN users u ON u.id = p.deleted_by
         LEFT JOIN pages parent ON parent.id = p.parent_id
         WHERE p.space_id = $1 AND p.deleted_at IS NOT NULL AND (parent.id IS NULL OR parent.deleted_at IS NULL OR parent.deleted_at <> p.deleted_at)
         ORDER BY p.deleted_at DESC LIMIT 500",
    )
    .bind(space.id)
    .fetch_all(&state.db)
    .await?;
    let pairs: Vec<(Uuid, Option<Uuid>)> = sqlx::query_as("SELECT id, parent_id FROM pages WHERE space_id = $1").bind(space.id).fetch_all(&state.db).await?;
    let visible = visible_page_ids(&state, &p, space.id, role, &pairs).await?;
    Ok(Json(rows.into_iter().filter(|r| visible.contains(&r.id)).collect()))
}

#[derive(Serialize, sqlx::FromRow)]
#[serde(rename_all = "camelCase")]
pub struct BlogItem {
    id: Uuid,
    title: String,
    excerpt: String,
    created_at: chrono::DateTime<chrono::Utc>,
    created_by_name: Option<String>,
    comment_count: i64,
}

/// Blog posts of a space, newest first.
pub async fn blog(State(state): State<AppState>, MaybeUser(user): MaybeUser, Path(key): Path<String>) -> ApiResult<Json<Vec<BlogItem>>> {
    let p = Principal::load(&state, user).await?;
    let (space, role) = load_space(&state, &p, &key).await?;
    let rows = sqlx::query_as::<_, BlogItem>(
        "SELECT p.id, p.title, left(p.text_content, 300) AS excerpt, p.created_at, u.name AS created_by_name,
                (SELECT count(*) FROM comments c WHERE c.page_id = p.id AND c.deleted_at IS NULL) AS comment_count
         FROM pages p LEFT JOIN users u ON u.id = p.created_by WHERE p.space_id = $1 AND p.kind = 'blog' AND p.deleted_at IS NULL ORDER BY p.created_at DESC LIMIT 200",
    )
    .bind(space.id)
    .fetch_all(&state.db)
    .await?;
    let pairs: Vec<(Uuid, Option<Uuid>)> =
        sqlx::query_as("SELECT id, parent_id FROM pages WHERE space_id = $1 AND deleted_at IS NULL").bind(space.id).fetch_all(&state.db).await?;
    let visible = visible_page_ids(&state, &p, space.id, role, &pairs).await?;
    Ok(Json(rows.into_iter().filter(|r| visible.contains(&r.id)).collect()))
}

/// `PUT /api/spaces/{key}/watch`: notifications for new and updated pages in the space.
pub async fn watch(State(state): State<AppState>, CurrentUser(user): CurrentUser, Path(key): Path<String>) -> ApiResult<StatusCode> {
    let p = Principal::load(&state, Some(user.clone())).await?;
    let (space, _) = load_space(&state, &p, &key).await?;
    sqlx::query("INSERT INTO space_watches (user_id, space_id) VALUES ($1, $2) ON CONFLICT DO NOTHING").bind(user.id).bind(space.id).execute(&state.db).await?;
    Ok(StatusCode::NO_CONTENT)
}

/// `DELETE /api/spaces/{key}/watch`
pub async fn unwatch(State(state): State<AppState>, CurrentUser(user): CurrentUser, Path(key): Path<String>) -> ApiResult<StatusCode> {
    let p = Principal::load(&state, Some(user.clone())).await?;
    let (space, _) = load_space(&state, &p, &key).await?;
    sqlx::query("DELETE FROM space_watches WHERE user_id = $1 AND space_id = $2").bind(user.id).bind(space.id).execute(&state.db).await?;
    Ok(StatusCode::NO_CONTENT)
}
