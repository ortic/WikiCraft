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

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Default)]
#[serde(rename_all = "lowercase")]
pub enum Role {
    #[default]
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

/// Space permissions (Confluence's set). A role brings the ones at or below it; a space grant may
/// leave some out (`space_permissions.denied`). Admin grants always carry all of them. Viewing is
/// the role itself, and so is editing page content (editor and up).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Perm {
    AddPages,
    DeletePages,
    AddBlog,
    DeleteBlog,
    AddComments,
    DeleteComments,
    AddAttachments,
    DeleteAttachments,
    Restrict,
    Export,
    Archive,
    /// delete pages, blog posts, comments and files one created, without the delete permissions
    DeleteOwn,
}

impl Perm {
    pub const ALL: [Perm; 12] = [
        Perm::AddPages,
        Perm::DeletePages,
        Perm::AddBlog,
        Perm::DeleteBlog,
        Perm::AddComments,
        Perm::DeleteComments,
        Perm::AddAttachments,
        Perm::DeleteAttachments,
        Perm::Restrict,
        Perm::Export,
        Perm::Archive,
        Perm::DeleteOwn,
    ];

    pub fn key(self) -> &'static str {
        match self {
            Perm::AddPages => "page.add",
            Perm::DeletePages => "page.delete",
            Perm::AddBlog => "blog.add",
            Perm::DeleteBlog => "blog.delete",
            Perm::AddComments => "comment.add",
            Perm::DeleteComments => "comment.delete",
            Perm::AddAttachments => "attachment.add",
            Perm::DeleteAttachments => "attachment.delete",
            Perm::Restrict => "restrict",
            Perm::Export => "export",
            Perm::Archive => "archive",
            Perm::DeleteOwn => "delete.own",
        }
    }

    pub fn parse(s: &str) -> Option<Perm> {
        Perm::ALL.into_iter().find(|p| p.key() == s)
    }

    /// The least role that brings the permission.
    pub fn min_role(self) -> Role {
        match self {
            Perm::Export => Role::Viewer,
            Perm::AddComments | Perm::DeleteOwn => Role::Commenter,
            _ => Role::Editor,
        }
    }

    /// The Confluence space permission it stands for.
    pub fn confluence(self) -> &'static str {
        match self {
            Perm::AddPages => "EDITSPACE",
            Perm::DeletePages => "REMOVEPAGE",
            Perm::AddBlog => "EDITBLOG",
            Perm::DeleteBlog => "REMOVEBLOG",
            Perm::AddComments => "COMMENT",
            Perm::DeleteComments => "REMOVECOMMENT",
            Perm::AddAttachments => "CREATEATTACHMENT",
            Perm::DeleteAttachments => "REMOVEATTACHMENT",
            Perm::Restrict => "SETPAGEPERMISSIONS",
            Perm::Export => "EXPORTSPACE",
            Perm::Archive => "ARCHIVEPAGE",
            Perm::DeleteOwn => "REMOVEOWNCONTENT",
        }
    }

    fn bit(self) -> u16 {
        1 << (self as u16)
    }
}

/// A set of [`Perm`]s (serialised as their keys).
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct Perms(u16);

impl Perms {
    pub fn all() -> Perms {
        Perms(Perm::ALL.iter().fold(0, |acc, p| acc | p.bit()))
    }

    /// What a role brings when nothing is left out.
    pub fn of_role(role: Role) -> Perms {
        if role == Role::Admin {
            return Perms::all();
        }
        Perms(Perm::ALL.iter().filter(|p| role >= p.min_role()).fold(0, |acc, p| acc | p.bit()))
    }

    /// What a grant brings: its role's permissions minus the denied ones (admins keep all).
    pub fn of_grant(role: Role, denied: &[String]) -> Perms {
        if role == Role::Admin {
            return Perms::all();
        }
        let mut perms = Perms::of_role(role);
        for d in denied.iter().filter_map(|d| Perm::parse(d)) {
            perms.0 &= !d.bit();
        }
        perms
    }

    pub fn has(self, p: Perm) -> bool {
        self.0 & p.bit() != 0
    }

    pub fn with(self, p: Perm) -> Perms {
        Perms(self.0 | p.bit())
    }

    pub fn union(self, other: Perms) -> Perms {
        Perms(self.0 | other.0)
    }

    pub fn intersect(self, other: Perms) -> Perms {
        Perms(self.0 & other.0)
    }

    pub fn keys(self) -> Vec<&'static str> {
        Perm::ALL.into_iter().filter(|p| self.has(*p)).map(Perm::key).collect()
    }
}

impl Serialize for Perms {
    fn serialize<S: serde::Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
        self.keys().serialize(s)
    }
}

/// The permissions a grant of `role` leaves out, cleaned: known keys the role would bring, in
/// canonical order (nothing for admins).
pub fn clean_denied(role: Role, denied: &[String]) -> Vec<String> {
    if role == Role::Admin {
        return Vec::new();
    }
    let full = Perms::of_role(role);
    Perm::ALL.into_iter().filter(|p| full.has(*p) && denied.iter().any(|d| d == p.key())).map(|p| p.key().to_string()).collect()
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
    /// permissions the default role leaves out
    #[sqlx(default)]
    pub default_denied: Vec<String>,
    pub anonymous_access: bool,
    pub personal_owner_id: Option<Uuid>,
    /// archived spaces are read-only for everyone
    #[sqlx(default)]
    pub archived: bool,
}

/// Effective role of `p` in a space (at most viewer while the space is archived).
pub async fn space_role(state: &AppState, p: &Principal, space: &SpaceAccessRow) -> ApiResult<Role> {
    Ok(space_access(state, p, space).await?.role)
}

/// Role and permissions of a principal in a space.
#[derive(Debug, Clone, Copy, Default)]
pub struct SpaceAccess {
    pub role: Role,
    pub perms: Perms,
}

/// Effective role and permissions of `p` in a space: the best role over the person's grants (and
/// the space default for members), with the permissions of all of them together. Archived spaces
/// are read-only: at most viewer.
pub async fn space_access(state: &AppState, p: &Principal, space: &SpaceAccessRow) -> ApiResult<SpaceAccess> {
    let a = space_access_unarchived(state, p, space).await?;
    Ok(if space.archived {
        let role = a.role.min(Role::Viewer);
        SpaceAccess { role, perms: a.perms.intersect(Perms::of_role(role)) }
    } else {
        a
    })
}

async fn space_access_unarchived(state: &AppState, p: &Principal, space: &SpaceAccessRow) -> ApiResult<SpaceAccess> {
    let full = |role: Role| SpaceAccess { role, perms: Perms::of_role(role) };
    let Some(user) = &p.user else {
        return Ok(full(if space.anonymous_access { Role::Viewer } else { Role::None }));
    };
    if user.workspace_id != space.workspace_id {
        return Ok(full(Role::None));
    }
    if user.is_admin() || space.personal_owner_id == Some(user.id) {
        return Ok(full(Role::Admin));
    }
    let groups: Vec<Uuid> = p.groups.iter().copied().collect();
    let grants: Vec<(String, Vec<String>)> = sqlx::query_as(
        "SELECT role, denied FROM space_permissions WHERE space_id = $1 AND ((subject_type = 'user' AND subject_id = $2) OR (subject_type = 'group' AND subject_id = ANY($3)))",
    )
    .bind(space.id)
    .bind(user.id)
    .bind(&groups)
    .fetch_all(&state.db)
    .await?;
    let mut a = SpaceAccess::default();
    for (role, denied) in &grants {
        let role = Role::parse(role);
        a.role = a.role.max(role);
        a.perms = a.perms.union(Perms::of_grant(role, denied));
    }
    if !user.is_guest() {
        let d = Role::parse(&space.default_role);
        a.role = a.role.max(d);
        a.perms = a.perms.union(Perms::of_grant(d, &space.default_denied));
    }
    if space.anonymous_access {
        a.role = a.role.max(Role::Viewer);
        a.perms = a.perms.union(Perms::of_role(Role::Viewer));
    }
    if a.role == Role::Admin {
        a.perms = Perms::all();
    }
    Ok(a)
}

pub async fn load_space_access(state: &AppState, space_id: Uuid) -> ApiResult<SpaceAccessRow> {
    sqlx::query_as::<_, SpaceAccessRow>(
        "SELECT id, workspace_id, default_role, default_denied, anonymous_access, personal_owner_id, archived_at IS NOT NULL AS archived FROM spaces WHERE id = $1",
    )
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
    /// The archived page this one is (or is under): read-only until restored.
    pub archived_page: Option<Uuid>,
    /// The caller's space permissions (none while the page can't be changed: archived).
    pub perms: Perms,
    /// The caller created the page (what "delete own" covers).
    pub own: bool,
}

impl PageAccess {
    pub fn can(&self, perm: Perm) -> bool {
        self.perms.has(perm)
    }

    /// 403 unless the caller has `perm` in the page's space.
    pub fn require(&self, perm: Perm) -> ApiResult<()> {
        if self.can(perm) { Ok(()) } else { Err(ApiError::Forbidden) }
    }

    /// May delete something: with the delete permission, or with "delete own" when `mine`.
    pub fn may_delete(&self, perm: Perm, mine: bool) -> bool {
        self.can(perm) || (mine && self.can(Perm::DeleteOwn))
    }
}

#[derive(Debug, sqlx::FromRow)]
struct ChainRow {
    id: Uuid,
    depth: i32,
    space_id: Uuid,
    deleted: bool,
    archived: bool,
    /// an unpublished draft by someone else (hides it and everything below it)
    foreign_draft: bool,
    /// created by the caller
    mine: bool,
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
            SELECT id, parent_id, space_id, deleted_at IS NOT NULL AS deleted, archived_at IS NOT NULL AS archived,
                   draft AND created_by IS DISTINCT FROM $2 AS foreign_draft, COALESCE(created_by = $2, false) AS mine, 0 AS depth FROM pages WHERE id = $1
            UNION ALL
            SELECT p.id, p.parent_id, p.space_id, p.deleted_at IS NOT NULL, p.archived_at IS NOT NULL,
                   p.draft AND p.created_by IS DISTINCT FROM $2, COALESCE(p.created_by = $2, false), c.depth + 1 FROM pages p JOIN chain c ON p.id = c.parent_id WHERE c.depth < 200
         ) SELECT id, depth, space_id, deleted, archived, foreign_draft, mine FROM chain",
    )
    .bind(page_id)
    .bind(p.user_id())
    .fetch_all(&state.db)
    .await?;
    let Some(first) = chain.iter().find(|c| c.depth == 0) else { return Err(ApiError::NotFound("page")) };
    if first.deleted {
        // trashed pages are only reachable through the trash/restore endpoints
        return Err(ApiError::NotFound("page"));
    }
    let space_id = first.space_id;
    let space = load_space_access(state, space_id).await?;
    let sa = space_access(state, p, &space).await?;
    let role = sa.role;
    let mut access = PageAccess { space_role: Some(role), own: first.mine, ..Default::default() };
    if role == Role::None || chain.iter().any(|c| c.foreign_draft) {
        // someone else's unpublished draft: not even space admins see it
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
    // archived (itself or an ancestor): read-only, but whoever could edit may restore it
    let archived = chain.iter().filter(|c| c.archived).min_by_key(|c| c.depth).map(|c| c.id);
    let could_edit = view_ok && edit_ok && role >= Role::Editor && p.user.is_some();
    access.view = view_ok;
    access.comment = view_ok && role >= Role::Commenter && p.user.is_some() && archived.is_none() && sa.perms.has(Perm::AddComments);
    access.edit = could_edit && archived.is_none();
    access.admin = view_ok && (role == Role::Admin || could_edit);
    access.archived_page = archived;
    // archived pages: only exporting and restoring from the archive is left
    access.perms = if !view_ok {
        Perms::default()
    } else if archived.is_some() {
        sa.perms.intersect(Perms::of_role(Role::Viewer).with(Perm::Archive))
    } else if !edit_ok {
        // an edit restriction keeps the page's content and files as they are
        sa.perms.intersect(Perms::of_role(Role::Commenter))
    } else {
        sa.perms
    };
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
    // other people's unpublished drafts are hidden from everyone, space admins included
    let mut denied_here: HashSet<Uuid> = foreign_drafts(state, p, space_id).await?;
    let restrictions = if role == Role::Admin {
        Vec::new()
    } else {
        sqlx::query_as::<_, RestrictionRow>(
            "SELECT r.page_id, r.kind, r.subject_type, r.subject_id FROM page_restrictions r JOIN pages p ON p.id = r.page_id WHERE p.space_id = $1 AND r.kind = 'view'",
        )
        .bind(space_id)
        .fetch_all(&state.db)
        .await?
    };
    if restrictions.is_empty() && denied_here.is_empty() {
        return Ok(pages.iter().map(|(id, _)| *id).collect());
    }
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

/// Whether the page or a page above it is an unpublished draft (only its creator sees it, so
/// nobody else is notified about it and webhooks stay quiet).
pub async fn in_draft(state: &AppState, page_id: Uuid) -> bool {
    sqlx::query_scalar::<_, Option<bool>>(
        "WITH RECURSIVE up AS (
            SELECT id, parent_id, draft, 0 AS depth FROM pages WHERE id = $1
            UNION ALL SELECT p.id, p.parent_id, p.draft, up.depth + 1 FROM pages p JOIN up ON p.id = up.parent_id WHERE up.depth < 200
         ) SELECT bool_or(draft) FROM up",
    )
    .bind(page_id)
    .fetch_one(&state.db)
    .await
    .ok()
    .flatten()
    .unwrap_or(false)
}

/// Unpublished drafts in a space that belong to someone other than `p`.
pub async fn foreign_drafts(state: &AppState, p: &Principal, space_id: Uuid) -> ApiResult<HashSet<Uuid>> {
    Ok(sqlx::query_scalar("SELECT id FROM pages WHERE space_id = $1 AND draft AND created_by IS DISTINCT FROM $2")
        .bind(space_id)
        .bind(p.user_id())
        .fetch_all(&state.db)
        .await?
        .into_iter()
        .collect())
}

/// The pages among `ids` that are archived or under an archived page.
pub async fn archived_among(state: &AppState, ids: &[Uuid]) -> ApiResult<std::collections::HashSet<Uuid>> {
    if ids.is_empty() {
        return Ok(Default::default());
    }
    let rows: Vec<Uuid> = sqlx::query_scalar(
        "WITH RECURSIVE up AS (
            SELECT id AS start, parent_id, archived_at, 0 AS depth FROM pages WHERE id = ANY($1)
            UNION ALL
            SELECT up.start, p.parent_id, p.archived_at, up.depth + 1 FROM pages p JOIN up ON p.id = up.parent_id WHERE up.depth < 200
         ) SELECT DISTINCT start FROM up WHERE archived_at IS NOT NULL",
    )
    .bind(ids)
    .fetch_all(&state.db)
    .await?;
    Ok(rows.into_iter().collect())
}
