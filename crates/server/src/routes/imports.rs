use std::collections::HashMap;
use std::io::Read;
use std::path::PathBuf;

use axum::Json;
use axum::extract::{Multipart, Path, State};
use axum::http::StatusCode;
use bytes::Bytes;
use serde::Serialize;
use serde_json::{Value, json};
use tokio::io::AsyncWriteExt;
use uuid::Uuid;
use wikicraft_doc_model::{Node, markdown, nodes};

use crate::app::{AppState, new_id};
use crate::auth::{CurrentUser, User, require_admin};
use crate::authz::{Perm, Principal, Role};
use crate::error::{ApiError, ApiResult};
use crate::importer::{self, ImportOptions};
use crate::routes::pages::{NewPage, insert_page};
use crate::routes::spaces::load_space_with_access;

/// Streams the uploaded file field to disk. Returns (path, other text fields).
pub(crate) async fn receive_upload(state: &AppState, mp: &mut Multipart) -> ApiResult<(PathBuf, HashMap<String, String>, Option<String>)> {
    let mut fields = HashMap::new();
    let mut path = None;
    let mut filename = None;
    while let Some(mut field) = mp.next_field().await.map_err(|e| ApiError::bad(format!("multipart: {e}")))? {
        let name = field.name().unwrap_or("").to_string();
        if field.file_name().is_some() && path.is_none() {
            filename = field.file_name().map(String::from);
            let p = state.config.upload_dir().join(format!("{}.upload", new_id()));
            let mut f = tokio::fs::File::create(&p).await.map_err(|e| ApiError::Internal(e.into()))?;
            let mut total: u64 = 0;
            let max = state.config.max_upload_mb * 1024 * 1024;
            while let Some(chunk) = field.chunk().await.map_err(|e| ApiError::bad(format!("upload failed: {e}")))? {
                total += chunk.len() as u64;
                if total > max {
                    drop(f);
                    let _ = tokio::fs::remove_file(&p).await;
                    return Err(ApiError::TooLarge);
                }
                f.write_all(&chunk).await.map_err(|e| ApiError::Internal(e.into()))?;
            }
            f.flush().await.map_err(|e| ApiError::Internal(e.into()))?;
            path = Some(p);
        } else {
            let v = field.text().await.map_err(|e| ApiError::bad(format!("multipart: {e}")))?;
            fields.insert(name, v);
        }
    }
    let path = path.ok_or(ApiError::bad("no file uploaded"))?;
    Ok((path, fields, filename))
}

#[derive(Serialize, sqlx::FromRow)]
#[serde(rename_all = "camelCase")]
pub struct JobRow {
    id: Uuid,
    kind: String,
    status: String,
    progress: Value,
    result: Option<Value>,
    error: Option<String>,
    created_at: chrono::DateTime<chrono::Utc>,
    started_at: Option<chrono::DateTime<chrono::Utc>>,
    finished_at: Option<chrono::DateTime<chrono::Utc>>,
    /// a running job was asked to stop and will at its next progress update
    cancelling: bool,
    /// how many times it ran (retries count)
    attempts: i32,
    /// failed or cancelled, and what it needs to run again is still there
    retryable: bool,
}

const JOB_COLUMNS: &str = "id, kind, status, progress, error, created_at, started_at, finished_at, cancel_requested AND status = 'running' AS cancelling, attempts,
    status IN ('failed', 'cancelled') AND (kind = 'export.space' OR (kind IN ('import.confluence', 'import.backup') AND COALESCE(input ? 'upload', false))) AS retryable";

/// Jobs that can be retried, and whether they need their upload for it.
const RETRYABLE: &[(&str, bool)] = &[("import.confluence", true), ("import.backup", true), ("export.space", false)];

/// `POST /api/imports/confluence` (multipart: `file`, optional `options` JSON) → job.
pub async fn upload_confluence(State(state): State<AppState>, CurrentUser(user): CurrentUser, mut mp: Multipart) -> ApiResult<(StatusCode, Json<Value>)> {
    require_admin(&user)?;
    let (path, fields, _) = receive_upload(&state, &mut mp).await?;
    let opts: ImportOptions = match fields.get("options") {
        Some(o) if !o.trim().is_empty() => serde_json::from_str(o)?,
        _ => ImportOptions::default(),
    };
    let job = start_confluence_import(&state, &user, path, opts).await?;
    Ok((StatusCode::ACCEPTED, Json(json!({ "jobId": job }))))
}

/// Stores the upload in the blob store and queues the job; any replica's worker picks it up.
pub async fn start_confluence_import(state: &AppState, user: &User, path: PathBuf, opts: ImportOptions) -> ApiResult<Uuid> {
    let job = new_id();
    let key = format!("uploads/{job}.zip");
    let stored = state.blobs.put_file(&key, &path).await;
    let _ = tokio::fs::remove_file(&path).await;
    stored.map_err(ApiError::Internal)?;
    sqlx::query("INSERT INTO jobs (id, workspace_id, kind, created_by, progress, input) VALUES ($1, $2, 'import.confluence', $3, $4, $5)")
        .bind(job)
        .bind(user.workspace_id)
        .bind(user.id)
        .bind(json!({"phase": "queued"}))
        .bind(json!({"upload": key, "options": opts}))
        .execute(&state.db)
        .await?;
    Ok(job)
}

/// `POST /api/imports/confluence/analyze` — dry run, nothing is written.
pub async fn analyze_confluence(State(state): State<AppState>, CurrentUser(user): CurrentUser, mut mp: Multipart) -> ApiResult<Json<Value>> {
    require_admin(&user)?;
    let (path, _, _) = receive_upload(&state, &mut mp).await?;
    Ok(Json(analyze_file(&state, &user, path).await?))
}

/// The dry-run report of a Confluence export on disk (removed afterwards), with the spaces an
/// earlier import brought in already.
pub async fn analyze_file(state: &AppState, user: &User, path: PathBuf) -> ApiResult<Value> {
    let p = path.clone();
    let res = tokio::task::spawn_blocking(move || importer::analyze(&p)).await.map_err(|e| ApiError::Internal(e.into()))?;
    let _ = tokio::fs::remove_file(&path).await;
    let mut report = res.map_err(|e| ApiError::bad(format!("could not read export: {e:#}")))?;
    // spaces imported before: an update import can bring them up to date instead of duplicating them
    if let Some(spaces) = report["spaces"].as_array_mut() {
        for s in spaces {
            let source_id = s["sourceId"].as_str().unwrap_or_default().to_string();
            let earlier: Option<(Uuid, String, String)> = sqlx::query_as(
                "SELECT s.id, s.key, s.name FROM import_mappings m JOIN spaces s ON s.id = m.target_id
                  WHERE m.workspace_id = $1 AND m.source = 'confluence' AND m.source_type = 'space' AND m.source_id = $2",
            )
            .bind(user.workspace_id)
            .bind(&source_id)
            .fetch_optional(&state.db)
            .await?;
            s["earlierImport"] = earlier.map(|(id, key, name)| json!({"spaceId": id, "key": key, "name": name})).unwrap_or(Value::Null);
        }
    }
    Ok(report)
}

pub async fn list_jobs(State(state): State<AppState>, CurrentUser(user): CurrentUser) -> ApiResult<Json<Vec<JobRow>>> {
    require_admin(&user)?;
    Ok(Json(
        sqlx::query_as::<_, JobRow>(sqlx::AssertSqlSafe(format!(
            "SELECT {JOB_COLUMNS}, NULL::jsonb AS result FROM jobs WHERE workspace_id = $1 ORDER BY created_at DESC LIMIT 100"
        )))
        .bind(user.workspace_id)
        .fetch_all(&state.db)
        .await?,
    ))
}

async fn load_job(state: &AppState, user: &User, id: Uuid) -> ApiResult<JobRow> {
    sqlx::query_as::<_, JobRow>(sqlx::AssertSqlSafe(format!(
        "SELECT {JOB_COLUMNS}, result FROM jobs WHERE id = $1 AND workspace_id = $2 AND (created_by = $3 OR $4)"
    )))
    .bind(id)
    .bind(user.workspace_id)
    .bind(user.id)
    .bind(user.is_admin())
    .fetch_optional(&state.db)
    .await?
    .ok_or(ApiError::NotFound("job"))
}

pub async fn get_job(State(state): State<AppState>, CurrentUser(user): CurrentUser, Path(id): Path<Uuid>) -> ApiResult<Json<JobRow>> {
    Ok(Json(load_job(&state, &user, id).await?))
}

/// `POST /api/jobs/{id}/cancel`: a queued job stops at once, a running one at its next progress
/// update (cleaning up as on a failure: a restore removes its half-restored space).
pub async fn cancel_job(State(state): State<AppState>, CurrentUser(user): CurrentUser, Path(id): Path<Uuid>) -> ApiResult<Json<JobRow>> {
    let job = load_job(&state, &user, id).await?;
    if !matches!(job.status.as_str(), "queued" | "running") {
        return Err(ApiError::Conflict("this job has already finished".into()));
    }
    let stopped = sqlx::query(
        "UPDATE jobs SET status = 'cancelled', error = 'cancelled', finished_at = now(), cancel_requested = true WHERE id = $1 AND status = 'queued'",
    )
    .bind(id)
    .execute(&state.db)
    .await?
    .rows_affected();
    if stopped == 0 {
        // it started meanwhile (or was running): it stops at its next progress update
        sqlx::query("UPDATE jobs SET cancel_requested = true WHERE id = $1 AND status = 'running'").bind(id).execute(&state.db).await?;
    }
    crate::audit::log(&state, user.workspace_id, Some(user.id), "job.cancelled", Some("job"), Some(id), json!({"kind": job.kind})).await;
    Ok(Json(load_job(&state, &user, id).await?))
}

/// `POST /api/jobs/{id}/retry`: runs a failed or cancelled job again. A Confluence import runs
/// as an update of what the earlier attempt imported, so it carries on instead of duplicating.
pub async fn retry_job(State(state): State<AppState>, CurrentUser(user): CurrentUser, Path(id): Path<Uuid>) -> ApiResult<(StatusCode, Json<JobRow>)> {
    let job = load_job(&state, &user, id).await?;
    if !matches!(job.status.as_str(), "failed" | "cancelled") {
        return Err(ApiError::Conflict("only failed or cancelled jobs can be retried".into()));
    }
    let Some(&(_, needs_upload)) = RETRYABLE.iter().find(|(k, _)| *k == job.kind) else {
        return Err(ApiError::bad("this kind of job can't be retried"));
    };
    if job.kind.starts_with("import.") {
        require_admin(&user)?;
    }
    let mut input: Value = sqlx::query_scalar("SELECT input FROM jobs WHERE id = $1").bind(id).fetch_one(&state.db).await?;
    if needs_upload && input.get("upload").and_then(Value::as_str).is_none() {
        return Err(ApiError::Conflict(format!(
            "the uploaded file is gone (it is kept {} days after a failure); upload it again",
            crate::jobs::RETRY_UPLOAD_DAYS
        )));
    }
    if job.kind == "import.confluence"
        && let Some(o) = input.as_object_mut()
    {
        let opts = o.entry("options").or_insert_with(|| json!({}));
        if let Some(opts) = opts.as_object_mut() {
            opts.insert("update".into(), Value::Bool(true));
        }
    }
    let n = sqlx::query(
        "UPDATE jobs SET status = 'queued', error = NULL, result = NULL, progress = '{}', started_at = NULL, finished_at = NULL, heartbeat_at = NULL,
                cancel_requested = false, attempts = attempts + 1, input = $2
         WHERE id = $1 AND status IN ('failed', 'cancelled')",
    )
    .bind(id)
    .bind(&input)
    .execute(&state.db)
    .await?
    .rows_affected();
    if n == 0 {
        return Err(ApiError::Conflict("the job changed meanwhile".into()));
    }
    crate::audit::log(&state, user.workspace_id, Some(user.id), "job.retried", Some("job"), Some(id), json!({"kind": job.kind})).await;
    Ok((StatusCode::ACCEPTED, Json(load_job(&state, &user, id).await?)))
}

/// `POST /api/imports/markdown` (multipart: `spaceKey`, optional `parentId`, one `.md` file or a
/// `.zip` of Markdown files/folders with images). Folders become pages; `index.md`/`README.md`
/// inside a folder becomes that folder's page content. Relative images become attachments.
pub async fn upload_markdown(State(state): State<AppState>, CurrentUser(user): CurrentUser, mut mp: Multipart) -> ApiResult<(StatusCode, Json<Value>)> {
    let (path, fields, filename) = receive_upload(&state, &mut mp).await?;
    let res = import_markdown(&state, &user, &path, &fields, filename.as_deref().unwrap_or("import.md")).await;
    let _ = tokio::fs::remove_file(&path).await;
    res.map(|v| (StatusCode::CREATED, Json(v)))
}

/// Markdown imports hold their files in memory: at most two at a time.
static MARKDOWN_IMPORTS: tokio::sync::Semaphore = tokio::sync::Semaphore::const_new(2);

async fn import_markdown(state: &AppState, user: &User, path: &std::path::Path, fields: &HashMap<String, String>, filename: &str) -> ApiResult<Value> {
    let _slot = MARKDOWN_IMPORTS.acquire().await.map_err(|e| ApiError::Internal(e.into()))?;
    let p = Principal::load(state, Some(user.clone())).await?;
    let key = fields.get("spaceKey").ok_or(ApiError::bad("spaceKey is required"))?;
    let (space, a) = load_space_with_access(state, &p, key).await?;
    if a.role < Role::Editor || !a.perms.has(Perm::AddPages) {
        return Err(ApiError::Forbidden);
    }
    let root_parent: Option<Uuid> = fields.get("parentId").and_then(|s| s.parse().ok());
    if let Some(par) = root_parent {
        let (_, ps) = crate::authz::require_page(state, &p, par, crate::authz::Need::Edit).await?;
        if ps != space.id {
            return Err(ApiError::bad("parentId belongs to another space"));
        }
    }
    // collect files: name → bytes
    let mut files: Vec<(String, Bytes)> = Vec::new();
    if filename.to_lowercase().ends_with(".zip") {
        let p = path.to_path_buf();
        files = tokio::task::spawn_blocking(move || -> anyhow::Result<Vec<(String, Bytes)>> {
            let mut zip = zip::ZipArchive::new(std::fs::File::open(&p)?)?;
            const MAX_FILES: usize = 5_000;
            const MAX_ENTRY: u64 = 64 * 1024 * 1024;
            const MAX_TOTAL: u64 = 512 * 1024 * 1024;
            let mut out = Vec::new();
            let mut total = 0u64;
            if zip.len() > MAX_FILES * 4 {
                anyhow::bail!("too many entries in zip");
            }
            let mut nested: Vec<Vec<u8>> = Vec::new();
            for i in 0..zip.len() {
                let f = zip.by_index(i)?;
                if f.is_dir() {
                    continue;
                }
                let Some(name) = f.enclosed_name().map(|n| n.to_string_lossy().replace('\\', "/")) else { continue };
                if name.starts_with("__MACOSX") || name.split('/').any(|s| s.starts_with('.')) {
                    continue;
                }
                if out.len() >= MAX_FILES {
                    anyhow::bail!("more than {MAX_FILES} files");
                }
                // never trust declared sizes (zip bombs): count the bytes actually read
                let mut buf = Vec::new();
                let read = f.take(MAX_ENTRY + 1).read_to_end(&mut buf)? as u64;
                if read > MAX_ENTRY {
                    anyhow::bail!("{name} is larger than {} MB", MAX_ENTRY / 1024 / 1024);
                }
                total += read;
                if total > MAX_TOTAL {
                    anyhow::bail!("zip content larger than {} MB", MAX_TOTAL / 1024 / 1024);
                }
                if name.to_lowercase().ends_with(".zip") && !name.contains('/') {
                    nested.push(buf);
                    continue;
                }
                out.push((name, Bytes::from(buf)));
            }
            // one level of nesting (Notion: "Export-<id>.zip" inside the download)
            if out.is_empty() {
                for inner in nested {
                    let mut z = zip::ZipArchive::new(std::io::Cursor::new(inner))?;
                    for i in 0..z.len() {
                        let f = z.by_index(i)?;
                        if f.is_dir() {
                            continue;
                        }
                        let Some(name) = f.enclosed_name().map(|n| n.to_string_lossy().replace('\\', "/")) else { continue };
                        if name.starts_with("__MACOSX") || name.split('/').any(|s| s.starts_with('.')) {
                            continue;
                        }
                        if out.len() >= MAX_FILES {
                            anyhow::bail!("more than {MAX_FILES} files");
                        }
                        let mut buf = Vec::new();
                        let read = f.take(MAX_ENTRY + 1).read_to_end(&mut buf)? as u64;
                        if read > MAX_ENTRY {
                            anyhow::bail!("{name} is larger than {} MB", MAX_ENTRY / 1024 / 1024);
                        }
                        total += read;
                        if total > MAX_TOTAL {
                            anyhow::bail!("zip content larger than {} MB", MAX_TOTAL / 1024 / 1024);
                        }
                        out.push((name, Bytes::from(buf)));
                    }
                }
            }
            Ok(out)
        })
        .await
        .map_err(|e| ApiError::Internal(e.into()))?
        .map_err(|e| ApiError::bad(format!("invalid zip: {e}")))?;
    } else {
        files.push((filename.to_string(), Bytes::from(tokio::fs::read(path).await.map_err(|e| ApiError::Internal(e.into()))?)));
    }
    // Word documents are converted up front; their images become files next to them. With
    // `splitHeadings` (1–3) each heading of that level or above starts a page of its own.
    let split_level: i64 = fields.get("splitHeadings").and_then(|v| v.trim().parse().ok()).filter(|l| (1..=3).contains(l)).unwrap_or(0);
    let is_docx = |n: &str| n.to_lowercase().ends_with(".docx");
    let mut word: HashMap<String, (Option<String>, Node)> = HashMap::new();
    let docx_files: Vec<(String, Bytes)> = files.iter().filter(|(n, _)| is_docx(n)).cloned().collect();
    // images extracted from Word files count against one budget for the whole import
    const MAX_EXTRACTED: usize = 512 << 20;
    let mut extracted = 0usize;
    for (name, data) in docx_files {
        let file = name.rsplit('/').next().unwrap_or(&name).to_string();
        let prefix = format!("{file}.media/");
        let converted = tokio::task::spawn_blocking(move || crate::docx_import::convert(&data, &prefix))
            .await
            .map_err(|e| ApiError::Internal(e.into()))?
            .map_err(|e| ApiError::bad(format!("{name}: {e}")))?;
        for (img, bytes) in converted.media {
            extracted += bytes.len();
            if extracted > MAX_EXTRACTED {
                return Err(ApiError::bad(format!("the Word files hold more than {} MB of images; import them in smaller batches", MAX_EXTRACTED >> 20)));
            }
            files.push((format!("{name}.media/{img}"), Bytes::from(bytes)));
        }
        if split_level > 0 {
            // one page per heading: the sections become pages in a folder named like the
            // document, which the document's own page then holds
            let (intro, sections) = split_sections(converted.doc.content, 1, split_level);
            if !sections.is_empty() {
                let stem = name.strip_suffix(".docx").or_else(|| name.strip_suffix(".DOCX")).unwrap_or(&name).to_string();
                add_sections(&mut files, &mut word, &stem, sections);
                let intro = if intro.is_empty() { vec![Node::paragraph(vec![])] } else { intro };
                word.insert(name, (converted.title, Node::doc(intro)));
                continue;
            }
            word.insert(name, (converted.title, Node::doc(sections_fallback(intro))));
            continue;
        }
        word.insert(name, (converted.title, converted.doc));
    }
    // HTML pages too (plain sites, Notion and Confluence HTML exports); Confluence's export
    // also names each page's parent (breadcrumbs) and its attachments
    let is_html = |n: &str| {
        let l = n.to_lowercase();
        l.ends_with(".html") || l.ends_with(".htm")
    };
    let html_files: Vec<(String, Bytes)> = files.iter().filter(|(n, _)| is_html(n)).cloned().collect();
    let mut html_meta: HashMap<String, (Option<String>, Vec<(String, String)>)> = HashMap::new();
    if !html_files.is_empty() {
        let pages = tokio::task::spawn_blocking(move || {
            html_files.into_iter().map(|(n, d)| (n, crate::html_import::convert(&String::from_utf8_lossy(&d)))).collect::<Vec<_>>()
        })
        .await
        .map_err(|e| ApiError::Internal(e.into()))?;
        for (name, p) in pages {
            word.insert(name.clone(), (p.title, p.doc));
            html_meta.insert(name, (p.parent_href, p.attachments));
        }
    }
    files.sort_by(|a, b| a.0.cmp(&b.0));
    let is_md = |n: &str| n.to_lowercase().ends_with(".md") || n.to_lowercase().ends_with(".markdown");
    let is_page = |n: &str| is_md(n) || is_docx(n) || is_html(n);
    let assets: HashMap<String, Bytes> = files.iter().filter(|(n, _)| !is_page(n)).cloned().collect();
    // every directory that holds files ("A.md" next to "A/" is that folder's page)
    let dirs: std::collections::HashSet<String> = files
        .iter()
        .flat_map(|(n, _)| {
            let parts: Vec<&str> = n.split('/').collect();
            (1..parts.len()).map(move |i| parts[..i].join("/")).collect::<Vec<_>>()
        })
        .collect();

    // folder pages first (sorted paths guarantee parents first)
    let mut folder_pages: HashMap<String, Uuid> = HashMap::new();
    let mut created = 0;
    let mut title_index: HashMap<String, Uuid> = HashMap::new();
    let mut path_index: HashMap<String, Uuid> = HashMap::new();
    let mut pending_docs: Vec<(Uuid, String, Node)> = Vec::new();
    let mut parent_of: HashMap<Uuid, Option<Uuid>> = HashMap::new();
    let mut pos = 0.0;
    async fn ensure_folder(
        state: &AppState,
        user: &User,
        space: Uuid,
        root: Option<Uuid>,
        folders: &mut HashMap<String, Uuid>,
        dir: &str,
        pos: &mut f64,
        created: &mut usize,
    ) -> ApiResult<Option<Uuid>> {
        if dir.is_empty() {
            return Ok(root);
        }
        if let Some(id) = folders.get(dir) {
            return Ok(Some(*id));
        }
        let (parent_dir, name) = dir.rsplit_once('/').unwrap_or(("", dir));
        let name = clean_name(name);
        let parent = Box::pin(ensure_folder(state, user, space, root, folders, parent_dir, pos, created)).await?;
        *pos += 1.0;
        let id = insert_page(
            &state.db,
            NewPage {
                id: new_id(),
                workspace_id: user.workspace_id,
                space_id: space,
                parent_id: parent,
                kind: "page".into(),
                title: name.to_string(),
                content: Node::doc(vec![Node::new(nodes::CHILD_PAGES)]),
                position: *pos,
                created_by: Some(user.id),
                created_at: None,
                source_ref: None,
                draft: false,
            },
        )
        .await?;
        *created += 1;
        folders.insert(dir.to_string(), id);
        Ok(Some(id))
    }
    for (name, data) in files.iter().filter(|(n, _)| is_page(n)) {
        let (dir, file) = name.rsplit_once('/').unwrap_or(("", name.as_str()));
        let stem = file.rsplit_once('.').map(|(s, _)| s).unwrap_or(file);
        let from_word = word.remove(name.as_str());
        let is_word = from_word.is_some();
        let imported = match from_word {
            Some((title, doc)) => markdown::MarkdownImport { doc, title, front_matter: None },
            None => markdown::markdown_to_pm(&String::from_utf8_lossy(data)),
        };
        let own_dir = if dir.is_empty() { stem.to_string() } else { format!("{dir}/{stem}") };
        // README/index inside a folder, or "Page.md" next to "Page/" (Notion, Obsidian, Docusaurus)
        let folder_dir: Option<String> = if matches!(stem.to_lowercase().as_str(), "index" | "readme") && !dir.is_empty() {
            Some(dir.to_string())
        } else if dirs.contains(&own_dir) {
            Some(own_dir.clone())
        } else {
            None
        };
        let (fm_title, fm_labels) = imported.front_matter.as_deref().map(front_matter).unwrap_or_default();
        let title: String =
            fm_title.or_else(|| imported.title.clone()).unwrap_or_else(|| clean_name(stem).replace(['-', '_'], " ")).chars().take(255).collect();
        let mut doc = imported.doc;
        // drop the H1 used as the title
        if !is_word && doc.content.first().is_some_and(|n| n.kind == nodes::HEADING && n.attr_i64("level") == Some(1)) && imported.title.is_some() {
            doc.content.remove(0);
            if doc.content.is_empty() {
                doc.content.push(Node::paragraph(vec![]));
            }
        }
        let id = if let Some(fd) = &folder_dir {
            let folder =
                ensure_folder(state, user, space.id, root_parent, &mut folder_pages, fd, &mut pos, &mut created).await?.ok_or(ApiError::bad("bad folder"))?;
            sqlx::query("UPDATE pages SET title = $2 WHERE id = $1").bind(folder).bind(&title).execute(&state.db).await?;
            folder
        } else {
            let parent = ensure_folder(state, user, space.id, root_parent, &mut folder_pages, dir, &mut pos, &mut created).await?;
            pos += 1.0;
            let id = insert_page(
                &state.db,
                NewPage {
                    id: new_id(),
                    workspace_id: user.workspace_id,
                    space_id: space.id,
                    parent_id: parent,
                    kind: "page".into(),
                    title: title.clone(),
                    content: Node::empty_doc(),
                    position: pos,
                    created_by: Some(user.id),
                    created_at: None,
                    source_ref: None,
                    draft: false,
                },
            )
            .await?;
            created += 1;
            parent_of.insert(id, parent);
            id
        };
        for l in fm_labels.iter().filter_map(|l| crate::routes::pages::normalize_label(l)).take(50) {
            sqlx::query("INSERT INTO labels (page_id, name) VALUES ($1, $2) ON CONFLICT DO NOTHING").bind(id).bind(l).execute(&state.db).await?;
        }
        // other front matter keys become page properties
        let fm_props = imported.front_matter.as_deref().map(crate::routes::properties::from_front_matter).unwrap_or_default();
        if !fm_props.is_empty()
            && let Err(e) = crate::routes::properties::save(state, &p, user.workspace_id, id, fm_props).await
        {
            tracing::warn!("{name}: front matter properties skipped: {e}");
        }
        title_index.insert(title.to_lowercase(), id);
        title_index.insert(stem.to_lowercase(), id);
        title_index.insert(clean_name(stem).to_lowercase(), id);
        path_index.insert(name.clone(), id);
        pending_docs.push((id, dir.to_string(), doc));
    }
    // CSV files (Notion databases) become table pages; a "DB.csv" next to "DB/" is that
    // folder's page, and first-column cells link to the row pages
    let csv_names: Vec<&String> = files.iter().map(|(n, _)| n).filter(|n| n.to_lowercase().ends_with(".csv")).collect();
    for name in csv_names.iter().copied() {
        let lower = name.to_lowercase();
        // Notion writes "DB.csv" and "DB_all.csv" (all properties): prefer the latter
        if !lower.ends_with("_all.csv") && csv_names.iter().any(|o| o.to_lowercase() == format!("{}_all.csv", &lower[..lower.len() - 4])) {
            continue;
        }
        let Some(data) = assets.get(name.as_str()) else { continue };
        let (dir, file) = name.rsplit_once('/').unwrap_or(("", name.as_str()));
        let stem = file[..file.len() - 4].trim_end_matches("_all");
        let rows = parse_csv(&String::from_utf8_lossy(data));
        if rows.is_empty() {
            continue;
        }
        let cell = |kind: &str, text: &str, link: bool| {
            let mut t = Node::text(text);
            if link && !text.is_empty() {
                t.marks.push(wikicraft_doc_model::Mark::new("link").attr("href", format!("wikilink:{text}")));
            }
            Node::new(kind).with_content(vec![Node::paragraph(if text.is_empty() { vec![] } else { vec![t] })])
        };
        let width = rows[0].len().clamp(1, 30);
        let table_rows: Vec<Node> = rows
            .iter()
            .take(1001)
            .enumerate()
            .map(|(i, r)| {
                let cells = (0..width).map(|c| {
                    let text = r.get(c).map(String::as_str).unwrap_or("").trim();
                    let link = i > 0 && c == 0 && title_index.contains_key(&text.to_lowercase());
                    cell(if i == 0 { nodes::TABLE_HEADER } else { nodes::TABLE_CELL }, text, link)
                });
                Node::new(nodes::TABLE_ROW).with_content(cells.collect())
            })
            .collect();
        let doc = Node::doc(vec![Node::new(nodes::TABLE).with_content(table_rows)]);
        let title = clean_name(stem);
        let own_dir = if dir.is_empty() { stem.to_string() } else { format!("{dir}/{stem}") };
        let id = if dirs.contains(&own_dir) {
            let folder = ensure_folder(state, user, space.id, root_parent, &mut folder_pages, &own_dir, &mut pos, &mut created)
                .await?
                .ok_or(ApiError::bad("bad folder"))?;
            sqlx::query("UPDATE pages SET title = $2 WHERE id = $1").bind(folder).bind(&title).execute(&state.db).await?;
            folder
        } else {
            let parent = ensure_folder(state, user, space.id, root_parent, &mut folder_pages, dir, &mut pos, &mut created).await?;
            pos += 1.0;
            created += 1;
            insert_page(
                &state.db,
                NewPage {
                    id: new_id(),
                    workspace_id: user.workspace_id,
                    space_id: space.id,
                    parent_id: parent,
                    kind: "page".into(),
                    title: title.clone(),
                    content: Node::empty_doc(),
                    position: pos,
                    created_by: Some(user.id),
                    created_at: None,
                    source_ref: None,
                    draft: false,
                },
            )
            .await?
        };
        // the database's rows are the pages in its folder: the other columns become their
        // properties, and the "Key: Value" lines Notion writes under their titles go
        let prefix = format!("{own_dir}/");
        let mut row_pages: HashMap<String, Option<Uuid>> = HashMap::new();
        for (path, pid) in &path_index {
            if let Some(rest) = path.strip_prefix(&prefix)
                && !rest.contains('/')
                && is_page(rest)
            {
                let stem = rest.rsplit_once('.').map(|(s, _)| s).unwrap_or(rest);
                // two rows with the same title: neither gets properties
                row_pages.entry(clean_name(stem).to_lowercase()).and_modify(|v| *v = None).or_insert(Some(*pid));
            }
        }
        if !row_pages.is_empty() {
            let headers: Vec<String> = rows[0].iter().map(|h| crate::routes::properties::clean_key(h).chars().take(100).collect()).collect();
            let kinds: Vec<&str> = (0..headers.len()).map(|c| column_kind(rows.iter().skip(1).filter_map(|r| r.get(c)).map(|v| v.trim()))).collect();
            for r in rows.iter().skip(1).take(5000) {
                let Some(Some(pid)) = r.first().and_then(|t| row_pages.get(&t.trim().to_lowercase())) else { continue };
                let mut seen = std::collections::HashSet::new();
                let props: Vec<crate::routes::properties::PropertyIn> = headers
                    .iter()
                    .enumerate()
                    .skip(1)
                    .filter(|(_, h)| !h.is_empty() && seen.insert(h.to_lowercase()))
                    .take(50)
                    .map(|(c, h)| {
                        let v = r.get(c).map(|v| v.trim()).unwrap_or("");
                        let value = match kinds[c] {
                            _ if v.is_empty() => Value::Null,
                            "date" => notion_date(v).map(|d| Value::String(d.to_string())).unwrap_or(Value::Null),
                            _ => Value::String(v.chars().take(5000).collect()),
                        };
                        crate::routes::properties::PropertyIn { key: h.clone(), kind: kinds[c].into(), value }
                    })
                    .collect();
                if props.is_empty() {
                    continue;
                }
                if let Err(e) = crate::routes::properties::save(state, &p, user.workspace_id, *pid, props).await {
                    tracing::warn!("{name}: properties of a row skipped: {e}");
                    continue;
                }
                if let Some((_, _, doc)) = pending_docs.iter_mut().find(|(id, _, _)| id == pid)
                    && doc
                        .content
                        .first()
                        .is_some_and(|n| n.kind == nodes::PARAGRAPH && is_property_lines(&wikicraft_doc_model::text::plain_text(n), &headers[1..]))
                {
                    doc.content.remove(0);
                    if doc.content.is_empty() {
                        doc.content.push(Node::paragraph(vec![]));
                    }
                }
            }
        }
        title_index.entry(title.to_lowercase()).or_insert(id);
        path_index.insert(name.to_string(), id);
        if lower.ends_with("_all.csv") {
            path_index.insert(format!("{}.csv", &name[..name.len() - 8]), id);
        }
        pending_docs.push((id, dir.to_string(), doc));
    }

    // Confluence HTML export: all pages sit in one folder, the breadcrumbs give the hierarchy
    for (dir, id) in &folder_pages {
        let pd = dir.rsplit_once('/').map(|(p, _)| p).unwrap_or("");
        parent_of.entry(*id).or_insert(if pd.is_empty() { root_parent } else { folder_pages.get(pd).copied() });
    }
    let mut html_names: Vec<&String> = html_meta.keys().collect();
    html_names.sort();
    for name in html_names {
        let Some(href) = &html_meta[name].0 else { continue };
        let dir = name.rsplit_once('/').map(|(d, _)| d).unwrap_or("");
        let target = local_target(dir, href);
        let (Some(&child), Some(&parent)) = (path_index.get(name), path_index.get(&target)) else { continue };
        // never create a cycle (hostile or inconsistent breadcrumbs)
        let mut cur = Some(parent);
        let mut cyclic = false;
        for _ in 0..=parent_of.len() {
            match cur {
                Some(c) if c == child => {
                    cyclic = true;
                    break;
                }
                Some(c) if Some(c) == root_parent => break,
                Some(c) => cur = parent_of.get(&c).copied().flatten(),
                None => break,
            }
        }
        if cyclic || cur.is_some_and(|c| Some(c) != root_parent) {
            continue;
        }
        parent_of.insert(child, Some(parent));
        sqlx::query("UPDATE pages SET parent_id = $2 WHERE id = $1").bind(child).bind(parent).execute(&state.db).await?;
    }
    let html_pages: HashMap<Uuid, &String> = html_meta.keys().filter_map(|n| path_index.get(n).map(|id| (*id, n))).collect();

    // second pass: images and linked files → attachments, wiki links → page links
    let mut attachments = 0;
    let mut skipped_files: Vec<String> = Vec::new();
    let find_asset = |joined: &str, decoded: &str| -> Option<(String, Bytes)> {
        assets
            .get_key_value(joined)
            .or_else(|| assets.iter().find(|(k, _)| k.ends_with(&format!("/{decoded}")) || **k == decoded))
            .map(|(k, v)| (k.clone(), v.clone()))
    };
    for (id, dir, mut doc) in pending_docs {
        // file names the page lists for its attachments (Confluence HTML export)
        let listed: Vec<(String, String)> = html_pages.get(&id).map(|n| html_meta[*n].1.clone()).unwrap_or_default();
        let names: HashMap<String, String> = listed.iter().map(|(h, n)| (local_target(&dir, h), n.clone())).collect();
        let mut refs: Vec<String> = Vec::new();
        doc.walk(&mut |n| {
            if n.kind == nodes::IMAGE
                && let Some(src) = n.attr_str("src").filter(|s| is_local_file(s))
            {
                refs.push(src.to_string());
            }
            for m in &n.marks {
                if m.kind == "link"
                    && let Some(href) = m.attr_str("href").filter(|h| is_local_file(h) && !is_page_link(h))
                {
                    refs.push(href.to_string());
                }
            }
        });
        refs.extend(listed.iter().map(|(h, _)| h.clone()).filter(|h| is_local_file(h) && !is_page_link(h)));
        // one attachment per file, however often the page refers to it
        let mut stored: HashMap<String, Uuid> = HashMap::new();
        let mut urls: HashMap<String, String> = HashMap::new();
        for src in refs {
            let decoded = percent_decode(src.split(['?', '#']).next().unwrap_or(&src));
            let joined = normalize_path(&if dir.is_empty() { decoded.clone() } else { format!("{dir}/{decoded}") });
            let Some((key, data)) = find_asset(&joined, &decoded) else { continue };
            let att = match stored.get(&key) {
                Some(a) => *a,
                None => {
                    let fname = names.get(&key).cloned().unwrap_or_else(|| decoded.rsplit('/').next().unwrap_or(&decoded).to_string());
                    // the wiki's upload rules apply: files they refuse stay out (and are reported)
                    let clean = crate::routes::attachments::clean_filename(&fname);
                    if let Err(e) = crate::routes::attachments::check_upload(state, user.workspace_id, space.id, &clean, data.len() as u64).await {
                        if skipped_files.len() < 50 {
                            skipped_files.push(e.to_string());
                        }
                        continue;
                    }
                    if let Some(sig) = crate::antivirus::screen(state, &data).await? {
                        crate::audit::log(
                            state,
                            user.workspace_id,
                            Some(user.id),
                            "upload.infected",
                            Some("page"),
                            Some(id),
                            json!({"file": clean, "signature": sig}),
                        )
                        .await;
                        if skipped_files.len() < 50 {
                            skipped_files.push(format!("{clean}: the virus scanner found {sig}"));
                        }
                        continue;
                    }
                    let a = crate::routes::attachments::store_attachment(state, user.workspace_id, id, Some(user.id), &fname, data, None, None).await?;
                    attachments += 1;
                    stored.insert(key, a);
                    a
                }
            };
            urls.insert(src.clone(), format!("/api/attachments/{att}/content"));
        }
        doc.walk_mut(&mut |n| {
            if n.kind == nodes::IMAGE
                && let Some(u) = n.attr_str("src").and_then(|s| urls.get(s)).cloned()
            {
                n.attrs.insert("src".into(), Value::String(u));
            }
            for m in &mut n.marks {
                if m.kind == "link" {
                    let href = m.attr_str("href").unwrap_or("").to_string();
                    let (path, anchor) = href.split_once('#').map(|(a, b)| (a.to_string(), Some(b.to_string()))).unwrap_or((href.clone(), None));
                    let hash = anchor.map(|a| format!("#{a}")).unwrap_or_default();
                    let new = if let Some(u) = urls.get(&href) {
                        Some(u.clone())
                    } else if let Some(t) = href.strip_prefix("wikilink:") {
                        let (t, a) = t.split_once('#').map(|(x, y)| (x, format!("#{y}"))).unwrap_or((t, String::new()));
                        Some(match title_index.get(&t.to_lowercase()) {
                            Some(pid) => format!("/pages/{pid}{a}"),
                            None => format!("/missing?space={}&title={}", space.key, t),
                        })
                    } else if is_local_file(&path) && is_page_link(&path) {
                        let decoded = percent_decode(path.split('?').next().unwrap_or(&path));
                        // the exact file first (relative to this page), then by file name / title
                        let joined = normalize_path(&if dir.is_empty() { decoded.clone() } else { format!("{dir}/{decoded}") });
                        let stem = decoded.rsplit('/').next().unwrap_or(&decoded).rsplit_once('.').map(|(s, _)| s).unwrap_or(&decoded).to_string();
                        Some(match path_index.get(&joined).or_else(|| title_index.get(&stem.to_lowercase())) {
                            Some(pid) => format!("/pages/{pid}{hash}"),
                            None => format!("/missing?space={}&title={}", space.key, stem),
                        })
                    } else {
                        None
                    };
                    if let Some(new) = new {
                        m.attrs.insert("href".into(), Value::String(new));
                    }
                }
            }
        });
        sqlx::query("UPDATE pages SET content = $2, text_content = $3 WHERE id = $1")
            .bind(id)
            .bind(doc.to_json())
            .bind(wikicraft_doc_model::text::plain_text(&doc))
            .execute(&state.db)
            .await?;
        sqlx::query("UPDATE page_versions SET content = $2 WHERE page_id = $1 AND version = 1").bind(id).bind(doc.to_json()).execute(&state.db).await?;
        state.search.enqueue(id);
    }
    for id in folder_pages.values() {
        state.search.enqueue(*id);
    }
    Ok(json!({ "pages": created, "attachments": attachments, "skippedFiles": skipped_files }))
}

/// A relative reference to a file of the import (not a URL, absolute path, anchor or `data:`).
fn is_local_file(href: &str) -> bool {
    let h = href.trim();
    let scheme = h.split_once(':').is_some_and(|(s, _)| !s.is_empty() && !s.contains('/') && s.chars().all(|c| c.is_ascii_alphanumeric() || "+.-".contains(c)));
    !h.is_empty() && !scheme && !h.starts_with('/') && !h.starts_with('#') && !h.starts_with('?')
}

/// A link to another page of the import (Markdown, Notion database CSV, HTML).
fn is_page_link(href: &str) -> bool {
    let p = href.split(['?', '#']).next().unwrap_or(href).to_lowercase();
    [".md", ".markdown", ".csv", ".html", ".htm"].iter().any(|e| p.ends_with(e))
}

/// The import path a relative reference from a file in `dir` points to.
fn local_target(dir: &str, href: &str) -> String {
    let decoded = percent_decode(href.split(['?', '#']).next().unwrap_or(href));
    normalize_path(&if dir.is_empty() { decoded } else { format!("{dir}/{decoded}") })
}

/// Removes the 32-hex-digit id Notion appends to every file and folder name.
fn clean_name(name: &str) -> String {
    let n = name.trim();
    match n.rsplit_once(' ') {
        Some((base, id)) if id.len() == 32 && id.chars().all(|c| c.is_ascii_hexdigit()) && !base.trim().is_empty() => base.trim().to_string(),
        _ => n.to_string(),
    }
}

/// Minimal RFC 4180 CSV reader (quoted fields, escaped quotes, newlines in quotes, BOM).
fn parse_csv(text: &str) -> Vec<Vec<String>> {
    let text = text.strip_prefix('\u{feff}').unwrap_or(text);
    let (mut rows, mut row, mut field) = (Vec::new(), Vec::new(), String::new());
    let mut chars = text.chars().peekable();
    let mut quoted = false;
    while let Some(c) = chars.next() {
        if quoted {
            match c {
                '"' if chars.peek() == Some(&'"') => {
                    field.push('"');
                    chars.next();
                }
                '"' => quoted = false,
                _ => field.push(c),
            }
            continue;
        }
        match c {
            '"' if field.is_empty() => quoted = true,
            ',' => row.push(std::mem::take(&mut field)),
            '\r' => {}
            '\n' => {
                row.push(std::mem::take(&mut field));
                rows.push(std::mem::take(&mut row));
            }
            _ => field.push(c),
        }
        if rows.len() > 5000 {
            break;
        }
    }
    if !field.is_empty() || !row.is_empty() {
        row.push(field);
        rows.push(row);
    }
    rows.retain(|r: &Vec<String>| r.iter().any(|f| !f.trim().is_empty()));
    rows
}

/// `%XX` escapes → bytes (invalid sequences are kept as they are).
fn percent_decode(s: &str) -> String {
    let b = s.as_bytes();
    let hex = |c: u8| (c as char).to_digit(16).map(|d| d as u8);
    let mut out = Vec::with_capacity(b.len());
    let mut i = 0;
    while i < b.len() {
        if b[i] == b'%'
            && i + 2 < b.len()
            && let (Some(h), Some(l)) = (hex(b[i + 1]), hex(b[i + 2]))
        {
            out.push(h * 16 + l);
            i += 3;
            continue;
        }
        out.push(b[i]);
        i += 1;
    }
    String::from_utf8_lossy(&out).into_owned()
}

/// `title` and `labels` from simple front matter (`key: value`, values as YAML/JSON scalars or
/// flow lists).
fn front_matter(fm: &str) -> (Option<String>, Vec<String>) {
    let scalar = |v: &str| -> String {
        let v = v.trim();
        serde_json::from_str::<String>(v).unwrap_or_else(|_| v.trim_matches(['\'', '"']).to_string())
    };
    let mut title = None;
    let mut labels = Vec::new();
    for line in fm.lines() {
        let Some((k, v)) = line.split_once(':') else { continue };
        match k.trim().to_lowercase().as_str() {
            "title" => title = Some(scalar(v)).filter(|t| !t.is_empty()),
            "labels" | "tags" => {
                let v = v.trim();
                labels = serde_json::from_str::<Vec<String>>(v)
                    .unwrap_or_else(|_| v.trim_start_matches('[').trim_end_matches(']').split(',').map(scalar).filter(|l| !l.is_empty()).collect());
            }
            _ => {}
        }
    }
    (title, labels)
}

/// A date as Notion writes it (`December 1, 2026`, `2026/12/01`, ISO, with a time, or the start
/// of a range `… → …`).
fn notion_date(v: &str) -> Option<chrono::NaiveDate> {
    let v = v.split('→').next().unwrap_or(v).trim();
    for f in ["%Y-%m-%d", "%B %d, %Y", "%b %d, %Y", "%Y/%m/%d"] {
        if let Ok(d) = chrono::NaiveDate::parse_from_str(v, f) {
            return Some(d);
        }
    }
    for f in ["%B %d, %Y %I:%M %p", "%B %d, %Y %H:%M", "%Y-%m-%d %H:%M", "%Y-%m-%dT%H:%M:%S%.fZ"] {
        if let Ok(d) = chrono::NaiveDateTime::parse_from_str(v, f) {
            return Some(d.date());
        }
    }
    None
}

/// The property type a database column's values fit: number, date, web address, select
/// (a few short values that repeat) or text.
fn column_kind<'a>(values: impl Iterator<Item = &'a str>) -> &'static str {
    let values: Vec<&str> = values.filter(|v| !v.is_empty()).collect();
    if values.is_empty() {
        return "text";
    }
    if values.iter().all(|v| v.parse::<f64>().is_ok_and(f64::is_finite)) {
        return "number";
    }
    if values.iter().all(|v| notion_date(v).is_some()) {
        return "date";
    }
    if values.iter().all(|v| (v.starts_with("https://") || v.starts_with("http://")) && !v.contains(char::is_whitespace)) {
        return "url";
    }
    let distinct: std::collections::HashSet<String> = values.iter().map(|v| v.to_lowercase()).collect();
    if values.len() >= 2 && distinct.len() < values.len() && distinct.len() <= 20 && values.iter().all(|v| v.chars().count() <= 40 && !v.contains(',')) {
        return "select";
    }
    "text"
}

/// Whether a paragraph is only `Key: value` pairs for the given keys (Notion's property lines
/// under a database row's title; soft line breaks became spaces).
fn is_property_lines(text: &str, keys: &[String]) -> bool {
    let keys: Vec<&String> = keys.iter().filter(|k| !k.is_empty()).collect();
    let mut rest = text.trim();
    let mut found = 0;
    while !rest.is_empty() {
        let Some(k) = keys.iter().filter(|k| rest.strip_prefix(k.as_str()).is_some_and(|r| r.starts_with(':'))).max_by_key(|k| k.len()) else { return false };
        rest = rest[k.len() + 1..].trim_start();
        found += 1;
        // the value runs to the next " Key:"
        let next = keys.iter().filter_map(|k| rest.find(&format!(" {k}:"))).min();
        rest = next.map(|i| &rest[i + 1..]).unwrap_or("");
    }
    found > 0
}

/// A Word section: its heading text, content and the sections below it.
struct Section {
    title: String,
    content: Vec<Node>,
    children: Vec<Section>,
}

/// Splits blocks at headings of `level` (recursing to `max`): the blocks before the first one,
/// and the sections.
fn split_sections(blocks: Vec<Node>, level: i64, max: i64) -> (Vec<Node>, Vec<Section>) {
    let is_cut = |n: &Node| n.kind == nodes::HEADING && n.attr_i64("level") == Some(level);
    if level > max || !blocks.iter().any(is_cut) {
        // no heading of this level: try the next one down for this part
        if level < max && blocks.iter().any(|n| n.kind == nodes::HEADING && n.attr_i64("level").is_some_and(|l| l > level && l <= max)) {
            return split_sections(blocks, level + 1, max);
        }
        return (blocks, vec![]);
    }
    let mut intro = Vec::new();
    let mut sections: Vec<(String, Vec<Node>)> = Vec::new();
    for b in blocks {
        if is_cut(&b) {
            let title = wikicraft_doc_model::text::plain_text(&b).split_whitespace().collect::<Vec<_>>().join(" ");
            sections.push((if title.is_empty() { "Untitled".into() } else { title.chars().take(255).collect() }, vec![]));
        } else if let Some((_, c)) = sections.last_mut() {
            c.push(b);
        } else {
            intro.push(b);
        }
    }
    let out = sections
        .into_iter()
        .map(|(title, content)| {
            let (content, children) = if level < max { split_sections(content, level + 1, max) } else { (content, vec![]) };
            Section { title, content, children }
        })
        .collect();
    (intro, out)
}

fn sections_fallback(intro: Vec<Node>) -> Vec<Node> {
    if intro.is_empty() { vec![Node::paragraph(vec![])] } else { intro }
}

/// Adds sections as Word "files" in a folder `dir`: `dir/01 Title.docx` (+ `dir/01 Title/` for
/// their own sections), numbered so they keep the document's order.
fn add_sections(files: &mut Vec<(String, Bytes)>, word: &mut HashMap<String, (Option<String>, Node)>, dir: &str, sections: Vec<Section>) {
    for (i, s) in sections.into_iter().enumerate().take(500) {
        let safe: String = s
            .title
            .chars()
            .map(|c| if matches!(c, '/' | '\\' | ':' | '*' | '?' | '"' | '<' | '>' | '|') || c.is_control() { ' ' } else { c })
            .take(80)
            .collect();
        let stem = format!("{dir}/{:03} {}", i + 1, safe.trim());
        let name = format!("{stem}.docx");
        files.push((name.clone(), Bytes::new()));
        word.insert(name, (Some(s.title), Node::doc(sections_fallback(s.content))));
        if !s.children.is_empty() {
            add_sections(files, word, &stem, s.children);
        }
    }
}

fn normalize_path(p: &str) -> String {
    let mut out: Vec<&str> = Vec::new();
    for seg in p.split('/') {
        match seg {
            "" | "." => {}
            ".." => {
                out.pop();
            }
            s => out.push(s),
        }
    }
    out.join("/")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn notion_names_and_csv() {
        assert_eq!(clean_name("Project plan 0123456789abcdef0123456789abcdef"), "Project plan");
        assert_eq!(clean_name("Plain name"), "Plain name");
        assert_eq!(clean_name("0123456789abcdef0123456789abcdef"), "0123456789abcdef0123456789abcdef");
        let rows = parse_csv("\u{feff}Name,Status,Notes\r\nAlpha,Done,\"multi\nline, with comma\"\r\n\"Quote \"\"x\"\"\",Open,\n\n");
        assert_eq!(rows.len(), 3);
        assert_eq!(rows[1][2], "multi\nline, with comma");
        assert_eq!(rows[2][0], "Quote \"x\"");
        assert_eq!(rows[2][2], "");
    }

    #[test]
    fn notion_database_columns() {
        assert_eq!(column_kind(["3", "4.5", ""].into_iter()), "number");
        assert_eq!(column_kind(["December 1, 2026", "2026/12/03", "January 5, 2027 3:00 PM"].into_iter()), "date");
        assert_eq!(column_kind(["https://a.example", "http://b.example/x"].into_iter()), "url");
        assert_eq!(column_kind(["Active", "Paused", "Active"].into_iter()), "select");
        assert_eq!(column_kind(["Active", "Paused"].into_iter()), "text", "no value repeats");
        assert_eq!(column_kind(["Ben, Bo", "Ann", "Ann"].into_iter()), "text", "multi-select lists stay text");
        assert_eq!(notion_date("December 1, 2026 → December 5, 2026"), chrono::NaiveDate::from_ymd_opt(2026, 12, 1));
        let keys = vec!["Status".to_string(), "Owner".to_string(), "Due date".to_string()];
        assert!(is_property_lines("Status: Active Owner: Ann Lee Due date: December 1, 2026", &keys));
        assert!(is_property_lines("Owner:", &keys));
        assert!(!is_property_lines("Status: Active and then some prose. Details: more", &keys[1..]));
        assert!(!is_property_lines("Details follow.", &keys));
    }

    #[test]
    fn decoding_and_front_matter() {
        assert_eq!(percent_decode("Setup-%20step%201-2.md"), "Setup- step 1-2.md");
        assert_eq!(percent_decode("%C3%A4rger%"), "ärger%");
        assert_eq!(percent_decode("ä%2"), "ä%2");
        let (t, l) = front_matter("title: \"A: b/c\"\nlabels: [\"x\", \"y\"]\nother: 1");
        assert_eq!(t.as_deref(), Some("A: b/c"));
        assert_eq!(l, vec!["x", "y"]);
        let (t, l) = front_matter("title: Plain title\ntags: a, b");
        assert_eq!(t.as_deref(), Some("Plain title"));
        assert_eq!(l, vec!["a", "b"]);
    }
}
