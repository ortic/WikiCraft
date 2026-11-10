//! Turns a parsed [`Export`] into an import plan: what to create, in which order, with which
//! history. Document conversion is lazy ([`Planner::convert`]) so huge histories don't have to
//! be held in memory as ProseMirror trees.

use std::collections::{BTreeMap, HashMap, HashSet};

use serde::Serialize;
use wikicraft_doc_model::Node;

use crate::model::*;
use crate::storage::{self, Resolver, Stats};

/// Options controlling what gets imported.
#[derive(Debug, Clone, Serialize)]
pub struct PlanOptions {
    pub include_history: bool,
    /// Never-published pages (status `draft`, no `prevver`): imported as private drafts of their
    /// creator. Drafts of published pages (unpublished edits) are never imported.
    pub include_drafts: bool,
    /// Pages in the space trash (status `deleted`): imported into the trash.
    pub include_deleted: bool,
    pub include_comments: bool,
    pub include_attachments: bool,
}

impl Default for PlanOptions {
    fn default() -> Self {
        Self { include_history: true, include_drafts: true, include_deleted: true, include_comments: true, include_attachments: true }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum NodeKind {
    Page,
    Folder,
    BlogPost,
    Whiteboard,
    Database,
    Embed,
}

#[derive(Debug, Clone, Serialize)]
pub struct VersionPlan {
    pub source_id: String,
    pub version: i64,
    pub title: String,
    pub author: Option<String>,
    pub created_at: Option<String>,
    pub message: Option<String>,
}

#[derive(Debug, Clone, Serialize)]
pub struct AttachmentVersionPlan {
    pub source_id: String,
    pub version: i64,
    /// Path of the binary inside the export (None = binary missing).
    pub path: Option<String>,
    pub author: Option<String>,
    pub created_at: Option<String>,
    pub comment: Option<String>,
}

#[derive(Debug, Clone, Serialize)]
pub struct AttachmentPlan {
    pub source_id: String,
    pub filename: String,
    pub media_type: Option<String>,
    pub size: Option<i64>,
    pub created_by: Option<String>,
    pub created_at: Option<String>,
    /// Oldest first; the last entry is the current version.
    pub versions: Vec<AttachmentVersionPlan>,
}

#[derive(Debug, Clone, Serialize)]
pub struct CommentPlan {
    pub source_id: String,
    pub parent_source_id: Option<String>,
    pub author: Option<String>,
    pub created_at: Option<String>,
    pub updated_at: Option<String>,
    /// Inline comment: marker ref used in the page body (`ac:inline-comment-marker ac:ref`).
    pub inline_marker_ref: Option<String>,
    pub inline_original_selection: Option<String>,
    pub resolved: bool,
}

#[derive(Debug, Clone, Serialize)]
pub struct PagePlan {
    pub source_id: String,
    pub parent_source_id: Option<String>,
    pub kind: NodeKind,
    pub title: String,
    pub position: Option<i64>,
    pub status: ContentStatus,
    pub version: i64,
    pub created_by: Option<String>,
    pub created_at: Option<String>,
    pub updated_by: Option<String>,
    pub updated_at: Option<String>,
    pub labels: Vec<String>,
    /// Previous versions, oldest first (excluding the current one).
    pub history: Vec<VersionPlan>,
    pub attachments: Vec<AttachmentPlan>,
    pub comments: Vec<CommentPlan>,
    pub view_restrictions: Vec<(Option<String>, Option<String>)>,
    pub edit_restrictions: Vec<(Option<String>, Option<String>)>,
    pub page_status: Option<String>,
    /// user keys of people watching the page
    pub watchers: Vec<String>,
    /// user keys of people who marked the page as favourite
    pub favourited_by: Vec<String>,
}

#[derive(Debug, Clone, Serialize)]
pub struct SpacePlan {
    pub source_id: String,
    pub key: String,
    pub name: String,
    pub description_source_id: Option<String>,
    pub home_page_source_id: Option<String>,
    /// Pages in creation order (parents before children, siblings by position then title).
    pub pages: Vec<PagePlan>,
    pub blog_posts: Vec<PagePlan>,
    pub permissions: Vec<SpacePermission>,
    /// user keys of people watching the whole space
    pub watchers: Vec<String>,
    /// user keys of people who marked the space as favourite
    pub favourited_by: Vec<String>,
}

#[derive(Debug, Clone, Serialize)]
pub struct UserPlan {
    pub key: String,
    pub username: Option<String>,
    pub display_name: Option<String>,
    pub email: Option<String>,
    pub account_id: Option<String>,
    /// Number of content items created or modified by this user.
    pub activity: u64,
}

#[derive(Debug, Clone, Default, Serialize)]
pub struct Report {
    pub format: Option<ExportFormat>,
    pub source: Option<String>,
    pub build_number: Option<String>,
    pub spaces: Vec<String>,
    pub counts: BTreeMap<String, u64>,
    pub warnings: Vec<String>,
    pub stats: Stats,
}

impl Report {
    fn count(&mut self, k: &str, n: u64) {
        *self.counts.entry(k.to_string()).or_default() += n;
    }

    fn warn(&mut self, w: impl Into<String>) {
        if self.warnings.len() < 200 {
            self.warnings.push(w.into());
        }
    }
}

#[derive(Debug, Clone, Serialize)]
pub struct ImportPlan {
    pub spaces: Vec<SpacePlan>,
    pub users: Vec<UserPlan>,
    pub report: Report,
}

/// Indexes over an export plus lazy conversion.
pub struct Planner<'e> {
    pub export: &'e Export,
    opts: PlanOptions,
    space_key_by_id: HashMap<&'e str, &'e str>,
    /// (space key, title) → current page/blog id
    titles: HashMap<(String, String), String>,
    /// container id → filename → current attachment id
    attachments_by_container: HashMap<String, HashMap<String, String>>,
    /// current id → historical ids (sorted by version)
    history: HashMap<String, Vec<&'e Content>>,
    users: HashMap<String, String>,
    /// old versions dropped as exact duplicates of another
    duplicate_versions: u64,
}

impl<'e> Planner<'e> {
    pub fn new(export: &'e Export, opts: PlanOptions) -> Self {
        let space_key_by_id = export.spaces.iter().map(|s| (s.id.as_str(), s.key.as_str())).collect::<HashMap<_, _>>();
        let mut titles = HashMap::new();
        let mut attachments_by_container: HashMap<String, HashMap<String, String>> = HashMap::new();
        let mut history: HashMap<String, Vec<&Content>> = HashMap::new();
        for c in export.contents.values() {
            if let Some(orig) = &c.original_version_id {
                // a draft that points at a published page holds unpublished edits (Cloud keeps
                // one for every page edited in the new editor), not an old version
                if c.status != ContentStatus::Draft {
                    history.entry(orig.clone()).or_default().push(c);
                }
                continue;
            }
            match c.kind {
                ContentKind::Page | ContentKind::BlogPost | ContentKind::Folder | ContentKind::Whiteboard | ContentKind::Database | ContentKind::Embed
                    if matches!(c.status, ContentStatus::Current | ContentStatus::Archived) =>
                {
                    let key = c.space_id.as_deref().and_then(|s| space_key_by_id.get(s)).copied().unwrap_or("");
                    // prefer pages over blogs with the same title
                    titles
                        .entry((key.to_string(), c.title.clone()))
                        .and_modify(|e: &mut String| {
                            if c.kind == ContentKind::Page {
                                *e = c.id.clone();
                            }
                        })
                        .or_insert_with(|| c.id.clone());
                }
                ContentKind::Attachment if c.status != ContentStatus::Deleted || Self::in_trashed_page(export, c, &opts) => {
                    if let Some(container) = &c.container_id {
                        attachments_by_container.entry(container.clone()).or_default().insert(c.title.clone(), c.id.clone());
                    }
                }
                _ => {}
            }
        }
        // Exports can carry buggy history: the same version number twice. Order by version, then
        // date (then id), and drop exact duplicates (same number, date and body).
        let body_of = |c: &Content| export.bodies.get(&c.id).map(|b| b.body.as_str());
        let mut duplicate_versions = 0u64;
        for v in history.values_mut() {
            v.sort_by(|a, b| a.version.cmp(&b.version).then_with(|| a.modified.cmp(&b.modified)).then_with(|| a.id.cmp(&b.id)));
            let before = v.len();
            v.dedup_by(|b, a| a.kind != ContentKind::Attachment && a.version == b.version && a.modified == b.modified && body_of(a) == body_of(b));
            duplicate_versions += (before - v.len()) as u64;
        }
        // Former titles (from version history) also resolve to the page, unless a current page
        // has that title. Old versions — and stale cross-space links — use titles from before renames.
        for (current_id, versions) in &history {
            let Some(cur) = export.contents.get(current_id) else { continue };
            if !matches!(cur.kind, ContentKind::Page | ContentKind::BlogPost) {
                continue;
            }
            let key = cur.space_id.as_deref().and_then(|s| space_key_by_id.get(s)).copied().unwrap_or("");
            for v in versions {
                titles.entry((key.to_string(), v.title.clone())).or_insert_with(|| current_id.clone());
            }
        }
        let mut users = HashMap::new();
        for u in export.users.values() {
            let name = u.display_name.clone().or_else(|| u.username.clone()).unwrap_or_else(|| u.key.clone());
            users.insert(u.key.clone(), name.clone());
            if let Some(un) = &u.username {
                users.entry(un.clone()).or_insert_with(|| name.clone());
            }
            if let Some(a) = &u.account_id {
                users.entry(a.clone()).or_insert_with(|| name.clone());
            }
        }
        Self { export, opts, space_key_by_id, titles, attachments_by_container, history, users, duplicate_versions }
    }

    pub fn space_key_of(&self, c: &Content) -> &str {
        c.space_id.as_deref().and_then(|s| self.space_key_by_id.get(s)).copied().unwrap_or("")
    }

    /// Converts the body of any content (current or historical) to ProseMirror JSON.
    pub fn convert(&self, content_id: &str, stats: &mut Stats) -> Node {
        let Some(content) = self.export.contents.get(content_id) else { return Node::empty_doc() };
        let Some(body) = self.export.body(content_id) else { return Node::empty_doc() };
        // attachments/links resolve relative to the *current* version's container
        let container = content.original_version_id.clone().unwrap_or_else(|| content.container_id.clone().unwrap_or_else(|| content_id.to_string()));
        let container = if matches!(content.kind, ContentKind::Comment) { content.container_id.clone().unwrap_or(container) } else { container };
        // historical versions and comments often lack `space`: inherit from the current version / container
        let via = |id: Option<&String>| id.and_then(|c| self.export.contents.get(c)).and_then(|c| c.space_id.as_deref());
        let space_key = content
            .space_id
            .as_deref()
            .or_else(|| via(content.original_version_id.as_ref()))
            .or_else(|| via(content.container_id.as_ref()))
            .and_then(|s| self.space_key_by_id.get(s))
            .copied()
            .unwrap_or("");
        let r = Ctx { p: self, space_key: space_key.to_string(), content_id: container };
        match body.kind {
            BodyKind::Storage => storage::convert(&body.body, &r, stats),
            BodyKind::WikiMarkup => storage::convert_wiki(&body.body, &r, stats),
            BodyKind::Raw => Node::empty_doc(),
        }
    }

    /// Attachments and comments of a page in the trash are marked deleted along with it: they
    /// go to the trash with the page (and come back when it's restored).
    fn in_trashed_page(export: &Export, c: &Content, opts: &PlanOptions) -> bool {
        opts.include_deleted
            && c.status == ContentStatus::Deleted
            && c.container_id.as_ref().and_then(|id| export.contents.get(id)).is_some_and(|p| p.status == ContentStatus::Deleted)
    }

    fn include(&self, c: &Content) -> bool {
        match c.status {
            ContentStatus::Current | ContentStatus::Archived => true,
            ContentStatus::Draft => self.opts.include_drafts,
            ContentStatus::Deleted => self.opts.include_deleted,
            ContentStatus::Other => false,
        }
    }

    pub fn plan(&self) -> ImportPlan {
        let ex = self.export;
        let mut report = Report {
            format: ex.format,
            source: ex.descriptor.get("source").cloned().or_else(|| Some("server".into())),
            build_number: ex.descriptor.get("buildNumber").cloned(),
            spaces: ex.spaces.iter().map(|s| s.key.clone()).collect(),
            ..Default::default()
        };
        for w in &ex.read_warnings {
            report.warn(w.clone());
        }
        if self.duplicate_versions > 0 {
            report.count("duplicate_versions_dropped", self.duplicate_versions);
        }
        let pending_edits = ex.contents.values().filter(|c| c.is_historical() && c.status == ContentStatus::Draft && c.kind != ContentKind::Attachment).count();
        if pending_edits > 0 {
            report.count("skipped_drafts_of_published_pages", pending_edits as u64);
        }

        // labels / restrictions / comments / attachments by owner
        let mut labels: HashMap<&str, Vec<String>> = HashMap::new();
        for l in &ex.labellings {
            let name = if l.namespace == "my" || l.namespace == "personal" { continue } else { l.name.clone() };
            labels.entry(l.content_id.as_str()).or_default().push(name);
        }
        // watches and favourites by content / space id (a reference to an old version means the page)
        let current = |id: &'e str| -> &'e str { ex.contents.get(id).and_then(|c| c.original_version_id.as_deref()).unwrap_or(id) };
        let mut watchers: HashMap<&str, Vec<String>> = HashMap::new();
        let mut space_watchers: HashMap<&str, Vec<String>> = HashMap::new();
        for w in &ex.watches {
            match (&w.content_id, &w.space_id) {
                (Some(c), _) => watchers.entry(current(c)).or_default().push(w.user_key.clone()),
                (None, Some(s)) => space_watchers.entry(s.as_str()).or_default().push(w.user_key.clone()),
                _ => {}
            }
        }
        let mut favourites: HashMap<&str, Vec<String>> = HashMap::new();
        for f in &ex.favourites {
            favourites.entry(current(&f.content_id)).or_default().push(f.user_key.clone());
        }
        // older Confluence: a personal `my:favourite` label (on a space: on its description)
        for l in &ex.labellings {
            if matches!(l.namespace.as_str(), "my" | "personal")
                && matches!(l.name.to_ascii_lowercase().as_str(), "favourite" | "favorite")
                && let Some(owner) = &l.owner
            {
                favourites.entry(current(&l.content_id)).or_default().push(owner.clone());
            }
        }
        let people = |m: &HashMap<&str, Vec<String>>, id: Option<&str>| -> Vec<String> {
            let mut v = id.and_then(|id| m.get(id)).cloned().unwrap_or_default();
            v.sort();
            v.dedup();
            v
        };
        let mut comments: HashMap<&str, Vec<&Content>> = HashMap::new();
        let mut attachments: HashMap<&str, Vec<&Content>> = HashMap::new();
        let mut children_count: HashMap<&str, usize> = HashMap::new();
        for c in ex.contents.values() {
            if c.is_historical() {
                continue;
            }
            match c.kind {
                ContentKind::Comment if self.opts.include_comments => {
                    if c.status == ContentStatus::Deleted && !Self::in_trashed_page(ex, c, &self.opts) {
                        report.count("skipped_deleted_comments", 1);
                        continue;
                    }
                    if let Some(cont) = &c.container_id {
                        comments.entry(cont.as_str()).or_default().push(c);
                    }
                }
                ContentKind::Attachment if self.opts.include_attachments => {
                    if c.status == ContentStatus::Deleted && !Self::in_trashed_page(ex, c, &self.opts) {
                        report.count("skipped_deleted_attachments", 1);
                        continue;
                    }
                    if let Some(cont) = &c.container_id {
                        attachments.entry(cont.as_str()).or_default().push(c);
                    }
                }
                _ => {}
            }
            if let Some(p) = &c.parent_id {
                *children_count.entry(p.as_str()).or_default() += 1;
            }
        }

        let mut spaces = Vec::new();
        for s in &ex.spaces {
            let mut tree_nodes: Vec<&Content> = Vec::new();
            let mut blogs: Vec<&Content> = Vec::new();
            for c in ex.contents.values() {
                if c.is_historical() || c.space_id.as_deref() != Some(s.id.as_str()) {
                    continue;
                }
                if c.kind.is_tree_node() || c.kind == ContentKind::BlogPost {
                    if !self.include(c) {
                        report.count(&format!("skipped_{:?}_{:?}", c.kind, c.status).to_ascii_lowercase(), 1);
                        continue;
                    }
                    // a draft becomes its creator's private draft: without a creator nobody
                    // could ever see it, without a body there's nothing to keep
                    if c.status == ContentStatus::Draft {
                        if c.creator.as_deref().is_none_or(str::is_empty) {
                            report.count("skipped_drafts_without_author", 1);
                            continue;
                        }
                        if !ex.bodies.contains_key(&c.id) {
                            report.count("skipped_drafts_without_body", 1);
                            continue;
                        }
                    }
                    if c.kind == ContentKind::BlogPost {
                        blogs.push(c);
                    } else {
                        tree_nodes.push(c);
                    }
                }
            }
            // order: parents before children, siblings by (position, title)
            let ids: HashSet<&str> = tree_nodes.iter().map(|c| c.id.as_str()).collect();
            let by_id: HashMap<&str, &Content> = tree_nodes.iter().map(|c| (c.id.as_str(), *c)).collect();
            // A page that isn't in the trash never sits below one: like Confluence (which moves
            // children up when a page is deleted), it goes to the nearest ancestor outside the trash.
            let parent_of = |c: &Content| -> Option<String> {
                let mut parent = c.parent_id.as_deref().filter(|p| ids.contains(p));
                if c.status != ContentStatus::Deleted {
                    let mut hops = 0;
                    while let Some(pc) = parent.and_then(|p| by_id.get(p))
                        && pc.status == ContentStatus::Deleted
                        && hops < 200
                    {
                        parent = pc.parent_id.as_deref().filter(|p| ids.contains(p));
                        hops += 1;
                    }
                }
                parent.map(String::from)
            };
            let parents: HashMap<&str, Option<String>> = tree_nodes.iter().map(|c| (c.id.as_str(), parent_of(c))).collect();
            let mut by_parent: BTreeMap<Option<&str>, Vec<&Content>> = BTreeMap::new();
            let mut orphans = 0;
            for c in &tree_nodes {
                if c.parent_id.as_deref().is_some_and(|p| !ids.contains(p)) {
                    orphans += 1;
                    report.warn(format!("{}: page '{}' has a missing parent (attached to space root)", s.key, c.title));
                }
                by_parent.entry(parents[c.id.as_str()].as_deref()).or_default().push(c);
            }
            if orphans > 0 {
                report.count("orphan_pages", orphans);
            }
            for v in by_parent.values_mut() {
                v.sort_by(|a, b| {
                    // Confluence: explicit positions first, unpositioned pages alphabetically after
                    match (a.position, b.position) {
                        (Some(x), Some(y)) => x.cmp(&y).then_with(|| a.title.to_lowercase().cmp(&b.title.to_lowercase())),
                        (Some(_), None) => std::cmp::Ordering::Less,
                        (None, Some(_)) => std::cmp::Ordering::Greater,
                        (None, None) => a.title.to_lowercase().cmp(&b.title.to_lowercase()),
                    }
                });
            }
            let mut ordered: Vec<&Content> = Vec::with_capacity(tree_nodes.len());
            let mut stack: Vec<&Content> = by_parent.get(&None).cloned().unwrap_or_default().into_iter().rev().collect();
            let mut visited: HashSet<&str> = HashSet::new();
            while let Some(c) = stack.pop() {
                if !visited.insert(c.id.as_str()) {
                    report.count("tree_cycles_broken", 1);
                    continue;
                }
                ordered.push(c);
                if let Some(ch) = by_parent.get(&Some(c.id.as_str())) {
                    for k in ch.iter().rev() {
                        stack.push(k);
                    }
                }
            }
            // nodes unreachable from the root (cycles) → attach to root
            for c in &tree_nodes {
                if !visited.contains(c.id.as_str()) {
                    report.count("tree_cycles_broken", 1);
                    ordered.push(c);
                }
            }

            let make = |c: &Content, report: &mut Report| -> PagePlan {
                let kind = match c.kind {
                    ContentKind::Folder => NodeKind::Folder,
                    ContentKind::BlogPost => NodeKind::BlogPost,
                    ContentKind::Whiteboard => NodeKind::Whiteboard,
                    ContentKind::Database => NodeKind::Database,
                    ContentKind::Embed => NodeKind::Embed,
                    _ => NodeKind::Page,
                };
                if matches!(kind, NodeKind::Whiteboard | NodeKind::Database | NodeKind::Embed) {
                    report.warn(format!("{}: '{}' is a {:?}; imported as a placeholder page", s.key, c.title, kind));
                }
                if let Some(h) = self.history.get(&c.id) {
                    let without = h.iter().filter(|v| !ex.bodies.contains_key(&v.id)).count();
                    if without > 0 {
                        report.count("page_versions_without_body", without as u64);
                    }
                }
                let history: Vec<VersionPlan> = if self.opts.include_history {
                    self.history
                        .get(&c.id)
                        .map(|h| {
                            h.iter()
                                .filter(|v| ex.bodies.contains_key(&v.id))
                                .map(|v| VersionPlan {
                                    source_id: v.id.clone(),
                                    version: v.version,
                                    title: v.title.clone(),
                                    author: v.last_modifier.clone().or_else(|| v.creator.clone()),
                                    created_at: v.modified.clone().or_else(|| v.created.clone()),
                                    message: v.version_comment.clone().filter(|m| !m.is_empty()),
                                })
                                .collect()
                        })
                        .unwrap_or_default()
                } else {
                    vec![]
                };
                report.count("page_versions", history.len() as u64);
                let atts: Vec<AttachmentPlan> = attachments
                    .get(c.id.as_str())
                    .map(|v| {
                        let mut v = v.clone();
                        v.sort_by(|a, b| a.title.cmp(&b.title));
                        v.iter().map(|a| self.attachment_plan(a, report)).collect()
                    })
                    .unwrap_or_default();
                let cmts: Vec<CommentPlan> = comments
                    .get(c.id.as_str())
                    .map(|v| {
                        let mut v = v.clone();
                        v.sort_by(|a, b| a.created.cmp(&b.created).then(a.id.cmp(&b.id)));
                        v.iter().map(|cm| self.comment_plan(cm, report)).collect()
                    })
                    .unwrap_or_default();
                let mut view = Vec::new();
                let mut edit = Vec::new();
                for r in ex.restrictions.iter().filter(|r| r.content_id == c.id) {
                    match r.kind {
                        RestrictionKind::View => view.push((r.user_key.clone(), r.group.clone())),
                        RestrictionKind::Edit => edit.push((r.user_key.clone(), r.group.clone())),
                    }
                }
                if !view.is_empty() || !edit.is_empty() {
                    report.count("restricted_pages", 1);
                }
                PagePlan {
                    source_id: c.id.clone(),
                    parent_source_id: parents.get(c.id.as_str()).cloned().flatten(),
                    kind,
                    title: if c.title.trim().is_empty() { "Untitled".into() } else { c.title.clone() },
                    position: c.position,
                    status: c.status,
                    version: c.version,
                    created_by: c.creator.clone(),
                    created_at: c.created.clone(),
                    updated_by: c.last_modifier.clone(),
                    updated_at: c.modified.clone(),
                    labels: labels.get(c.id.as_str()).cloned().unwrap_or_default(),
                    history,
                    attachments: atts,
                    comments: cmts,
                    view_restrictions: view,
                    edit_restrictions: edit,
                    page_status: ex.property(&c.id, "status").map(String::from),
                    watchers: people(&watchers, Some(&c.id)),
                    favourited_by: people(&favourites, Some(&c.id)),
                }
            };

            let pages: Vec<PagePlan> = ordered.iter().map(|c| make(c, &mut report)).collect();
            let mut blog_posts: Vec<PagePlan> = blogs.iter().map(|c| make(c, &mut report)).collect();
            blog_posts.sort_by(|a, b| a.created_at.cmp(&b.created_at));
            let live = |p: &&PagePlan| matches!(p.status, ContentStatus::Current | ContentStatus::Archived);
            report.count("pages", pages.iter().filter(live).filter(|p| p.kind == NodeKind::Page).count() as u64);
            report.count("folders", pages.iter().filter(live).filter(|p| p.kind == NodeKind::Folder).count() as u64);
            report.count("blog_posts", blog_posts.iter().filter(live).count() as u64);
            report.count("drafts", pages.iter().chain(&blog_posts).filter(|p| p.status == ContentStatus::Draft).count() as u64);
            report.count("trashed_pages", pages.iter().chain(&blog_posts).filter(|p| p.status == ContentStatus::Deleted).count() as u64);
            let att_count: usize = pages.iter().chain(&blog_posts).map(|p| p.attachments.len()).sum();
            report.count("attachments", att_count as u64);
            let com_count: usize = pages.iter().chain(&blog_posts).map(|p| p.comments.len()).sum();
            report.count("comments", com_count as u64);
            report.count(
                "inline_comments",
                pages
                    .iter()
                    .chain(&blog_posts)
                    .flat_map(|p| &p.comments)
                    .filter(|c| c.inline_marker_ref.is_some() || c.inline_original_selection.is_some())
                    .count() as u64,
            );
            report.count("labels", pages.iter().chain(&blog_posts).map(|p| p.labels.len() as u64).sum());
            report.count("watches", pages.iter().chain(&blog_posts).map(|p| p.watchers.len() as u64).sum());
            report.count("favourites", pages.iter().chain(&blog_posts).map(|p| p.favourited_by.len() as u64).sum());
            let space_watchers = people(&space_watchers, Some(&s.id));
            let space_favourites = people(&favourites, s.description_id.as_deref());
            report.count("space_watches", space_watchers.len() as u64);
            report.count("space_favourites", space_favourites.len() as u64);
            spaces.push(SpacePlan {
                source_id: s.id.clone(),
                key: s.key.clone(),
                name: s.name.clone(),
                description_source_id: s.description_id.clone(),
                home_page_source_id: s.home_page_id.clone(),
                pages,
                blog_posts,
                permissions: ex.space_permissions.iter().filter(|p| p.space_id == s.id).cloned().collect(),
                watchers: space_watchers,
                favourited_by: space_favourites,
            });
        }

        // users with activity counts
        let mut activity: HashMap<&str, u64> = HashMap::new();
        for c in ex.contents.values() {
            for u in [&c.creator, &c.last_modifier].into_iter().flatten() {
                *activity.entry(u.as_str()).or_default() += 1;
            }
        }
        let mut users: Vec<UserPlan> = ex
            .users
            .values()
            .map(|u| UserPlan {
                key: u.key.clone(),
                username: u.username.clone(),
                display_name: u.display_name.clone(),
                email: u.email.clone(),
                account_id: u.account_id.clone(),
                activity: activity.get(u.key.as_str()).copied().unwrap_or(0),
            })
            .collect();
        // authors not in the user list
        let known: HashSet<&str> = ex.users.keys().map(String::as_str).collect();
        let mut unknown = 0;
        for (k, n) in &activity {
            if !known.contains(k) {
                unknown += 1;
                users.push(UserPlan { key: k.to_string(), username: None, display_name: None, email: None, account_id: None, activity: *n });
            }
        }
        report.count("users", users.len() as u64);
        if unknown > 0 {
            report.count("users_unknown", unknown);
        }
        users.sort_by(|a, b| b.activity.cmp(&a.activity).then(a.key.cmp(&b.key)));
        ImportPlan { spaces, users, report }
    }

    fn attachment_plan(&self, a: &Content, report: &mut Report) -> AttachmentPlan {
        let mut versions: Vec<AttachmentVersionPlan> = self
            .history
            .get(&a.id)
            .map(|h| {
                h.iter()
                    .map(|v| AttachmentVersionPlan {
                        source_id: v.id.clone(),
                        version: v.version,
                        path: self
                            .export
                            .attachment_files
                            .get(&a.id)
                            .and_then(|m| m.get(&v.version))
                            .or_else(|| self.export.attachment_files.get(&v.id).and_then(|m| m.get(&v.version)))
                            .cloned(),
                        author: v.creator.clone(),
                        created_at: v.created.clone(),
                        comment: v.version_comment.clone().filter(|c| !c.is_empty()),
                    })
                    .collect()
            })
            .unwrap_or_default();
        let path = self.export.attachment_path(&a.id, a.version).map(String::from);
        if path.is_none() {
            report.count("attachment_binaries_missing", 1);
        }
        versions.push(AttachmentVersionPlan {
            source_id: a.id.clone(),
            version: a.version,
            path,
            author: a.last_modifier.clone().or_else(|| a.creator.clone()),
            created_at: a.modified.clone().or_else(|| a.created.clone()),
            comment: a.version_comment.clone().filter(|c| !c.is_empty()),
        });
        report.count("attachment_versions", versions.len() as u64);
        AttachmentPlan {
            source_id: a.id.clone(),
            filename: a.title.clone(),
            media_type: self.export.property(&a.id, "MEDIA_TYPE").map(String::from),
            size: self.export.property(&a.id, "FILESIZE").and_then(|s| s.parse().ok()),
            created_by: a.creator.clone(),
            created_at: a.created.clone(),
            versions,
        }
    }

    fn comment_plan(&self, c: &Content, _report: &mut Report) -> CommentPlan {
        let props = self.export.properties.get(&c.id);
        let get = |k: &str| props.and_then(|p| p.get(k)).cloned().filter(|v| !v.is_empty());
        let status = get("status").or_else(|| get("inline-comment-status")).unwrap_or_default();
        // Cloud CSV: `inline-serialized-highlights` = [["selected text", "<path>", offset, length], ...]
        let highlight = get("inline-serialized-highlights").and_then(|v| {
            let parsed: serde_json::Value = serde_json::from_str(&v).ok()?;
            parsed.get(0)?.get(0)?.as_str().map(String::from)
        });
        CommentPlan {
            source_id: c.id.clone(),
            parent_source_id: c.parent_comment_id.clone(),
            author: c.creator.clone(),
            created_at: c.created.clone(),
            updated_at: c.modified.clone(),
            inline_marker_ref: get("inline-marker-ref"),
            inline_original_selection: get("inline-original-selection").or(highlight),
            resolved: status == "resolved" || get("resolved").is_some_and(|v| v == "true"),
        }
    }
}

struct Ctx<'p, 'e> {
    p: &'p Planner<'e>,
    space_key: String,
    content_id: String,
}

impl Resolver for Ctx<'_, '_> {
    fn space_key(&self) -> &str {
        &self.space_key
    }
    fn content_id(&self) -> &str {
        &self.content_id
    }
    fn page(&self, space_key: &str, title: &str) -> Option<String> {
        self.p.titles.get(&(space_key.to_string(), title.to_string())).cloned()
    }
    fn content_exists(&self, id: &str) -> bool {
        self.p.export.contents.contains_key(id)
    }
    fn attachment(&self, container_id: &str, filename: &str) -> Option<String> {
        self.p.attachments_by_container.get(container_id).and_then(|m| m.get(filename)).cloned()
    }
    fn user_name(&self, key: &str) -> Option<String> {
        self.p.users.get(key).cloned()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn content(id: &str, status: ContentStatus, parent: Option<&str>, original: Option<&str>) -> Content {
        Content {
            id: id.into(),
            kind: ContentKind::Page,
            title: format!("Page {id}"),
            status,
            version: 1,
            original_version_id: original.map(String::from),
            parent_id: parent.map(String::from),
            position: None,
            space_id: Some("S".into()),
            container_id: None,
            parent_comment_id: None,
            creator: Some("alice".into()),
            created: None,
            last_modifier: None,
            modified: None,
            version_comment: None,
            plugin_key: None,
            property_ids: vec![],
        }
    }

    fn export(contents: Vec<Content>) -> Export {
        let mut ex = Export::default();
        ex.spaces.push(Space {
            id: "S".into(),
            key: "KEY".into(),
            name: "Space".into(),
            description_id: None,
            home_page_id: None,
            creator: None,
            created: None,
            space_type: None,
            status: None,
        });
        for c in contents {
            ex.bodies.insert(c.id.clone(), Body { content_id: c.id.clone(), kind: BodyKind::Storage, body: format!("<p>{}</p>", c.title) });
            ex.contents.insert(c.id.clone(), c);
        }
        ex
    }

    #[test]
    fn content_statuses_map_to_drafts_trash_and_history() {
        let mut ex = export(vec![
            content("1", ContentStatus::Current, None, None),
            // an old version and Cloud's shared draft of page 1 (unpublished edits)
            content("2", ContentStatus::Current, None, Some("1")),
            content("3", ContentStatus::Draft, None, Some("1")),
            // a page in the trash with a live child and a trashed child
            content("4", ContentStatus::Deleted, Some("1"), None),
            content("5", ContentStatus::Current, Some("4"), None),
            content("6", ContentStatus::Deleted, Some("4"), None),
            // never-published drafts: kept, unless there's no author or no body
            content("7", ContentStatus::Draft, Some("1"), None),
            content("8", ContentStatus::Draft, None, None),
            content("9", ContentStatus::Draft, None, None),
            content("10", ContentStatus::Archived, None, None),
        ]);
        ex.contents.get_mut("8").unwrap().creator = None;
        ex.bodies.remove("9");
        let plan = Planner::new(&ex, PlanOptions::default()).plan();
        let pages = &plan.spaces[0].pages;
        let get = |id: &str| pages.iter().find(|p| p.source_id == id);

        // the shared draft is not an old version
        let history: Vec<&str> = get("1").unwrap().history.iter().map(|v| v.source_id.as_str()).collect();
        assert_eq!(history, ["2"]);
        assert_eq!(plan.report.counts["skipped_drafts_of_published_pages"], 1);

        // trash keeps its own structure; live pages move up out of it
        assert_eq!(get("4").unwrap().status, ContentStatus::Deleted);
        assert_eq!(get("5").unwrap().parent_source_id.as_deref(), Some("1"));
        assert_eq!(get("6").unwrap().parent_source_id.as_deref(), Some("4"));

        assert_eq!(get("7").unwrap().status, ContentStatus::Draft);
        assert_eq!(get("7").unwrap().parent_source_id.as_deref(), Some("1"));
        assert!(get("8").is_none() && get("9").is_none());
        assert_eq!(plan.report.counts["skipped_drafts_without_author"], 1);
        assert_eq!(plan.report.counts["skipped_drafts_without_body"], 1);

        assert_eq!(plan.report.counts["pages"], 3, "1, 5 and the archived 10");
        assert_eq!(plan.report.counts["drafts"], 1);
        assert_eq!(plan.report.counts["trashed_pages"], 2);

        // both can be switched off
        let opts = PlanOptions { include_drafts: false, include_deleted: false, ..Default::default() };
        let plan = Planner::new(&ex, opts).plan();
        let ids: Vec<&str> = plan.spaces[0].pages.iter().map(|p| p.source_id.as_str()).collect();
        assert_eq!(ids, ["1", "10", "5"], "root pages sort by title");
        assert_eq!(plan.spaces[0].pages[2].parent_source_id, None, "its parent isn't imported");
    }

    #[test]
    fn watches_and_favourites() {
        let mut ex = export(vec![content("1", ContentStatus::Current, None, None), content("2", ContentStatus::Current, None, Some("1"))]);
        ex.spaces[0].description_id = Some("d".into());
        let watch = |user: &str, content: Option<&str>, space: Option<&str>| Watch {
            user_key: user.into(),
            content_id: content.map(String::from),
            space_id: space.map(String::from),
        };
        ex.watches = vec![watch("bob", Some("1"), None), watch("ann", Some("2"), None), watch("bob", Some("1"), None), watch("cid", None, Some("S"))];
        ex.favourites = vec![Favourite { user_key: "ann".into(), content_id: "1".into() }];
        let label = |content: &str, name: &str, namespace: &str, owner: Option<&str>| Labelling {
            content_id: content.into(),
            name: name.into(),
            namespace: namespace.into(),
            owner: owner.map(String::from),
        };
        ex.labellings = vec![label("1", "favourite", "my", Some("bob")), label("d", "favorite", "my", Some("ann")), label("1", "favourite", "global", None)];
        let plan = Planner::new(&ex, PlanOptions::default()).plan();
        let space = &plan.spaces[0];
        let page = &space.pages[0];
        // a watch on an old version is a watch on the page; duplicates collapse
        assert_eq!(page.watchers, ["ann", "bob"]);
        assert_eq!(page.favourited_by, ["ann", "bob"]);
        assert_eq!(page.labels, ["favourite"], "a global label is just a label");
        assert_eq!(space.watchers, ["cid"]);
        assert_eq!(space.favourited_by, ["ann"], "favourite label on the space description");
        assert_eq!(plan.report.counts["watches"], 2);
        assert_eq!(plan.report.counts["space_favourites"], 1);
    }
}
