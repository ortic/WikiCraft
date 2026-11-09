//! Authorization: workspace roles → space roles → page restrictions.
//!
//! * Workspace `owner`/`admin`: full access everywhere.
//! * Space role = max(explicit user grant, group grants, space default role for non-guest
//!   members); anonymous users get `viewer` on spaces with anonymous access.
//! * Page **view** restrictions are inherited: a user must satisfy the restriction of the page
//!   and of every restricted ancestor. **Edit** restrictions apply to the page itself.
//! * Space admins bypass page restrictions (they can always fix them).

use std::collections::{HashMap, HashSet};

use serde::Serialize;
use uuid::Uuid;

use crate::app::AppState;
use crate::auth::User;
use crate::error::{ApiError, ApiResult};

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum Role {
    None,
    Viewer,
    Commenter,
    Editor,
    Admin,
}

impl Role {
    pub fn parse(s: &str) -> Role {
        match s {
            "viewer" => Role::Viewer,
            "commenter" => Role::Commenter,
            "editor" => Role::Editor,
            "admin" => Role::Admin,
            _ => Role::None,
        }
    }

    pub fn as_str(self) -> &'static str {
        match self {
            Role::None => "none",
            Role::Viewer => "viewer",
            Role::Commenter => "commenter",
            Role::Editor => "editor",
            Role::Admin => "admin",
        }
    }
}

/// Who is asking: user (or anonymous) plus group memberships.
#[derive(Debug, Clone)]
pub struct Principal {
    pub user: Option<User>,
    pub groups: HashSet<Uuid>,
}

impl Principal {
    pub async fn load(state: &AppState, user: Option<User>) -> ApiResult<Self> {
        let groups = match &user {
            Some(u) => sqlx::query_scalar::<_, Uuid>("SELECT group_id FROM group_members WHERE user_id = $1")
                .bind(u.id)
                .fetch_all(&state.db)
                .await?
                .into_iter()
                .collect(),
            None => HashSet::new(),
        };
        Ok(Self { user, groups })
    }

    pub fn user_id(&self) -> Option<Uuid> {
        self.user.as_ref().map(|u| u.id)
    }

    pub fn is_admin(&self) -> bool {
        self.user.as_ref().is_some_and(User::is_admin)
    }

    pub fn matches(&self, subject_type: &str, subject_id: Uuid) -> bool {
        match subject_type {
            "user" => self.user_id() == Some(subject_id),
            "group" => self.groups.contains(&subject_id),
            _ => false,
        }
    }

    pub fn require_user(&self) -> ApiResult<&User> {
        self.user.as_ref().ok_or(ApiError::Unauthorized)
    }
}

#[derive(Debug, Clone, sqlx::FromRow)]
pub struct SpaceAccessRow {
    pub id: Uuid,
    pub workspace_id: Uuid,
    pub default_role: String,
    pub anonymous_access: bool,
    pub personal_owner_id: Option<Uuid>,
}

/// Effective role of `p` in a space.
pub async fn space_role(state: &AppState, p: &Principal, space: &SpaceAccessRow) -> ApiResult<Role> {
    let Some(user) = &p.user else {
        return Ok(if space.anonymous_access { Role::Viewer } else { Role::None });
    };
    if user.workspace_id != space.workspace_id {
        return Ok(Role::None);
    }
    if user.is_admin() || space.personal_owner_id == Some(user.id) {
        return Ok(Role::Admin);
    }
    let groups: Vec<Uuid> = p.groups.iter().copied().collect();
    let grants: Vec<String> = sqlx::query_scalar(
        "SELECT role FROM space_permissions WHERE space_id = $1 AND ((subject_type = 'user' AND subject_id = $2) OR (subject_type = 'group' AND subject_id = ANY($3)))",
    )
    .bind(space.id)
    .bind(user.id)
    .bind(&groups)
    .fetch_all(&state.db)
    .await?;
    let mut role = grants.iter().map(|r| Role::parse(r)).max().unwrap_or(Role::None);
    if !user.is_guest() {
        role = role.max(Role::parse(&space.default_role));
    }
    if space.anonymous_access {
        role = role.max(Role::Viewer);
    }
    Ok(role)
}

pub async fn load_space_access(state: &AppState, space_id: Uuid) -> ApiResult<SpaceAccessRow> {
    sqlx::query_as::<_, SpaceAccessRow>("SELECT id, workspace_id, default_role, anonymous_access, personal_owner_id FROM spaces WHERE id = $1")
        .bind(space_id)
        .fetch_optional(&state.db)
        .await?
        .ok_or(ApiError::NotFound("space"))
}

/// What a principal may do with a page.
#[derive(Debug, Clone, Copy, Serialize, Default)]
#[serde(rename_all = "camelCase")]
pub struct PageAccess {
    pub view: bool,
    pub comment: bool,
    pub edit: bool,
    /// May change restrictions, delete and move.
    pub admin: bool,
    /// Effective space role.
    #[serde(skip)]
    pub space_role: Option<Role>,
}

#[derive(Debug, sqlx::FromRow)]
struct ChainRow {
    id: Uuid,
    depth: i32,
    space_id: Uuid,
    deleted: bool,
}

#[derive(Debug, sqlx::FromRow)]
struct RestrictionRow {
    page_id: Uuid,
    kind: String,
    subject_type: String,
    subject_id: Uuid,
}

/// Computes access for one page (walks the ancestor chain for inherited view restrictions).
pub async fn page_access(state: &AppState, p: &Principal, page_id: Uuid) -> ApiResult<(PageAccess, Uuid)> {
    let chain = sqlx::query_as::<_, ChainRow>(
        "WITH RECURSIVE chain AS (
            SELECT id, parent_id, space_id, deleted_at IS NOT NULL AS deleted, 0 AS depth FROM pages WHERE id = $1
            UNION ALL
            SELECT p.id, p.parent_id, p.space_id, p.deleted_at IS NOT NULL, c.depth + 1 FROM pages p JOIN chain c ON p.id = c.parent_id WHERE c.depth < 200
         ) SELECT id, depth, space_id, deleted FROM chain",
    )
    .bind(page_id)
    .fetch_all(&state.db)
    .await?;
    let Some(first) = chain.iter().find(|c| c.depth == 0) else { return Err(ApiError::NotFound("page")) };
    if first.deleted {
        // trashed pages are only reachable through the trash/restore endpoints
        return Err(ApiError::NotFound("page"));
    }
    let space_id = first.space_id;
    let space = load_space_access(state, space_id).await?;
    let role = space_role(state, p, &space).await?;
    let mut access = PageAccess { space_role: Some(role), ..Default::default() };
    if role == Role::None {
        return Ok((access, space_id));
    }
    let ids: Vec<Uuid> = chain.iter().map(|c| c.id).collect();
    let restrictions = sqlx::query_as::<_, RestrictionRow>("SELECT page_id, kind, subject_type, subject_id FROM page_restrictions WHERE page_id = ANY($1)")
        .bind(&ids)
        .fetch_all(&state.db)
        .await?;
    let bypass = role == Role::Admin;
    let mut view_ok = true;
    let mut edit_ok = true;
    let mut by_page: HashMap<(Uuid, &str), Vec<&RestrictionRow>> = HashMap::new();
    for r in &restrictions {
        by_page.entry((r.page_id, r.kind.as_str())).or_default().push(r);
    }
    for c in &chain {
        if let Some(rs) = by_page.get(&(c.id, "view"))
            && !rs.iter().any(|r| p.matches(&r.subject_type, r.subject_id))
        {
            view_ok = false;
        }
    }
    if let Some(rs) = by_page.get(&(page_id, "edit"))
        && !rs.iter().any(|r| p.matches(&r.subject_type, r.subject_id))
    {
        edit_ok = false;
    }
    if bypass {
        view_ok = true;
        edit_ok = true;
    }
    access.view = view_ok;
    access.comment = view_ok && role >= Role::Commenter && p.user.is_some();
    access.edit = view_ok && edit_ok && role >= Role::Editor && p.user.is_some();
    access.admin = view_ok && (role == Role::Admin || (access.edit && p.user.is_some()));
    Ok((access, space_id))
}

pub async fn require_page(state: &AppState, p: &Principal, page_id: Uuid, need: Need) -> ApiResult<(PageAccess, Uuid)> {
    let (a, space) = page_access(state, p, page_id).await?;
    if !a.view {
        // don't reveal existence of pages the user can't see
        return Err(if p.user.is_none() { ApiError::Unauthorized } else { ApiError::NotFound("page") });
    }
    let ok = match need {
        Need::View => true,
        Need::Comment => a.comment,
        Need::Edit => a.edit,
        Need::Admin => a.admin,
    };
    if !ok {
        return Err(if p.user.is_none() { ApiError::Unauthorized } else { ApiError::Forbidden });
    }
    Ok((a, space))
}

#[derive(Debug, Clone, Copy)]
pub enum Need {
    View,
    Comment,
    Edit,
    Admin,
}

/// Given all pages of a space (id, parent) and their view restrictions, returns the set of
/// page ids visible to `p` (inherited view restrictions; space admins see everything).
pub async fn visible_page_ids(state: &AppState, p: &Principal, space_id: Uuid, role: Role, pages: &[(Uuid, Option<Uuid>)]) -> ApiResult<HashSet<Uuid>> {
    if role == Role::None {
        return Ok(HashSet::new());
    }
    if role == Role::Admin {
        return Ok(pages.iter().map(|(id, _)| *id).collect());
    }
    let restrictions = sqlx::query_as::<_, RestrictionRow>(
        "SELECT r.page_id, r.kind, r.subject_type, r.subject_id FROM page_restrictions r JOIN pages p ON p.id = r.page_id WHERE p.space_id = $1 AND r.kind = 'view'",
    )
    .bind(space_id)
    .fetch_all(&state.db)
    .await?;
    if restrictions.is_empty() {
        return Ok(pages.iter().map(|(id, _)| *id).collect());
    }
    let mut denied_here: HashSet<Uuid> = HashSet::new();
    let mut by_page: HashMap<Uuid, Vec<&RestrictionRow>> = HashMap::new();
    for r in &restrictions {
        by_page.entry(r.page_id).or_default().push(r);
    }
    for (pid, rs) in &by_page {
        if !rs.iter().any(|r| p.matches(&r.subject_type, r.subject_id)) {
            denied_here.insert(*pid);
        }
    }
    let parent: HashMap<Uuid, Option<Uuid>> = pages.iter().copied().collect();
    let mut memo: HashMap<Uuid, bool> = HashMap::new();
    fn visible(id: Uuid, parent: &HashMap<Uuid, Option<Uuid>>, denied: &HashSet<Uuid>, memo: &mut HashMap<Uuid, bool>, depth: usize) -> bool {
        if let Some(v) = memo.get(&id) {
            return *v;
        }
        let v = !denied.contains(&id)
            && match parent.get(&id).copied().flatten() {
                Some(par) if depth < 500 && parent.contains_key(&par) => visible(par, parent, denied, memo, depth + 1),
                _ => true,
            };
        memo.insert(id, v);
        v
    }
    Ok(pages.iter().filter(|(id, _)| visible(*id, &parent, &denied_here, &mut memo, 0)).map(|(id, _)| *id).collect())
}
