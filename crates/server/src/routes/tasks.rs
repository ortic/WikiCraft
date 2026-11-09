//! Task index ("my tasks"): task items are extracted from page content on every save; a task
//! is assigned to the first user @mentioned in it, its due date is the first date chip.

use axum::Json;
use axum::extract::{Path, Query, State};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use sqlx::PgConnection;
use uuid::Uuid;
use wikicraft_doc_model::{Node, nodes, text};

use crate::app::AppState;
use crate::auth::CurrentUser;
use crate::authz::{Need, Principal, require_page};
use crate::error::{ApiError, ApiResult};

pub struct TaskEntry {
    pub text: String,
    pub done: bool,
    pub assignee: Option<Uuid>,
    pub due: Option<chrono::NaiveDate>,
}

/// Task items of a document in document order.
pub fn extract(doc: &Node) -> Vec<TaskEntry> {
    let mut out = Vec::new();
    doc.walk(&mut |n| {
        if n.kind != nodes::TASK_ITEM {
            return;
        }
        // only the item's own first block (nested sub-tasks are separate entries)
        let first = n.content.first();
        let mut assignee = None;
        let mut due = None;
        if let Some(f) = first {
            f.walk(&mut |c| {
                if c.kind == nodes::MENTION && assignee.is_none() {
                    assignee = c.attr_str("id").and_then(|s| s.parse().ok());
                }
                if c.kind == nodes::DATE && due.is_none() {
                    due = c.attr_str("date").and_then(|d| chrono::NaiveDate::parse_from_str(d.get(..10).unwrap_or(d), "%Y-%m-%d").ok());
                }
            });
        }
        out.push(TaskEntry {
            // without the assignee / due date (shown separately), whitespace collapsed
            text: first.map(|f| text::inline_text(&without_people_and_dates(f))).unwrap_or_default().split_whitespace().collect::<Vec<_>>().join(" "),
            done: n.attr_bool("checked").unwrap_or(false),
            assignee,
            due,
        });
    });
    out
}

fn without_people_and_dates(n: &Node) -> Node {
    let mut n = n.clone();
    n.content.retain(|c| c.kind != nodes::MENTION && c.kind != nodes::DATE);
    n.content = n.content.iter().map(without_people_and_dates).collect();
    n
}

/// Replaces the task index of a page.
pub async fn index_page(db: &mut PgConnection, page_id: Uuid, doc: &Node) -> sqlx::Result<()> {
    sqlx::query("DELETE FROM tasks WHERE page_id = $1").bind(page_id).execute(&mut *db).await?;
    for (i, t) in extract(doc).into_iter().enumerate().take(2000) {
        sqlx::query("INSERT INTO tasks (page_id, idx, text, done, assignee_id, due) SELECT $1, $2, $3, $4, (SELECT id FROM users WHERE id = $5), $6")
            .bind(page_id)
            .bind(i as i32)
            .bind(t.text.chars().take(1000).collect::<String>())
            .bind(t.done)
            .bind(t.assignee)
            .bind(t.due)
            .execute(&mut *db)
            .await?;
    }
    Ok(())
}

#[derive(Deserialize)]
pub struct TaskQuery {
    /// `open` (default), `done` or `all`
    status: Option<String>,
}

#[derive(Serialize, sqlx::FromRow)]
#[serde(rename_all = "camelCase")]
pub struct TaskRow {
    page_id: Uuid,
    idx: i32,
    text: String,
    done: bool,
    due: Option<chrono::NaiveDate>,
    page_title: String,
    space_id: Uuid,
    space_key: String,
    space_name: String,
}

/// `GET /api/tasks` — tasks assigned to the current user.
pub async fn mine(State(state): State<AppState>, CurrentUser(user): CurrentUser, Query(q): Query<TaskQuery>) -> ApiResult<Json<Vec<TaskRow>>> {
    let status = q.status.unwrap_or_else(|| "open".into());
    let rows = sqlx::query_as::<_, TaskRow>(
        "SELECT t.page_id, t.idx, t.text, t.done, t.due, p.title AS page_title, p.space_id, s.key AS space_key, s.name AS space_name
         FROM tasks t JOIN pages p ON p.id = t.page_id JOIN spaces s ON s.id = p.space_id
         WHERE t.assignee_id = $1 AND p.deleted_at IS NULL AND ($2 = 'all' OR t.done = ($2 = 'done'))
         ORDER BY t.done, t.due NULLS LAST, p.updated_at DESC LIMIT 500",
    )
    .bind(user.id)
    .bind(&status)
    .fetch_all(&state.db)
    .await?;
    let p = Principal::load(&state, Some(user)).await?;
    let spaces = crate::routes::search::visible_spaces(&state, &p).await?;
    let pairs: Vec<(Uuid, Uuid)> = rows.iter().map(|r| (r.page_id, r.space_id)).collect();
    let visible = crate::routes::search::filter_visible(&state, &p, &spaces, &pairs).await?;
    Ok(Json(rows.into_iter().filter(|r| visible.contains(&r.page_id)).collect()))
}

/// `POST /api/pages/{id}/tasks/{idx}/toggle` — check/uncheck a task item (live for editors).
pub async fn toggle(State(state): State<AppState>, CurrentUser(user): CurrentUser, Path((page_id, idx)): Path<(Uuid, usize)>) -> ApiResult<Json<Value>> {
    let p = Principal::load(&state, Some(user.clone())).await?;
    require_page(&state, &p, page_id, Need::Edit).await?;
    let room = state.rooms.get_or_load(&state, page_id).await?;
    let mut doc = room.snapshot();
    let mut i = 0usize;
    let mut new_state = None;
    doc.walk_mut(&mut |n| {
        if n.kind == nodes::TASK_ITEM {
            if i == idx {
                let done = !n.attr_bool("checked").unwrap_or(false);
                n.attrs.insert("checked".into(), Value::Bool(done));
                new_state = Some(done);
            }
            i += 1;
        }
    });
    let done = new_state.ok_or(ApiError::NotFound("task"))?;
    room.replace_content(&doc, Some(user.id));
    crate::collab::persist_with(&state, &room, false).await?;
    Ok(Json(json!({ "done": done })))
}

#[cfg(test)]
mod tests {
    use super::*;
    use wikicraft_doc_model::markdown::markdown_to_pm;

    #[test]
    fn extracts_tasks() {
        let mut doc = markdown_to_pm("- [x] done thing\n- [ ] open thing").doc;
        let uid = Uuid::now_v7();
        doc.content[0].content[1].content[0].content.push(Node::new(nodes::MENTION).attr("id", uid.to_string()).attr("label", "Ann"));
        doc.content[0].content[1].content[0].content.push(Node::new(nodes::DATE).attr("date", "2026-12-24"));
        let t = extract(&doc);
        assert_eq!(t.len(), 2);
        assert!(t[0].done && !t[1].done);
        assert_eq!(t[1].assignee, Some(uid));
        assert_eq!(t[1].due.unwrap().to_string(), "2026-12-24");
        assert_eq!(t[1].text, "open thing", "assignee and date are not part of the text");
    }
}
