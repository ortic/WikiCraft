use std::collections::HashMap;

use axum::Json;
use axum::extract::{Path, Query, State};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use uuid::Uuid;

use crate::app::AppState;
use crate::auth::{CurrentUser, MaybeUser};
use crate::authz::{Principal, Role, space_role, visible_page_ids};
use crate::error::{ApiError, ApiResult};
use crate::routes::spaces::{SPACE_COLUMNS, SpaceRow, workspace_of};

/// Spaces visible to the principal with their roles.
pub async fn visible_spaces(state: &AppState, p: &Principal) -> ApiResult<Vec<(SpaceRow, Role)>> {
    let ws = workspace_of(state, p).await?;
    let spaces = sqlx::query_as::<_, SpaceRow>(sqlx::AssertSqlSafe(format!("SELECT {SPACE_COLUMNS} FROM spaces WHERE workspace_id = $1")))
        .bind(ws)
        .fetch_all(&state.db)
        .await?;
    let mut out = Vec::new();
    for s in spaces {
        let r = space_role(state, p, &s.access_row()).await?;
        if r > Role::None {
            out.push((s, r));
        }
    }
    Ok(out)
}

/// Filters page ids down to the ones the principal may view (inherited restrictions).
pub async fn filter_visible(
    state: &AppState,
    p: &Principal,
    spaces: &[(SpaceRow, Role)],
    pages: &[(Uuid, Uuid)],
) -> ApiResult<std::collections::HashSet<Uuid>> {
    let mut by_space: HashMap<Uuid, Vec<Uuid>> = HashMap::new();
    for (page, space) in pages {
        by_space.entry(*space).or_default().push(*page);
    }
    let roles: HashMap<Uuid, Role> = spaces.iter().map(|(s, r)| (s.id, *r)).collect();
    let mut out = std::collections::HashSet::new();
    for (space, ids) in by_space {
        let role = roles.get(&space).copied().unwrap_or(Role::None);
        if role == Role::None {
            continue;
        }
        let (any_restrictions, any_drafts): (bool, bool) = sqlx::query_as(
            "SELECT EXISTS (SELECT 1 FROM page_restrictions r JOIN pages p ON p.id = r.page_id WHERE p.space_id = $1 AND r.kind = 'view'),
                    EXISTS (SELECT 1 FROM pages WHERE space_id = $1 AND draft AND created_by IS DISTINCT FROM $2)",
        )
        .bind(space)
        .bind(p.user_id())
        .fetch_one(&state.db)
        .await?;
        if !any_drafts && (!any_restrictions || role == Role::Admin) {
            out.extend(ids);
            continue;
        }
        let pairs: Vec<(Uuid, Option<Uuid>)> = sqlx::query_as("SELECT id, parent_id FROM pages WHERE space_id = $1").bind(space).fetch_all(&state.db).await?;
        let vis = visible_page_ids(state, p, space, role, &pairs).await?;
        out.extend(ids.into_iter().filter(|i| vis.contains(i)));
    }
    Ok(out)
}

#[derive(Deserialize)]
pub struct SearchQuery {
    pub q: String,
    pub space: Option<String>,
    pub limit: Option<usize>,
    /// `page` | `blog`
    pub kind: Option<String>,
    /// a label the page must have
    pub label: Option<String>,
    /// a user id: pages they created or last updated
    pub contributor: Option<Uuid>,
    /// modified within the last N days
    pub days: Option<i64>,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SearchResult {
    pub page_id: Uuid,
    pub title: String,
    pub snippet: String,
    pub space_key: String,
    pub space_name: String,
    pub score: f32,
}

pub async fn search(State(state): State<AppState>, MaybeUser(user): MaybeUser, Query(q): Query<SearchQuery>) -> ApiResult<Json<Vec<SearchResult>>> {
    let p = Principal::load(&state, user).await?;
    let limit = q.limit.unwrap_or(20).clamp(1, 100);
    let filtered = q.kind.is_some() || q.label.is_some() || q.contributor.is_some() || q.days.is_some();
    if !filtered {
        return Ok(Json(visible_hits(&state, &p, &q.q, q.space.as_deref(), limit).await?));
    }
    // filters narrow the ranked hits: look further down the ranking first
    let hits = visible_hits(&state, &p, &q.q, q.space.as_deref(), 100).await?;
    let ids: Vec<Uuid> = hits.iter().map(|h| h.page_id).collect();
    let label = q.label.as_deref().and_then(crate::routes::pages::normalize_label);
    let keep: std::collections::HashSet<Uuid> = sqlx::query_scalar(
        "SELECT p.id FROM pages p WHERE p.id = ANY($1)
           AND ($2::text IS NULL OR p.kind = $2)
           AND ($3::text IS NULL OR EXISTS (SELECT 1 FROM labels l WHERE l.page_id = p.id AND l.name = $3))
           AND ($4::uuid IS NULL OR p.created_by = $4 OR p.updated_by = $4 OR EXISTS (SELECT 1 FROM page_versions v WHERE v.page_id = p.id AND v.created_by = $4))
           AND ($5::int IS NULL OR p.updated_at >= now() - make_interval(days => $5))",
    )
    .bind(&ids)
    .bind(q.kind.as_deref().filter(|k| matches!(*k, "page" | "blog")))
    .bind(label)
    .bind(q.contributor)
    .bind(q.days.map(|d| d.clamp(1, 3650) as i32))
    .fetch_all(&state.db)
    .await?
    .into_iter()
    .collect();
    Ok(Json(hits.into_iter().filter(|h| keep.contains(&h.page_id)).take(limit).collect()))
}

/// Full-text hits for `query` among the pages `p` may see (optionally in one space).
pub async fn visible_hits(state: &AppState, p: &Principal, query: &str, space: Option<&str>, limit: usize) -> ApiResult<Vec<SearchResult>> {
    let mut spaces = visible_spaces(state, p).await?;
    // archived spaces stay readable but drop out of search
    spaces.retain(|(s, _)| s.archived_at.is_none());
    if let Some(k) = space {
        spaces.retain(|(s, _)| s.key.eq_ignore_ascii_case(k));
    }
    let ids: Vec<Uuid> = spaces.iter().map(|(s, _)| s.id).collect();
    let limit = limit.clamp(1, 100);
    let mut hits = state.search.search(query, &ids, limit * 2).await.map_err(ApiError::Internal)?;
    // the index may lag behind moves/deletes: use the database's current space and state
    let hit_ids: Vec<Uuid> = hits.iter().map(|h| h.page_id).collect();
    let current: std::collections::HashMap<Uuid, Uuid> =
        sqlx::query_as::<_, (Uuid, Uuid)>("SELECT id, space_id FROM pages WHERE id = ANY($1) AND deleted_at IS NULL")
            .bind(&hit_ids)
            .fetch_all(&state.db)
            .await?
            .into_iter()
            .collect();
    hits.retain_mut(|h| match current.get(&h.page_id) {
        Some(sp) => {
            h.space_id = *sp;
            true
        }
        None => false,
    });
    // archived pages (and what's under them) stay readable but leave search
    let archived = crate::authz::archived_among(state, &hits.iter().map(|h| h.page_id).collect::<Vec<_>>()).await?;
    hits.retain(|h| !archived.contains(&h.page_id));
    let pairs: Vec<(Uuid, Uuid)> = hits.iter().map(|h| (h.page_id, h.space_id)).collect();
    let visible = filter_visible(state, p, &spaces, &pairs).await?;
    let by_id: HashMap<Uuid, &SpaceRow> = spaces.iter().map(|(s, _)| (s.id, s)).collect();
    let out = hits
        .into_iter()
        .filter(|h| visible.contains(&h.page_id))
        .take(limit)
        .map(|h| {
            let s = by_id.get(&h.space_id);
            SearchResult {
                page_id: h.page_id,
                title: h.title,
                snippet: h.snippet,
                space_key: s.map(|s| s.key.clone()).unwrap_or_default(),
                space_name: s.map(|s| s.name.clone()).unwrap_or_default(),
                score: h.score,
            }
        })
        .collect();
    Ok(out)
}

#[derive(Serialize, sqlx::FromRow)]
#[serde(rename_all = "camelCase")]
pub struct PageListItem {
    id: Uuid,
    title: String,
    space_id: Uuid,
    space_key: String,
    space_name: String,
    updated_at: chrono::DateTime<chrono::Utc>,
    updated_by_name: Option<String>,
    at: Option<chrono::DateTime<chrono::Utc>>,
}

async fn filtered(state: &AppState, p: &Principal, rows: Vec<PageListItem>) -> ApiResult<Vec<PageListItem>> {
    let spaces = visible_spaces(state, p).await?;
    let pairs: Vec<(Uuid, Uuid)> = rows.iter().map(|r| (r.id, r.space_id)).collect();
    let visible = filter_visible(state, p, &spaces, &pairs).await?;
    Ok(rows.into_iter().filter(|r| visible.contains(&r.id)).collect())
}

const LIST_SELECT: &str = "SELECT p.id, p.title, p.space_id, s.key AS space_key, s.name AS space_name, p.updated_at, u.name AS updated_by_name";

#[derive(Serialize, sqlx::FromRow)]
#[serde(rename_all = "camelCase")]
pub struct PopularItem {
    #[serde(flatten)]
    #[sqlx(flatten)]
    page: PageListItem,
    views: i64,
    viewers: i64,
}

/// `GET /api/popular?days=30`: the most viewed pages the caller can see (people, not bots:
/// signed-in viewers only), excluding archived spaces and pages.
pub async fn popular(State(state): State<AppState>, CurrentUser(user): CurrentUser, Query(q): Query<PopularQuery>) -> ApiResult<Json<Vec<PopularItem>>> {
    let p = Principal::load(&state, Some(user.clone())).await?;
    let days = q.days.unwrap_or(30).clamp(1, 365);
    let rows = sqlx::query_as::<_, PopularItem>(sqlx::AssertSqlSafe(format!(
        "{LIST_SELECT}, NULL::timestamptz AS at, v.views, v.viewers
         FROM (SELECT page_id, sum(views)::bigint AS views, count(DISTINCT user_id) AS viewers FROM page_views
               WHERE day >= current_date - $2::int AND user_id IS NOT NULL GROUP BY page_id) v
         JOIN pages p ON p.id = v.page_id JOIN spaces s ON s.id = p.space_id LEFT JOIN users u ON u.id = p.updated_by
         WHERE p.workspace_id = $1 AND p.deleted_at IS NULL AND p.archived_at IS NULL AND s.archived_at IS NULL
         ORDER BY v.viewers DESC, v.views DESC LIMIT 80"
    )))
    .bind(user.workspace_id)
    .bind(days as i32)
    .fetch_all(&state.db)
    .await?;
    let spaces = visible_spaces(&state, &p).await?;
    let pairs: Vec<(Uuid, Uuid)> = rows.iter().map(|r| (r.page.id, r.page.space_id)).collect();
    let visible = filter_visible(&state, &p, &spaces, &pairs).await?;
    let archived = crate::authz::archived_among(&state, &rows.iter().map(|r| r.page.id).collect::<Vec<_>>()).await?;
    Ok(Json(rows.into_iter().filter(|r| visible.contains(&r.page.id) && !archived.contains(&r.page.id)).take(20).collect()))
}

#[derive(Deserialize)]
pub struct PopularQuery {
    days: Option<i64>,
}

/// Recently viewed pages of the user plus recently updated pages in the workspace.
pub async fn recent(State(state): State<AppState>, CurrentUser(user): CurrentUser) -> ApiResult<Json<Value>> {
    let p = Principal::load(&state, Some(user.clone())).await?;
    let viewed = sqlx::query_as::<_, PageListItem>(sqlx::AssertSqlSafe(format!(
        "{LIST_SELECT}, rv.viewed_at AS at FROM recent_views rv JOIN pages p ON p.id = rv.page_id JOIN spaces s ON s.id = p.space_id LEFT JOIN users u ON u.id = p.updated_by
         WHERE rv.user_id = $1 AND p.deleted_at IS NULL ORDER BY rv.viewed_at DESC LIMIT 20"
    )))
    .bind(user.id)
    .fetch_all(&state.db)
    .await?;
    let updated = sqlx::query_as::<_, PageListItem>(sqlx::AssertSqlSafe(format!(
        "{LIST_SELECT}, p.updated_at AS at FROM pages p JOIN spaces s ON s.id = p.space_id LEFT JOIN users u ON u.id = p.updated_by
         WHERE p.workspace_id = $1 AND p.deleted_at IS NULL AND s.archived_at IS NULL ORDER BY p.updated_at DESC LIMIT 60"
    )))
    .bind(user.workspace_id)
    .fetch_all(&state.db)
    .await?;
    let viewed = filtered(&state, &p, viewed).await?;
    let mut updated = filtered(&state, &p, updated).await?;
    updated.truncate(20);
    Ok(Json(json!({ "viewed": viewed, "updated": updated })))
}

pub async fn favorites(State(state): State<AppState>, CurrentUser(user): CurrentUser) -> ApiResult<Json<Vec<PageListItem>>> {
    let p = Principal::load(&state, Some(user.clone())).await?;
    let rows = sqlx::query_as::<_, PageListItem>(sqlx::AssertSqlSafe(format!(
        "{LIST_SELECT}, f.created_at AS at FROM favorites f JOIN pages p ON p.id = f.page_id JOIN spaces s ON s.id = p.space_id LEFT JOIN users u ON u.id = p.updated_by
         WHERE f.user_id = $1 AND p.deleted_at IS NULL ORDER BY f.created_at DESC"
    )))
    .bind(user.id)
    .fetch_all(&state.db)
    .await?;
    Ok(Json(filtered(&state, &p, rows).await?))
}

pub async fn by_label(State(state): State<AppState>, MaybeUser(user): MaybeUser, Path(name): Path<String>) -> ApiResult<Json<Vec<PageListItem>>> {
    let p = Principal::load(&state, user).await?;
    let ws = workspace_of(&state, &p).await?;
    let rows = sqlx::query_as::<_, PageListItem>(sqlx::AssertSqlSafe(format!(
        "{LIST_SELECT}, NULL::timestamptz AS at FROM labels l JOIN pages p ON p.id = l.page_id JOIN spaces s ON s.id = p.space_id LEFT JOIN users u ON u.id = p.updated_by
         WHERE l.name = $1 AND p.workspace_id = $2 AND p.deleted_at IS NULL ORDER BY lower(p.title) LIMIT 500"
    )))
    .bind(name.to_lowercase())
    .bind(ws)
    .fetch_all(&state.db)
    .await?;
    Ok(Json(filtered(&state, &p, rows).await?))
}

#[derive(Deserialize)]
pub struct ResolveQuery {
    /// space key + title (missing-page links from imports, wiki links)
    space: Option<String>,
    title: Option<String>,
    /// legacy Confluence page id (old URL redirects): `confluence:<id>`
    source: Option<String>,
}

/// Resolves a page by (space, title) or by imported source id.
pub async fn resolve(State(state): State<AppState>, MaybeUser(user): MaybeUser, Query(q): Query<ResolveQuery>) -> ApiResult<Json<Value>> {
    let p = Principal::load(&state, user).await?;
    let ws = workspace_of(&state, &p).await?;
    let id: Option<Uuid> = if let Some(src) = &q.source {
        let (system, sid) = src.split_once(':').ok_or(ApiError::bad("source must look like confluence:<id>"))?;
        sqlx::query_scalar("SELECT target_id FROM import_mappings WHERE workspace_id = $1 AND source = $2 AND source_type = 'page' AND source_id = $3")
            .bind(ws)
            .bind(system)
            .bind(sid)
            .fetch_optional(&state.db)
            .await?
    } else if let (Some(space), Some(title)) = (&q.space, &q.title) {
        sqlx::query_scalar("SELECT p.id FROM pages p JOIN spaces s ON s.id = p.space_id WHERE s.workspace_id = $1 AND lower(s.key) = lower($2) AND lower(p.title) = lower($3) AND p.deleted_at IS NULL ORDER BY p.created_at LIMIT 1")
            .bind(ws)
            .bind(space)
            .bind(title)
            .fetch_optional(&state.db)
            .await?
    } else {
        return Err(ApiError::bad("space+title or source required"));
    };
    let Some(id) = id else { return Ok(Json(json!({ "pageId": null }))) };
    let (access, _) = crate::authz::page_access(&state, &p, id).await?;
    Ok(Json(json!({ "pageId": if access.view { Some(id) } else { None } })))
}

#[derive(Deserialize)]
pub struct SaveSearch {
    name: String,
    /// the search page's query string, e.g. `q=release&space=ENG&days=30`
    params: String,
}

/// `GET /api/me/searches`
pub async fn saved_searches(State(state): State<AppState>, CurrentUser(user): CurrentUser) -> ApiResult<Json<Value>> {
    let rows: Vec<(Uuid, String, String)> =
        sqlx::query_as("SELECT id, name, params FROM saved_searches WHERE user_id = $1 ORDER BY created_at").bind(user.id).fetch_all(&state.db).await?;
    Ok(Json(json!(rows.into_iter().map(|(id, name, params)| json!({ "id": id, "name": name, "params": params })).collect::<Vec<_>>())))
}

/// `POST /api/me/searches {name, params}`
pub async fn save_search(
    State(state): State<AppState>,
    CurrentUser(user): CurrentUser,
    Json(req): Json<SaveSearch>,
) -> ApiResult<(axum::http::StatusCode, Json<Value>)> {
    let name = req.name.trim();
    if name.is_empty() || name.chars().count() > 120 || req.params.len() > 2000 {
        return Err(crate::error::ApiError::bad("give the search a name (and keep it short)"));
    }
    // only the search page's own parameters
    let params: String = req
        .params
        .trim_start_matches('?')
        .split('&')
        .filter(|kv| matches!(kv.split('=').next(), Some("q" | "space" | "kind" | "days" | "label" | "mine")))
        .collect::<Vec<_>>()
        .join("&");
    if !params.split('&').any(|kv| kv.starts_with("q=") && kv.len() > 2) {
        return Err(crate::error::ApiError::bad("a saved search needs a query"));
    }
    let count: i64 = sqlx::query_scalar("SELECT count(*) FROM saved_searches WHERE user_id = $1").bind(user.id).fetch_one(&state.db).await?;
    if count >= 50 {
        return Err(crate::error::ApiError::bad("you can keep up to 50 saved searches"));
    }
    let id = crate::app::new_id();
    sqlx::query("INSERT INTO saved_searches (id, user_id, name, params) VALUES ($1, $2, $3, $4)")
        .bind(id)
        .bind(user.id)
        .bind(name)
        .bind(&params)
        .execute(&state.db)
        .await?;
    Ok((axum::http::StatusCode::CREATED, Json(json!({ "id": id, "name": name, "params": params }))))
}

/// `DELETE /api/me/searches/{id}`
pub async fn delete_search(State(state): State<AppState>, CurrentUser(user): CurrentUser, Path(id): Path<Uuid>) -> ApiResult<axum::http::StatusCode> {
    sqlx::query("DELETE FROM saved_searches WHERE id = $1 AND user_id = $2").bind(id).bind(user.id).execute(&state.db).await?;
    Ok(axum::http::StatusCode::NO_CONTENT)
}

#[derive(Deserialize)]
pub struct LabelSuggestQuery {
    q: Option<String>,
}

/// `GET /labels?q=` — label names for autocomplete: the most used ones starting with `q`,
/// counted over the pages the caller can see (labels of restricted pages don't leak).
pub async fn suggest_labels(State(state): State<AppState>, MaybeUser(user): MaybeUser, Query(q): Query<LabelSuggestQuery>) -> ApiResult<Json<Vec<Value>>> {
    let p = Principal::load(&state, user).await?;
    let ws = workspace_of(&state, &p).await?;
    let prefix: String = q.q.unwrap_or_default().trim().to_lowercase().chars().filter(|c| !matches!(c, '%' | '_' | '\\')).take(60).collect();
    let rows: Vec<(String, Uuid, Uuid)> = sqlx::query_as(
        "SELECT l.name, p.id, p.space_id FROM labels l JOIN pages p ON p.id = l.page_id
          WHERE p.workspace_id = $1 AND p.deleted_at IS NULL AND l.name LIKE $2 ORDER BY l.name LIMIT 3000",
    )
    .bind(ws)
    .bind(format!("{prefix}%"))
    .fetch_all(&state.db)
    .await?;
    let spaces = visible_spaces(&state, &p).await?;
    let pairs: Vec<(Uuid, Uuid)> = rows.iter().map(|r| (r.1, r.2)).collect();
    let visible = filter_visible(&state, &p, &spaces, &pairs).await?;
    let mut counts: HashMap<String, i64> = HashMap::new();
    for (name, id, _) in rows {
        if visible.contains(&id) {
            *counts.entry(name).or_default() += 1;
        }
    }
    let mut out: Vec<(String, i64)> = counts.into_iter().collect();
    out.sort_by(|a, b| b.1.cmp(&a.1).then_with(|| a.0.cmp(&b.0)));
    Ok(Json(out.into_iter().take(10).map(|(name, count)| json!({"name": name, "count": count})).collect()))
}
