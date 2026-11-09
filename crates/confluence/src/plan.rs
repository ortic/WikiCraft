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
    pub include_drafts: bool,
    pub include_deleted: bool,
    pub include_comments: bool,
    pub include_attachments: bool,
}

impl Default for PlanOptions {
    fn default() -> Self {
        Self { include_history: true, include_drafts: false, include_deleted: false, include_comments: true, include_attachments: true }
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
}

impl<'e> Planner<'e> {
    pub fn new(export: &'e Export, opts: PlanOptions) -> Self {
        let space_key_by_id = export.spaces.iter().map(|s| (s.id.as_str(), s.key.as_str())).collect::<HashMap<_, _>>();
        let mut titles = HashMap::new();
        let mut attachments_by_container: HashMap<String, HashMap<String, String>> = HashMap::new();
        let mut history: HashMap<String, Vec<&Content>> = HashMap::new();
        for c in export.contents.values() {
            if let Some(orig) = &c.original_version_id {
                history.entry(orig.clone()).or_default().push(c);
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
                ContentKind::Attachment if c.status != ContentStatus::Deleted => {
                    if let Some(container) = &c.container_id {
                        attachments_by_container.entry(container.clone()).or_default().insert(c.title.clone(), c.id.clone());
                    }
                }
                _ => {}
            }
        }
        for v in history.values_mut() {
            v.sort_by_key(|c| c.version);
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
        Self { export, opts, space_key_by_id, titles, attachments_by_container, history, users }
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
            BodyKind::WikiMarkup => storage::convert_wiki(&body.body, stats),
            BodyKind::Raw => Node::empty_doc(),
        }
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

        // labels / restrictions / comments / attachments by owner
        let mut labels: HashMap<&str, Vec<String>> = HashMap::new();
        for l in &ex.labellings {
            let name = if l.namespace == "my" || l.namespace == "personal" { continue } else { l.name.clone() };
            labels.entry(l.content_id.as_str()).or_default().push(name);
        }
        let mut comments: HashMap<&str, Vec<&Content>> = HashMap::new();
        let mut attachments: HashMap<&str, Vec<&Content>> = HashMap::new();
        let mut children_count: HashMap<&str, usize> = HashMap::new();
        for c in ex.contents.values() {
            if c.is_historical() {
                continue;
            }
            match c.kind {
                ContentKind::Comment if self.opts.include_comments => {
                    if c.status == ContentStatus::Deleted {
                        report.count("skipped_deleted_comments", 1);
                        continue;
                    }
                    if let Some(cont) = &c.container_id {
                        comments.entry(cont.as_str()).or_default().push(c);
                    }
                }
                ContentKind::Attachment if self.opts.include_attachments => {
                    if c.status == ContentStatus::Deleted {
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
                    if c.kind == ContentKind::BlogPost {
                        blogs.push(c);
                    } else {
                        tree_nodes.push(c);
                    }
                }
            }
            // order: parents before children, siblings by (position, title)
            let ids: HashSet<&str> = tree_nodes.iter().map(|c| c.id.as_str()).collect();
            let mut by_parent: BTreeMap<Option<&str>, Vec<&Content>> = BTreeMap::new();
            let mut orphans = 0;
            for c in &tree_nodes {
                let parent = c.parent_id.as_deref().filter(|p| ids.contains(p));
                if c.parent_id.is_some() && parent.is_none() {
                    orphans += 1;
                    report.warn(format!("{}: page '{}' has a missing parent (attached to space root)", s.key, c.title));
                }
                by_parent.entry(parent).or_default().push(c);
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
                    parent_source_id: c.parent_id.clone().filter(|p| ids.contains(p.as_str())),
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
                }
            };

            let pages: Vec<PagePlan> = ordered.iter().map(|c| make(c, &mut report)).collect();
            let mut blog_posts: Vec<PagePlan> = blogs.iter().map(|c| make(c, &mut report)).collect();
            blog_posts.sort_by(|a, b| a.created_at.cmp(&b.created_at));
            report.count("pages", pages.iter().filter(|p| p.kind == NodeKind::Page).count() as u64);
            report.count("folders", pages.iter().filter(|p| p.kind == NodeKind::Folder).count() as u64);
            report.count("blog_posts", blog_posts.len() as u64);
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
            spaces.push(SpacePlan {
                source_id: s.id.clone(),
                key: s.key.clone(),
                name: s.name.clone(),
                description_source_id: s.description_id.clone(),
                home_page_source_id: s.home_page_id.clone(),
                pages,
                blog_posts,
                permissions: ex.space_permissions.iter().filter(|p| p.space_id == s.id).cloned().collect(),
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
