//! Confluence import job: turns a Confluence export (XML or Cloud CSV) into WikiCraft spaces,
//! pages (with history), attachments, comments, labels, restrictions and users.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use bytes::Bytes;
use serde::Deserialize;
use serde_json::{Value, json};
use uuid::Uuid;
use wikicraft_confluence::model::ContentStatus;
use wikicraft_confluence::plan::{CommentPlan, NodeKind, PagePlan};
use wikicraft_confluence::{Export, PlanOptions, Planner, Source, Stats};
use wikicraft_doc_model::{Mark, Node, marks, nodes, text, ydoc};

use crate::app::{AppState, new_id};

#[derive(Debug, Clone, Deserialize, serde::Serialize, Default)]
#[serde(rename_all = "camelCase", default)]
pub struct ImportOptions {
    /// Import a single-space export under this key instead of the original one.
    pub space_key: Option<String>,
    pub skip_history: bool,
    pub skip_comments: bool,
    pub skip_attachments: bool,
    /// Leave out never-published pages (otherwise they become private drafts of their author).
    pub skip_drafts: bool,
    /// Leave out pages in the Confluence trash (otherwise they go to the space trash).
    pub skip_trash: bool,
    /// Bring an earlier import of the same spaces up to date instead of creating new spaces:
    /// what changed in Confluence since is added (new pages, versions, attachments, comments).
    pub update: bool,
    /// With `update`: pages changed both in Confluence and here take Confluence's content (the
    /// local versions stay in the history). Otherwise they're left alone and reported.
    pub overwrite_local_changes: bool,
}

pub async fn set_progress(state: &AppState, job: Uuid, progress: Value) {
    let _ = sqlx::query("UPDATE jobs SET progress = $2 WHERE id = $1").bind(job).bind(progress).execute(&state.db).await;
}

/// Runs an import job to completion and records the outcome on the job row.
pub async fn run(state: AppState, job: Uuid, workspace: Uuid, actor: Uuid, path: PathBuf, opts: ImportOptions) {
    let _ = sqlx::query("UPDATE jobs SET status = 'running', started_at = now() WHERE id = $1").bind(job).execute(&state.db).await;
    let res = import(&state, job, workspace, actor, &path, &opts).await;
    match res {
        Ok(result) => {
            let _ = sqlx::query("UPDATE jobs SET status = 'succeeded', result = $2, finished_at = now(), progress = '{\"phase\":\"done\"}' WHERE id = $1")
                .bind(job)
                .bind(result)
                .execute(&state.db)
                .await;
        }
        Err(e) => {
            tracing::error!(%job, "import failed: {e:#}");
            let _ = sqlx::query("UPDATE jobs SET status = 'failed', error = $2, finished_at = now() WHERE id = $1")
                .bind(job)
                .bind(format!("{e:#}"))
                .execute(&state.db)
                .await;
        }
    }
    let _ = tokio::fs::remove_file(&path).await;
}

fn parse_date(s: Option<&str>) -> Option<chrono::DateTime<chrono::Utc>> {
    let s = s?.trim();
    if s.is_empty() {
        return None;
    }
    for fmt in ["%Y-%m-%d %H:%M:%S%.f", "%Y-%m-%d %H:%M:%S", "%Y-%m-%dT%H:%M:%S%.f"] {
        if let Ok(n) = chrono::NaiveDateTime::parse_from_str(s, fmt) {
            return Some(n.and_utc());
        }
    }
    chrono::DateTime::parse_from_rfc3339(s).ok().map(|d| d.with_timezone(&chrono::Utc))
}

/// Maps placeholder references produced by the converter to WikiCraft ids.
struct Links {
    pages: HashMap<String, Uuid>,
    attachments: HashMap<String, Uuid>,
    users: HashMap<String, Uuid>,
    inline_refs: HashMap<String, Uuid>,
    space_keys: HashMap<String, String>,
}

#[derive(Default, Debug, serde::Serialize)]
#[serde(rename_all = "camelCase")]
struct RewriteStats {
    links_to_pages: u64,
    links_to_missing_pages: u64,
    attachments_linked: u64,
    attachments_missing: u64,
    mentions: u64,
    mentions_unknown: u64,
    inline_anchors: u64,
}

fn decode_component(s: &str) -> String {
    let b = s.as_bytes();
    let mut out = Vec::with_capacity(b.len());
    let mut i = 0;
    while i < b.len() {
        match b[i] {
            b'+' => out.push(b' '),
            b'%' if i + 2 < b.len() => match std::str::from_utf8(&b[i + 1..i + 3]).ok().and_then(|h| u8::from_str_radix(h, 16).ok()) {
                Some(v) => {
                    out.push(v);
                    i += 3;
                    continue;
                }
                None => out.push(b'%'),
            },
            c => out.push(c),
        }
        i += 1;
    }
    String::from_utf8_lossy(&out).to_string()
}

fn query_escape(s: &str) -> String {
    s.bytes().map(|b| if b.is_ascii_alphanumeric() || b"-_.~".contains(&b) { (b as char).to_string() } else { format!("%{b:02X}") }).collect()
}

fn rewrite_href(href: &str, l: &Links, st: &mut RewriteStats) -> Option<String> {
    let Some(rest) = href.strip_prefix("wcimport:") else { return Some(href.to_string()) };
    let (path, anchor) = match rest.split_once('#') {
        Some((p, a)) => (p, Some(a)),
        None => (rest, None),
    };
    let hash = anchor.map(|a| format!("#{a}")).unwrap_or_default();
    if let Some(id) = path.strip_prefix("page/") {
        return Some(match l.pages.get(id) {
            Some(u) => {
                st.links_to_pages += 1;
                format!("/pages/{u}{hash}")
            }
            None => {
                st.links_to_missing_pages += 1;
                format!("/missing?source=confluence%3A{id}")
            }
        });
    }
    if let Some(rest) = path.strip_prefix("missing/").or_else(|| path.strip_prefix("wikilink/")) {
        st.links_to_missing_pages += 1;
        let (space, title) = rest.split_once('/').unwrap_or(("", rest));
        let space = l.space_keys.get(&decode_component(space)).cloned().unwrap_or_else(|| decode_component(space));
        return Some(format!("/missing?space={}&title={}", query_escape(&space), query_escape(&decode_component(title))));
    }
    if let Some(id) = path.strip_prefix("attachment/") {
        return match l.attachments.get(id) {
            Some(u) => {
                st.attachments_linked += 1;
                Some(format!("/api/attachments/{u}/content"))
            }
            None => {
                st.attachments_missing += 1;
                None
            }
        };
    }
    if path.starts_with("missing-attachment/") {
        st.attachments_missing += 1;
        return None;
    }
    if let Some(key) = path.strip_prefix("space/") {
        let key = decode_component(key);
        return Some(format!("/spaces/{}", query_escape(l.space_keys.get(&key).unwrap_or(&key))));
    }
    None
}

fn rewrite(doc: &mut Node, l: &Links, st: &mut RewriteStats) {
    doc.walk_mut(&mut |n| {
        // marks
        let mut drop = Vec::new();
        for (i, m) in n.marks.iter_mut().enumerate() {
            match m.kind.as_str() {
                marks::LINK => {
                    let href = m.attr_str("href").unwrap_or("").to_string();
                    match rewrite_href(&href, l, st) {
                        Some(h) => {
                            m.attrs.insert("href".into(), Value::String(h));
                        }
                        None => drop.push(i),
                    }
                }
                marks::COMMENT => {
                    let id = m.attr_str("commentId").unwrap_or("").to_string();
                    match id.strip_prefix("wcimport:inline/").and_then(|r| l.inline_refs.get(r)) {
                        Some(u) => {
                            st.inline_anchors += 1;
                            m.attrs.insert("commentId".into(), Value::String(u.to_string()));
                        }
                        None if id.starts_with("wcimport:") => drop.push(i),
                        None => {}
                    }
                }
                _ => {}
            }
        }
        for i in drop.into_iter().rev() {
            n.marks.remove(i);
        }
        match n.kind.as_str() {
            nodes::IMAGE => {
                let src = n.attr_str("src").unwrap_or("").to_string();
                if src.starts_with("wcimport:") {
                    match rewrite_href(&src, l, st) {
                        Some(h) => {
                            n.attrs.insert("src".into(), Value::String(h));
                        }
                        None => {
                            // binary not part of the export: keep a visible marker instead of a broken image
                            let name = n.attr_str("alt").or(n.attr_str("title")).unwrap_or("image").to_string();
                            n.kind = nodes::TEXT.into();
                            n.attrs.clear();
                            n.text = Some(format!("[missing image: {name}]"));
                            n.marks = vec![Mark::new(marks::ITALIC)];
                        }
                    }
                }
            }
            nodes::ATTACHMENT => {
                let href = n.attr_str("href").unwrap_or("").to_string();
                let new = rewrite_href(&href, l, st);
                n.attrs.insert("href".into(), new.map(Value::String).unwrap_or(Value::Null));
                if n.attrs.get("href").is_some_and(Value::is_null) {
                    n.attrs.remove("href");
                    n.attrs.insert("missing".into(), Value::Bool(true));
                }
            }
            nodes::INCLUDE => {
                if let Some(r) = n.attr_str("pageRef").map(String::from) {
                    if let Some(h) = rewrite_href(&r, l, st) {
                        if let Some(id) = h.strip_prefix("/pages/") {
                            n.attrs.insert("pageId".into(), Value::String(id.split('#').next().unwrap_or(id).to_string()));
                        }
                        n.attrs.insert("href".into(), Value::String(h));
                    }
                    n.attrs.remove("pageRef");
                }
            }
            nodes::MENTION => {
                let id = n.attr_str("id").unwrap_or("").to_string();
                if let Some(key) = id.strip_prefix("wcimport:user/") {
                    match l.users.get(key) {
                        Some(u) => {
                            st.mentions += 1;
                            n.attrs.insert("id".into(), Value::String(u.to_string()));
                        }
                        None => {
                            st.mentions_unknown += 1;
                            n.attrs.insert("id".into(), Value::String(String::new()));
                        }
                    }
                }
            }
            _ => {}
        }
    });
}

/// Adds a `comment` mark on the first occurrence of `needle` inside a single text node.
fn anchor_by_text(doc: &mut Node, needle: &str, comment_id: Uuid) -> bool {
    if needle.trim().is_empty() {
        return false;
    }
    fn go(n: &mut Node, needle: &str, id: Uuid) -> bool {
        for i in 0..n.content.len() {
            let c = &n.content[i];
            if c.is_text() {
                let t = c.text.clone().unwrap_or_default();
                if let Some(pos) = t.find(needle) {
                    let marks = c.marks.clone();
                    let mut with = marks.clone();
                    with.push(Mark::new(marks::COMMENT).attr("commentId", id.to_string()));
                    wikicraft_doc_model::node::sort_marks(&mut with);
                    let mut parts = Vec::new();
                    if pos > 0 {
                        parts.push(Node::text_marked(&t[..pos], marks.clone()));
                    }
                    parts.push(Node::text_marked(needle, with));
                    if pos + needle.len() < t.len() {
                        parts.push(Node::text_marked(&t[pos + needle.len()..], marks));
                    }
                    n.content.splice(i..=i, parts);
                    return true;
                }
            } else if go(&mut n.content[i], needle, id) {
                return true;
            }
        }
        false
    }
    go(doc, needle, comment_id)
}

fn role_for_confluence(perms: &[&str]) -> Option<&'static str> {
    if perms.contains(&"SETSPACEPERMISSIONS") {
        Some("admin")
    } else if perms.contains(&"EDITSPACE") {
        Some("editor")
    } else if perms.contains(&"COMMENT") {
        Some("commenter")
    } else if perms.contains(&"VIEWSPACE") {
        Some("viewer")
    } else {
        None
    }
}

async fn import(state: &AppState, job: Uuid, ws: Uuid, actor: Uuid, path: &Path, opts: &ImportOptions) -> anyhow::Result<Value> {
    set_progress(state, job, json!({"phase": "reading export"})).await;
    let p = path.to_path_buf();
    let export: Arc<Export> = Arc::new(tokio::task::spawn_blocking(move || wikicraft_confluence::open(&p)).await??);
    let plan_opts = PlanOptions {
        include_history: !opts.skip_history,
        include_drafts: !opts.skip_drafts,
        include_deleted: !opts.skip_trash,
        include_comments: !opts.skip_comments,
        include_attachments: !opts.skip_attachments,
    };
    let plan = {
        let e = export.clone();
        let po = plan_opts.clone();
        tokio::task::spawn_blocking(move || Planner::new(&e, po).plan()).await?
    };
    let planner = Planner::new(&export, plan_opts);
    let source = Source::open(path)?;
    let mut stats = Stats::default();
    let mut rs = RewriteStats::default();
    let mut created = serde_json::Map::new();
    let mut count = |k: &str, n: i64| {
        let e = created.entry(k.to_string()).or_insert(json!(0));
        *e = json!(e.as_i64().unwrap_or(0) + n);
    };

    // ---- users ------------------------------------------------------------------------
    set_progress(state, job, json!({"phase": "users", "total": plan.users.len()})).await;
    let mut users: HashMap<String, Uuid> = HashMap::new();
    let mut utx = state.db.begin().await?;
    for u in &plan.users {
        let ext = format!("confluence:{}", u.key);
        let mut id: Option<Uuid> =
            sqlx::query_scalar("SELECT id FROM users WHERE workspace_id = $1 AND external_ref = $2").bind(ws).bind(&ext).fetch_optional(&mut *utx).await?;
        if id.is_none()
            && let Some(email) = u.email.as_deref().map(str::to_lowercase).filter(|e| e.contains('@'))
        {
            id = sqlx::query_scalar("SELECT id FROM users WHERE workspace_id = $1 AND lower(email) = $2")
                .bind(ws)
                .bind(&email)
                .fetch_optional(&mut *utx)
                .await?;
        }
        let id = match id {
            Some(id) => id,
            None => {
                let name = u
                    .display_name
                    .clone()
                    .or_else(|| u.username.clone())
                    .unwrap_or_else(|| format!("Confluence user {}", u.key.chars().rev().take(6).collect::<String>().chars().rev().collect::<String>()));
                let id = new_id();
                let email = u.email.as_deref().map(str::to_lowercase).filter(|e| e.contains('@'));
                let email = match email {
                    Some(e) => {
                        let taken: bool = sqlx::query_scalar("SELECT EXISTS (SELECT 1 FROM users WHERE workspace_id = $1 AND lower(email) = $2)")
                            .bind(ws)
                            .bind(&e)
                            .fetch_one(&mut *utx)
                            .await?;
                        (!taken).then_some(e)
                    }
                    None => None,
                };
                sqlx::query(
                    "INSERT INTO users (id, workspace_id, email, name, role, status, external_ref) VALUES ($1, $2, $3, $4, 'member', 'placeholder', $5)",
                )
                .bind(id)
                .bind(ws)
                .bind(&email)
                .bind(&name)
                .bind(&ext)
                .execute(&mut *utx)
                .await?;
                count("placeholderUsers", 1);
                id
            }
        };
        users.insert(u.key.clone(), id);
        for alias in [&u.username, &u.account_id].into_iter().flatten() {
            users.entry(alias.clone()).or_insert(id);
        }
    }
    // users that only appear in restrictions/permissions still need an account, otherwise those
    // restrictions would silently be dropped (fail-open)
    let mut referenced: std::collections::BTreeSet<String> = std::collections::BTreeSet::new();
    for s in &plan.spaces {
        for pg in s.pages.iter().chain(&s.blog_posts) {
            for (u, _) in pg.view_restrictions.iter().chain(&pg.edit_restrictions) {
                if let Some(u) = u {
                    referenced.insert(u.clone());
                }
            }
        }
        for perm in &s.permissions {
            if let Some(u) = &perm.user_key {
                referenced.insert(u.clone());
            }
        }
    }
    for key in referenced {
        if users.contains_key(&key) {
            continue;
        }
        let ext = format!("confluence:{key}");
        let existing: Option<Uuid> =
            sqlx::query_scalar("SELECT id FROM users WHERE workspace_id = $1 AND external_ref = $2").bind(ws).bind(&ext).fetch_optional(&mut *utx).await?;
        let id = match existing {
            Some(id) => id,
            None => {
                let id = new_id();
                let short: String = key.chars().rev().take(6).collect::<String>().chars().rev().collect();
                sqlx::query("INSERT INTO users (id, workspace_id, name, role, status, external_ref) VALUES ($1, $2, $3, 'member', 'placeholder', $4)")
                    .bind(id)
                    .bind(ws)
                    .bind(format!("Confluence user {short}"))
                    .bind(&ext)
                    .execute(&mut *utx)
                    .await?;
                count("placeholderUsers", 1);
                id
            }
        };
        users.insert(key, id);
    }
    utx.commit().await?;
    let user_of = |k: &Option<String>| k.as_ref().and_then(|k| users.get(k)).copied();

    // ---- spaces -----------------------------------------------------------------------
    let mut space_keys: HashMap<String, String> = HashMap::new();
    let mut new_spaces: Vec<(String, Uuid)> = Vec::new();
    // an update import continues the spaces an earlier import of them created
    let mut earlier: HashMap<String, (Uuid, String)> = HashMap::new();
    if opts.update {
        for s in &plan.spaces {
            let found: Option<(Uuid, String)> = sqlx::query_as(
                "SELECT s.id, s.key FROM import_mappings m JOIN spaces s ON s.id = m.target_id
                  WHERE m.workspace_id = $1 AND m.source = 'confluence' AND m.source_type = 'space' AND m.source_id = $2",
            )
            .bind(ws)
            .bind(&s.source_id)
            .fetch_optional(&state.db)
            .await?;
            if let Some(f) = found {
                earlier.insert(s.source_id.clone(), f);
            }
        }
    }
    for s in &plan.spaces {
        if let Some((_, key)) = earlier.get(&s.source_id) {
            space_keys.insert(s.key.clone(), key.clone());
            continue;
        }
        let wanted = if plan.spaces.len() == 1 { opts.space_key.clone().unwrap_or_else(|| s.key.clone()) } else { s.key.clone() };
        let mut key = crate::routes::spaces::validate_key(&wanted).unwrap_or_else(|_| "IMPORT".into());
        let base = key.clone();
        let mut n = 2;
        while sqlx::query_scalar::<_, bool>("SELECT EXISTS (SELECT 1 FROM spaces WHERE workspace_id = $1 AND lower(key) = lower($2))")
            .bind(ws)
            .bind(&key)
            .fetch_one(&state.db)
            .await?
        {
            key = format!("{base}{n}");
            n += 1;
        }
        space_keys.insert(s.key.clone(), key.clone());
        new_spaces.push((s.source_id.clone(), Uuid::nil()));
    }

    let mut page_ids: HashMap<String, Uuid> = HashMap::new();
    let mut att_ids: HashMap<String, Uuid> = HashMap::new();
    let mut comment_ids: HashMap<String, Uuid> = HashMap::new();
    let mut inline_refs: HashMap<String, Uuid> = HashMap::new();
    // previously imported pages (other spaces) resolve too
    let prior: Vec<(String, Uuid)> =
        sqlx::query_as("SELECT source_id, target_id FROM import_mappings WHERE workspace_id = $1 AND source = 'confluence' AND source_type = 'page'")
            .bind(ws)
            .fetch_all(&state.db)
            .await?;
    page_ids.extend(prior);
    // what earlier imports brought into the spaces this one continues keeps its ids
    let mut known_pages: HashMap<String, Uuid> = HashMap::new();
    let mut known_atts: HashMap<String, Uuid> = HashMap::new();
    let mut known_comments: HashMap<String, Uuid> = HashMap::new();
    for (space_id, _) in earlier.values() {
        let pages: Vec<(String, Uuid)> = sqlx::query_as(
            "SELECT m.source_id, m.target_id FROM import_mappings m JOIN pages p ON p.id = m.target_id
              WHERE m.workspace_id = $1 AND m.source = 'confluence' AND m.source_type = 'page' AND p.space_id = $2",
        )
        .bind(ws)
        .bind(space_id)
        .fetch_all(&state.db)
        .await?;
        known_pages.extend(pages);
        let atts: Vec<(String, Uuid)> = sqlx::query_as(
            "SELECT substr(a.source_ref, 12), a.id FROM attachments a JOIN pages p ON p.id = a.page_id
              WHERE p.space_id = $1 AND a.source_ref LIKE 'confluence:%' AND a.deleted_at IS NULL",
        )
        .bind(space_id)
        .fetch_all(&state.db)
        .await?;
        known_atts.extend(atts);
        let comments: Vec<(String, Uuid)> = sqlx::query_as(
            "SELECT substr(c.source_ref, 12), c.id FROM comments c JOIN pages p ON p.id = c.page_id WHERE p.space_id = $1 AND c.source_ref LIKE 'confluence:%'",
        )
        .bind(space_id)
        .fetch_all(&state.db)
        .await?;
        known_comments.extend(comments);
    }
    for s in &plan.spaces {
        for pg in s.pages.iter().chain(&s.blog_posts) {
            page_ids.insert(pg.source_id.clone(), known_pages.get(&pg.source_id).copied().unwrap_or_else(new_id));
            for a in &pg.attachments {
                att_ids.insert(a.source_id.clone(), known_atts.get(&a.source_id).copied().unwrap_or_else(new_id));
            }
            for c in &pg.comments {
                let id = known_comments.get(&c.source_id).copied().unwrap_or_else(new_id);
                comment_ids.insert(c.source_id.clone(), id);
                if let Some(r) = &c.inline_marker_ref {
                    inline_refs.insert(r.clone(), id);
                }
            }
        }
    }
    let mut conflicts: Vec<Value> = Vec::new();
    let links = Links { pages: page_ids.clone(), attachments: att_ids.clone(), users: users.clone(), inline_refs, space_keys: space_keys.clone() };

    let total_pages: usize = plan.spaces.iter().map(|s| s.pages.len() + s.blog_posts.len()).sum();
    let mut done = 0usize;
    let mut result_spaces = Vec::new();
    for s in &plan.spaces {
        let key = space_keys[&s.key].clone();
        // an update import continues the space; its settings and permissions stay as admins left them
        let continued = earlier.get(&s.source_id).map(|(id, _)| *id);
        let mut anonymous_in_confluence = false;
        let mut default_role: &str = "none";
        let space_id = match continued {
            Some(id) => {
                count("spacesUpdated", 1);
                id
            }
            None => {
                let description = s.description_source_id.as_ref().map(|d| text::plain_text(&planner.convert(d, &mut stats))).filter(|d| !d.trim().is_empty());
                let space = crate::routes::spaces::create_space(
                    state,
                    ws,
                    Some(actor),
                    &key,
                    &s.name,
                    description.as_deref(),
                    None,
                    "none",
                    Some(&format!("confluence:space:{}", s.source_id)),
                )
                .await
                .map_err(|e| anyhow::anyhow!("creating space {key}: {e}"))?;
                count("spaces", 1);
                sqlx::query("INSERT INTO space_permissions (space_id, subject_type, subject_id, role) VALUES ($1, 'user', $2, 'admin') ON CONFLICT DO NOTHING")
                    .bind(space.id)
                    .bind(actor)
                    .execute(&state.db)
                    .await?;
                sqlx::query("INSERT INTO import_mappings (workspace_id, source, source_type, source_id, target_id, job_id) VALUES ($1, 'confluence', 'space', $2, $3, $4) ON CONFLICT (workspace_id, source, source_type, source_id) DO UPDATE SET target_id = EXCLUDED.target_id")
                .bind(ws)
                .bind(&s.source_id)
                .bind(space.id)
                .bind(job)
                .execute(&state.db)
                .await?;

                // remember the original key: old /display/KEY/Title URLs keep working after renames
                sqlx::query("INSERT INTO import_mappings (workspace_id, source, source_type, source_id, target_id, job_id) VALUES ($1, 'confluence', 'space_key', $2, $3, $4) ON CONFLICT (workspace_id, source, source_type, source_id) DO UPDATE SET target_id = EXCLUDED.target_id")
                .bind(ws)
                .bind(&s.key)
                .bind(space.id)
                .bind(job)
                .execute(&state.db)
                .await?;

                // permissions → roles
                let default_group = export.descriptor.get("defaultUsersGroup").cloned().unwrap_or_else(|| "confluence-users".into());
                let mut by_subject: HashMap<(String, String), Vec<&str>> = HashMap::new();
                let mut all_users_perms: Vec<&str> = Vec::new();
                for perm in &s.permissions {
                    if perm.anonymous && perm.kind == "VIEWSPACE" {
                        anonymous_in_confluence = true;
                    }
                    if perm.all_users {
                        all_users_perms.push(perm.kind.as_str());
                    }
                    if let Some(g) = &perm.group {
                        by_subject.entry(("group".into(), g.clone())).or_default().push(perm.kind.as_str());
                    }
                    if let Some(u) = &perm.user_key {
                        by_subject.entry(("user".into(), u.clone())).or_default().push(perm.kind.as_str());
                    }
                }
                default_role = if s.permissions.is_empty() { "editor" } else { "none" };
                if let Some(r) = role_for_confluence(&all_users_perms) {
                    default_role = r.max(default_role);
                }
                for ((ty, subject), perms) in &by_subject {
                    let Some(role) = role_for_confluence(perms) else { continue };
                    if ty == "group" {
                        if subject.eq_ignore_ascii_case(&default_group) || subject == "confluence-users" || subject == "users" {
                            default_role = role.max(default_role);
                            continue;
                        }
                        let gid = ensure_group(state, ws, subject).await?;
                        sqlx::query(
                            "INSERT INTO space_permissions (space_id, subject_type, subject_id, role) VALUES ($1, 'group', $2, $3) ON CONFLICT DO NOTHING",
                        )
                        .bind(space.id)
                        .bind(gid)
                        .bind(role)
                        .execute(&state.db)
                        .await?;
                    } else if let Some(uid) = users.get(subject) {
                        sqlx::query(
                            "INSERT INTO space_permissions (space_id, subject_type, subject_id, role) VALUES ($1, 'user', $2, $3) ON CONFLICT DO NOTHING",
                        )
                        .bind(space.id)
                        .bind(uid)
                        .bind(role)
                        .execute(&state.db)
                        .await?;
                    }
                }
                // never publish an imported space automatically: report it, an admin decides
                sqlx::query("UPDATE spaces SET default_role = $2, anonymous_access = false WHERE id = $1")
                    .bind(space.id)
                    .bind(default_role)
                    .execute(&state.db)
                    .await?;

                space.id
            }
        };
        let default_role: String = match continued {
            Some(id) => sqlx::query_scalar("SELECT default_role FROM spaces WHERE id = $1").bind(id).fetch_one(&state.db).await?,
            None => default_role.to_string(),
        };
        for (table, keys, counter) in [("space_watches", &s.watchers, "spaceWatches"), ("space_favorites", &s.favourited_by, "spaceFavourites")] {
            for key in keys {
                let Some(uid) = user_of(&Some(key.clone())) else {
                    count("watchesAndFavouritesWithoutUser", 1);
                    continue;
                };
                let sql = format!("INSERT INTO {table} (user_id, space_id) VALUES ($1, $2) ON CONFLICT DO NOTHING");
                let r = sqlx::query(sqlx::AssertSqlSafe(sql)).bind(uid).bind(space_id).execute(&state.db).await?;
                count(counter, r.rows_affected() as i64);
            }
        }
        // ---- pages ----
        let mut sibling_pos: HashMap<Option<Uuid>, f64> = HashMap::new();
        // batch writes: one transaction per 25 pages (single-row autocommits are ~10x slower)
        let mut tx = state.db.begin().await?;
        let mut in_tx = 0;
        for pg in s.pages.iter().chain(&s.blog_posts) {
            let id = page_ids[&pg.source_id];
            let parent = pg.parent_source_id.as_ref().and_then(|p| page_ids.get(p)).copied();
            // (source version, target version) to remember; None: keep what the last import noted
            let mut versions: (Option<i32>, Option<i32>) = (None, None);
            let known = known_pages.contains_key(&pg.source_id);
            if known {
                let synced =
                    sync_page(state, &mut tx, actor, ws, id, pg, &planner, &links, &user_of, &mut stats, &mut rs, opts.overwrite_local_changes).await?;
                match synced {
                    Synced::Unchanged => count("pagesUnchanged", 1),
                    Synced::Updated(v) => {
                        count("pagesUpdated", 1);
                        versions = (Some(pg.version as i32), Some(v));
                    }
                    Synced::Trashed => count("pagesTrashedInConfluence", 1),
                    Synced::BeingEdited | Synced::Conflict => {
                        count(if synced == Synced::Conflict { "conflicts" } else { "pagesBeingEdited" }, 1);
                        if conflicts.len() < 200 {
                            conflicts.push(json!({"id": id, "title": pg.title, "reason": if synced == Synced::Conflict { "changed here and in Confluence" } else { "open in an editor" }}));
                        }
                    }
                }
                if pg.status != ContentStatus::Deleted && sync_parent(&mut tx, id, parent).await? {
                    count("pagesMoved", 1);
                }
            } else {
                // a new page: after its siblings (also the ones earlier imports or people added)
                if let std::collections::hash_map::Entry::Vacant(slot) = sibling_pos.entry(parent) {
                    let max: Option<f64> = sqlx::query_scalar("SELECT max(position) FROM pages WHERE space_id = $1 AND parent_id IS NOT DISTINCT FROM $2")
                        .bind(space_id)
                        .bind(parent)
                        .fetch_one(&mut *tx)
                        .await?;
                    slot.insert(max.map(|m| m + 1.0).unwrap_or(0.0));
                }
                let pos = sibling_pos.get_mut(&parent).expect("inserted above");
                let position = *pos;
                *pos += 1.0;
                let v = import_page(&mut tx, actor, ws, space_id, id, parent, position, pg, &planner, &links, &user_of, &mut stats, &mut rs).await?;
                versions = (Some(pg.version as i32), Some(v));
            }
            if !opts.skip_attachments {
                import_attachments(state, &mut tx, ws, id, pg, &source, &att_ids, &known_atts, &user_of, &mut count).await?;
            }
            if !opts.skip_comments {
                let added = import_comments(&mut tx, id, pg, &planner, &links, &comment_ids, &known_comments, &user_of, &mut stats, &mut rs).await?;
                if known {
                    count("commentsAdded", added as i64);
                }
            }
            // watches and favourites (a draft only keeps its author's: nobody else sees it)
            let author = user_of(&pg.created_by);
            for (table, keys, counter) in [("watches", &pg.watchers, "watches"), ("favorites", &pg.favourited_by, "favourites")] {
                for key in keys {
                    let Some(uid) = user_of(&Some(key.clone())) else {
                        count("watchesAndFavouritesWithoutUser", 1);
                        continue;
                    };
                    if pg.status == ContentStatus::Draft && Some(uid) != author {
                        continue;
                    }
                    let sql = format!("INSERT INTO {table} (user_id, page_id) VALUES ($1, $2) ON CONFLICT DO NOTHING");
                    let r = sqlx::query(sqlx::AssertSqlSafe(sql)).bind(uid).bind(id).execute(&mut *tx).await?;
                    count(counter, r.rows_affected() as i64);
                }
            }
            sqlx::query(
                "INSERT INTO import_mappings (workspace_id, source, source_type, source_id, target_id, job_id, source_version, target_version) VALUES ($1, 'confluence', 'page', $2, $3, $4, $5, $6)
                 ON CONFLICT (workspace_id, source, source_type, source_id) DO UPDATE SET target_id = EXCLUDED.target_id,
                     source_version = COALESCE(EXCLUDED.source_version, import_mappings.source_version), target_version = COALESCE(EXCLUDED.target_version, import_mappings.target_version)",
            )
            .bind(ws)
            .bind(&pg.source_id)
            .bind(id)
            .bind(job)
            .bind(versions.0)
            .bind(versions.1)
            .execute(&mut *tx)
            .await?;
            in_tx += 1;
            if in_tx >= 25 {
                tx.commit().await?;
                tx = state.db.begin().await?;
                in_tx = 0;
            }
            if !known {
                count(
                    match pg.status {
                        ContentStatus::Draft => "drafts",
                        ContentStatus::Deleted => "trashedPages",
                        _ if pg.kind == NodeKind::BlogPost => "blogPosts",
                        _ => "pages",
                    },
                    1,
                );
                count("versions", pg.history.len() as i64 + 1);
                count("comments", pg.comments.len() as i64);
            }
            done += 1;
            if done.is_multiple_of(25) || done == total_pages {
                set_progress(state, job, json!({"phase": "pages", "done": done, "total": total_pages, "space": key})).await;
            }
        }
        tx.commit().await?;
        let home: Option<Uuid> = if continued.is_some() {
            // pages an earlier import brought in that this export doesn't have (deleted and
            // purged in Confluence, or a partial export): reported, not touched
            let in_export: Vec<String> = s.pages.iter().chain(&s.blog_posts).map(|p| p.source_id.clone()).collect();
            let missing: i64 = sqlx::query_scalar(
                "SELECT count(*) FROM import_mappings m JOIN pages p ON p.id = m.target_id
                  WHERE m.workspace_id = $1 AND m.source = 'confluence' AND m.source_type = 'page' AND p.space_id = $2 AND p.deleted_at IS NULL
                    AND NOT (m.source_id = ANY($3))",
            )
            .bind(ws)
            .bind(space_id)
            .bind(&in_export)
            .fetch_one(&state.db)
            .await?;
            if missing > 0 {
                count("pagesNotInExport", missing);
            }
            // the home page stays what it is now
            sqlx::query_scalar("SELECT home_page_id FROM spaces WHERE id = $1").bind(space_id).fetch_one(&state.db).await?
        } else {
            // the home page must be a page everyone can see: not a draft and not in the trash
            let live = |p: &&PagePlan| matches!(p.status, ContentStatus::Current | ContentStatus::Archived);
            let home = s
                .home_page_source_id
                .as_ref()
                .filter(|h| s.pages.iter().filter(live).any(|p| &p.source_id == *h))
                .and_then(|h| page_ids.get(h))
                .copied()
                .or_else(|| s.pages.iter().find(|p| live(p) && p.parent_source_id.is_none()).map(|p| page_ids[&p.source_id]));
            sqlx::query("UPDATE spaces SET home_page_id = $2 WHERE id = $1").bind(space_id).bind(home).execute(&state.db).await?;
            home
        };
        for pg in s.pages.iter().chain(&s.blog_posts) {
            state.search.enqueue(page_ids[&pg.source_id]);
        }
        // attachment contents (PDF, Office, text) become searchable in the background
        let atts: Vec<Uuid> = sqlx::query_scalar(
            "SELECT a.id FROM attachments a JOIN pages p ON p.id = a.page_id WHERE p.space_id = $1 AND a.blob_key IS NOT NULL AND a.text_content IS NULL",
        )
        .bind(space_id)
        .fetch_all(&state.db)
        .await?;
        for a in atts {
            crate::extract::spawn(state, a);
        }
        result_spaces.push(json!({"key": key, "originalKey": s.key, "id": space_id, "updated": continued.is_some(), "homePageId": home, "name": s.name, "anonymousAccessInConfluence": anonymous_in_confluence, "defaultRole": default_role}));
    }
    let _ = new_spaces;
    crate::audit::log(state, ws, Some(actor), "import.confluence", Some("job"), Some(job), json!({"spaces": result_spaces})).await;
    Ok(json!({
        "spaces": result_spaces,
        "created": created,
        "links": rs,
        "conflicts": conflicts,
        "report": plan.report,
        "conversion": stats,
    }))
}

async fn ensure_group(state: &AppState, ws: Uuid, name: &str) -> anyhow::Result<Uuid> {
    if let Some(id) = sqlx::query_scalar::<_, Uuid>("SELECT id FROM groups WHERE workspace_id = $1 AND lower(name) = lower($2)")
        .bind(ws)
        .bind(name)
        .fetch_optional(&state.db)
        .await?
    {
        return Ok(id);
    }
    let id = new_id();
    sqlx::query("INSERT INTO groups (id, workspace_id, name, external_ref) VALUES ($1, $2, $3, $4) ON CONFLICT DO NOTHING")
        .bind(id)
        .bind(ws)
        .bind(name)
        .bind(format!("confluence:{name}"))
        .execute(&state.db)
        .await?;
    Ok(sqlx::query_scalar("SELECT id FROM groups WHERE workspace_id = $1 AND lower(name) = lower($2)").bind(ws).bind(name).fetch_one(&state.db).await?)
}

/// What an update import did with a page that an earlier import created.
#[derive(Debug, PartialEq, Eq)]
enum Synced {
    /// not changed in Confluence since the last import
    Unchanged,
    /// Confluence's new versions added; the page's version is now this
    Updated(i32),
    /// changed in Confluence and here: left as it is (unless overwriting was asked for)
    Conflict,
    /// open in an editor right now: left as it is
    BeingEdited,
    /// deleted in Confluence since: moved to the trash
    Trashed,
}

/// Brings a previously imported page up to date with the export: Confluence's versions since the
/// last import are appended to its history and the current one becomes its content. Pages also
/// changed here are left alone (a conflict) unless `overwrite`; local versions then stay in the
/// history below Confluence's.
#[allow(clippy::too_many_arguments)]
async fn sync_page(
    state: &AppState,
    tx: &mut sqlx::PgConnection,
    actor: Uuid,
    ws: Uuid,
    id: Uuid,
    pg: &PagePlan,
    planner: &Planner<'_>,
    links: &Links,
    user_of: &impl Fn(&Option<String>) -> Option<Uuid>,
    stats: &mut Stats,
    rs: &mut RewriteStats,
    overwrite: bool,
) -> anyhow::Result<Synced> {
    let row: Option<(i32, Value, Option<chrono::DateTime<chrono::Utc>>, Option<chrono::DateTime<chrono::Utc>>)> =
        sqlx::query_as("SELECT version, content, deleted_at, archived_at FROM pages WHERE id = $1 FOR UPDATE").bind(id).fetch_optional(&mut *tx).await?;
    let Some((our_version, content, deleted_at, archived_at)) = row else { return Ok(Synced::Unchanged) };
    let source_ref = format!("confluence:{}", pg.source_id);
    let (source_version, target_version): (Option<i32>, Option<i32>) = sqlx::query_as(
        "SELECT source_version, target_version FROM import_mappings WHERE workspace_id = $1 AND source = 'confluence' AND source_type = 'page' AND source_id = $2",
    )
    .bind(ws)
    .bind(&pg.source_id)
    .fetch_optional(&mut *tx)
    .await?
    .unwrap_or((None, None));
    // imports from before these were recorded: the version the import made of the page
    let imported: Option<i32> = sqlx::query_scalar("SELECT max(version) FROM page_versions WHERE page_id = $1 AND source_ref = $2")
        .bind(id)
        .bind(&source_ref)
        .fetch_one(&mut *tx)
        .await?;
    let target_version = target_version.or(imported).unwrap_or(our_version);
    let source_version = source_version.or(imported).map(i64::from).unwrap_or(pg.version);
    let deleted_there = pg.status == ContentStatus::Deleted && deleted_at.is_none();
    if pg.version <= source_version && !deleted_there {
        return Ok(Synced::Unchanged);
    }
    if state.rooms.get(id).is_some() {
        return Ok(Synced::BeingEdited);
    }
    let base: Option<Value> = sqlx::query_scalar("SELECT content FROM page_versions WHERE page_id = $1 AND version = $2")
        .bind(id)
        .bind(target_version)
        .fetch_optional(&mut *tx)
        .await?;
    let changed_here = our_version > target_version || base.as_ref() != Some(&content);
    if changed_here && !overwrite {
        return Ok(Synced::Conflict);
    }
    if pg.status == ContentStatus::Deleted {
        sqlx::query("UPDATE pages SET deleted_at = clock_timestamp(), deleted_by = $2 WHERE id = $1 AND deleted_at IS NULL")
            .bind(id)
            .bind(actor)
            .execute(&mut *tx)
            .await?;
        return Ok(Synced::Trashed);
    }
    let mut next: i32 = sqlx::query_scalar::<_, Option<i32>>("SELECT max(version) FROM page_versions WHERE page_id = $1")
        .bind(id)
        .fetch_one(&mut *tx)
        .await?
        .unwrap_or(our_version)
        + 1;
    for h in pg.history.iter().filter(|h| h.version > source_version) {
        let href = format!("confluence:{}", h.source_id);
        let known: bool = sqlx::query_scalar("SELECT EXISTS (SELECT 1 FROM page_versions WHERE page_id = $1 AND source_ref = $2)")
            .bind(id)
            .bind(&href)
            .fetch_one(&mut *tx)
            .await?;
        if known {
            continue;
        }
        let mut hdoc = tokio::task::block_in_place(|| planner.convert(&h.source_id, stats));
        rewrite(&mut hdoc, links, &mut RewriteStats::default());
        sqlx::query("INSERT INTO page_versions (id, page_id, version, title, content, created_by, created_at, message, source_ref) VALUES ($1, $2, $3, $4, $5, $6, COALESCE($7, now()), $8, $9)")
            .bind(new_id())
            .bind(id)
            .bind(next)
            .bind(&h.title)
            .bind(hdoc.to_json())
            .bind(user_of(&h.author))
            .bind(parse_date(h.created_at.as_deref()))
            .bind(&h.message)
            .bind(href)
            .execute(&mut *tx)
            .await?;
        next += 1;
    }
    let mut doc = tokio::task::block_in_place(|| planner.convert(&pg.source_id, stats));
    rewrite(&mut doc, links, rs);
    // blocks that didn't change keep their ids (links to them keep working)
    if let Ok(old) = Node::from_json(&content) {
        wikicraft_doc_model::blocks::adopt_ids(&old, &mut doc);
    }
    wikicraft_doc_model::blocks::ensure_ids(&mut doc);
    let updated_at = parse_date(pg.updated_at.as_deref());
    let updated_by = user_of(&pg.updated_by).or_else(|| user_of(&pg.created_by));
    sqlx::query("INSERT INTO page_versions (id, page_id, version, title, content, created_by, created_at, message, source_ref) VALUES ($1, $2, $3, $4, $5, $6, COALESCE($7, now()), $8, $9)")
        .bind(new_id())
        .bind(id)
        .bind(next)
        .bind(&pg.title)
        .bind(doc.to_json())
        .bind(updated_by)
        .bind(updated_at)
        .bind("Updated from Confluence")
        .bind(&source_ref)
        .execute(&mut *tx)
        .await?;
    sqlx::query(
        "UPDATE pages SET title = $2, content = $3, text_content = $4, ydoc = $5, version = $6, updated_by = $7, updated_at = COALESCE($8, now()),
                deleted_at = NULL, deleted_by = NULL, archived_at = CASE WHEN $9 THEN COALESCE(archived_at, now()) ELSE archived_at END
          WHERE id = $1",
    )
    .bind(id)
    .bind(&pg.title)
    .bind(doc.to_json())
    .bind(text::plain_text(&doc))
    .bind(ydoc::pm_to_update(&doc))
    .bind(next)
    .bind(updated_by)
    .bind(updated_at)
    .bind(pg.status == ContentStatus::Archived && archived_at.is_none())
    .execute(&mut *tx)
    .await?;
    crate::routes::tasks::index_page(&mut *tx, id, &doc).await?;
    for l in &pg.labels {
        if let Some(l) = crate::routes::pages::normalize_label(l) {
            sqlx::query("INSERT INTO labels (page_id, name) VALUES ($1, $2) ON CONFLICT DO NOTHING").bind(id).bind(l).execute(&mut *tx).await?;
        }
    }
    Ok(Synced::Updated(next))
}

/// Moves a page under the parent it has in Confluence (Confluence doesn't count moves as
/// versions, so this is checked for every previously imported page), unless that would put it
/// below itself. Returns whether it moved.
async fn sync_parent(tx: &mut sqlx::PgConnection, id: Uuid, parent: Option<Uuid>) -> anyhow::Result<bool> {
    let current: Option<Option<Uuid>> = sqlx::query_scalar("SELECT parent_id FROM pages WHERE id = $1").bind(id).fetch_optional(&mut *tx).await?;
    let Some(current) = current else { return Ok(false) };
    if current == parent {
        return Ok(false);
    }
    if let Some(p) = parent {
        let below_itself: bool = sqlx::query_scalar(
            "WITH RECURSIVE up AS (SELECT id, parent_id, 0 AS depth FROM pages WHERE id = $1
                                   UNION ALL SELECT p.id, p.parent_id, up.depth + 1 FROM pages p JOIN up ON p.id = up.parent_id WHERE up.depth < 200)
             SELECT EXISTS (SELECT 1 FROM up WHERE id = $2)",
        )
        .bind(p)
        .bind(id)
        .fetch_one(&mut *tx)
        .await?;
        if below_itself {
            return Ok(false);
        }
    }
    sqlx::query("UPDATE pages SET parent_id = $2 WHERE id = $1").bind(id).bind(parent).execute(&mut *tx).await?;
    Ok(true)
}

#[allow(clippy::too_many_arguments)]
async fn import_page(
    tx: &mut sqlx::PgConnection,
    actor: Uuid,
    ws: Uuid,
    space: Uuid,
    id: Uuid,
    parent: Option<Uuid>,
    position: f64,
    pg: &PagePlan,
    planner: &Planner<'_>,
    links: &Links,
    user_of: &impl Fn(&Option<String>) -> Option<Uuid>,
    stats: &mut Stats,
    rs: &mut RewriteStats,
) -> anyhow::Result<i32> {
    let mut doc = tokio::task::block_in_place(|| planner.convert(&pg.source_id, stats));
    rewrite(&mut doc, links, rs);
    wikicraft_doc_model::blocks::ensure_ids(&mut doc);
    let kind = match pg.kind {
        NodeKind::Folder => "folder",
        NodeKind::BlogPost => "blog",
        _ => "page",
    };
    let created_at = parse_date(pg.created_at.as_deref());
    let updated_at = parse_date(pg.updated_at.as_deref()).or(created_at);
    let created_by = user_of(&pg.created_by);
    let updated_by = user_of(&pg.updated_by).or(created_by);
    let status = if pg.status == ContentStatus::Archived { "archived" } else { "current" };
    sqlx::query(
        // archived Confluence pages land in the archive (Space settings → Archived), not in the
        // tree; drafts become their author's private drafts; trashed pages go to the trash, each
        // with its own time (restoring one doesn't bring back others) that starts the retention
        // period now, so admins get the full period to look through them
        "INSERT INTO pages (id, workspace_id, space_id, parent_id, kind, title, position, content, text_content, ydoc, version, status, page_status, created_by, created_at, updated_by, updated_at, source_ref, archived_at, draft, deleted_at, deleted_by)
         VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10, $11, $12, $13, $14, COALESCE($15, now()), $16, COALESCE($17, now()), $18,
                 CASE WHEN $12 = 'archived' THEN COALESCE($17, now()) END, $19, CASE WHEN $20 THEN clock_timestamp() END, CASE WHEN $20 THEN $21::uuid END)",
    )
    .bind(id)
    .bind(ws)
    .bind(space)
    .bind(parent)
    .bind(kind)
    .bind(&pg.title)
    .bind(position)
    .bind(doc.to_json())
    .bind(text::plain_text(&doc))
    .bind(ydoc::pm_to_update(&doc))
    .bind(pg.version.max(1) as i32)
    .bind(status)
    .bind(&pg.page_status)
    .bind(created_by)
    .bind(created_at)
    .bind(updated_by)
    .bind(updated_at)
    .bind(format!("confluence:{}", pg.source_id))
    .bind(pg.status == ContentStatus::Draft)
    .bind(pg.status == ContentStatus::Deleted)
    .bind(actor)
    .execute(&mut *tx)
    .await?;
    let mut last_version = 0;
    for h in &pg.history {
        let mut hdoc = tokio::task::block_in_place(|| planner.convert(&h.source_id, stats));
        rewrite(&mut hdoc, links, &mut RewriteStats::default());
        let v = (h.version as i32).max(last_version + 1);
        last_version = v;
        sqlx::query("INSERT INTO page_versions (id, page_id, version, title, content, created_by, created_at, message, source_ref) VALUES ($1, $2, $3, $4, $5, $6, COALESCE($7, now()), $8, $9)")
            .bind(new_id())
            .bind(id)
            .bind(v)
            .bind(&h.title)
            .bind(hdoc.to_json())
            .bind(user_of(&h.author))
            .bind(parse_date(h.created_at.as_deref()))
            .bind(&h.message)
            .bind(format!("confluence:{}", h.source_id))
            .execute(&mut *tx)
            .await?;
    }
    let current_version = (pg.version as i32).max(last_version + 1);
    sqlx::query("INSERT INTO page_versions (id, page_id, version, title, content, created_by, created_at, message, source_ref) VALUES ($1, $2, $3, $4, $5, $6, COALESCE($7, now()), $8, $9)")
        .bind(new_id())
        .bind(id)
        .bind(current_version)
        .bind(&pg.title)
        .bind(doc.to_json())
        .bind(updated_by)
        .bind(updated_at)
        .bind(Some("Imported from Confluence"))
        .bind(format!("confluence:{}", pg.source_id))
        .execute(&mut *tx)
        .await?;
    sqlx::query("UPDATE pages SET version = $2 WHERE id = $1").bind(id).bind(current_version).execute(&mut *tx).await?;
    crate::routes::tasks::index_page(&mut *tx, id, &doc).await?;
    for l in &pg.labels {
        if let Some(l) = crate::routes::pages::normalize_label(l) {
            sqlx::query("INSERT INTO labels (page_id, name) VALUES ($1, $2) ON CONFLICT DO NOTHING").bind(id).bind(l).execute(&mut *tx).await?;
        }
    }
    for (kind, entries) in [("view", &pg.view_restrictions), ("edit", &pg.edit_restrictions)] {
        let mut mapped = 0;
        for (user, group) in entries {
            if let Some(uid) = user_of(user) {
                sqlx::query("INSERT INTO page_restrictions (page_id, kind, subject_type, subject_id) VALUES ($1, $2, 'user', $3) ON CONFLICT DO NOTHING")
                    .bind(id)
                    .bind(kind)
                    .bind(uid)
                    .execute(&mut *tx)
                    .await?;
                mapped += 1;
            }
            if let Some(g) = group {
                let gid: Option<Uuid> = sqlx::query_scalar("SELECT id FROM groups WHERE workspace_id = $1 AND lower(name) = lower($2)")
                    .bind(ws)
                    .bind(g)
                    .fetch_optional(&mut *tx)
                    .await?;
                let gid = match gid {
                    Some(g) => g,
                    None => {
                        let gid = new_id();
                        sqlx::query("INSERT INTO groups (id, workspace_id, name, external_ref) VALUES ($1, $2, $3, $4)")
                            .bind(gid)
                            .bind(ws)
                            .bind(g)
                            .bind(format!("confluence:{g}"))
                            .execute(&mut *tx)
                            .await?;
                        gid
                    }
                };
                sqlx::query("INSERT INTO page_restrictions (page_id, kind, subject_type, subject_id) VALUES ($1, $2, 'group', $3) ON CONFLICT DO NOTHING")
                    .bind(id)
                    .bind(kind)
                    .bind(gid)
                    .execute(&mut *tx)
                    .await?;
                mapped += 1;
            }
        }
        if !entries.is_empty() && mapped == 0 {
            // nothing could be mapped: fail closed (importing admin only) instead of opening the page
            sqlx::query("INSERT INTO page_restrictions (page_id, kind, subject_type, subject_id) VALUES ($1, $2, 'user', $3) ON CONFLICT DO NOTHING")
                .bind(id)
                .bind(kind)
                .bind(actor)
                .execute(&mut *tx)
                .await?;
        }
    }
    Ok(current_version)
}

#[allow(clippy::too_many_arguments)]
async fn import_attachments(
    state: &AppState,
    tx: &mut sqlx::PgConnection,
    ws: Uuid,
    page_id: Uuid,
    pg: &PagePlan,
    source: &Source,
    att_ids: &HashMap<String, Uuid>,
    existing: &HashMap<String, Uuid>,
    user_of: &impl Fn(&Option<String>) -> Option<Uuid>,
    count: &mut impl FnMut(&str, i64),
) -> anyhow::Result<()> {
    for a in &pg.attachments {
        let id = att_ids[&a.source_id];
        let filename = crate::routes::attachments::clean_filename(&a.filename);
        // brought in by an earlier import: only its versions since then
        let known = existing.contains_key(&a.source_id);
        let have: i32 = if known {
            sqlx::query_scalar::<_, Option<i32>>("SELECT max(version) FROM attachment_versions WHERE attachment_id = $1")
                .bind(id)
                .fetch_one(&mut *tx)
                .await?
                .unwrap_or(0)
        } else {
            0
        };
        let mut current: Option<(String, i64, String, i32)> = None;
        let mut versions = Vec::new();
        for (i, v) in a.versions.iter().enumerate() {
            if (i + 1) as i32 <= have {
                continue;
            }
            let data = match &v.path {
                Some(p) => tokio::task::block_in_place(|| source.read_bytes(p)).ok(),
                None => None,
            };
            let (key, size, media) = match data {
                Some(bytes) => {
                    let media = a.media_type.clone().unwrap_or_else(|| crate::routes::attachments::guess_media_type(&filename, &bytes));
                    let size = bytes.len() as i64;
                    let key = state.blobs.put(&mut *tx, Bytes::from(bytes)).await?;
                    (Some(key), size, media)
                }
                None => {
                    // exports often omit binaries of old attachment versions; the current one matters most
                    count(if i + 1 == a.versions.len() { "attachmentBinariesMissing" } else { "attachmentHistoryBinariesMissing" }, 1);
                    (
                        None,
                        a.size.unwrap_or(0),
                        a.media_type.clone().unwrap_or_else(|| mime_guess::from_path(&filename).first_or_octet_stream().essence_str().to_string()),
                    )
                }
            };
            let version = (i + 1) as i32;
            versions.push((version, key.clone(), size, media.clone(), user_of(&v.author), parse_date(v.created_at.as_deref()), v.comment.clone()));
            current = Some((key.unwrap_or_default(), size, media, version));
        }
        let Some((key, size, media, version)) = current else { continue };
        let key = (!key.is_empty()).then_some(key);
        if known {
            for (version, key, size, media, author, at, comment) in versions {
                sqlx::query("INSERT INTO attachment_versions (id, attachment_id, version, blob_key, size, media_type, created_by, created_at, comment) VALUES ($1, $2, $3, $4, $5, $6, $7, COALESCE($8, now()), $9)")
                    .bind(new_id())
                    .bind(id)
                    .bind(version)
                    .bind(key)
                    .bind(size)
                    .bind(media)
                    .bind(author)
                    .bind(at)
                    .bind(comment)
                    .execute(&mut *tx)
                    .await?;
                count("attachmentVersionsAdded", 1);
            }
            sqlx::query(
                "UPDATE attachments SET blob_key = $2, size = $3, media_type = $4, version = $5, updated_at = now(), text_content = NULL WHERE id = $1",
            )
            .bind(id)
            .bind(&key)
            .bind(size)
            .bind(&media)
            .bind(version)
            .execute(&mut *tx)
            .await?;
            continue;
        }
        // same file name twice on a page (Confluence allows it for deleted/renamed attachments): suffix it
        let mut name = filename.clone();
        let mut n = 2;
        while sqlx::query_scalar::<_, bool>("SELECT EXISTS (SELECT 1 FROM attachments WHERE page_id = $1 AND filename = $2 AND deleted_at IS NULL)")
            .bind(page_id)
            .bind(&name)
            .fetch_one(&mut *tx)
            .await?
        {
            name = format!("{n}-{filename}");
            n += 1;
        }
        sqlx::query("INSERT INTO attachments (id, workspace_id, page_id, filename, media_type, size, blob_key, version, created_by, created_at, updated_at, source_ref) VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, COALESCE($10, now()), COALESCE($11, now()), $12)")
            .bind(id)
            .bind(ws)
            .bind(page_id)
            .bind(&name)
            .bind(&media)
            .bind(size)
            .bind(&key)
            .bind(version)
            .bind(user_of(&a.created_by))
            .bind(parse_date(a.created_at.as_deref()))
            .bind(versions.last().and_then(|v| v.5))
            .bind(format!("confluence:{}", a.source_id))
            .execute(&mut *tx)
            .await?;
        for (version, key, size, media, author, at, comment) in versions {
            sqlx::query("INSERT INTO attachment_versions (id, attachment_id, version, blob_key, size, media_type, created_by, created_at, comment) VALUES ($1, $2, $3, $4, $5, $6, $7, COALESCE($8, now()), $9)")
                .bind(new_id())
                .bind(id)
                .bind(version)
                .bind(key)
                .bind(size)
                .bind(media)
                .bind(author)
                .bind(at)
                .bind(comment)
                .execute(&mut *tx)
                .await?;
        }
        count("attachments", 1);
    }
    Ok(())
}

#[allow(clippy::too_many_arguments)]
async fn import_comments(
    tx: &mut sqlx::PgConnection,
    page_id: Uuid,
    pg: &PagePlan,
    planner: &Planner<'_>,
    links: &Links,
    comment_ids: &HashMap<String, Uuid>,
    existing: &HashMap<String, Uuid>,
    user_of: &impl Fn(&Option<String>) -> Option<Uuid>,
    stats: &mut Stats,
    rs: &mut RewriteStats,
) -> anyhow::Result<usize> {
    // comments an earlier import brought in stay as they are
    let new: Vec<&CommentPlan> = pg.comments.iter().filter(|c| !existing.contains_key(&c.source_id)).collect();
    if new.is_empty() {
        return Ok(0);
    }
    let mut text_anchored: Vec<(&CommentPlan, Uuid)> = Vec::new();
    for c in new.iter().copied() {
        let id = comment_ids[&c.source_id];
        let mut doc = tokio::task::block_in_place(|| planner.convert(&c.source_id, stats));
        rewrite(&mut doc, links, rs);
        let inline = c.inline_original_selection.as_ref().map(|t| json!({"text": t})).or_else(|| c.inline_marker_ref.as_ref().map(|_| json!({"text": ""})));
        let created = parse_date(c.created_at.as_deref());
        let updated = parse_date(c.updated_at.as_deref()).or(created);
        sqlx::query("INSERT INTO comments (id, page_id, author_id, content, text_content, inline, resolved_at, created_at, updated_at, source_ref) VALUES ($1, $2, $3, $4, $5, $6, $7, COALESCE($8, now()), COALESCE($9, now()), $10)")
            .bind(id)
            .bind(page_id)
            .bind(user_of(&c.author))
            .bind(doc.to_json())
            .bind(text::plain_text(&doc))
            .bind(inline)
            .bind(if c.resolved { updated.or(Some(chrono::Utc::now())) } else { None })
            .bind(created)
            .bind(updated)
            .bind(format!("confluence:{}", c.source_id))
            .execute(&mut *tx)
            .await?;
        if c.inline_marker_ref.is_none() && c.inline_original_selection.is_some() {
            text_anchored.push((c, id));
        }
    }
    for c in new.iter().copied() {
        if let Some(parent) = c.parent_source_id.as_ref().and_then(|p| comment_ids.get(p)) {
            sqlx::query("UPDATE comments SET parent_id = $2 WHERE id = $1 AND EXISTS (SELECT 1 FROM comments WHERE id = $2)")
                .bind(comment_ids[&c.source_id])
                .bind(parent)
                .execute(&mut *tx)
                .await?;
        }
    }
    // anchor text-only inline comments into the page content
    if !text_anchored.is_empty() {
        let content: Value = sqlx::query_scalar("SELECT content FROM pages WHERE id = $1").bind(page_id).fetch_one(&mut *tx).await?;
        let mut doc = Node::from_json(&content)?;
        let mut changed = false;
        for (c, id) in text_anchored {
            if anchor_by_text(&mut doc, c.inline_original_selection.as_deref().unwrap_or(""), id) {
                rs.inline_anchors += 1;
                changed = true;
            }
        }
        if changed {
            sqlx::query("UPDATE pages SET content = $2, ydoc = $3 WHERE id = $1")
                .bind(page_id)
                .bind(doc.to_json())
                .bind(ydoc::pm_to_update(&doc))
                .execute(&mut *tx)
                .await?;
        }
    }
    Ok(new.len())
}

/// Dry run: reads + plans + converts every current body; returns the report without writing anything.
pub fn analyze(path: &Path) -> anyhow::Result<Value> {
    let export = wikicraft_confluence::open(path)?;
    let planner = Planner::new(&export, PlanOptions::default());
    let plan = planner.plan();
    let mut stats = Stats::default();
    for s in &plan.spaces {
        for p in s.pages.iter().chain(&s.blog_posts) {
            planner.convert(&p.source_id, &mut stats);
            for c in &p.comments {
                planner.convert(&c.source_id, &mut stats);
            }
        }
    }
    Ok(json!({
        "format": export.format,
        "descriptor": export.descriptor,
        "spaces": plan.spaces.iter().map(|s| json!({"key": s.key, "name": s.name, "sourceId": s.source_id, "pages": s.pages.len(), "blogPosts": s.blog_posts.len()})).collect::<Vec<_>>(),
        "users": plan.users.len(),
        "report": plan.report,
        "conversion": stats,
    }))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn decodes_placeholder_components() {
        assert_eq!(decode_component("Hello+World%2FX"), "Hello World/X");
        assert_eq!(decode_component("100%"), "100%");
    }

    #[test]
    fn anchors_comment_by_text() {
        let mut doc = Node::doc(vec![Node::paragraph(vec![Node::text("the level of assurance matters")])]);
        let id = Uuid::nil();
        assert!(anchor_by_text(&mut doc, "level of assurance", id));
        let p = &doc.content[0];
        assert_eq!(p.content.len(), 3);
        assert_eq!(p.content[1].text.as_deref(), Some("level of assurance"));
        assert_eq!(p.content[1].marks[0].kind, "comment");
    }

    #[test]
    fn rewrites_links() {
        let mut pages = HashMap::new();
        let pid = Uuid::now_v7();
        pages.insert("12".to_string(), pid);
        let l = Links {
            pages,
            attachments: HashMap::new(),
            users: HashMap::new(),
            inline_refs: HashMap::new(),
            space_keys: HashMap::from([("OLD".to_string(), "NEW".to_string())]),
        };
        let mut st = RewriteStats::default();
        assert_eq!(rewrite_href("wcimport:page/12#intro", &l, &mut st).unwrap(), format!("/pages/{pid}#intro"));
        assert_eq!(rewrite_href("wcimport:missing/OLD/A+Page", &l, &mut st).unwrap(), "/missing?space=NEW&title=A%20Page");
        assert_eq!(rewrite_href("wcimport:missing-attachment/x.png", &l, &mut st), None);
        assert_eq!(rewrite_href("https://example.com", &l, &mut st).unwrap(), "https://example.com");
    }
}
