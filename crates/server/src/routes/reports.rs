//! Live queries behind Confluence's report macros: pages by label (`contentbylabel`),
//! recently updated pages (`recently-updated`), page properties reports (`detailssummary`,
//! `content-report-table`) and task reports (`tasks-report-macro`). Every result is filtered
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
use crate::error::ApiResult;
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
async fn pages(state: &AppState, p: &Principal, q: &ReportQuery, require_labels: bool, default_max: i64) -> ApiResult<Vec<ReportPage>> {
    let ws = workspace_of(state, p).await?;
    let labels = q.labels();
    if require_labels && labels.is_empty() {
        return Ok(vec![]);
    }
    let spaces = q.spaces();
    let order = match q.sort.as_deref() {
        Some("title") => "lower(p.title)",
        Some("created") | Some("creation") => "p.created_at DESC",
        _ => "p.updated_at DESC",
    };
    let max = q.max(default_max);
    let rows = sqlx::query_as::<_, ReportPage>(sqlx::AssertSqlSafe(format!(
        "SELECT p.id, p.title, p.kind, p.space_id, s.key AS space_key, s.name AS space_name, p.created_at, p.updated_at, u.name AS updated_by_name,
                COALESCE((SELECT array_agg(l.name ORDER BY l.name) FROM labels l WHERE l.page_id = p.id), '{{}}') AS labels, p.content
         FROM pages p JOIN spaces s ON s.id = p.space_id LEFT JOIN users u ON u.id = p.updated_by
         WHERE p.workspace_id = $1 AND p.deleted_at IS NULL AND s.archived_at IS NULL
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
    .fetch_all(&state.db)
    .await?;
    let visible_sp = visible_spaces(state, p).await?;
    let pairs: Vec<(Uuid, Uuid)> = rows.iter().map(|r| (r.id, r.space_id)).collect();
    let visible = filter_visible(state, p, &visible_sp, &pairs).await?;
    Ok(rows.into_iter().filter(|r| visible.contains(&r.id)).take(max as usize).collect())
}

/// `GET /api/reports/pages?labels=&spaces=&operator=&sort=&max=` (contentbylabel, recently-updated)
pub async fn pages_report(State(state): State<AppState>, MaybeUser(user): MaybeUser, Query(q): Query<ReportQuery>) -> ApiResult<Json<Vec<ReportPage>>> {
    let p = Principal::load(&state, user).await?;
    Ok(Json(pages(&state, &p, &q, false, 20).await?))
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

/// `GET /api/reports/properties?labels=&spaces=&max=` (detailssummary / content-report-table)
pub async fn properties_report(State(state): State<AppState>, MaybeUser(user): MaybeUser, Query(q): Query<ReportQuery>) -> ApiResult<Json<Value>> {
    let p = Principal::load(&state, user).await?;
    let rows = pages(&state, &p, &q, true, 100).await?;
    let mut columns: Vec<String> = Vec::new();
    let mut seen: HashSet<String> = HashSet::new();
    let mut out = Vec::new();
    for r in rows {
        // the live document is newer than the stored content
        let doc = match state.rooms.get(r.id) {
            Some(room) => room.snapshot(),
            None => Node::from_json(&r.content).unwrap_or_else(|_| Node::empty_doc()),
        };
        let props = properties(&doc);
        for (k, _) in &props {
            if seen.insert(k.to_lowercase()) {
                columns.push(k.clone());
            }
        }
        let map: serde_json::Map<String, Value> = props.into_iter().map(|(k, v)| (k, Value::String(v))).collect();
        out.push(json!({ "page": r, "properties": map }));
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
    let ws = workspace_of(&state, &p).await?;
    let status = q.status.clone().unwrap_or_else(|| "open".into());
    let rows = sqlx::query_as::<_, TaskReportRow>(
        "SELECT t.page_id, t.idx, t.text, t.done, t.due, t.assignee_id, a.name AS assignee_name, p.title AS page_title, p.space_id, s.key AS space_key
         FROM tasks t JOIN pages p ON p.id = t.page_id JOIN spaces s ON s.id = p.space_id LEFT JOIN users a ON a.id = t.assignee_id
         WHERE p.workspace_id = $1 AND p.deleted_at IS NULL AND s.archived_at IS NULL
           AND ($2 = 'all' OR t.done = ($2 = 'done'))
           AND (cardinality($3::text[]) = 0 OR upper(s.key) = ANY($3))
           AND (cardinality($4::text[]) = 0 OR EXISTS (SELECT 1 FROM labels l WHERE l.page_id = p.id AND l.name = ANY($4)))
         ORDER BY t.done, t.due NULLS LAST, p.updated_at DESC, t.idx LIMIT 2000",
    )
    .bind(ws)
    .bind(&status)
    .bind(q.spaces())
    .bind(q.labels())
    .fetch_all(&state.db)
    .await?;
    let visible_sp = visible_spaces(&state, &p).await?;
    let pairs: Vec<(Uuid, Uuid)> = rows.iter().map(|r| (r.page_id, r.space_id)).collect();
    let visible = filter_visible(&state, &p, &visible_sp, &pairs).await?;
    Ok(Json(rows.into_iter().filter(|r| visible.contains(&r.page_id)).take(q.max(50) as usize).collect()))
}

#[cfg(test)]
mod tests {
    use wikicraft_doc_model::markdown::markdown_to_pm;

    #[test]
    fn horizontal_properties_table() {
        let doc = markdown_to_pm("intro\n\n| Owner | Status | Due |\n|---|---|---|\n| Ann | In progress | 2026-12-01 |\n").doc;
        let props = super::properties(&doc);
        assert_eq!(props, vec![("Owner".into(), "Ann".into()), ("Status".into(), "In progress".into()), ("Due".into(), "2026-12-01".into())]);
    }
}
