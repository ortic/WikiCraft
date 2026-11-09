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
use crate::authz::{Principal, Role};
use crate::error::{ApiError, ApiResult};
use crate::importer::{self, ImportOptions};
use crate::routes::pages::{NewPage, insert_page};
use crate::routes::spaces::load_space;

/// Streams the uploaded file field to disk. Returns (path, other text fields).
async fn receive_upload(state: &AppState, mp: &mut Multipart) -> ApiResult<(PathBuf, HashMap<String, String>, Option<String>)> {
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
}

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

pub async fn start_confluence_import(state: &AppState, user: &User, path: PathBuf, opts: ImportOptions) -> ApiResult<Uuid> {
    let job = new_id();
    sqlx::query("INSERT INTO jobs (id, workspace_id, kind, created_by, progress) VALUES ($1, $2, 'import.confluence', $3, $4)")
        .bind(job)
        .bind(user.workspace_id)
        .bind(user.id)
        .bind(json!({"phase": "queued"}))
        .execute(&state.db)
        .await?;
    tokio::spawn(importer::run(state.clone(), job, user.workspace_id, user.id, path, opts));
    Ok(job)
}

/// `POST /api/imports/confluence/analyze` — dry run, nothing is written.
pub async fn analyze_confluence(State(state): State<AppState>, CurrentUser(user): CurrentUser, mut mp: Multipart) -> ApiResult<Json<Value>> {
    require_admin(&user)?;
    let (path, _, _) = receive_upload(&state, &mut mp).await?;
    let p = path.clone();
    let res = tokio::task::spawn_blocking(move || importer::analyze(&p)).await.map_err(|e| ApiError::Internal(e.into()))?;
    let _ = tokio::fs::remove_file(&path).await;
    res.map(Json).map_err(|e| ApiError::bad(format!("could not read export: {e:#}")))
}

pub async fn list_jobs(State(state): State<AppState>, CurrentUser(user): CurrentUser) -> ApiResult<Json<Vec<JobRow>>> {
    require_admin(&user)?;
    Ok(Json(
        sqlx::query_as::<_, JobRow>("SELECT id, kind, status, progress, NULL::jsonb AS result, error, created_at, started_at, finished_at FROM jobs WHERE workspace_id = $1 ORDER BY created_at DESC LIMIT 100")
            .bind(user.workspace_id)
            .fetch_all(&state.db)
            .await?,
    ))
}

pub async fn get_job(State(state): State<AppState>, CurrentUser(user): CurrentUser, Path(id): Path<Uuid>) -> ApiResult<Json<JobRow>> {
    let job = sqlx::query_as::<_, JobRow>("SELECT id, kind, status, progress, result, error, created_at, started_at, finished_at FROM jobs WHERE id = $1 AND workspace_id = $2 AND (created_by = $3 OR $4)")
        .bind(id)
        .bind(user.workspace_id)
        .bind(user.id)
        .bind(user.is_admin())
        .fetch_optional(&state.db)
        .await?
        .ok_or(ApiError::NotFound("job"))?;
    Ok(Json(job))
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

async fn import_markdown(state: &AppState, user: &User, path: &std::path::Path, fields: &HashMap<String, String>, filename: &str) -> ApiResult<Value> {
    let p = Principal::load(state, Some(user.clone())).await?;
    let key = fields.get("spaceKey").ok_or(ApiError::bad("spaceKey is required"))?;
    let (space, role) = load_space(state, &p, key).await?;
    if role < Role::Editor {
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
            const MAX_TOTAL: u64 = 1024 * 1024 * 1024;
            let mut out = Vec::new();
            let mut total = 0u64;
            if zip.len() > MAX_FILES * 4 {
                anyhow::bail!("too many entries in zip");
            }
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
                out.push((name, Bytes::from(buf)));
            }
            Ok(out)
        })
        .await
        .map_err(|e| ApiError::Internal(e.into()))?
        .map_err(|e| ApiError::bad(format!("invalid zip: {e}")))?;
    } else {
        files.push((filename.to_string(), Bytes::from(tokio::fs::read(path).await.map_err(|e| ApiError::Internal(e.into()))?)));
    }
    files.sort_by(|a, b| a.0.cmp(&b.0));
    let is_md = |n: &str| n.to_lowercase().ends_with(".md") || n.to_lowercase().ends_with(".markdown");
    let assets: HashMap<String, Bytes> = files.iter().filter(|(n, _)| !is_md(n)).cloned().collect();

    // folder pages first (sorted paths guarantee parents first)
    let mut folder_pages: HashMap<String, Uuid> = HashMap::new();
    let mut created = 0;
    let mut title_index: HashMap<String, Uuid> = HashMap::new();
    let mut pending_docs: Vec<(Uuid, String, Node)> = Vec::new();
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
            },
        )
        .await?;
        *created += 1;
        folders.insert(dir.to_string(), id);
        Ok(Some(id))
    }
    for (name, data) in files.iter().filter(|(n, _)| is_md(n)) {
        let (dir, file) = name.rsplit_once('/').unwrap_or(("", name.as_str()));
        let stem = file.rsplit_once('.').map(|(s, _)| s).unwrap_or(file);
        let src = String::from_utf8_lossy(data);
        let imported = markdown::markdown_to_pm(&src);
        let is_index = matches!(stem.to_lowercase().as_str(), "index" | "readme") && !dir.is_empty();
        let title = imported.title.clone().unwrap_or_else(|| stem.replace(['-', '_'], " "));
        let mut doc = imported.doc;
        // drop the H1 used as the title
        if doc.content.first().is_some_and(|n| n.kind == nodes::HEADING && n.attr_i64("level") == Some(1)) && imported.title.is_some() {
            doc.content.remove(0);
            if doc.content.is_empty() {
                doc.content.push(Node::paragraph(vec![]));
            }
        }
        let id = if is_index {
            let folder =
                ensure_folder(state, user, space.id, root_parent, &mut folder_pages, dir, &mut pos, &mut created).await?.ok_or(ApiError::bad("bad folder"))?;
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
                },
            )
            .await?;
            created += 1;
            id
        };
        title_index.insert(title.to_lowercase(), id);
        title_index.insert(stem.to_lowercase(), id);
        pending_docs.push((id, dir.to_string(), doc));
    }
    // second pass: images → attachments, wiki links → page links
    let mut attachments = 0;
    for (id, dir, mut doc) in pending_docs {
        let mut images: Vec<String> = Vec::new();
        doc.walk(&mut |n| {
            if n.kind == nodes::IMAGE
                && let Some(src) = n.attr_str("src").filter(|s| !s.contains("://") && !s.starts_with('/') && !s.starts_with("data:"))
            {
                images.push(src.to_string());
            }
        });
        let mut urls: HashMap<String, String> = HashMap::new();
        for src in images {
            let decoded = src.replace("%20", " ");
            let joined = normalize_path(&if dir.is_empty() { decoded.clone() } else { format!("{dir}/{decoded}") });
            let data = assets.get(&joined).or_else(|| assets.iter().find(|(k, _)| k.ends_with(&format!("/{decoded}")) || **k == decoded).map(|(_, v)| v));
            if let Some(data) = data {
                let fname = decoded.rsplit('/').next().unwrap_or(&decoded).to_string();
                let att = crate::routes::attachments::store_attachment(state, user.workspace_id, id, Some(user.id), &fname, data.clone(), None, None).await?;
                attachments += 1;
                urls.insert(src.clone(), format!("/api/attachments/{att}/content"));
            }
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
                    let target = href.strip_prefix("wikilink:").map(String::from).or_else(|| {
                        (!href.contains("://") && (href.ends_with(".md") || href.contains(".md#")))
                            .then(|| href.rsplit('/').next().unwrap_or(&href).split(".md").next().unwrap_or("").replace("%20", " "))
                    });
                    if let Some(t) = target {
                        let (t, anchor) = t.split_once('#').map(|(a, b)| (a.to_string(), Some(b.to_string()))).unwrap_or((t, None));
                        let new = match title_index.get(&t.to_lowercase()) {
                            Some(pid) => format!("/pages/{pid}{}", anchor.map(|a| format!("#{a}")).unwrap_or_default()),
                            None => format!("/missing?space={}&title={}", space.key, t),
                        };
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
    Ok(json!({ "pages": created, "attachments": attachments }))
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
