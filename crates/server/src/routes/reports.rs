//! Live queries behind Confluence's report macros: pages by label (`contentbylabel`),
//! recently updated pages (`recently-updated`), page properties reports (`detailssummary`,
//! `content-report-table`), task reports (`tasks-report-macro`) and decision logs (`decisionreport`). Every result is filtered
//! by what the caller may see.

use std::collections::HashSet;

use axum::Json;
use axum::extract::{Query, State};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use uuid::Uuid;
use wikicraft_doc_model::{Node, nodes, text};

use crate::app::AppState;
use crate::auth::MaybeUser;
use crate::authz::Principal;
use crate::error::{ApiError, ApiResult};
use crate::routes::properties::{self as page_props, Property};
use crate::routes::search::{filter_visible, visible_spaces};
use crate::routes::spaces::workspace_of;

#[derive(Deserialize, Default)]
pub struct ReportQuery {
    /// comma-separated labels
    labels: Option<String>,
    /// comma-separated space keys (empty = all visible spaces)
    spaces: Option<String>,
    /// `all` = every label must match (default: any)
    operator: Option<String>,
    /// `title` | `updated` (default) | `created`
    sort: Option<String>,
    max: Option<i64>,
    /// tasks: `open` (default) | `done` | `all`
    status: Option<String>,
    /// properties: conditions such as `Status = Done; Effort >= 3`
    #[serde(rename = "where")]
    filter: Option<String>,
    /// tasks: comma-separated user ids, `me`, or imported Confluence user keys
    assignees: Option<String>,
    /// tasks due on or after / before this day (YYYY-MM-DD; Confluence's dd-MM-yyyy too)
    #[serde(rename = "dueFrom")]
    due_from: Option<String>,
    #[serde(rename = "dueTo")]
    due_to: Option<String>,
}

fn parse_day(s: &Option<String>) -> ApiResult<Option<chrono::NaiveDate>> {
    let Some(s) = s.as_deref().map(str::trim).filter(|s| !s.is_empty()) else { return Ok(None) };
    ["%Y-%m-%d", "%d-%m-%Y", "%d.%m.%Y"]
        .iter()
        .find_map(|f| chrono::NaiveDate::parse_from_str(s, f).ok())
        .map(Some)
        .ok_or_else(|| ApiError::bad(format!("“{s}” isn't a date (YYYY-MM-DD)")))
}

/// Task assignees from a report filter: user ids, `me` (`@me`, `currentUser()`), or the user
/// keys of imported Confluence accounts. None = no filter.
async fn assignee_ids(state: &AppState, p: &Principal, ws: Uuid, q: &ReportQuery) -> ApiResult<Option<Vec<Uuid>>> {
    let tokens = list(&q.assignees);
    if tokens.is_empty() {
        return Ok(None);
    }
    let mut ids = Vec::new();
    for t in tokens {
        if matches!(t.to_lowercase().as_str(), "me" | "@me" | "currentuser()" | "@self") {
            ids.extend(p.user_id());
        } else if let Ok(id) = t.parse::<Uuid>() {
            ids.push(id);
        } else {
            let found: Option<Uuid> = sqlx::query_scalar("SELECT id FROM users WHERE workspace_id = $1 AND external_ref = $2")
                .bind(ws)
                .bind(format!("confluence:{t}"))
                .fetch_optional(&state.db)
                .await?;
            ids.extend(found);
        }
    }
    Ok(Some(ids))
}

fn list(s: &Option<String>) -> Vec<String> {
    s.as_deref().unwrap_or("").split(',').map(|x| x.trim().trim_matches('"').trim_matches('\'').to_string()).filter(|x| !x.is_empty()).collect()
}

impl ReportQuery {
    fn labels(&self) -> Vec<String> {
        list(&self.labels).into_iter().map(|l| l.to_lowercase()).collect()
    }
    fn spaces(&self) -> Vec<String> {
        list(&self.spaces).into_iter().map(|s| s.to_uppercase()).collect()
    }
    fn max(&self, default: i64) -> i64 {
        self.max.unwrap_or(default).clamp(1, 500)
    }
    fn all_labels(&self) -> bool {
        self.operator.as_deref().is_some_and(|o| o.eq_ignore_ascii_case("all") || o.eq_ignore_ascii_case("and"))
    }
}

#[derive(Serialize, sqlx::FromRow, Clone)]
#[serde(rename_all = "camelCase")]
pub struct ReportPage {
    id: Uuid,
    title: String,
    kind: String,
    space_id: Uuid,
    space_key: String,
    space_name: String,
    created_at: chrono::DateTime<chrono::Utc>,
    updated_at: chrono::DateTime<chrono::Utc>,
    updated_by_name: Option<String>,
    labels: Vec<String>,
    #[serde(skip)]
    content: Value,
}

/// Candidate pages (label / space filters in SQL), then permission filtering.
async fn pages(state: &AppState, p: &Principal, q: &ReportQuery, require_labels: bool, max: i64, with_content: bool) -> ApiResult<Vec<ReportPage>> {
    let ws = workspace_of(state, p).await?;
    let labels = q.labels();
    if require_labels && labels.is_empty() {
        return Ok(vec![]);
    }
    // only spaces the caller can see are queried at all (anonymous visitors of a private wiki
    // get nothing without touching pages)
    let visible_sp = visible_spaces(state, p).await?;
    if visible_sp.is_empty() {
        return Ok(vec![]);
    }
    let space_ids: Vec<Uuid> = visible_sp.iter().map(|(s, _)| s.id).collect();
    let spaces = q.spaces();
    let order = match q.sort.as_deref() {
        Some("title") => "lower(p.title)",
        Some("created") | Some("creation") => "p.created_at DESC",
        _ => "p.updated_at DESC",
    };
    let rows = sqlx::query_as::<_, ReportPage>(sqlx::AssertSqlSafe(format!(
        "SELECT p.id, p.title, p.kind, p.space_id, s.key AS space_key, s.name AS space_name, p.created_at, p.updated_at, u.name AS updated_by_name,
                COALESCE((SELECT array_agg(l.name ORDER BY l.name) FROM labels l WHERE l.page_id = p.id), '{{}}') AS labels,
                CASE WHEN $6 THEN p.content ELSE 'null'::jsonb END AS content
         FROM pages p JOIN spaces s ON s.id = p.space_id LEFT JOIN users u ON u.id = p.updated_by
         WHERE p.workspace_id = $1 AND p.deleted_at IS NULL AND s.archived_at IS NULL AND p.space_id = ANY($5)
           AND (cardinality($2::text[]) = 0 OR upper(s.key) = ANY($2))
           AND (cardinality($3::text[]) = 0 OR (
                CASE WHEN $4 THEN (SELECT count(DISTINCT l.name) FROM labels l WHERE l.page_id = p.id AND l.name = ANY($3)) = cardinality($3)
                     ELSE EXISTS (SELECT 1 FROM labels l WHERE l.page_id = p.id AND l.name = ANY($3)) END))
         ORDER BY {order} LIMIT 2000"
    )))
    .bind(ws)
    .bind(&spaces)
    .bind(&labels)
    .bind(q.all_labels())
    .bind(&space_ids)
    .bind(with_content)
    .fetch_all(&state.db)
    .await?;
    let pairs: Vec<(Uuid, Uuid)> = rows.iter().map(|r| (r.id, r.space_id)).collect();
    let visible = filter_visible(state, p, &visible_sp, &pairs).await?;
    let archived = crate::authz::archived_among(state, &rows.iter().map(|r| r.id).collect::<Vec<_>>()).await?;
    Ok(rows.into_iter().filter(|r| visible.contains(&r.id) && !archived.contains(&r.id)).take(max as usize).collect())
}

/// `GET /api/reports/pages?labels=&spaces=&operator=&sort=&max=` (contentbylabel, recently-updated)
pub async fn pages_report(State(state): State<AppState>, MaybeUser(user): MaybeUser, Query(q): Query<ReportQuery>) -> ApiResult<Json<Vec<ReportPage>>> {
    let p = Principal::load(&state, user).await?;
    Ok(Json(pages(&state, &p, &q, false, q.max(20), false).await?))
}

/// Key/value pairs of a page's properties table: the first table whose rows all have two
/// cells, with the key in a header cell (Confluence's "page properties" layout) or, for
/// horizontal tables, a header row and a single value row.
pub fn properties(doc: &Node) -> Vec<(String, String)> {
    let mut out = Vec::new();
    let mut found = false;
    doc.walk(&mut |n| {
        if found || n.kind != nodes::TABLE {
            return;
        }
        let rows: Vec<&Node> = n.content.iter().filter(|r| r.kind == nodes::TABLE_ROW).collect();
        let cell_text = |c: &Node| text::plain_text(c).split_whitespace().collect::<Vec<_>>().join(" ");
        if !rows.is_empty() && rows.iter().all(|r| r.content.len() == 2 && r.content[0].kind == nodes::TABLE_HEADER) {
            out = rows.iter().map(|r| (cell_text(&r.content[0]), cell_text(&r.content[1]))).filter(|(k, _)| !k.is_empty()).collect();
            found = true;
        } else if rows.len() == 2 && rows[0].content.iter().all(|c| c.kind == nodes::TABLE_HEADER) && rows[0].content.len() == rows[1].content.len() {
            out = rows[0].content.iter().zip(&rows[1].content).map(|(k, v)| (cell_text(k), cell_text(v))).filter(|(k, _)| !k.is_empty()).collect();
            found = true;
        }
    });
    out
}

/// A properties report row: the page, its properties as text (page properties first, then
/// the ones in its properties table) and its typed page properties.
type PropertyRow = (ReportPage, Vec<(String, String)>, Vec<Property>);

/// Pages of a properties report: by label (Confluence's report needs one) or, with
/// `where` conditions or spaces, any page that has properties.
async fn property_rows(state: &AppState, p: &Principal, q: &ReportQuery) -> ApiResult<Vec<PropertyRow>> {
    let conds = q.filter.as_deref().map(page_props::parse_conditions).unwrap_or_default();
    let by_label = !q.labels().is_empty();
    if !by_label && conds.is_empty() && q.spaces().is_empty() {
        return Ok(vec![]);
    }
    let max = q.max(100);
    // filtering happens after loading, so load more candidates
    let candidates = if conds.is_empty() && by_label { max } else { 2000 };
    let rows = pages(state, p, q, false, candidates, true).await?;
    let ids: Vec<Uuid> = rows.iter().map(|r| r.id).collect();
    let mut typed_by_page = page_props::load_many(state, p, &ids).await?;
    let mut out = Vec::new();
    for r in rows {
        let typed = typed_by_page.remove(&r.id).unwrap_or_default();
        // the live document is newer than the stored content
        let doc = match state.rooms.get(r.id) {
            Some(room) => room.snapshot(),
            None => Node::from_json(&r.content).unwrap_or_else(|_| Node::empty_doc()),
        };
        let mut props: Vec<(String, String)> = typed.iter().map(|t| (t.key.clone(), t.display())).collect();
        for (k, v) in properties(&doc) {
            if !props.iter().any(|(x, _)| x.eq_ignore_ascii_case(&k)) {
                props.push((k, v));
            }
        }
        if (!by_label && props.is_empty()) || !page_props::matches(&props, &conds) {
            continue;
        }
        out.push((r, props, typed));
        if out.len() >= max as usize {
            break;
        }
    }
    Ok(out)
}

/// `GET /api/reports/properties?labels=&spaces=&where=&max=` (detailssummary / content-report-table)
pub async fn properties_report(State(state): State<AppState>, MaybeUser(user): MaybeUser, Query(q): Query<ReportQuery>) -> ApiResult<Json<Value>> {
    let p = Principal::load(&state, user).await?;
    let mut columns: Vec<String> = Vec::new();
    let mut seen: HashSet<String> = HashSet::new();
    let mut out = Vec::new();
    for (r, props, typed) in property_rows(&state, &p, &q).await? {
        for (k, _) in &props {
            if seen.insert(k.to_lowercase()) {
                columns.push(k.clone());
            }
        }
        let map: serde_json::Map<String, Value> = props.into_iter().map(|(k, v)| (k, Value::String(v))).collect();
        let values: serde_json::Map<String, Value> = typed.into_iter().map(|t| (t.key.clone(), json!(t))).collect();
        out.push(json!({ "page": r, "properties": map, "values": values }));
    }
    Ok(Json(json!({ "columns": columns, "rows": out })))
}

#[derive(Serialize, sqlx::FromRow)]
#[serde(rename_all = "camelCase")]
pub struct TaskReportRow {
    page_id: Uuid,
    idx: i32,
    text: String,
    done: bool,
    due: Option<chrono::NaiveDate>,
    assignee_id: Option<Uuid>,
    assignee_name: Option<String>,
    page_title: String,
    space_id: Uuid,
    space_key: String,
}

/// `GET /api/reports/tasks?spaces=&labels=&status=&max=` (tasks-report-macro)
pub async fn tasks_report(State(state): State<AppState>, MaybeUser(user): MaybeUser, Query(q): Query<ReportQuery>) -> ApiResult<Json<Vec<TaskReportRow>>> {
    let p = Principal::load(&state, user).await?;
    tasks_for(&state, &p, &q).await
}

async fn tasks_for(state: &AppState, p: &Principal, q: &ReportQuery) -> ApiResult<Json<Vec<TaskReportRow>>> {
    let ws = workspace_of(state, p).await?;
    let visible_sp = visible_spaces(state, p).await?;
    if visible_sp.is_empty() {
        return Ok(Json(vec![]));
    }
    let space_ids: Vec<Uuid> = visible_sp.iter().map(|(s, _)| s.id).collect();
    let status = q.status.clone().unwrap_or_else(|| "open".into());
    let assignees = assignee_ids(state, p, ws, q).await?;
    if assignees.as_ref().is_some_and(|a| a.is_empty()) {
        // asked for people who aren't here
        return Ok(Json(vec![]));
    }
    let (due_from, due_to) = (parse_day(&q.due_from)?, parse_day(&q.due_to)?);
    let rows = sqlx::query_as::<_, TaskReportRow>(
        "SELECT t.page_id, t.idx, t.text, t.done, t.due, t.assignee_id, a.name AS assignee_name, p.title AS page_title, p.space_id, s.key AS space_key
         FROM tasks t JOIN pages p ON p.id = t.page_id JOIN spaces s ON s.id = p.space_id LEFT JOIN users a ON a.id = t.assignee_id
         WHERE p.workspace_id = $1 AND p.deleted_at IS NULL AND s.archived_at IS NULL AND p.space_id = ANY($5)
           AND ($2 = 'all' OR t.done = ($2 = 'done'))
           AND (cardinality($3::text[]) = 0 OR upper(s.key) = ANY($3))
           AND (cardinality($4::text[]) = 0 OR EXISTS (SELECT 1 FROM labels l WHERE l.page_id = p.id AND l.name = ANY($4)))
           AND ($6::uuid[] IS NULL OR t.assignee_id = ANY($6))
           AND ($7::date IS NULL OR t.due >= $7) AND ($8::date IS NULL OR t.due <= $8)
         ORDER BY t.done, t.due NULLS LAST, p.updated_at DESC, t.idx LIMIT 2000",
    )
    .bind(ws)
    .bind(&status)
    .bind(q.spaces())
    .bind(q.labels())
    .bind(&space_ids)
    .bind(&assignees)
    .bind(due_from)
    .bind(due_to)
    .fetch_all(&state.db)
    .await?;
    let pairs: Vec<(Uuid, Uuid)> = rows.iter().map(|r| (r.page_id, r.space_id)).collect();
    let visible = filter_visible(state, p, &visible_sp, &pairs).await?;
    Ok(Json(rows.into_iter().filter(|r| visible.contains(&r.page_id)).take(q.max(50) as usize).collect()))
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct DecisionRow {
    page_id: Uuid,
    page_title: String,
    space_key: String,
    text: String,
    decided: bool,
    updated_at: chrono::DateTime<chrono::Utc>,
}

/// Decision items (`decisionItem` blocks) in the visible pages of the requested spaces, newest
/// pages first. Only the decision items leave the database (not whole documents), at most
/// 300 pages are looked at, and a space is required so a call never scans the whole wiki.
/// (`strict` mode: lax `.**` visits nodes twice.)
async fn decisions_for(state: &AppState, p: &Principal, q: &ReportQuery) -> ApiResult<Vec<DecisionRow>> {
    let max = q.max(50) as usize;
    let spaces = q.spaces();
    if spaces.is_empty() || spaces.len() > 20 {
        return Err(ApiError::bad("name 1 to 20 spaces"));
    }
    let ws = workspace_of(state, p).await?;
    let visible_sp = visible_spaces(state, p).await?;
    let space_ids: Vec<Uuid> = visible_sp.iter().filter(|(s, _)| spaces.contains(&s.key.to_uppercase())).map(|(s, _)| s.id).collect();
    if space_ids.is_empty() {
        return Ok(vec![]);
    }
    #[derive(sqlx::FromRow)]
    struct Row {
        id: Uuid,
        title: String,
        space_id: Uuid,
        space_key: String,
        updated_at: chrono::DateTime<chrono::Utc>,
        items: Value,
    }
    let rows = sqlx::query_as::<_, Row>(
        "SELECT p.id, p.title, p.space_id, s.key AS space_key, p.updated_at,
                jsonb_path_query_array(p.content, 'strict $.** ? (@.type == \"decisionItem\")', '{}', true) AS items
         FROM pages p JOIN spaces s ON s.id = p.space_id
         WHERE p.workspace_id = $1 AND p.space_id = ANY($2) AND p.deleted_at IS NULL AND p.archived_at IS NULL AND s.archived_at IS NULL
           AND (cardinality($3::text[]) = 0 OR EXISTS (SELECT 1 FROM labels l WHERE l.page_id = p.id AND l.name = ANY($3)))
           AND jsonb_path_exists(p.content, 'strict $.** ? (@.type == \"decisionItem\")', '{}', true)
         ORDER BY p.updated_at DESC LIMIT 300",
    )
    .bind(ws)
    .bind(&space_ids)
    .bind(q.labels())
    .fetch_all(&state.db)
    .await?;
    let pairs: Vec<(Uuid, Uuid)> = rows.iter().map(|r| (r.id, r.space_id)).collect();
    let visible = filter_visible(state, p, &visible_sp, &pairs).await?;
    let mut out = Vec::new();
    for page in rows.into_iter().filter(|r| visible.contains(&r.id)) {
        let mut items = Vec::new();
        for item in page.items.as_array().into_iter().flatten() {
            if let Ok(n) = Node::from_json(item) {
                collect_decisions(&n, &mut items);
            }
        }
        for (text, decided) in items {
            if q.status.as_deref() == Some("decided") && !decided {
                continue;
            }
            out.push(DecisionRow {
                page_id: page.id,
                page_title: page.title.clone(),
                space_key: page.space_key.clone(),
                text,
                decided,
                updated_at: page.updated_at,
            });
            if out.len() >= max {
                return Ok(out);
            }
        }
    }
    Ok(out)
}

fn collect_decisions(n: &Node, out: &mut Vec<(String, bool)>) {
    if n.kind == nodes::DECISION_ITEM {
        let t = text::plain_text(&Node::doc(n.content.clone())).split_whitespace().collect::<Vec<_>>().join(" ");
        if !t.is_empty() {
            out.push((t.chars().take(500).collect(), !n.attr_str("state").is_some_and(|s| s.eq_ignore_ascii_case("undecided"))));
        }
        return;
    }
    for c in &n.content {
        collect_decisions(c, out);
    }
}

/// `GET /api/reports/decisions?spaces=&labels=&status=&max=` (decisionreport): the decision
/// log of a space. `status=decided` leaves out undecided items.
pub async fn decisions_report(State(state): State<AppState>, MaybeUser(user): MaybeUser, Query(q): Query<ReportQuery>) -> ApiResult<Json<Vec<DecisionRow>>> {
    let p = Principal::load(&state, user).await?;
    Ok(Json(decisions_for(&state, &p, &q).await?))
}

/// A report macro evaluated once, for static exports (Word): a titled table.
pub struct Snapshot {
    pub title: String,
    pub header: Vec<String>,
    /// cells: text and an optional link
    pub rows: Vec<Vec<(String, Option<String>)>>,
    pub empty: &'static str,
}

/// Values of `key = x`, `key = "x"` and `key in (a, b)` clauses of a CQL query.
fn cql_values(cql: &str, key: &str, extra: &str) -> Vec<String> {
    let lower = cql.to_ascii_lowercase();
    let mut out = Vec::new();
    let mut from = 0;
    while let Some(i) = lower[from..].find(key) {
        let start = from + i;
        from = start + key.len();
        if start > 0 && lower.as_bytes()[start - 1].is_ascii_alphanumeric() {
            continue;
        }
        let rest = cql[from..].trim_start();
        let value = if let Some(r) = rest.strip_prefix('=') {
            r
        } else if rest.get(..2).is_some_and(|op| op.eq_ignore_ascii_case("in")) && !rest[2..].starts_with(|c: char| c.is_alphanumeric()) {
            &rest[2..]
        } else {
            continue;
        };
        let v = value.trim_start();
        if let Some(r) = v.strip_prefix('(') {
            if let Some(end) = r.find(')') {
                out.extend(list(&Some(r[..end].to_string())));
            }
        } else if let Some(q) = v.chars().next().filter(|c| *c == '"' || *c == '\'') {
            if let Some(end) = v[1..].find(q) {
                out.push(v[1..1 + end].to_string());
            }
        } else {
            let word: String = v.chars().take_while(|c| c.is_alphanumeric() || *c == '_' || extra.contains(*c)).collect();
            if !word.is_empty() {
                out.push(word);
            }
        }
    }
    out
}

/// Labels, spaces and the AND flag from classic macro parameters or a Cloud CQL query
/// (the server side of the editor's `querySpec`).
fn query_spec(params: &serde_json::Map<String, Value>, current_space: &str) -> (Vec<String>, Vec<String>, bool) {
    let get = |k: &str| params.get(k).and_then(|v| v.as_str()).map(str::to_string);
    let cql = get("cql").unwrap_or_default();
    let mut labels: Vec<String> = Vec::new();
    for l in list(&get("labels")).into_iter().chain(list(&get("label"))).chain(cql_values(&cql, "label", ".-")) {
        let l = l.to_lowercase();
        if !labels.contains(&l) {
            labels.push(l);
        }
    }
    let mut spaces: Vec<String> = Vec::new();
    for s in list(&get("spaces").or(get("space")).or(get("spaceKey"))).into_iter().chain(cql_values(&cql, "space", ".()-")) {
        let s = if s == "@self" || s.eq_ignore_ascii_case("currentspace()") { current_space.to_string() } else { s };
        let s = s.to_uppercase();
        if !s.is_empty() && !matches!(s.as_str(), "@ALL" | "@GLOBAL" | "@PERSONAL") && !spaces.contains(&s) {
            spaces.push(s);
        }
    }
    let lower = cql.to_lowercase();
    let all = get("operator").is_some_and(|o| o.eq_ignore_ascii_case("and")) || (lower.matches("label").count() >= 2 && lower.contains(" and "));
    (labels, spaces, all)
}

/// Evaluates a live report macro on page `page_id` (in space `space_key`) for `p`.
/// `None` for macros that aren't reports.
pub async fn snapshot(
    state: &AppState,
    p: &Principal,
    name: &str,
    params: &serde_json::Map<String, Value>,
    page_id: Uuid,
    space_key: &str,
    base: &str,
) -> ApiResult<Option<Snapshot>> {
    let get = |k: &str| params.get(k).and_then(|v| v.as_str()).map(str::to_string);
    let num = |keys: &[&str], default: i64| keys.iter().find_map(|k| get(k).and_then(|v| v.trim().parse::<i64>().ok())).unwrap_or(default);
    let (labels, spaces, all) = query_spec(params, space_key);
    let link = |id: Uuid| Some(format!("{base}/pages/{id}"));
    // dates as the workspace writes them
    let ws: Option<Uuid> = sqlx::query_scalar("SELECT workspace_id FROM pages WHERE id = $1").bind(page_id).fetch_optional(&state.db).await?;
    let fmt = match ws {
        Some(ws) => crate::routes::branding::date_format(state, ws).await?,
        None => "auto".into(),
    };
    let date = |d: chrono::DateTime<chrono::Utc>| crate::routes::branding::format_day(&fmt, d.date_naive());
    let snap = match name {
        "contentbylabel" | "recently-updated" => {
            let recent = name == "recently-updated";
            let spaces =
                if spaces.is_empty() && recent && !get("spaces").unwrap_or_default().contains("@all") { vec![space_key.to_uppercase()] } else { spaces };
            if !recent && labels.is_empty() {
                return Ok(None);
            }
            let sort = if recent {
                "updated"
            } else {
                match get("sort").as_deref() {
                    None | Some("title") => "title",
                    Some("creation") => "created",
                    _ => "updated",
                }
            };
            let q = ReportQuery {
                labels: Some(labels.join(",")),
                spaces: Some(spaces.join(",")),
                operator: all.then(|| "all".into()),
                sort: Some(sort.into()),
                max: Some(num(&["max", "maxResults"], if recent { 15 } else { 20 })),
                ..Default::default()
            };
            let mut rows: Vec<ReportPage> = pages(state, p, &q, false, q.max(20), false).await?.into_iter().filter(|r| recent || r.id != page_id).collect();
            if get("reverse").as_deref() == Some("true") {
                rows.reverse();
            }
            let mut header = vec!["Page".to_string(), "Space".into(), "Updated".into()];
            if !recent {
                header.push("Labels".into());
            }
            Snapshot {
                title: if recent { "Recently updated".into() } else { format!("Pages labelled {}", labels.join(if all { " and " } else { ", " })) },
                header,
                rows: rows
                    .into_iter()
                    .map(|r| {
                        let updated = format!("{}{}", date(r.updated_at), r.updated_by_name.as_ref().map(|n| format!(" by {n}")).unwrap_or_default());
                        let mut row = vec![(r.title.clone(), link(r.id)), (r.space_name.clone(), None), (updated, None)];
                        if !recent {
                            row.push((r.labels.join(", "), None));
                        }
                        row
                    })
                    .collect(),
                empty: "No pages found.",
            }
        }
        "detailssummary" | "content-report-table" => {
            let filter = get("where").filter(|w| !w.trim().is_empty());
            if labels.is_empty() && filter.is_none() && spaces.is_empty() {
                return Ok(None);
            }
            let q = ReportQuery {
                labels: Some(labels.join(",")),
                spaces: Some(spaces.join(",")),
                sort: Some("title".into()),
                max: Some(num(&["pageSize", "max", "maxResults"], 100)),
                filter,
                ..Default::default()
            };
            let mut columns: Vec<String> = Vec::new();
            let mut table = Vec::new();
            for (r, props, typed) in property_rows(state, p, &q).await? {
                for (k, _) in &props {
                    if !columns.iter().any(|c| c.eq_ignore_ascii_case(k)) {
                        columns.push(k.clone());
                    }
                }
                table.push((r, props, typed));
            }
            let headings = list(&get("headings"));
            let columns: Vec<String> = if headings.is_empty() { columns.into_iter().take(8).collect() } else { headings };
            Snapshot {
                title: if labels.is_empty() { "Page properties".into() } else { format!("Page properties: {}", labels.join(", ")) },
                header: std::iter::once("Page".to_string()).chain(columns.iter().cloned()).collect(),
                rows: table
                    .into_iter()
                    .map(|(r, props, typed)| {
                        std::iter::once((r.title.clone(), link(r.id)))
                            .chain(columns.iter().map(|c| {
                                let text = props.iter().find(|(k, _)| k.eq_ignore_ascii_case(c)).map(|(_, v)| v.clone()).unwrap_or_default();
                                // web addresses, people and visible pages stay links
                                let target = typed.iter().find(|t| t.key.eq_ignore_ascii_case(c)).and_then(|t| {
                                    let v = t.value.as_ref()?.as_str()?;
                                    match t.kind.as_str() {
                                        "url" => Some(v.to_string()),
                                        "user" => Some(format!("{base}/people/{v}")),
                                        "page" if t.label.is_some() => Some(format!("{base}/pages/{v}")),
                                        _ => None,
                                    }
                                });
                                (text, target)
                            }))
                            .collect()
                    })
                    .collect(),
                empty: "No pages with page properties and this label.",
            }
        }
        "tasks-report-macro" => {
            let spaces = if spaces.is_empty() { vec![space_key.to_uppercase()] } else { spaces };
            let st = get("status").unwrap_or_default().to_lowercase();
            let status = if st == "complete" {
                "done"
            } else if st.contains("all") {
                "all"
            } else {
                "open"
            };
            let q = ReportQuery {
                spaces: Some(spaces.join(",")),
                labels: Some(labels.join(",")),
                status: Some(status.into()),
                max: Some(num(&["pageSize"], 20)),
                assignees: get("assignees").or_else(|| get("assignee")),
                due_from: get("duedateFrom").or_else(|| get("dueFrom")),
                due_to: get("duedateTo").or_else(|| get("dueTo")),
                ..Default::default()
            };
            // a filter that doesn't parse shows the unfiltered list rather than nothing
            let Json(rows) = match tasks_for(state, p, &q).await {
                Ok(r) => r,
                Err(_) => tasks_for(state, p, &ReportQuery { due_from: None, due_to: None, ..q }).await?,
            };
            Snapshot {
                title: format!(
                    "Tasks ({})",
                    if status == "open" {
                        "open"
                    } else if status == "done" {
                        "completed"
                    } else {
                        "all"
                    }
                ),
                // the done column only says something when finished tasks are listed too
                header: ["Done", "Task", "Assignee", "Due", "Page"].iter().skip(usize::from(status == "open")).map(|h| h.to_string()).collect(),
                rows: rows
                    .into_iter()
                    .map(|t| {
                        let row = vec![
                            ((if t.done { "Yes" } else { "No" }).to_string(), None),
                            (t.text, None),
                            (t.assignee_name.unwrap_or_default(), None),
                            (t.due.map(|d| crate::routes::branding::format_day(&fmt, d)).unwrap_or_default(), None),
                            (t.page_title, link(t.page_id)),
                        ];
                        row.into_iter().skip(usize::from(status == "open")).collect()
                    })
                    .collect(),
                empty: "No tasks.",
            }
        }
        "decisionreport" => {
            let spaces = if spaces.is_empty() { vec![space_key.to_uppercase()] } else { spaces };
            let q = ReportQuery {
                spaces: Some(spaces.join(",")),
                labels: Some(labels.join(",")),
                max: Some(num(&["max", "maxResults"], 30)),
                ..Default::default()
            };
            let rows = decisions_for(state, p, &q).await?;
            Snapshot {
                title: "Decision log".into(),
                header: vec!["Decision".into(), "Page".into(), "Updated".into()],
                rows: rows.into_iter().map(|d| vec![(d.text, None), (d.page_title, link(d.page_id)), (date(d.updated_at), None)]).collect(),
                empty: "No decisions recorded yet.",
            }
        }
        "attachments" => {
            let rows: Vec<(Uuid, String, i64)> =
                sqlx::query_as("SELECT id, filename, size FROM attachments WHERE page_id = $1 AND deleted_at IS NULL ORDER BY created_at DESC")
                    .bind(page_id)
                    .fetch_all(&state.db)
                    .await?;
            Snapshot {
                title: "Attachments".into(),
                header: vec!["File".into(), "Size".into()],
                rows: rows
                    .into_iter()
                    .map(|(id, name, size)| vec![(name, Some(format!("{base}/api/attachments/{id}/content"))), (human_size(size), None)])
                    .collect(),
                empty: "No attachments on this page.",
            }
        }
        _ => return Ok(None),
    };
    Ok(Some(snap))
}

fn human_size(n: i64) -> String {
    match n {
        n if n >= 1 << 20 => format!("{:.1} MB", n as f64 / (1 << 20) as f64),
        n if n >= 1 << 10 => format!("{:.0} KB", n as f64 / 1024.0),
        n => format!("{n} B"),
    }
}

#[cfg(test)]
mod tests {
    use wikicraft_doc_model::markdown::markdown_to_pm;

    #[test]
    fn cql_and_classic_parameters() {
        let params = |v: serde_json::Value| v.as_object().unwrap().clone();
        let (labels, spaces, all) =
            super::query_spec(&params(serde_json::json!({"cql": "label = \"Project\" and label in (q1, 'Q2') and space = currentSpace()"})), "eng");
        assert_eq!(labels, vec!["project", "q1", "q2"]);
        assert_eq!(spaces, vec!["ENG"]);
        assert!(all);
        let (labels, spaces, all) = super::query_spec(&params(serde_json::json!({"labels": "a, b", "spaces": "@all", "cql": "labels = x"})), "eng");
        assert_eq!(labels, vec!["a", "b"]);
        assert!(spaces.is_empty() && !all);
        let (_, spaces, _) = super::query_spec(&params(serde_json::json!({"cql": "space in (\"DOC\", ops) AND type = page"})), "eng");
        assert_eq!(spaces, vec!["DOC", "OPS"]);
    }

    #[test]
    fn horizontal_properties_table() {
        let doc = markdown_to_pm("intro\n\n| Owner | Status | Due |\n|---|---|---|\n| Ann | In progress | 2026-12-01 |\n").doc;
        let props = super::properties(&doc);
        assert_eq!(props, vec![("Owner".into(), "Ann".into()), ("Status".into(), "In progress".into()), ("Due".into(), "2026-12-01".into())]);
    }
}
