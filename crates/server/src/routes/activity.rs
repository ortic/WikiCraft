//! Activity feed: who created, edited or commented on which page, newest first, limited to
//! what the caller can see. `scope=watching` keeps pages and spaces the caller watches,
//! `scope=following` what the people they follow did.

use axum::Json;
use axum::extract::{Query, State};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

use crate::app::AppState;
use crate::auth::CurrentUser;
use crate::authz::Principal;
use crate::error::ApiResult;
use crate::routes::search::{filter_visible, visible_spaces};

#[derive(Deserialize)]
pub struct ActivityQuery {
    /// `all` (default), `watching` or `following`
    scope: Option<String>,
    limit: Option<i64>,
}

#[derive(Serialize, sqlx::FromRow)]
#[serde(rename_all = "camelCase")]
pub struct Event {
    /// `created` | `edited` | `commented`
    kind: String,
    page_id: Uuid,
    page_title: String,
    space_id: Uuid,
    space_key: String,
    space_name: String,
    actor_id: Option<Uuid>,
    actor_name: Option<String>,
    at: chrono::DateTime<chrono::Utc>,
    /// version message or comment excerpt
    detail: Option<String>,
    /// consecutive edits of one page by one person are folded into one event
    #[sqlx(default)]
    count: i64,
}

/// `GET /api/activity?scope=all|watching|following&limit=`
pub async fn feed(State(state): State<AppState>, CurrentUser(user): CurrentUser, Query(q): Query<ActivityQuery>) -> ApiResult<Json<Vec<Event>>> {
    let p = Principal::load(&state, Some(user.clone())).await?;
    let watching = q.scope.as_deref() == Some("watching");
    // only what the followed people did (looked up before the newest-first cut-off)
    let actors: Option<Vec<Uuid>> = match q.scope.as_deref() {
        Some("following") => Some(sqlx::query_scalar("SELECT followee_id FROM user_follows WHERE follower_id = $1").bind(user.id).fetch_all(&state.db).await?),
        _ => None,
    };
    if actors.as_ref().is_some_and(|a| a.is_empty()) {
        return Ok(Json(Vec::new()));
    }
    let limit = q.limit.unwrap_or(40).clamp(1, 100) as usize;
    let rows = sqlx::query_as::<_, Event>(
        "WITH ev AS (
            (SELECT CASE WHEN v.version = 1 THEN 'created' ELSE 'edited' END AS kind, v.page_id, v.created_by AS actor_id, v.created_at AS at,
                    v.message AS detail
             FROM page_versions v JOIN pages p ON p.id = v.page_id
             WHERE p.workspace_id = $1 AND ($4::uuid[] IS NULL OR v.created_by = ANY($4)) ORDER BY v.created_at DESC LIMIT 400)
            UNION ALL
            (SELECT 'commented', c.page_id, c.author_id, c.created_at, left(c.text_content, 160)
             FROM comments c JOIN pages p ON p.id = c.page_id
             WHERE p.workspace_id = $1 AND ($4::uuid[] IS NULL OR c.author_id = ANY($4)) ORDER BY c.created_at DESC LIMIT 200)
         )
         SELECT ev.kind, ev.page_id, p.title AS page_title, p.space_id, s.key AS space_key, s.name AS space_name,
                ev.actor_id, u.name AS actor_name, ev.at, ev.detail, 1::bigint AS count
         FROM ev JOIN pages p ON p.id = ev.page_id JOIN spaces s ON s.id = p.space_id LEFT JOIN users u ON u.id = ev.actor_id
         WHERE p.deleted_at IS NULL AND p.archived_at IS NULL AND s.archived_at IS NULL
           AND (NOT $2 OR EXISTS (SELECT 1 FROM watches w WHERE w.user_id = $3 AND w.page_id = p.id)
                       OR EXISTS (SELECT 1 FROM space_watches sw WHERE sw.user_id = $3 AND sw.space_id = p.space_id))
         ORDER BY ev.at DESC LIMIT 600",
    )
    .bind(user.workspace_id)
    .bind(watching)
    .bind(user.id)
    .bind(actors)
    .fetch_all(&state.db)
    .await?;
    let spaces = visible_spaces(&state, &p).await?;
    let pairs: Vec<(Uuid, Uuid)> = rows.iter().map(|r| (r.page_id, r.space_id)).collect();
    let visible = filter_visible(&state, &p, &spaces, &pairs).await?;
    let mut out: Vec<Event> = Vec::new();
    for e in rows.into_iter().filter(|e| visible.contains(&e.page_id)) {
        // fold a run of edits (same page, same person, same day) into the newest one
        if let Some(last) = out.last_mut()
            && e.kind == "edited"
            && last.kind == "edited"
            && last.page_id == e.page_id
            && last.actor_id == e.actor_id
            && last.at.date_naive() == e.at.date_naive()
        {
            last.count += 1;
            continue;
        }
        if out.len() == limit {
            break;
        }
        out.push(e);
    }
    Ok(Json(out))
}
