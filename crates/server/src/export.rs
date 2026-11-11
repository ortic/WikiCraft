//! Space export and restore (background jobs).
//!
//! Formats:
//! - `html`: a static, browsable website (index with the page tree, one file per page,
//!   attachments next to them) — for archiving or offline reading;
//! - `markdown`: a folder tree of Markdown files (pages with children become folders with an
//!   `index.md`), which the Markdown importer reads back;
//! - `backup`: everything needed to rebuild the space — page tree, content, labels, comments,
//!   attachments and restrictions (users by e-mail, groups by name) — restorable with
//!   `POST /api/imports/backup`.
//!
//! HTML and Markdown exports contain the pages the requesting user can see; backups need space
//! admin rights. Archives are written to the blob store and expire after a day.

use std::collections::{HashMap, HashSet};
use std::io::{Read, Write};

use bytes::Bytes;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use uuid::Uuid;
use wikicraft_doc_model::node::{Node, nodes};
use wikicraft_doc_model::{html, markdown, text};
use zip::write::SimpleFileOptions;

use crate::app::{AppState, new_id};
use crate::auth::{USER_COLUMNS, User};
use crate::authz::{Principal, Role, space_role, visible_page_ids};
use crate::routes::pages::{NewPage, insert_page};
use crate::routes::properties::Property;

pub const BACKUP_FORMAT: &str = "wikicraft-space-backup";
/// How long finished export archives are kept.
pub const EXPORT_TTL_HOURS: i64 = 24;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum Format {
    Html,
    Markdown,
    Backup,
}

#[derive(sqlx::FromRow)]
struct PageRow {
    id: Uuid,
    parent_id: Option<Uuid>,
    kind: String,
    title: String,
    icon: Option<String>,
    position: f64,
    content: Value,
    page_status: Option<String>,
    full_width: bool,
    created_at: chrono::DateTime<chrono::Utc>,
    updated_at: chrono::DateTime<chrono::Utc>,
    created_by: Option<String>,
    updated_by: Option<String>,
    /// display name for HTML/Markdown (e-mails stay in backups only)
    updated_by_name: Option<String>,
}

#[derive(sqlx::FromRow)]
struct AttachmentRow {
    id: Uuid,
    page_id: Uuid,
    filename: String,
    media_type: String,
    size: i64,
    blob_key: Option<String>,
    created_at: chrono::DateTime<chrono::Utc>,
    created_by: Option<String>,
    labels: Vec<String>,
}

struct Page {
    row: PageRow,
    doc: Node,
    labels: Vec<String>,
    props: Vec<Property>,
    /// path inside the archive without extension, e.g. `Engineering/Runbooks/index`
    path: String,
}

// ---- backup manifest ---------------------------------------------------------------------

#[derive(Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Manifest {
    pub format: String,
    pub version: u32,
    pub exported_at: chrono::DateTime<chrono::Utc>,
    pub space: BackupSpace,
    pub pages: Vec<BackupPage>,
    #[serde(default)]
    pub attachments: Vec<BackupAttachment>,
    #[serde(default)]
    pub comments: Vec<BackupComment>,
    #[serde(default)]
    pub restrictions: Vec<BackupRestriction>,
}

#[derive(Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct BackupSpace {
    pub key: String,
    pub name: String,
    pub description: Option<String>,
    pub icon: Option<String>,
    pub default_role: String,
    pub home_page_id: Option<Uuid>,
    /// sidebar links `[{title, url}]`
    #[serde(default)]
    pub shortcuts: Value,
    /// the space's look `{accentColor?, hideBlog?}`
    #[serde(default)]
    pub look: Value,
}

#[derive(Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct BackupPage {
    pub id: Uuid,
    pub parent_id: Option<Uuid>,
    pub kind: String,
    pub title: String,
    pub icon: Option<String>,
    pub position: f64,
    pub content: Value,
    pub page_status: Option<String>,
    #[serde(default)]
    pub full_width: bool,
    #[serde(default)]
    pub labels: Vec<String>,
    #[serde(default)]
    pub properties: Vec<BackupProperty>,
    pub created_at: chrono::DateTime<chrono::Utc>,
    pub created_by: Option<String>,
    pub updated_at: chrono::DateTime<chrono::Utc>,
    pub updated_by: Option<String>,
}

/// A page property; `text` is the readable value (a person's name, a page's title), used
/// when the person or page doesn't exist where the backup is restored.
#[derive(Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct BackupProperty {
    pub key: String,
    #[serde(rename = "type")]
    pub kind: String,
    pub value: Option<Value>,
    #[serde(default)]
    pub text: String,
}

#[derive(Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct BackupAttachment {
    pub id: Uuid,
    pub page_id: Uuid,
    pub filename: String,
    pub media_type: String,
    pub size: i64,
    pub created_at: chrono::DateTime<chrono::Utc>,
    pub created_by: Option<String>,
    /// path of the file inside the archive
    pub file: String,
    #[serde(default)]
    pub labels: Vec<String>,
}

#[derive(Serialize, Deserialize, sqlx::FromRow)]
#[serde(rename_all = "camelCase")]
pub struct BackupComment {
    pub id: Uuid,
    pub page_id: Uuid,
    pub parent_id: Option<Uuid>,
    pub author: Option<String>,
    pub content: Value,
    pub inline: Option<Value>,
    pub resolved_at: Option<chrono::DateTime<chrono::Utc>>,
    pub created_at: chrono::DateTime<chrono::Utc>,
}

#[derive(Serialize, Deserialize, sqlx::FromRow)]
#[serde(rename_all = "camelCase")]
pub struct BackupRestriction {
    pub page_id: Uuid,
    pub kind: String,
    pub subject_type: String,
    /// user e-mail or group name
    pub subject: Option<String>,
}

// ---- export ------------------------------------------------------------------------------

async fn load_user(state: &AppState, id: Uuid) -> anyhow::Result<User> {
    Ok(sqlx::query_as::<_, User>(sqlx::AssertSqlSafe(format!("SELECT {USER_COLUMNS} FROM users WHERE id = $1"))).bind(id).fetch_one(&state.db).await?)
}

/// Runs an export job; returns the job result (archive key, file name, counts).
pub async fn run_export(state: &AppState, job_id: Uuid, actor: Uuid, input: &Value) -> anyhow::Result<Value> {
    let space_id: Uuid = serde_json::from_value(input.get("spaceId").cloned().unwrap_or_default())?;
    let format: Format = serde_json::from_value(input.get("format").cloned().unwrap_or(json!("html")))?;
    let user = load_user(state, actor).await?;
    let p = Principal::load(state, Some(user)).await?;
    let access = crate::authz::load_space_access(state, space_id).await?;
    let role = space_role(state, &p, &access).await?;
    if role == Role::None || (format == Format::Backup && role < Role::Admin) {
        anyhow::bail!("you don't have permission to export this space");
    }
    let (key, name, description, icon, default_role, home, shortcuts, look): (
        String,
        String,
        Option<String>,
        Option<String>,
        String,
        Option<Uuid>,
        Value,
        Value,
    ) = sqlx::query_as("SELECT key, name, description, icon, default_role, home_page_id, shortcuts, look FROM spaces WHERE id = $1")
        .bind(space_id)
        .fetch_one(&state.db)
        .await?;

    let rows = sqlx::query_as::<_, PageRow>(
        "SELECT p.id, p.parent_id, p.kind, p.title, p.icon, p.position, p.content, p.page_status, p.full_width, p.created_at, p.updated_at,
                cu.email AS created_by, uu.email AS updated_by, COALESCE(uu.name, cu.name) AS updated_by_name
         FROM pages p LEFT JOIN users cu ON cu.id = p.created_by LEFT JOIN users uu ON uu.id = p.updated_by
         WHERE p.space_id = $1 AND p.deleted_at IS NULL ORDER BY p.position, lower(p.title)",
    )
    .bind(space_id)
    .fetch_all(&state.db)
    .await?;
    let pairs: Vec<(Uuid, Option<Uuid>)> = rows.iter().map(|r| (r.id, r.parent_id)).collect();
    let visible = visible_page_ids(state, &p, space_id, role, &pairs).await?;
    let rows: Vec<PageRow> = rows.into_iter().filter(|r| visible.contains(&r.id)).collect();
    let ids: Vec<Uuid> = rows.iter().map(|r| r.id).collect();

    let mut labels: HashMap<Uuid, Vec<String>> = HashMap::new();
    for (page, label) in
        sqlx::query_as::<_, (Uuid, String)>("SELECT page_id, name FROM labels WHERE page_id = ANY($1) ORDER BY name").bind(&ids).fetch_all(&state.db).await?
    {
        labels.entry(page).or_default().push(label);
    }
    let mut props = crate::routes::properties::load_many(state, &p, &ids).await?;
    let attachments = sqlx::query_as::<_, AttachmentRow>(
        "SELECT a.id, a.page_id, a.filename, a.media_type, a.size, a.blob_key, a.created_at, u.email AS created_by,
                COALESCE((SELECT array_agg(l.name ORDER BY l.name) FROM attachment_labels l WHERE l.attachment_id = a.id), '{}') AS labels
         FROM attachments a LEFT JOIN users u ON u.id = a.created_by WHERE a.page_id = ANY($1) AND a.deleted_at IS NULL ORDER BY a.created_at",
    )
    .bind(&ids)
    .fetch_all(&state.db)
    .await?;

    // live documents are newer than the stored content
    let mut pages: Vec<Page> = Vec::with_capacity(rows.len());
    for row in rows {
        let doc = match state.rooms.get(row.id) {
            Some(room) => room.snapshot(),
            None => Node::from_json(&row.content).unwrap_or_else(|_| Node::empty_doc()),
        };
        let labels = labels.remove(&row.id).unwrap_or_default();
        let props = props.remove(&row.id).unwrap_or_default();
        pages.push(Page { row, doc, labels, props, path: String::new() });
    }
    assign_paths(&mut pages);

    let base = state.config.base_url.trim_end_matches('/').to_string();
    let stamp = chrono::Utc::now();
    let filename = format!("{key}-{}-{}.zip", format_name(format), stamp.format("%Y%m%d-%H%M"));
    let tmp = state.config.upload_dir().join(format!("{job_id}.export"));
    std::fs::create_dir_all(state.config.upload_dir())?;
    let mut zip = zip::ZipWriter::new(std::fs::File::create(&tmp)?);
    let deflate = SimpleFileOptions::default().compression_method(zip::CompressionMethod::Deflated).large_file(true);
    let stored = SimpleFileOptions::default().compression_method(zip::CompressionMethod::Stored).large_file(true);
    let add = |zip: &mut zip::ZipWriter<std::fs::File>, name: &str, data: &[u8], compress: bool| -> anyhow::Result<()> {
        tokio::task::block_in_place(|| -> anyhow::Result<()> {
            zip.start_file(name, if compress { deflate } else { stored })?;
            zip.write_all(data)?;
            Ok(())
        })
    };

    let page_files: HashMap<Uuid, String> = pages.iter().map(|pg| (pg.row.id, pg.path.clone())).collect();
    let att_files: HashMap<Uuid, String> =
        attachments.iter().filter(|a| a.blob_key.is_some()).map(|a| (a.id, format!("_attachments/{}/{}", a.id, safe_segment(&a.filename)))).collect();
    let result: anyhow::Result<()> = async {
        // attachments (the same file layout in every format)
        for (i, a) in attachments.iter().enumerate() {
            let Some(blob) = &a.blob_key else { continue };
            let data = state.blobs.get(blob).await?;
            let compress = !is_compressed(&a.media_type);
            add(&mut zip, &att_files[&a.id], &data, compress)?;
            if i % 50 == 0 {
                progress(state, job_id, json!({"phase": "attachments", "done": i, "total": attachments.len()})).await?;
            }
        }
        match format {
            Format::Html | Format::Markdown => {
                let ext = if format == Format::Html { "html" } else { "md" };
                let by_id: HashMap<Uuid, &Page> = pages.iter().map(|pg| (pg.row.id, pg)).collect();
                for (i, pg) in pages.iter().enumerate() {
                    let file = format!("{}.{ext}", pg.path);
                    let mut doc = pg.doc.clone();
                    relink(&mut doc, &file, ext, &page_files, &att_files, &base);
                    let body = if format == Format::Html {
                        html_page(pg, &doc, &file, &by_id, ext, &name)
                    } else {
                        markdown_page(pg, &doc)
                    };
                    add(&mut zip, &file, body.as_bytes(), true)?;
                    if i % 100 == 0 {
                        progress(state, job_id, json!({"phase": "pages", "done": i, "total": pages.len()})).await?;
                    }
                }
                if format == Format::Html {
                    add(&mut zip, "index.html", html_index(&name, description.as_deref(), &pages, home).as_bytes(), true)?;
                    add(&mut zip, "style.css", STYLE.as_bytes(), true)?;
                }
            }
            Format::Backup => {
                let comments = sqlx::query_as::<_, BackupComment>(
                    "SELECT c.id, c.page_id, c.parent_id, u.email AS author, c.content, c.inline, c.resolved_at, c.created_at
                     FROM comments c LEFT JOIN users u ON u.id = c.author_id WHERE c.page_id = ANY($1) AND c.deleted_at IS NULL ORDER BY c.created_at",
                )
                .bind(&ids)
                .fetch_all(&state.db)
                .await?;
                let restrictions = sqlx::query_as::<_, BackupRestriction>(
                    "SELECT r.page_id, r.kind, r.subject_type,
                            CASE r.subject_type WHEN 'user' THEN (SELECT email FROM users WHERE id = r.subject_id) ELSE (SELECT name FROM groups WHERE id = r.subject_id) END AS subject
                     FROM page_restrictions r WHERE r.page_id = ANY($1)",
                )
                .bind(&ids)
                .fetch_all(&state.db)
                .await?;
                let manifest = Manifest {
                    format: BACKUP_FORMAT.into(),
                    version: 1,
                    exported_at: stamp,
                    space: BackupSpace { key: key.clone(), name: name.clone(), description: description.clone(), icon: icon.clone(), default_role: default_role.clone(), home_page_id: home, shortcuts: shortcuts.clone(), look: look.clone() },
                    pages: pages
                        .iter()
                        .map(|pg| BackupPage {
                            id: pg.row.id,
                            parent_id: pg.row.parent_id,
                            kind: pg.row.kind.clone(),
                            title: pg.row.title.clone(),
                            icon: pg.row.icon.clone(),
                            position: pg.row.position,
                            content: pg.doc.to_json(),
                            page_status: pg.row.page_status.clone(),
                            full_width: pg.row.full_width,
                            labels: pg.labels.clone(),
                            properties: pg
                                .props
                                .iter()
                                .map(|pr| BackupProperty { key: pr.key.clone(), kind: pr.kind.clone(), value: pr.value.clone(), text: pr.display() })
                                .collect(),
                            created_at: pg.row.created_at,
                            created_by: pg.row.created_by.clone(),
                            updated_at: pg.row.updated_at,
                            updated_by: pg.row.updated_by.clone(),
                        })
                        .collect(),
                    attachments: attachments
                        .iter()
                        .filter(|a| a.blob_key.is_some())
                        .map(|a| BackupAttachment {
                            id: a.id,
                            page_id: a.page_id,
                            filename: a.filename.clone(),
                            media_type: a.media_type.clone(),
                            size: a.size,
                            created_at: a.created_at,
                            created_by: a.created_by.clone(),
                            file: att_files[&a.id].clone(),
                            labels: a.labels.clone(),
                        })
                        .collect(),
                    comments,
                    restrictions,
                };
                add(&mut zip, "manifest.json", &serde_json::to_vec_pretty(&manifest)?, true)?;
            }
        }
        tokio::task::block_in_place(|| zip.finish())?;
        Ok(())
    }
    .await;
    if let Err(e) = result {
        let _ = std::fs::remove_file(&tmp);
        return Err(e);
    }
    let blob_key = format!("exports/{job_id}.zip");
    let size = state.blobs.put_file(&blob_key, &tmp).await;
    let _ = std::fs::remove_file(&tmp);
    let size = size?;
    Ok(json!({
        "file": blob_key,
        "filename": filename,
        "size": size,
        "format": format,
        "spaceKey": key,
        "pages": pages.len(),
        "attachments": attachments.len(),
        "expiresAt": stamp + chrono::Duration::hours(EXPORT_TTL_HOURS),
    }))
}

fn format_name(f: Format) -> &'static str {
    match f {
        Format::Html => "html",
        Format::Markdown => "markdown",
        Format::Backup => "backup",
    }
}

/// Records progress; fails once someone asked to cancel the job.
async fn progress(state: &AppState, job: Uuid, p: Value) -> anyhow::Result<()> {
    crate::jobs::checkpoint(state, job, p).await
}

fn is_compressed(mime: &str) -> bool {
    mime.starts_with("image/") && mime != "image/svg+xml"
        || mime.starts_with("video/")
        || mime.starts_with("audio/")
        || mime.contains("zip")
        || mime.contains("compressed")
        || mime == "application/pdf"
}

/// A file/folder name that is safe on every OS (and inside zip archives).
pub fn safe_segment(s: &str) -> String {
    let cleaned: String =
        s.chars().map(|c| if c.is_control() || matches!(c, '/' | '\\' | ':' | '*' | '?' | '"' | '<' | '>' | '|' | '#' | '%') { '-' } else { c }).collect();
    let trimmed = cleaned.trim().trim_matches('.').trim();
    let mut out: String = trimmed.chars().take(100).collect();
    if out.is_empty() {
        out = "Untitled".into();
    }
    // names starting with a dot are hidden files; `_attachments` is reserved
    if out.starts_with('.') || out.eq_ignore_ascii_case("_attachments") {
        out.insert(0, '_');
    }
    out
}

/// Gives every page a unique archive path mirroring the tree: pages with children become a
/// folder with an `index` file. Blog posts go to `Blog/`.
fn assign_paths(pages: &mut [Page]) {
    let ids: HashSet<Uuid> = pages.iter().map(|p| p.row.id).collect();
    let mut children: HashMap<Option<Uuid>, Vec<usize>> = HashMap::new();
    for (i, p) in pages.iter().enumerate() {
        // parents that aren't exported (restricted, trashed) make the page a root
        let parent = p.row.parent_id.filter(|id| ids.contains(id));
        let parent = if p.row.kind == "blog" && parent.is_none() { Some(Uuid::nil()) } else { parent };
        children.entry(parent).or_default().push(i);
    }
    let mut paths: Vec<String> = vec![String::new(); pages.len()];
    // breadth-first from the roots; blog posts under a virtual `Blog` folder
    let mut queue: Vec<(Option<Uuid>, String)> = vec![(None, String::new())];
    if children.contains_key(&Some(Uuid::nil())) {
        queue.push((Some(Uuid::nil()), "Blog/".into()));
    }
    let mut guard = 0;
    while let Some((parent, prefix)) = queue.pop() {
        guard += 1;
        if guard > pages.len() + 2 {
            break;
        }
        let mut used: HashSet<String> = HashSet::new();
        if parent.is_none() {
            used.insert("index".into());
            used.insert("blog".into());
        }
        for &i in children.get(&parent).map(|v| v.as_slice()).unwrap_or(&[]) {
            let title = if pages[i].row.kind == "blog" {
                format!("{} {}", pages[i].row.created_at.format("%Y-%m-%d"), pages[i].row.title)
            } else {
                pages[i].row.title.clone()
            };
            let base = safe_segment(&title);
            let mut name = base.clone();
            let mut n = 2;
            while !used.insert(name.to_lowercase()) {
                name = format!("{base} ({n})");
                n += 1;
            }
            let id = pages[i].row.id;
            if children.contains_key(&Some(id)) {
                paths[i] = format!("{prefix}{name}/index");
                queue.push((Some(id), format!("{prefix}{name}/")));
            } else {
                paths[i] = format!("{prefix}{name}");
            }
        }
    }
    for (p, path) in pages.iter_mut().zip(paths) {
        p.path = if path.is_empty() { format!("_orphans/{}", p.row.id) } else { path };
    }
}

/// Relative URL from the archive file `from` to the archive file `to`.
fn relative(from: &str, to: &str) -> String {
    let from_dir: Vec<&str> = from.split('/').collect::<Vec<_>>().split_last().map(|(_, d)| d.to_vec()).unwrap_or_default();
    let to_parts: Vec<&str> = to.split('/').collect();
    let common = from_dir.iter().zip(&to_parts).take_while(|(a, b)| a == b).count();
    let mut out: Vec<String> = vec!["..".to_string(); from_dir.len() - common];
    out.extend(to_parts[common..].iter().map(|s| encode_segment(s)));
    out.join("/")
}

fn encode_segment(s: &str) -> String {
    let mut o = String::with_capacity(s.len());
    for b in s.bytes() {
        match b {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => o.push(b as char),
            _ => o.push_str(&format!("%{b:02X}")),
        }
    }
    o
}

/// Node attributes that may hold a link to a page or attachment.
const URL_ATTRS: [&str; 4] = ["href", "src", "url", "preview"];

fn parse_page_href(href: &str) -> Option<(Uuid, Option<&str>)> {
    let rest = href.strip_prefix("/pages/")?;
    let (id, anchor) = match rest.split_once('#') {
        Some((a, b)) => (a, Some(b)),
        None => (rest, None),
    };
    Some((id.split(['?', '/']).next()?.parse().ok()?, anchor))
}

fn parse_attachment_url(url: &str) -> Option<Uuid> {
    url.strip_prefix("/api/attachments/")?.split('/').next()?.parse().ok()
}

/// Rewrites links between pages and to attachments into relative archive paths; links to
/// pages that aren't part of the export point to the wiki.
fn relink(doc: &mut Node, file: &str, ext: &str, pages: &HashMap<Uuid, String>, atts: &HashMap<Uuid, String>, base: &str) {
    let fix_att = |url: &str| -> Option<String> {
        let id = parse_attachment_url(url)?;
        Some(match atts.get(&id) {
            Some(f) => relative(file, f),
            None => format!("{base}{url}"),
        })
    };
    let fix = |url: &str| -> Option<String> {
        if let Some((id, anchor)) = parse_page_href(url) {
            let hash = anchor.map(|a| format!("#{a}")).unwrap_or_default();
            Some(match pages.get(&id) {
                Some(target) => format!("{}{hash}", relative(file, &format!("{target}.{ext}"))),
                None => format!("{base}{url}"),
            })
        } else if url.starts_with("/api/attachments/") {
            fix_att(url)
        } else if url.starts_with('/') && !url.starts_with("//") {
            Some(format!("{base}{url}"))
        } else {
            None
        }
    };
    doc.walk_mut(&mut |n| {
        // images, attachments, includes, embeds, … and link marks
        for key in URL_ATTRS {
            if let Some(u) = n.attr_str(key).and_then(fix) {
                n.attrs.insert(key.into(), Value::String(u));
            }
        }
        for m in &mut n.marks {
            if m.kind == "link"
                && let Some(u) = m.attrs.get("href").and_then(Value::as_str).and_then(fix)
            {
                m.attrs.insert("href".into(), Value::String(u));
            }
        }
    });
}

/// Front matter (JSON-style scalars are valid YAML) + `# Title` + body.
fn markdown_page(pg: &Page, doc: &Node) -> String {
    let mut out = format!("---\ntitle: {}\n", serde_json::to_string(&pg.row.title).unwrap_or_default());
    if !pg.labels.is_empty() {
        out.push_str(&format!("labels: {}\n", serde_json::to_string(&pg.labels).unwrap_or_default()));
    }
    if !pg.props.is_empty() {
        // a flow mapping in page order, readable and re-importable (types are inferred on import)
        let fields: Vec<String> = pg
            .props
            .iter()
            .map(|pr| {
                let v = match (pr.kind.as_str(), &pr.value) {
                    ("number", Some(n)) => n.clone(),
                    _ => Value::String(pr.display()),
                };
                format!("{}: {v}", Value::String(pr.key.clone()))
            })
            .collect();
        out.push_str(&format!("properties: {{{}}}\n", fields.join(", ")));
    }
    out.push_str(&format!("created: {}\nupdated: {}\n---\n\n", pg.row.created_at.to_rfc3339(), pg.row.updated_at.to_rfc3339()));
    out.push_str(&format!("# {}\n\n", pg.row.title.replace('\n', " ")));
    out.push_str(&markdown::pm_to_markdown(doc));
    out
}

fn html_page(pg: &Page, doc: &Node, file: &str, by_id: &HashMap<Uuid, &Page>, ext: &str, space_name: &str) -> String {
    let e = html::escape;
    let root = relative(file, "index.html");
    let css = relative(file, "style.css");
    // breadcrumb: ancestors inside the export
    let mut crumbs = Vec::new();
    let mut cur = pg.row.parent_id;
    let mut guard = 0;
    while let Some(id) = cur {
        let Some(a) = by_id.get(&id) else { break };
        crumbs.push(format!("<a href=\"{}\">{}</a>", e(&relative(file, &format!("{}.{ext}", a.path))), e(&a.row.title)));
        cur = a.row.parent_id;
        guard += 1;
        if guard > 100 {
            break;
        }
    }
    crumbs.push(format!("<a href=\"{}\">{}</a>", e(&root), e(space_name)));
    crumbs.reverse();
    let mut kids: Vec<&&Page> = by_id.values().filter(|c| c.row.parent_id == Some(pg.row.id)).collect();
    kids.sort_by(|a, b| a.row.position.total_cmp(&b.row.position).then_with(|| a.row.title.to_lowercase().cmp(&b.row.title.to_lowercase())));
    let children = if kids.is_empty() {
        String::new()
    } else {
        let items: String =
            kids.iter().map(|c| format!("<li><a href=\"{}\">{}</a></li>", e(&relative(file, &format!("{}.{ext}", c.path))), e(&c.row.title))).collect();
        format!("<nav class=\"children\"><h2>Child pages</h2><ul>{items}</ul></nav>")
    };
    let labels = if pg.labels.is_empty() {
        String::new()
    } else {
        format!("<p class=\"labels\">{}</p>", pg.labels.iter().map(|l| format!("<span>{}</span>", e(l))).collect::<String>())
    };
    let props = if pg.props.is_empty() {
        String::new()
    } else {
        let rows: String = pg
            .props
            .iter()
            .map(|pr| {
                let text = e(&pr.display());
                let cell = match (pr.kind.as_str(), pr.value.as_ref().and_then(Value::as_str)) {
                    ("url", Some(u)) => format!("<a href=\"{}\">{text}</a>", e(u)),
                    _ => text,
                };
                format!("<tr><th>{}</th><td>{cell}</td></tr>", e(&pr.key))
            })
            .collect();
        format!("<table class=\"properties\">{rows}</table>")
    };
    let meta = format!(
        "Last updated {}{}",
        pg.row.updated_at.format("%Y-%m-%d"),
        pg.row.updated_by_name.as_deref().map(|u| format!(" by {}", e(u))).unwrap_or_default()
    );
    format!(
        "<!doctype html><html lang=\"en\"><head><meta charset=\"utf-8\"><meta name=\"viewport\" content=\"width=device-width,initial-scale=1\"><title>{title} · {space}</title><link rel=\"stylesheet\" href=\"{css}\"></head><body><header><nav class=\"crumbs\">{crumbs}</nav></header><main class=\"{width}\"><h1>{title}</h1><p class=\"meta\">{meta}</p>{labels}{props}<article>{body}</article>{children}</main></body></html>",
        title = e(&pg.row.title),
        space = e(space_name),
        css = e(&css),
        crumbs = crumbs.join(" <span>/</span> "),
        width = if pg.row.full_width { "wide" } else { "" },
        body = html::render(doc, &html::HtmlOptions { heading_ids: true }),
    )
}

/// Page properties from a backup: people and pages that don't exist here become text.
async fn restore_properties(state: &AppState, ws: Uuid, page: Uuid, props: &[BackupProperty], page_ids: &HashMap<Uuid, Uuid>) -> anyhow::Result<()> {
    let id_of = |v: &Option<Value>| v.as_ref().and_then(Value::as_str).and_then(|s| Uuid::parse_str(s).ok());
    let mut seen = HashSet::new();
    for (i, pr) in props.iter().take(50).enumerate() {
        let key = crate::routes::properties::clean_key(&pr.key);
        if key.is_empty() || key.chars().count() > 100 || !seen.insert(key.to_lowercase()) {
            continue;
        }
        let as_text = |t: &str| ("text".to_string(), (!t.is_empty()).then(|| Value::String(t.to_string())), t.to_string());
        let (kind, value, text) = match pr.kind.as_str() {
            "user" => match id_of(&pr.value) {
                Some(id) => {
                    let name: Option<String> =
                        sqlx::query_scalar("SELECT name FROM users WHERE id = $1 AND workspace_id = $2").bind(id).bind(ws).fetch_optional(&state.db).await?;
                    match name {
                        Some(n) => ("user".into(), pr.value.clone(), n),
                        None => as_text(&pr.text),
                    }
                }
                None => as_text(&pr.text),
            },
            "page" => match id_of(&pr.value).and_then(|id| page_ids.get(&id)) {
                Some(new) => ("page".into(), Some(Value::String(new.to_string())), String::new()),
                None => as_text(&pr.text),
            },
            k if crate::routes::properties::KINDS.contains(&k) => {
                let text = match &pr.value {
                    Some(Value::String(s)) => s.clone(),
                    Some(v @ Value::Number(_)) => v.to_string(),
                    _ => String::new(),
                };
                (k.to_string(), pr.value.clone().filter(|v| !v.is_null()), text)
            }
            _ => as_text(&pr.text),
        };
        sqlx::query("INSERT INTO page_properties (page_id, position, key, kind, value, text_value) VALUES ($1, $2, $3, $4, $5, $6) ON CONFLICT DO NOTHING")
            .bind(page)
            .bind(i as i32)
            .bind(&key)
            .bind(kind)
            .bind(value)
            .bind(text)
            .execute(&state.db)
            .await?;
    }
    Ok(())
}

fn html_index(space_name: &str, description: Option<&str>, pages: &[Page], home: Option<Uuid>) -> String {
    let e = html::escape;
    let ids: HashSet<Uuid> = pages.iter().map(|p| p.row.id).collect();
    let mut children: HashMap<Option<Uuid>, Vec<&Page>> = HashMap::new();
    for p in pages.iter().filter(|p| p.row.kind != "blog") {
        children.entry(p.row.parent_id.filter(|id| ids.contains(id))).or_default().push(p);
    }
    for v in children.values_mut() {
        v.sort_by(|a, b| a.row.position.total_cmp(&b.row.position).then_with(|| a.row.title.to_lowercase().cmp(&b.row.title.to_lowercase())));
    }
    fn tree(parent: Option<Uuid>, children: &HashMap<Option<Uuid>, Vec<&Page>>, depth: usize) -> String {
        let Some(list) = children.get(&parent) else { return String::new() };
        if depth > 60 {
            return String::new();
        }
        let items: String = list
            .iter()
            .map(|p| {
                format!(
                    "<li><a href=\"{}\">{}</a>{}</li>",
                    html::escape(&relative("index.html", &format!("{}.html", p.path))),
                    html::escape(&p.row.title),
                    tree(Some(p.row.id), children, depth + 1)
                )
            })
            .collect();
        format!("<ul>{items}</ul>")
    }
    let mut blog: Vec<&Page> = pages.iter().filter(|p| p.row.kind == "blog").collect();
    blog.sort_by_key(|p| std::cmp::Reverse(p.row.created_at));
    let blog_html = if blog.is_empty() {
        String::new()
    } else {
        let items: String = blog
            .iter()
            .map(|p| {
                format!(
                    "<li><a href=\"{}\">{}</a> <span class=\"meta\">{}</span></li>",
                    e(&relative("index.html", &format!("{}.html", p.path))),
                    e(&p.row.title),
                    p.row.created_at.format("%Y-%m-%d")
                )
            })
            .collect();
        format!("<h2>Blog</h2><ul>{items}</ul>")
    };
    let home_link = home
        .and_then(|h| pages.iter().find(|p| p.row.id == h))
        .map(|p| format!("<p><a class=\"home\" href=\"{}\">Open the space home page →</a></p>", e(&relative("index.html", &format!("{}.html", p.path)))))
        .unwrap_or_default();
    format!(
        "<!doctype html><html lang=\"en\"><head><meta charset=\"utf-8\"><meta name=\"viewport\" content=\"width=device-width,initial-scale=1\"><title>{name}</title><link rel=\"stylesheet\" href=\"style.css\"></head><body><main><h1>{name}</h1>{desc}{home_link}<h2>Pages</h2><nav class=\"tree\">{tree}</nav>{blog_html}<p class=\"meta\">Exported from WikiCraft on {date} · {count} pages</p></main></body></html>",
        name = e(space_name),
        desc = description.filter(|d| !d.trim().is_empty()).map(|d| format!("<p class=\"lead\">{}</p>", e(d))).unwrap_or_default(),
        tree = tree(None, &children, 0),
        date = chrono::Utc::now().format("%Y-%m-%d"),
        count = pages.len(),
    )
}

const STYLE: &str = "body{margin:0;font:16px/1.65 system-ui,-apple-system,'Segoe UI',sans-serif;color:#1f2328;background:#fff}
header{border-bottom:1px solid #e5e7eb;padding:10px 24px;font-size:14px}header a{color:#57606a;text-decoration:none}header a:hover{color:#2563eb}header span{color:#9ca3af;margin:0 4px}
main{max-width:860px;margin:0 auto;padding:24px}main.wide{max-width:none}h1{font-size:2rem;line-height:1.25;margin:.4em 0 .2em}
.meta{color:#6b7280;font-size:13px;margin:0 0 1rem}.lead{font-size:1.1rem;color:#4b5563}table[data-numbered]{counter-reset:row}table[data-numbered] tr:has(>td){counter-increment:row}table[data-numbered] tr:has(>td)>:first-child::before{content:counter(row) '. ';color:#6b7280}.properties{border-collapse:collapse;margin:0 0 1rem;font-size:14px}.properties th{text-align:left;color:#6b7280;font-weight:500;padding:3px 16px 3px 0;vertical-align:top}.properties td{padding:3px 0}.labels span{display:inline-block;background:#eef2ff;color:#3730a3;border-radius:999px;padding:1px 10px;font-size:12px;margin-right:6px}
a{color:#2563eb}pre{background:#f6f8fa;padding:12px 16px;border-radius:8px;overflow:auto}code{font:13px ui-monospace,SFMono-Regular,Menlo,monospace;background:#f6f8fa;padding:.1em .3em;border-radius:4px}pre code{padding:0;background:none}
table{border-collapse:collapse;margin:1rem 0}td,th{border:1px solid #d0d7de;padding:6px 10px;vertical-align:top}th{background:#f6f8fa}
img{max-width:100%;height:auto}.image-block{display:block;margin:1rem 0}.image-block.align-none{display:inline-block;margin:0}.image-block.align-center{text-align:center}.image-block.align-right{text-align:right}.image-block.align-wrap-left{float:left;margin:.3em 1.2em .6em 0;max-width:50%}.image-block.align-wrap-right{float:right;margin:.3em 0 .6em 1.2em;max-width:50%}h1,h2,h3,table,pre{clear:both}.image-caption{display:block;color:#6b7280;font-size:.9em;margin-top:4px}blockquote{margin:1rem 0;padding:0 1rem;border-left:4px solid #d0d7de;color:#57606a}
.callout{display:flex;gap:10px;border-left:4px solid #3b82f6;background:#eff6ff;padding:.6rem 1rem;margin:1rem 0;border-radius:6px}.callout-icon{flex:none;margin-top:.2rem;color:#2563eb}.callout-body{min-width:0;flex:1}.callout-body>:first-child{margin-top:0}.callout-body>:last-child{margin-bottom:0}
.callout-note{border-color:#8b5cf6;background:#f5f3ff}.callout-note .callout-icon{color:#7c3aed}.callout-warning{border-color:#f59e0b;background:#fffbeb}.callout-warning .callout-icon{color:#d97706}.callout-error{border-color:#ef4444;background:#fef2f2}.callout-error .callout-icon{color:#dc2626}.callout-success{border-color:#22c55e;background:#f0fdf4}.callout-success .callout-icon{color:#16a34a}
.attachment-icon{vertical-align:-3px;margin-right:2px}
.tree ul{list-style:none;padding-left:18px;margin:2px 0}.tree>ul{padding-left:0}.tree li{margin:3px 0}.children{margin-top:2rem;border-top:1px solid #e5e7eb}.children h2{font-size:1rem;color:#57606a}
@media (prefers-color-scheme:dark){body{background:#0d1117;color:#e6edf3}header{border-color:#30363d}pre,code,th{background:#161b22}td,th{border-color:#30363d}a{color:#58a6ff}.callout{background:#0c2d6b}}
";

// ---- restore -----------------------------------------------------------------------------

/// Reads the manifest of a backup archive (sync; call from a blocking context).
fn read_manifest(path: &std::path::Path) -> anyhow::Result<Manifest> {
    let mut zip = zip::ZipArchive::new(std::fs::File::open(path)?)?;
    let f = zip.by_name("manifest.json").map_err(|_| anyhow::anyhow!("not a WikiCraft space backup (manifest.json missing)"))?;
    let mut buf = Vec::new();
    // a manifest holds all page content; cap it to keep memory bounded
    let read = f.take(256 * 1024 * 1024 + 1).read_to_end(&mut buf)?;
    anyhow::ensure!(read <= 256 * 1024 * 1024, "manifest.json is too large");
    let m: Manifest = serde_json::from_slice(&buf)?;
    anyhow::ensure!(m.format == BACKUP_FORMAT, "not a WikiCraft space backup");
    anyhow::ensure!(m.version == 1, "unsupported backup version {}", m.version);
    Ok(m)
}

/// Quick validation before queueing a restore: (space key, space name, page count).
pub fn inspect_backup(path: &std::path::Path) -> anyhow::Result<(String, String, usize)> {
    let m = read_manifest(path)?;
    Ok((m.space.key, m.space.name, m.pages.len()))
}

fn read_entry(path: &std::path::Path, name: &str, max: u64) -> anyhow::Result<Bytes> {
    let mut zip = zip::ZipArchive::new(std::fs::File::open(path)?)?;
    let f = zip.by_name(name)?;
    let mut buf = Vec::new();
    let read = f.take(max + 1).read_to_end(&mut buf)? as u64;
    anyhow::ensure!(read <= max, "{name} is too large");
    Ok(Bytes::from(buf))
}

#[derive(Deserialize, Default)]
#[serde(rename_all = "camelCase")]
pub struct RestoreOptions {
    /// key of the new space (default: the key in the backup)
    pub space_key: Option<String>,
    pub space_name: Option<String>,
}

/// Restores a backup archive into a new space. Returns the job result.
pub async fn run_restore(state: &AppState, job_id: Uuid, actor: Uuid, archive: &std::path::Path, opts: RestoreOptions) -> anyhow::Result<Value> {
    let user = load_user(state, actor).await?;
    anyhow::ensure!(user.is_admin(), "only workspace admins can restore backups");
    let p = archive.to_path_buf();
    let m = tokio::task::spawn_blocking(move || read_manifest(&p)).await??;
    let key = crate::routes::spaces::validate_key(opts.space_key.as_deref().unwrap_or(&m.space.key)).map_err(|e| anyhow::anyhow!("{e}"))?;
    let name = opts.space_name.clone().filter(|n| !n.trim().is_empty()).unwrap_or_else(|| m.space.name.clone());
    let default_role =
        if matches!(m.space.default_role.as_str(), "none" | "viewer" | "commenter" | "editor" | "admin") { m.space.default_role.as_str() } else { "editor" };
    let space = crate::routes::spaces::create_space(
        state,
        user.workspace_id,
        Some(user.id),
        &key,
        &name,
        m.space.description.as_deref(),
        m.space.icon.as_deref(),
        // closed while restoring: content lands before its restrictions do
        "none",
        None,
    )
    .await
    .map_err(|e| anyhow::anyhow!("{e}"))?;
    sqlx::query("INSERT INTO space_permissions (space_id, subject_type, subject_id, role) VALUES ($1, 'user', $2, 'admin') ON CONFLICT DO NOTHING")
        .bind(space.id)
        .bind(user.id)
        .execute(&state.db)
        .await?;
    let outcome: anyhow::Result<Value> = async {

    // people and groups are matched by e-mail / name in this workspace
    let users: HashMap<String, Uuid> = sqlx::query_as::<_, (String, Uuid)>("SELECT lower(email), id FROM users WHERE workspace_id = $1 AND email IS NOT NULL")
        .bind(user.workspace_id)
        .fetch_all(&state.db)
        .await?
        .into_iter()
        .collect();
    let groups: HashMap<String, Uuid> = sqlx::query_as::<_, (String, Uuid)>("SELECT lower(name), id FROM groups WHERE workspace_id = $1")
        .bind(user.workspace_id)
        .fetch_all(&state.db)
        .await?
        .into_iter()
        .collect();
    let person = |email: &Option<String>| email.as_ref().and_then(|e| users.get(&e.to_lowercase())).copied();

    let page_ids: HashMap<Uuid, Uuid> = m.pages.iter().map(|pg| (pg.id, new_id())).collect();
    let comment_ids: HashMap<Uuid, Uuid> = m.comments.iter().map(|c| (c.id, new_id())).collect();

    // pages, parents first
    let by_id: HashMap<Uuid, &BackupPage> = m.pages.iter().map(|pg| (pg.id, pg)).collect();
    let depth = |mut id: Uuid| {
        let mut d = 0;
        while let Some(parent) = by_id.get(&id).and_then(|pg| pg.parent_id).filter(|p| by_id.contains_key(p)) {
            d += 1;
            id = parent;
            if d > 1000 {
                break;
            }
        }
        d
    };
    let mut order: Vec<&BackupPage> = m.pages.iter().collect();
    order.sort_by_key(|pg| depth(pg.id));
    for (i, pg) in order.iter().enumerate() {
        let kind = if matches!(pg.kind.as_str(), "page" | "blog" | "folder") { pg.kind.clone() } else { "page".into() };
        insert_page(
            &state.db,
            NewPage {
                id: page_ids[&pg.id],
                workspace_id: user.workspace_id,
                space_id: space.id,
                parent_id: pg.parent_id.and_then(|p| page_ids.get(&p).copied()),
                kind,
                title: pg.title.chars().take(255).collect(),
                content: Node::empty_doc(),
                position: pg.position,
                created_by: person(&pg.created_by).or(Some(user.id)),
                created_at: Some(pg.created_at),
                source_ref: None,
                draft: false,
            },
        )
        .await
        .map_err(|e| anyhow::anyhow!("{e}"))?;
        sqlx::query("UPDATE pages SET icon = $2, page_status = $3, full_width = $4, updated_at = $5, updated_by = $6 WHERE id = $1")
            .bind(page_ids[&pg.id])
            .bind(&pg.icon)
            .bind(&pg.page_status)
            .bind(pg.full_width)
            .bind(pg.updated_at)
            .bind(person(&pg.updated_by))
            .execute(&state.db)
            .await?;
        for l in pg.labels.iter().take(100) {
            sqlx::query("INSERT INTO labels (page_id, name) VALUES ($1, $2) ON CONFLICT DO NOTHING")
                .bind(page_ids[&pg.id])
                .bind(l.trim())
                .execute(&state.db)
                .await?;
        }
        restore_properties(state, user.workspace_id, page_ids[&pg.id], &pg.properties, &page_ids).await?;
        if i % 100 == 0 {
            progress(state, job_id, json!({"phase": "pages", "done": i, "total": m.pages.len()})).await?;
        }
    }
    // sidebar shortcuts, pointing at the restored pages
    if let Value::Array(list) = &m.space.shortcuts {
        let list: Vec<crate::routes::spaces::Shortcut> = list.iter().filter_map(|v| serde_json::from_value(v.clone()).ok()).collect();
        if let Ok(clean) = crate::routes::spaces::clean_shortcuts(list.into_iter().take(20).collect()) {
            let remapped: Value = serde_json::from_str(&crate::routes::pages::remap_uuids(&clean.to_string(), &page_ids)).unwrap_or(json!([]));
            sqlx::query("UPDATE spaces SET shortcuts = $2 WHERE id = $1").bind(space.id).bind(remapped).execute(&state.db).await?;
        }
    }
    if let Ok(look) = crate::routes::spaces::clean_look(&m.space.look) {
        sqlx::query("UPDATE spaces SET look = $2 WHERE id = $1").bind(space.id).bind(look).execute(&state.db).await?;
    }
    if let Some(home) = m.space.home_page_id.and_then(|h| page_ids.get(&h)) {
        sqlx::query("UPDATE spaces SET home_page_id = $2 WHERE id = $1").bind(space.id).bind(home).execute(&state.db).await?;
    }

    // attachments get new ids; content references are rewritten below
    let mut att_ids: HashMap<Uuid, Uuid> = HashMap::new();
    let mut infected = 0;
    for (i, a) in m.attachments.iter().enumerate() {
        let Some(&page) = page_ids.get(&a.page_id) else { continue };
        let p = archive.to_path_buf();
        let file = a.file.clone();
        let max = state.config.max_upload_mb.saturating_mul(1024 * 1024);
        let data = tokio::task::spawn_blocking(move || read_entry(&p, &file, max)).await??;
        if let Some(sig) = crate::antivirus::screen(state, &data).await.map_err(|e| anyhow::anyhow!("{e}"))? {
            crate::audit::log(state, user.workspace_id, Some(user.id), "upload.infected", Some("page"), Some(page), json!({"file": a.filename, "signature": sig})).await;
            infected += 1;
            continue;
        }
        let new =
            crate::routes::attachments::store_attachment(state, user.workspace_id, page, person(&a.created_by), &a.filename, data, Some(a.created_at), None)
                .await
                .map_err(|e| anyhow::anyhow!("{e}"))?;
        att_ids.insert(a.id, new);
        let labels: Vec<String> = a.labels.iter().filter_map(|l| crate::routes::pages::normalize_label(l)).take(20).collect();
        sqlx::query("INSERT INTO attachment_labels (attachment_id, name) SELECT $1, unnest($2::text[]) ON CONFLICT DO NOTHING")
            .bind(new)
            .bind(&labels)
            .execute(&state.db)
            .await?;
        if i % 50 == 0 {
            progress(state, job_id, json!({"phase": "attachments", "done": i, "total": m.attachments.len()})).await?;
        }
    }

    // content with links, images and inline comment anchors pointing at the new ids
    for pg in &m.pages {
        let mut doc = Node::from_json(&pg.content).unwrap_or_else(|_| Node::empty_doc());
        remap_content(&mut doc, &page_ids, &att_ids, &comment_ids);
        wikicraft_doc_model::blocks::ensure_ids(&mut doc);
        let id = page_ids[&pg.id];
        sqlx::query("UPDATE pages SET content = $2, text_content = $3 WHERE id = $1")
            .bind(id)
            .bind(doc.to_json())
            .bind(text::plain_text(&doc))
            .execute(&state.db)
            .await?;
        sqlx::query("UPDATE page_versions SET content = $2, title = $3 WHERE page_id = $1 AND version = 1")
            .bind(id)
            .bind(doc.to_json())
            .bind(&pg.title)
            .execute(&state.db)
            .await?;
        crate::routes::tasks::index_page(&mut *state.db.acquire().await?, id, &doc).await?;
    }

    // comments, parents first (created_at order, then retry the rest)
    let mut pending: Vec<&BackupComment> = m.comments.iter().filter(|c| page_ids.contains_key(&c.page_id)).collect();
    pending.sort_by_key(|c| c.created_at);
    let in_backup: HashSet<Uuid> = m.comments.iter().map(|c| c.id).collect();
    let mut done: HashSet<Uuid> = HashSet::new();
    let mut restored_comments = 0;
    while !pending.is_empty() {
        let before = pending.len();
        let mut rest = Vec::new();
        for c in pending {
            if let Some(parent) = c.parent_id
                && !done.contains(&parent)
                && in_backup.contains(&parent)
            {
                rest.push(c);
                continue;
            }
            let mut content = Node::from_json(&c.content).unwrap_or_else(|_| Node::empty_doc());
            remap_content(&mut content, &page_ids, &att_ids, &comment_ids);
            sqlx::query(
                "INSERT INTO comments (id, page_id, parent_id, author_id, content, text_content, inline, resolved_at, created_at, updated_at) VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, $9)",
            )
            .bind(comment_ids[&c.id])
            .bind(page_ids[&c.page_id])
            .bind(c.parent_id.filter(|p| done.contains(p)).map(|p| comment_ids[&p]))
            .bind(person(&c.author))
            .bind(content.to_json())
            .bind(text::plain_text(&content))
            .bind(&c.inline)
            .bind(c.resolved_at)
            .bind(c.created_at)
            .execute(&state.db)
            .await?;
            done.insert(c.id);
            restored_comments += 1;
        }
        if rest.len() == before {
            break;
        }
        pending = rest;
    }

    // restrictions: unknown people/groups fail closed (only the restoring admin keeps access)
    let mut unmapped = 0;
    for r in &m.restrictions {
        let Some(&page) = page_ids.get(&r.page_id) else { continue };
        if !matches!(r.kind.as_str(), "view" | "edit") {
            continue;
        }
        let subject = match r.subject_type.as_str() {
            "user" => r.subject.as_ref().and_then(|s| users.get(&s.to_lowercase())).map(|id| ("user", *id)),
            "group" => r.subject.as_ref().and_then(|s| groups.get(&s.to_lowercase())).map(|id| ("group", *id)),
            _ => None,
        };
        let (st, sid) = subject.unwrap_or_else(|| {
            unmapped += 1;
            ("user", user.id)
        });
        sqlx::query("INSERT INTO page_restrictions (page_id, kind, subject_type, subject_id) VALUES ($1, $2, $3, $4) ON CONFLICT DO NOTHING")
            .bind(page)
            .bind(&r.kind)
            .bind(st)
            .bind(sid)
            .execute(&state.db)
            .await?;
    }
    for id in page_ids.values() {
        state.search.enqueue(*id);
    }
    Ok(json!({
        "spaceKey": key,
        "pages": m.pages.len(),
        "attachments": att_ids.len(),
        "infectedAttachments": infected,
        "comments": restored_comments,
        "unmappedRestrictions": unmapped,
    }))
    }
    .await;
    match outcome {
        Ok(result) => {
            // open the space only now that every restriction is in place
            sqlx::query("UPDATE spaces SET default_role = $2 WHERE id = $1").bind(space.id).bind(default_role).execute(&state.db).await?;
            crate::audit::log(
                state,
                user.workspace_id,
                Some(user.id),
                "space.restored",
                Some("space"),
                Some(space.id),
                json!({"key": key, "pages": m.pages.len()}),
            )
            .await;
            Ok(result)
        }
        Err(e) => {
            // never leave a half-restored space (with content but missing restrictions) behind
            let _ = sqlx::query("DELETE FROM spaces WHERE id = $1").bind(space.id).execute(&state.db).await;
            Err(e)
        }
    }
}

/// Points page links, attachment URLs and comment anchors at the restored ids.
fn remap_content(doc: &mut Node, pages: &HashMap<Uuid, Uuid>, atts: &HashMap<Uuid, Uuid>, comments: &HashMap<Uuid, Uuid>) {
    let fix_att = |url: &str| -> Option<String> {
        let id = parse_attachment_url(url)?;
        let new = atts.get(&id)?;
        Some(url.replacen(&id.to_string(), &new.to_string(), 1))
    };
    let fix = |url: &str| -> Option<String> {
        if let Some((id, _)) = parse_page_href(url) {
            let new = pages.get(&id)?;
            return Some(url.replacen(&id.to_string(), &new.to_string(), 1));
        }
        fix_att(url)
    };
    doc.walk_mut(&mut |n| {
        for key in URL_ATTRS {
            if let Some(u) = n.attr_str(key).and_then(fix) {
                n.attrs.insert(key.into(), Value::String(u));
            }
        }
        if (n.kind == nodes::IMAGE || n.kind == nodes::ATTACHMENT)
            && let Some(new) = n.attr_str("id").and_then(|s| s.parse::<Uuid>().ok()).and_then(|id| atts.get(&id))
        {
            n.attrs.insert("id".into(), Value::String(new.to_string()));
        }
        if let Some(new) = n.attr_str("pageId").and_then(|s| s.parse::<Uuid>().ok()).and_then(|id| pages.get(&id)) {
            n.attrs.insert("pageId".into(), Value::String(new.to_string()));
        }
        for m in &mut n.marks {
            if m.kind == "link"
                && let Some(u) = m.attrs.get("href").and_then(Value::as_str).and_then(fix)
            {
                m.attrs.insert("href".into(), Value::String(u));
            }
            if m.kind == "comment"
                && let Some(new) = m.attrs.get("commentId").and_then(Value::as_str).and_then(|s| s.parse::<Uuid>().ok()).and_then(|id| comments.get(&id))
            {
                m.attrs.insert("commentId".into(), Value::String(new.to_string()));
            }
        }
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn relative_paths() {
        assert_eq!(relative("A/B/index.html", "A/C.html"), "../C.html");
        assert_eq!(relative("index.html", "A/B c.html"), "A/B%20c.html");
        assert_eq!(relative("A/x.md", "A/y.md"), "y.md");
        assert_eq!(relative("A/B/x.md", "_attachments/1/a b.png"), "../../_attachments/1/a%20b.png");
    }

    #[test]
    fn safe_names() {
        assert_eq!(safe_segment("a/b: c?"), "a-b- c-");
        assert_eq!(safe_segment("  ..  "), "Untitled");
        assert_eq!(safe_segment(".hidden"), "hidden");
        assert_eq!(safe_segment("_attachments"), "__attachments");
    }

    #[test]
    fn page_hrefs() {
        let id = Uuid::now_v7();
        assert_eq!(parse_page_href(&format!("/pages/{id}#intro")), Some((id, Some("intro"))));
        assert_eq!(parse_page_href(&format!("/pages/{id}")), Some((id, None)));
        assert_eq!(parse_page_href("/spaces/X"), None);
        assert_eq!(parse_attachment_url(&format!("/api/attachments/{id}/content")), Some(id));
    }
}
