use axum::Json;
use axum::extract::{Path, Query, State};
use axum::http::StatusCode;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
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
    /// the person responsible for the space
    #[sqlx(default)]
    pub owner_id: Option<Uuid>,
    /// sidebar links: `[{title, url}]`
    #[sqlx(default)]
    pub shortcuts: Value,
    /// the space's own look: `{accentColor?, hideBlog?}`
    #[sqlx(default)]
    pub look: Value,
}

pub const SPACE_COLUMNS: &str = "id, workspace_id, key, name, description, icon, home_page_id, default_role, anonymous_access, personal_owner_id, created_at, updated_at, archived_at, default_template, category, owner_id, shortcuts, look";

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
    /// the caller starred the space
    starred: bool,
    /// the owner's name (single space only)
    #[serde(skip_serializing_if = "Option::is_none")]
    owner_name: Option<String>,
}

#[derive(Deserialize, Default)]
pub struct ListQuery {
    /// `true`: the archived spaces the caller can still read, instead of the active ones
    #[serde(default)]
    pub archived: bool,
}

/// Lists the spaces visible to the caller (`?archived=true`: the archived ones).
pub async fn list(State(state): State<AppState>, MaybeUser(user): MaybeUser, Query(q): Query<ListQuery>) -> ApiResult<Json<Vec<SpaceView>>> {
    let p = Principal::load(&state, user).await?;
    let ws = workspace_of(&state, &p).await?;
    let spaces = sqlx::query_as::<_, SpaceRow>(sqlx::AssertSqlSafe(format!(
        "SELECT {SPACE_COLUMNS} FROM spaces WHERE workspace_id = $1 AND (archived_at IS NOT NULL) = $2 ORDER BY lower(name)"
    )))
    .bind(ws)
    .bind(q.archived)
    .fetch_all(&state.db)
    .await?;
    let counts: Vec<(Uuid, i64)> = sqlx::query_as("SELECT space_id, count(*) FROM pages WHERE workspace_id = $1 AND deleted_at IS NULL GROUP BY space_id")
        .bind(ws)
        .fetch_all(&state.db)
        .await?;
    let counts: std::collections::HashMap<Uuid, i64> = counts.into_iter().collect();
    let starred: HashSet<Uuid> = match &p.user {
        Some(u) => sqlx::query_scalar("SELECT space_id FROM space_favorites WHERE user_id = $1").bind(u.id).fetch_all(&state.db).await?.into_iter().collect(),
        None => HashSet::new(),
    };
    let mut out = Vec::new();
    for s in spaces {
        let role = space_role(&state, &p, &s.access_row()).await?;
        if role > Role::None {
            let page_count = counts.get(&s.id).copied().unwrap_or(0);
            let starred = starred.contains(&s.id);
            out.push(SpaceView { space: s, role, page_count, watching: false, starred, owner_name: None });
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

/// What new spaces start with (Admin → Spaces → New spaces): the role everyone signed in gets
/// unless the creator picks another, and people or groups added to every new space.
#[derive(Serialize, Deserialize, Clone, Default)]
#[serde(rename_all = "camelCase")]
pub struct NewSpaceDefaults {
    #[serde(default = "editor_role")]
    pub default_role: String,
    #[serde(default)]
    pub grants: Vec<DefaultGrant>,
}

#[derive(Serialize, Deserialize, Clone)]
#[serde(rename_all = "camelCase")]
pub struct DefaultGrant {
    pub subject_type: String,
    pub subject_id: Uuid,
    pub role: String,
}

fn editor_role() -> String {
    "editor".into()
}

/// The workspace's [`NewSpaceDefaults`] (unset: everyone edits, nobody added).
pub async fn new_space_defaults(state: &AppState, ws: Uuid) -> ApiResult<NewSpaceDefaults> {
    let v: Option<Value> = sqlx::query_scalar("SELECT settings->'newSpaces' FROM workspaces WHERE id = $1").bind(ws).fetch_optional(&state.db).await?.flatten();
    Ok(v.and_then(|v| serde_json::from_value(v).ok()).unwrap_or(NewSpaceDefaults { default_role: editor_role(), grants: Vec::new() }))
}

/// Checks [`NewSpaceDefaults`] from an admin: known roles, and people and groups of this
/// workspace (at most 20 grants, one per subject).
pub async fn clean_new_space_defaults(state: &AppState, ws: Uuid, v: &Value) -> ApiResult<NewSpaceDefaults> {
    let d: NewSpaceDefaults = serde_json::from_value(v.clone()).map_err(|e| ApiError::bad(format!("newSpaces: {e}")))?;
    validate_role(&d.default_role)?;
    if d.default_role == "admin" {
        return Err(ApiError::bad("everyone can't be admin of every new space; pick editor at most"));
    }
    if d.grants.len() > 20 {
        return Err(ApiError::bad("at most 20 people or groups"));
    }
    let mut seen = std::collections::HashSet::new();
    for g in &d.grants {
        validate_role(&g.role)?;
        if g.role == "none" || !seen.insert((g.subject_type.clone(), g.subject_id)) {
            return Err(ApiError::bad("each person or group once, with a role"));
        }
        let known: bool = match g.subject_type.as_str() {
            "user" => sqlx::query_scalar("SELECT EXISTS (SELECT 1 FROM users WHERE id = $1 AND workspace_id = $2 AND status = 'active')"),
            "group" => sqlx::query_scalar("SELECT EXISTS (SELECT 1 FROM groups WHERE id = $1 AND workspace_id = $2)"),
            _ => return Err(ApiError::bad("subjectType must be user or group")),
        }
        .bind(g.subject_id)
        .bind(ws)
        .fetch_one(&state.db)
        .await?;
        if !known {
            return Err(ApiError::bad("unknown person or group"));
        }
    }
    Ok(d)
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
    let defaults = new_space_defaults(&state, user.workspace_id).await?;
    let default_role = req.default_role.clone().unwrap_or_else(|| defaults.default_role.clone());
    validate_role(&default_role)?;
    let icon = req.icon.clone().or_else(|| req.template.as_deref().and_then(crate::routes::space_templates::icon));
    let space = create_space(&state, user.workspace_id, Some(user.id), &key, name, req.description.as_deref(), icon.as_deref(), &default_role, None).await?;
    // creator becomes space admin
    sqlx::query("INSERT INTO space_permissions (space_id, subject_type, subject_id, role) VALUES ($1, 'user', $2, 'admin') ON CONFLICT DO NOTHING")
        .bind(space.id)
        .bind(user.id)
        .execute(&state.db)
        .await?;
    // people and groups every new space gets (still existing ones)
    for g in &defaults.grants {
        sqlx::query(
            "INSERT INTO space_permissions (space_id, subject_type, subject_id, role)
             SELECT $1, $2::text, $3, $4
              WHERE ($2::text = 'user' AND EXISTS (SELECT 1 FROM users WHERE id = $3 AND status = 'active'))
                 OR ($2::text = 'group' AND EXISTS (SELECT 1 FROM groups WHERE id = $3))
             ON CONFLICT DO NOTHING",
        )
        .bind(space.id)
        .bind(&g.subject_type)
        .bind(g.subject_id)
        .bind(&g.role)
        .execute(&state.db)
        .await?;
    }
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
            draft: false,
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

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CopySpace {
    key: String,
    name: String,
    /// the copies get their own attachments (sharing the stored files); default yes
    include_attachments: Option<bool>,
    /// copy the space's people and groups too; default yes
    include_permissions: Option<bool>,
}

/// `POST /api/spaces/{key}/copy {key, name}` (space admins): a new space with this one's
/// settings, templates, permissions and every published, not archived page the caller can
/// see; links between the pages point at the copies. Up to 10,000 pages.
pub async fn copy(
    State(state): State<AppState>,
    CurrentUser(user): CurrentUser,
    Path(key): Path<String>,
    Json(req): Json<CopySpace>,
) -> ApiResult<(StatusCode, Json<serde_json::Value>)> {
    if user.is_guest() {
        return Err(ApiError::Forbidden);
    }
    let p = Principal::load(&state, Some(user.clone())).await?;
    let (src, role) = load_space(&state, &p, &key).await?;
    if role < Role::Admin {
        return Err(ApiError::Forbidden);
    }
    if src.personal_owner_id.is_some() {
        return Err(ApiError::bad("personal spaces can't be copied; copy their pages instead"));
    }
    let new_key = validate_key(&req.key)?;
    let name = req.name.trim();
    if name.is_empty() {
        return Err(ApiError::bad("name is required"));
    }
    let former: Option<String> =
        sqlx::query_scalar("SELECT s.name FROM space_key_aliases a JOIN spaces s ON s.id = a.space_id WHERE a.workspace_id = $1 AND lower(a.key) = lower($2)")
            .bind(user.workspace_id)
            .bind(&new_key)
            .fetch_optional(&state.db)
            .await?;
    if let Some(n) = former {
        return Err(ApiError::Conflict(format!("the key {new_key} was used by the space “{n}” and still links there")));
    }
    let space =
        create_space(&state, user.workspace_id, Some(user.id), &new_key, name, src.description.as_deref(), src.icon.as_deref(), &src.default_role, None)
            .await?;
    sqlx::query("UPDATE spaces SET category = $2, look = $3 WHERE id = $1").bind(space.id).bind(&src.category).bind(&src.look).execute(&state.db).await?;
    sqlx::query("INSERT INTO space_permissions (space_id, subject_type, subject_id, role) VALUES ($1, 'user', $2, 'admin') ON CONFLICT DO NOTHING")
        .bind(space.id)
        .bind(user.id)
        .execute(&state.db)
        .await?;
    if req.include_permissions.unwrap_or(true) {
        sqlx::query(
            "INSERT INTO space_permissions (space_id, subject_type, subject_id, role) SELECT $2, subject_type, subject_id, role FROM space_permissions WHERE space_id = $1
             ON CONFLICT DO NOTHING",
        )
        .bind(src.id)
        .bind(space.id)
        .execute(&state.db)
        .await?;
    }
    // the space's own templates (and which one new pages start from)
    let templates: Vec<Uuid> = sqlx::query_scalar("SELECT id FROM templates WHERE space_id = $1").bind(src.id).fetch_all(&state.db).await?;
    let mut default_template = src.default_template.clone();
    let mut template_ids: Vec<(String, String)> = Vec::new();
    for t in templates {
        let copy = new_id();
        sqlx::query(
            "INSERT INTO templates (id, workspace_id, space_id, name, description, icon, title, content, created_by)
             SELECT $2, workspace_id, $3, name, description, icon, title, content, $4 FROM templates WHERE id = $1",
        )
        .bind(t)
        .bind(copy)
        .bind(space.id)
        .bind(user.id)
        .execute(&state.db)
        .await?;
        if default_template.as_deref() == Some(t.to_string().as_str()) {
            default_template = Some(copy.to_string());
        }
        template_ids.push((t.to_string(), copy.to_string()));
    }
    sqlx::query("UPDATE spaces SET default_template = $2 WHERE id = $1").bind(space.id).bind(&default_template).execute(&state.db).await?;

    // pages: what the caller sees, without drafts and archived sections
    let rows: Vec<(Uuid, Option<Uuid>, bool)> =
        sqlx::query_as("SELECT id, parent_id, draft FROM pages WHERE space_id = $1 AND deleted_at IS NULL").bind(src.id).fetch_all(&state.db).await?;
    let pairs: Vec<(Uuid, Option<Uuid>)> = rows.iter().map(|(id, parent, _)| (*id, *parent)).collect();
    let mut visible = crate::authz::visible_page_ids(&state, &p, src.id, role, &pairs).await?;
    let all: Vec<Uuid> = rows.iter().map(|r| r.0).collect();
    let archived = crate::authz::archived_among(&state, &all).await?;
    visible.retain(|id| !archived.contains(id) && !rows.iter().any(|(r, _, draft)| r == id && *draft));
    let roots: Vec<Uuid> = sqlx::query_scalar("SELECT id FROM pages WHERE space_id = $1 AND parent_id IS NULL AND deleted_at IS NULL ORDER BY position")
        .bind(src.id)
        .fetch_all(&state.db)
        .await?;
    let roots: Vec<(Uuid, Option<Uuid>)> = roots.into_iter().filter(|r| visible.contains(r)).map(|r| (r, None)).collect();
    let opts = crate::routes::pages::TreeCopy {
        actor: user.id,
        workspace: user.workspace_id,
        target_space: space.id,
        include_children: true,
        with_attachments: req.include_attachments.unwrap_or(true),
        relink: true,
        first_title: None,
        visible: &visible,
        limit: 10_000,
    };
    let copied = crate::routes::pages::copy_tree(&state, &opts, roots).await?;
    // templates for new pages below a page follow the copied space templates
    let (olds, news): (Vec<String>, Vec<String>) = template_ids.into_iter().unzip();
    sqlx::query(
        "UPDATE pages p SET child_template = m.new FROM unnest($2::text[], $3::text[]) AS m(old, new) WHERE p.space_id = $1 AND p.child_template = m.old",
    )
    .bind(space.id)
    .bind(&olds)
    .bind(&news)
    .execute(&state.db)
    .await?;
    // sidebar shortcuts, pointing at the copied pages
    let page_map: std::collections::HashMap<Uuid, Uuid> = copied.iter().copied().collect();
    let shortcuts: Value = serde_json::from_str(&crate::routes::pages::remap_uuids(&src.shortcuts.to_string(), &page_map)).unwrap_or(Value::Array(vec![]));
    sqlx::query("UPDATE spaces SET shortcuts = $2 WHERE id = $1")
        .bind(space.id)
        .bind(if shortcuts.is_array() { shortcuts } else { json!([]) })
        .execute(&state.db)
        .await?;
    let home = src.home_page_id.and_then(|h| copied.iter().find(|(s, _)| *s == h).map(|(_, n)| *n));
    sqlx::query("UPDATE spaces SET home_page_id = $2 WHERE id = $1").bind(space.id).bind(home).execute(&state.db).await?;
    audit::log(
        &state,
        user.workspace_id,
        Some(user.id),
        "space.copy",
        Some("space"),
        Some(space.id),
        json!({"from": src.key, "key": new_key, "pages": copied.len()}),
    )
    .await;
    crate::webhooks::emit(&state, user.workspace_id, "space.created", Some(space.id), json!({"space": {"id": space.id, "key": space.key, "name": space.name}}))
        .await;
    Ok((StatusCode::CREATED, Json(json!({ "id": space.id, "key": space.key, "pages": copied.len(), "truncated": copied.len() >= 10_000 }))))
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
        "INSERT INTO spaces (id, workspace_id, key, name, description, icon, default_role, created_by, owner_id, source_ref) VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $8, $9) RETURNING {SPACE_COLUMNS}"
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
            draft: false,
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
    let (watching, starred): (bool, bool) = match &p.user {
        Some(u) => {
            sqlx::query_as(
                "SELECT EXISTS (SELECT 1 FROM space_watches WHERE user_id = $1 AND space_id = $2),
                        EXISTS (SELECT 1 FROM space_favorites WHERE user_id = $1 AND space_id = $2)",
            )
            .bind(u.id)
            .bind(space.id)
            .fetch_one(&state.db)
            .await?
        }
        None => (false, false),
    };
    let owner_name: Option<String> = match space.owner_id {
        Some(o) => sqlx::query_scalar("SELECT name FROM users WHERE id = $1").bind(o).fetch_optional(&state.db).await?,
        None => None,
    };
    Ok(Json(SpaceView { space, role, page_count, watching, starred, owner_name }))
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
    /// sidebar links (replaces the list)
    shortcuts: Option<Vec<Shortcut>>,
    /// the space's look (replaces it): `{accentColor?, hideBlog?}`
    look: Option<Value>,
}

/// Checks a space look: an accent colour (`#rrggbb`; empty = the workspace's) and whether the
/// sidebar leaves out the blog.
pub fn clean_look(v: &Value) -> ApiResult<Value> {
    let o = v.as_object().ok_or_else(|| ApiError::bad("look must be an object"))?;
    let mut out = serde_json::Map::new();
    for (k, val) in o {
        match k.as_str() {
            "accentColor" => {
                let c = val.as_str().unwrap_or_default();
                if !c.trim().is_empty() {
                    let c = crate::routes::branding::clean_color(c).ok_or_else(|| ApiError::bad("accentColor must look like #1f6feb"))?;
                    out.insert(k.clone(), json!(c));
                }
            }
            "hideBlog" => {
                if val.as_bool().ok_or_else(|| ApiError::bad("hideBlog must be true or false"))? {
                    out.insert(k.clone(), json!(true));
                }
            }
            other => return Err(ApiError::bad(format!("unknown look setting {other}"))),
        }
    }
    Ok(Value::Object(out))
}

#[derive(Deserialize, Serialize, Clone)]
pub struct Shortcut {
    pub title: String,
    pub url: String,
}

/// Checks sidebar links: a title and a web address (http/https) or a path in this wiki.
pub fn clean_shortcuts(list: Vec<Shortcut>) -> ApiResult<Value> {
    if list.len() > 20 {
        return Err(ApiError::bad("at most 20 shortcuts"));
    }
    let mut out = Vec::new();
    for s in list {
        let title = s.title.split_whitespace().collect::<Vec<_>>().join(" ");
        let url = s.url.trim().to_string();
        if title.is_empty() || title.chars().count() > 80 {
            return Err(ApiError::bad("a shortcut needs a title of up to 80 characters"));
        }
        let lower = url.to_lowercase();
        let web = (lower.starts_with("https://") || lower.starts_with("http://")) && url.len() <= 2000;
        let internal = url.starts_with('/') && !url.starts_with("//") && !url.contains('\\') && url.len() <= 500;
        if !(web || internal) || url.chars().any(|c| c.is_whitespace() || c.is_control()) {
            return Err(ApiError::bad(format!("“{title}”: the link must be a web address (https://…) or a path in this wiki (/pages/…)")));
        }
        out.push(json!({ "title": title, "url": url }));
    }
    Ok(Value::Array(out))
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
    if let Some(list) = req.shortcuts {
        let list = clean_shortcuts(list)?;
        sqlx::query("UPDATE spaces SET shortcuts = $2 WHERE id = $1").bind(space.id).bind(list).execute(&state.db).await?;
    }
    if let Some(look) = &req.look {
        let look = clean_look(look)?;
        sqlx::query("UPDATE spaces SET look = $2 WHERE id = $1").bind(space.id).bind(look).execute(&state.db).await?;
    }
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

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SetOwner {
    user_id: Uuid,
}

/// `PUT /api/spaces/{key}/owner {userId}`: hands a space over (space admins). The new owner
/// becomes a space admin and is notified; the previous owner keeps their role.
pub async fn set_owner(
    State(state): State<AppState>,
    CurrentUser(user): CurrentUser,
    Path(key): Path<String>,
    Json(req): Json<SetOwner>,
) -> ApiResult<Json<SpaceView>> {
    let p = Principal::load(&state, Some(user.clone())).await?;
    let (space, role) = load_space(&state, &p, &key).await?;
    if role < Role::Admin {
        return Err(ApiError::Forbidden);
    }
    if space.personal_owner_id.is_some() {
        return Err(ApiError::bad("a personal space belongs to its person"));
    }
    let target: Option<(String, String, String)> = sqlx::query_as("SELECT name, role, status FROM users WHERE id = $1 AND workspace_id = $2")
        .bind(req.user_id)
        .bind(user.workspace_id)
        .fetch_optional(&state.db)
        .await?;
    let Some((name, target_role, status)) = target else { return Err(ApiError::bad("unknown person")) };
    if status != "active" {
        return Err(ApiError::bad(format!("{name} has no active account")));
    }
    if target_role == "guest" {
        return Err(ApiError::bad("guests can't own spaces"));
    }
    let mut tx = state.db.begin().await?;
    sqlx::query("UPDATE spaces SET owner_id = $2, updated_at = now() WHERE id = $1").bind(space.id).bind(req.user_id).execute(&mut *tx).await?;
    sqlx::query(
        "INSERT INTO space_permissions (space_id, subject_type, subject_id, role) VALUES ($1, 'user', $2, 'admin')
         ON CONFLICT (space_id, subject_type, subject_id) DO UPDATE SET role = 'admin'",
    )
    .bind(space.id)
    .bind(req.user_id)
    .execute(&mut *tx)
    .await?;
    if req.user_id != user.id {
        sqlx::query("INSERT INTO notifications (id, user_id, kind, actor_id, data) VALUES ($1, $2, 'space_owner', $3, $4)")
            .bind(new_id())
            .bind(req.user_id)
            .bind(user.id)
            .bind(json!({"spaceKey": space.key, "spaceName": space.name}))
            .execute(&mut *tx)
            .await?;
    }
    tx.commit().await?;
    audit::log(&state, user.workspace_id, Some(user.id), "space.owner", Some("space"), Some(space.id), json!({"owner": req.user_id, "name": name})).await;
    get(State(state), MaybeUser(Some(user)), Path(space.key)).await
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
    /// your unpublished draft (other people's never appear)
    #[sqlx(default)]
    #[serde(skip_serializing_if = "std::ops::Not::not")]
    pub draft: bool,
}

/// Flat list of all pages in a space the caller can see (client builds the tree).
pub async fn tree(State(state): State<AppState>, MaybeUser(user): MaybeUser, Path(key): Path<String>) -> ApiResult<Json<serde_json::Value>> {
    let p = Principal::load(&state, user).await?;
    let (space, role) = load_space(&state, &p, &key).await?;
    let rows = sqlx::query_as::<_, TreeNode>(
        "SELECT p.id, p.parent_id, p.title, p.kind, p.icon, p.position,
                EXISTS (SELECT 1 FROM page_restrictions r WHERE r.page_id = p.id) AS restricted, p.archived_at IS NOT NULL AS archived, p.draft,
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

/// `POST /spaces/{key}/trash/empty` — deletes everything in the space's trash for good (space
/// admins; trashed pages they can't see because of restrictions stay).
pub async fn empty_trash(State(state): State<AppState>, CurrentUser(user): CurrentUser, Path(key): Path<String>) -> ApiResult<Json<serde_json::Value>> {
    let p = Principal::load(&state, Some(user.clone())).await?;
    let (space, role) = load_space(&state, &p, &key).await?;
    if role < Role::Admin {
        return Err(ApiError::Forbidden);
    }
    let trashed: Vec<Uuid> = sqlx::query_scalar(
        "SELECT p.id FROM pages p LEFT JOIN pages parent ON parent.id = p.parent_id
          WHERE p.space_id = $1 AND p.deleted_at IS NOT NULL AND (parent.id IS NULL OR parent.deleted_at IS NULL)",
    )
    .bind(space.id)
    .fetch_all(&state.db)
    .await?;
    let pairs: Vec<(Uuid, Option<Uuid>)> = sqlx::query_as("SELECT id, parent_id FROM pages WHERE space_id = $1").bind(space.id).fetch_all(&state.db).await?;
    let visible = visible_page_ids(&state, &p, space.id, role, &pairs).await?;
    let roots: Vec<Uuid> = trashed.iter().copied().filter(|id| visible.contains(id)).collect();
    let gone = crate::purge::purge_pages(&state, &roots).await?;
    audit::log(&state, user.workspace_id, Some(user.id), "space.trash_emptied", Some("space"), Some(space.id), json!({"count": gone.len()})).await;
    Ok(Json(json!({ "purged": gone.len(), "kept": trashed.len() - roots.len() })))
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

/// `PUT /api/spaces/{key}/star`: the space shows up first in the directory and in the sidebar.
pub async fn star(State(state): State<AppState>, CurrentUser(user): CurrentUser, Path(key): Path<String>) -> ApiResult<StatusCode> {
    let p = Principal::load(&state, Some(user.clone())).await?;
    let (space, _) = load_space(&state, &p, &key).await?;
    sqlx::query("INSERT INTO space_favorites (user_id, space_id) VALUES ($1, $2) ON CONFLICT DO NOTHING")
        .bind(user.id)
        .bind(space.id)
        .execute(&state.db)
        .await?;
    Ok(StatusCode::NO_CONTENT)
}

/// `DELETE /api/spaces/{key}/star`
pub async fn unstar(State(state): State<AppState>, CurrentUser(user): CurrentUser, Path(key): Path<String>) -> ApiResult<StatusCode> {
    let p = Principal::load(&state, Some(user.clone())).await?;
    let (space, _) = load_space(&state, &p, &key).await?;
    sqlx::query("DELETE FROM space_favorites WHERE user_id = $1 AND space_id = $2").bind(user.id).bind(space.id).execute(&state.db).await?;
    Ok(StatusCode::NO_CONTENT)
}

/// `DELETE /api/spaces/{key}/watch`
pub async fn unwatch(State(state): State<AppState>, CurrentUser(user): CurrentUser, Path(key): Path<String>) -> ApiResult<StatusCode> {
    let p = Principal::load(&state, Some(user.clone())).await?;
    let (space, _) = load_space(&state, &p, &key).await?;
    sqlx::query("DELETE FROM space_watches WHERE user_id = $1 AND space_id = $2").bind(user.id).bind(space.id).execute(&state.db).await?;
    Ok(StatusCode::NO_CONTENT)
}

fn xml_escape(s: &str) -> String {
    s.replace('&', "&amp;").replace('<', "&lt;").replace('>', "&gt;").replace('"', "&quot;").replace('\'', "&apos;")
}

/// `GET /api/spaces/{key}/blog.atom` — the space's blog as an Atom feed: the latest 20 posts
/// the reader may see, with their text (no drafts). Public spaces work without signing in;
/// feed readers can send a (read-only) API token as `Authorization: Bearer …`.
pub async fn blog_feed(State(state): State<AppState>, MaybeUser(user): MaybeUser, Path(key): Path<String>) -> ApiResult<axum::response::Response> {
    use axum::response::IntoResponse;
    let p = Principal::load(&state, user).await?;
    let (space, role) = load_space(&state, &p, &key).await?;
    type Row = (Uuid, String, serde_json::Value, chrono::DateTime<chrono::Utc>, chrono::DateTime<chrono::Utc>, Option<String>);
    let rows: Vec<Row> = sqlx::query_as(
        "SELECT p.id, p.title, p.content, p.created_at, p.updated_at, u.name FROM pages p LEFT JOIN users u ON u.id = p.created_by
          WHERE p.space_id = $1 AND p.kind = 'blog' AND p.deleted_at IS NULL AND p.archived_at IS NULL AND NOT p.draft
          ORDER BY p.created_at DESC LIMIT 60",
    )
    .bind(space.id)
    .fetch_all(&state.db)
    .await?;
    let pairs: Vec<(Uuid, Option<Uuid>)> =
        sqlx::query_as("SELECT id, parent_id FROM pages WHERE space_id = $1 AND deleted_at IS NULL").bind(space.id).fetch_all(&state.db).await?;
    let visible = visible_page_ids(&state, &p, space.id, role, &pairs).await?;
    let base = state.config.base_url.trim_end_matches('/').to_string();
    let posts: Vec<&Row> = rows.iter().filter(|r| visible.contains(&r.0)).take(20).collect();
    let updated = posts.iter().map(|r| r.4).max().unwrap_or(space.updated_at);
    let mut out = String::from("<?xml version=\"1.0\" encoding=\"utf-8\"?>\n<feed xmlns=\"http://www.w3.org/2005/Atom\">\n");
    out.push_str(&format!(
        "  <title>{} · Blog</title>\n  <id>{base}/spaces/{}/blog</id>\n  <link rel=\"alternate\" type=\"text/html\" href=\"{base}/spaces/{}/blog\"/>\n  <link rel=\"self\" href=\"{base}/api/spaces/{}/blog.atom\"/>\n  <updated>{}</updated>\n",
        xml_escape(&space.name),
        xml_escape(&space.key),
        xml_escape(&space.key),
        xml_escape(&space.key),
        updated.to_rfc3339()
    ));
    for (id, title, content, created, updated, author) in posts {
        let doc = wikicraft_doc_model::Node::from_json(content).unwrap_or_else(|_| wikicraft_doc_model::Node::empty_doc());
        // feed readers need absolute links (images, attachments, pages)
        let html = wikicraft_doc_model::html::render(&doc, &wikicraft_doc_model::html::HtmlOptions::default())
            .replace("src=\"/", &format!("src=\"{base}/"))
            .replace("href=\"/", &format!("href=\"{base}/"));
        out.push_str(&format!(
            "  <entry>\n    <title>{}</title>\n    <id>{base}/pages/{id}</id>\n    <link rel=\"alternate\" type=\"text/html\" href=\"{base}/pages/{id}\"/>\n    <published>{}</published>\n    <updated>{}</updated>\n    <author><name>{}</name></author>\n    <content type=\"html\">{}</content>\n  </entry>\n",
            xml_escape(title),
            created.to_rfc3339(),
            updated.to_rfc3339(),
            xml_escape(author.as_deref().unwrap_or("Unknown")),
            xml_escape(&html)
        ));
    }
    out.push_str("</feed>\n");
    Ok(([(axum::http::header::CONTENT_TYPE, "application/atom+xml; charset=utf-8"), (axum::http::header::CACHE_CONTROL, "private, max-age=300")], out)
        .into_response())
}
