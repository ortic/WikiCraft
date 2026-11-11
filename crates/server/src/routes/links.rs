//! The link graph ([`crate::links`]) for readers: the pages that link to a page ("Linked from"),
//! and a space's broken links and orphaned pages. Every list is filtered by what the caller
//! may see.

use std::collections::HashSet;

use axum::Json;
use axum::extract::{Path, State};
use serde::Serialize;
use uuid::Uuid;

use crate::app::AppState;
use crate::auth::MaybeUser;
use crate::authz::{Need, Principal, require_page, visible_page_ids};
use crate::error::ApiResult;
use crate::routes::search::{filter_visible, visible_spaces};
use crate::routes::spaces::load_space;

/// rows a report lists at most
const REPORT_LIMIT: usize = 1000;

/// A link to a page by name (`l`: `/wiki/KEY/slug`, `/display/KEY/Title`,
/// `/missing?space=KEY&title=T`) that leads to the page `t` ([`crate::paths`]); the slug test
/// narrows the candidates down by index before resolving.
const NAMED_LEADS_TO_T: &str = "l.missing_title IS NOT NULL
    AND wc_slug(l.missing_title) IN (SELECT wc_slug(t.title) UNION ALL SELECT h.slug FROM page_paths h WHERE h.page_id = t.id)
    AND wc_resolve(wc_space_by_key(t.workspace_id, l.missing_space), l.missing_title) = t.id";

/// A `/missing?source=…` link (`l`) that now leads to the page `t` (a later import brought it in).
const SOURCE_LEADS_TO_T: &str = "l.missing_source IS NOT NULL AND EXISTS (SELECT 1 FROM import_mappings m
    WHERE m.target_id = t.id AND m.source_type = 'page' AND m.source || ':' || m.source_id = l.missing_source)";

#[derive(Serialize, sqlx::FromRow)]
#[serde(rename_all = "camelCase")]
pub struct Backlink {
    id: Uuid,
    title: String,
    icon: Option<String>,
    kind: String,
    #[serde(skip)]
    space_id: Uuid,
    space_key: String,
    space_name: String,
    /// how it links: `link`, `include` or `property`
    via: String,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Backlinks {
    pages: Vec<Backlink>,
}

/// `GET /api/pages/{id}/backlinks` — the pages that link to this one (links, included pages,
/// page-link properties) and that the caller may see.
pub async fn backlinks(State(state): State<AppState>, MaybeUser(user): MaybeUser, Path(id): Path<Uuid>) -> ApiResult<Json<Backlinks>> {
    let p = Principal::load(&state, user).await?;
    require_page(&state, &p, id, Need::View).await?;
    let rows: Vec<Backlink> = sqlx::query_as(sqlx::AssertSqlSafe(format!(
        "SELECT DISTINCT ON (src.id) src.id, src.title, src.icon, src.kind, src.space_id, s.key AS space_key, s.name AS space_name, l.kind AS via
         FROM pages t
         JOIN page_links l ON l.kind <> 'children' AND l.source_id <> t.id
              AND (l.target_id = t.id OR ({NAMED_LEADS_TO_T}) OR ({SOURCE_LEADS_TO_T}))
         JOIN pages src ON src.id = l.source_id AND src.deleted_at IS NULL
         JOIN spaces s ON s.id = src.space_id
         WHERE t.id = $1
         ORDER BY src.id, CASE l.kind WHEN 'link' THEN 0 WHEN 'include' THEN 1 ELSE 2 END
         LIMIT 2000"
    )))
    .bind(id)
    .fetch_all(&state.db)
    .await?;
    let spaces = visible_spaces(&state, &p).await?;
    let pairs: Vec<(Uuid, Uuid)> = rows.iter().map(|r| (r.id, r.space_id)).collect();
    let visible = filter_visible(&state, &p, &spaces, &pairs).await?;
    let mut pages: Vec<Backlink> = rows.into_iter().filter(|r| visible.contains(&r.id)).collect();
    pages.sort_by(|a, b| a.space_name.to_lowercase().cmp(&b.space_name.to_lowercase()).then_with(|| a.title.to_lowercase().cmp(&b.title.to_lowercase())));
    Ok(Json(Backlinks { pages }))
}

#[derive(Serialize, sqlx::FromRow)]
#[serde(rename_all = "camelCase")]
pub struct BrokenLink {
    page_id: Uuid,
    page_title: String,
    block_id: Option<String>,
    /// the link's text (or the property's name)
    label: String,
    /// `link`, `include` or `property`
    via: String,
    /// `trashed` (the page is in the trash), `deleted` (deleted for good), `missing` (no page
    /// with that title in that space) or `notImported` (a page of another system that no import
    /// brought in)
    problem: String,
    missing_space: Option<String>,
    missing_title: Option<String>,
    missing_source: Option<String>,
}

#[derive(Serialize, sqlx::FromRow)]
#[serde(rename_all = "camelCase")]
pub struct OrphanPage {
    id: Uuid,
    title: String,
    icon: Option<String>,
    parent_id: Option<Uuid>,
    parent_title: Option<String>,
    updated_at: chrono::DateTime<chrono::Utc>,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct LinkReport {
    broken_links: Vec<BrokenLink>,
    /// pages no other page links to (or lists as a child page), apart from the space's home page
    orphans: Vec<OrphanPage>,
    /// a list was cut at the limit
    truncated: bool,
    /// pages of the workspace whose links haven't been read yet (shortly after an upgrade): the
    /// report may be incomplete
    pending: i64,
}

/// `GET /api/spaces/{key}/link-report` — links in the space's pages that lead nowhere, and the
/// pages no other page links to (both limited to the pages the caller may see).
pub async fn report(State(state): State<AppState>, MaybeUser(user): MaybeUser, Path(key): Path<String>) -> ApiResult<Json<LinkReport>> {
    let p = Principal::load(&state, user).await?;
    let (space, role) = load_space(&state, &p, &key).await?;
    let pairs: Vec<(Uuid, Option<Uuid>)> = sqlx::query_as("SELECT id, parent_id FROM pages WHERE space_id = $1").bind(space.id).fetch_all(&state.db).await?;
    let visible = visible_page_ids(&state, &p, space.id, role, &pairs).await?;

    let broken: Vec<BrokenLink> = sqlx::query_as(sqlx::AssertSqlSafe(format!(
        "SELECT * FROM (
            SELECT l.source_id AS page_id, src.title AS page_title, l.block_id, l.label, l.kind AS via,
                   l.missing_space, l.missing_title, l.missing_source, l.ord,
                   CASE
                     WHEN l.target_id IS NOT NULL THEN
                       CASE WHEN NOT EXISTS (SELECT 1 FROM pages t WHERE t.id = l.target_id) THEN 'deleted'
                            WHEN EXISTS (SELECT 1 FROM pages t WHERE t.id = l.target_id AND t.deleted_at IS NOT NULL) THEN 'trashed' END
                     WHEN l.missing_title IS NOT NULL THEN
                       CASE WHEN wc_resolve(wc_space_by_key($2, l.missing_space), l.missing_title) IS NULL THEN 'missing' END
                     ELSE
                       CASE WHEN NOT EXISTS (SELECT 1 FROM pages t WHERE t.deleted_at IS NULL AND t.space_id IN (SELECT id FROM spaces WHERE workspace_id = $2) AND ({SOURCE_LEADS_TO_T}))
                            THEN 'notImported' END
                   END AS problem
            FROM page_links l JOIN pages src ON src.id = l.source_id
            WHERE src.space_id = $1 AND src.deleted_at IS NULL AND l.kind <> 'children'
         ) b WHERE problem IS NOT NULL
         ORDER BY lower(page_title), page_id, ord"
    )))
    .bind(space.id)
    .bind(space.workspace_id)
    .fetch_all(&state.db)
    .await?;
    let mut broken_links: Vec<BrokenLink> = broken.into_iter().filter(|b| visible.contains(&b.page_id)).collect();

    let orphans: Vec<OrphanPage> = sqlx::query_as(sqlx::AssertSqlSafe(format!(
        "SELECT t.id, t.title, t.icon, t.parent_id, par.title AS parent_title, t.updated_at
         FROM pages t LEFT JOIN pages par ON par.id = t.parent_id
         WHERE t.space_id = $1 AND t.deleted_at IS NULL AND t.kind = 'page' AND NOT t.draft AND t.archived_at IS NULL
           AND t.id IS DISTINCT FROM $2
           AND NOT EXISTS (SELECT 1 FROM page_links l JOIN pages src ON src.id = l.source_id AND src.deleted_at IS NULL
                           WHERE l.target_id = t.id AND l.source_id <> t.id)
           AND NOT EXISTS (SELECT 1 FROM page_links l WHERE l.kind = 'children' AND l.source_id = t.parent_id AND l.target_id = t.parent_id)
           AND NOT EXISTS (SELECT 1 FROM page_links l JOIN pages src ON src.id = l.source_id AND src.deleted_at IS NULL
                           WHERE (({NAMED_LEADS_TO_T}) OR ({SOURCE_LEADS_TO_T})) AND l.source_id <> t.id)
         ORDER BY lower(t.title), t.id"
    )))
    .bind(space.id)
    .bind(space.home_page_id)
    .fetch_all(&state.db)
    .await?;
    let archived: HashSet<Uuid> = crate::authz::archived_among(&state, &orphans.iter().map(|o| o.id).collect::<Vec<_>>()).await?;
    let mut orphans: Vec<OrphanPage> = orphans.into_iter().filter(|o| visible.contains(&o.id) && !archived.contains(&o.id)).collect();
    // a parent the caller can't see stays unnamed
    for o in &mut orphans {
        if o.parent_id.is_some_and(|par| !visible.contains(&par)) {
            o.parent_title = None;
        }
    }

    let truncated = broken_links.len() > REPORT_LIMIT || orphans.len() > REPORT_LIMIT;
    broken_links.truncate(REPORT_LIMIT);
    orphans.truncate(REPORT_LIMIT);
    let pending: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM pages WHERE linked_at IS NULL AND deleted_at IS NULL AND space_id IN (SELECT id FROM spaces WHERE workspace_id = $1)",
    )
    .bind(space.workspace_id)
    .fetch_one(&state.db)
    .await?;
    Ok(Json(LinkReport { broken_links, orphans, truncated, pending }))
}
