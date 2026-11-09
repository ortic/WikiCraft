//! Redirects for old Confluence URLs, so bookmarks and links in tickets/chat keep working after
//! a migration:
//!
//! * `/display/{SPACE}/{Title}` (Server/DC)
//! * `/pages/viewpage.action?pageId=123` and `?spaceKey=KEY&title=Title`
//! * `/wiki/spaces/{KEY}/pages/{id}[/{slug}]` (Cloud) and `/spaces/{KEY}/pages/{id}[/{slug}]` (DC 8+)
//! * `/x/{tinyId}` and `/wiki/x/{tinyId}` (short links)

use std::collections::HashMap;

use axum::extract::{Path, Query, State};
use axum::response::Redirect;
use base64::Engine;
use uuid::Uuid;

use crate::app::AppState;
use crate::error::ApiResult;

/// Decodes a Confluence tiny-link id (little-endian page id, base64 with `-`/`_` substitutions
/// and stripped trailing `A`s/padding).
pub fn decode_tiny(tiny: &str) -> Option<u64> {
    let mut s: String = tiny
        .chars()
        .map(|c| match c {
            '-' => '/',
            '_' => '+',
            c => c,
        })
        .collect();
    if s.is_empty() || s.len() > 11 || !s.chars().all(|c| c.is_ascii_alphanumeric() || c == '/' || c == '+') {
        return None;
    }
    while s.len() < 11 {
        s.push('A');
    }
    let bytes = base64::engine::general_purpose::STANDARD_NO_PAD.decode(s.as_bytes()).ok()?;
    let mut buf = [0u8; 8];
    for (i, b) in bytes.iter().take(8).enumerate() {
        buf[i] = *b;
    }
    Some(u64::from_le_bytes(buf))
}

async fn workspace(state: &AppState) -> Option<Uuid> {
    sqlx::query_scalar("SELECT id FROM workspaces ORDER BY created_at LIMIT 1").fetch_optional(&state.db).await.ok().flatten()
}

async fn by_source_id(state: &AppState, id: &str) -> ApiResult<Redirect> {
    let ws = workspace(state).await;
    let target: Option<Uuid> = sqlx::query_scalar(
        "SELECT target_id FROM import_mappings WHERE workspace_id = $1 AND source = 'confluence' AND source_type = 'page' AND source_id = $2",
    )
    .bind(ws)
    .bind(id)
    .fetch_optional(&state.db)
    .await?;
    Ok(match target {
        Some(t) => Redirect::to(&format!("/pages/{t}")),
        None => Redirect::to(&format!("/missing?source=confluence%3A{}", urlencode(id))),
    })
}

async fn by_title(state: &AppState, space: &str, title: &str) -> ApiResult<Redirect> {
    let ws = workspace(state).await;
    // the space may have been imported under a different key
    let space_id: Option<Uuid> = sqlx::query_scalar(
        "SELECT target_id FROM import_mappings WHERE workspace_id = $1 AND source = 'confluence' AND source_type = 'space_key' AND lower(source_id) = lower($2)
         UNION ALL SELECT id FROM spaces WHERE workspace_id = $1 AND lower(key) = lower($2) LIMIT 1",
    )
    .bind(ws)
    .bind(space)
    .fetch_optional(&state.db)
    .await?;
    let page: Option<Uuid> = match space_id {
        Some(sid) => {
            sqlx::query_scalar("SELECT id FROM pages WHERE space_id = $1 AND lower(title) = lower($2) AND deleted_at IS NULL ORDER BY created_at LIMIT 1")
                .bind(sid)
                .bind(title)
                .fetch_optional(&state.db)
                .await?
        }
        None => None,
    };
    Ok(match page {
        Some(p) => Redirect::to(&format!("/pages/{p}")),
        None => Redirect::to(&format!("/missing?space={}&title={}", urlencode(space), urlencode(title))),
    })
}

fn urlencode(s: &str) -> String {
    s.bytes().map(|b| if b.is_ascii_alphanumeric() || b"-_.~".contains(&b) { (b as char).to_string() } else { format!("%{b:02X}") }).collect()
}

/// Confluence encodes spaces in `/display` titles as `+`.
fn plus_decode(s: &str) -> String {
    s.replace('+', " ")
}

pub async fn display(State(state): State<AppState>, Path((space, title)): Path<(String, String)>) -> ApiResult<Redirect> {
    by_title(&state, &space, &plus_decode(&title)).await
}

pub async fn display_space(State(state): State<AppState>, Path(space): Path<String>) -> ApiResult<Redirect> {
    let ws = workspace(&state).await;
    let key: Option<String> = sqlx::query_scalar(
        "SELECT s.key FROM spaces s WHERE s.workspace_id = $1 AND (lower(s.key) = lower($2) OR s.id IN (SELECT target_id FROM import_mappings WHERE source_type = 'space_key' AND lower(source_id) = lower($2))) LIMIT 1",
    )
    .bind(ws)
    .bind(&space)
    .fetch_optional(&state.db)
    .await?;
    Ok(Redirect::to(&format!("/spaces/{}", urlencode(&key.unwrap_or(space)))))
}

pub async fn viewpage(State(state): State<AppState>, Query(q): Query<HashMap<String, String>>) -> ApiResult<Redirect> {
    if let Some(id) = q.get("pageId") {
        return by_source_id(&state, id).await;
    }
    match (q.get("spaceKey"), q.get("title")) {
        (Some(s), Some(t)) => by_title(&state, s, t).await,
        _ => Ok(Redirect::to("/")),
    }
}

pub async fn cloud_page(State(state): State<AppState>, Path(params): Path<HashMap<String, String>>) -> ApiResult<Redirect> {
    match params.get("id") {
        Some(id) => by_source_id(&state, id).await,
        None => Ok(Redirect::to("/")),
    }
}

pub async fn tiny(State(state): State<AppState>, Path(tiny): Path<String>) -> ApiResult<Redirect> {
    match decode_tiny(&tiny) {
        Some(id) => by_source_id(&state, &id.to_string()).await,
        None => Ok(Redirect::to("/")),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn decodes_tiny_links() {
        // 65538 = 0x00010002 → LE bytes 02 00 01 00 … → base64 "AgABAAAAAAA" → tiny "AgAB"
        assert_eq!(decode_tiny("AgAB"), Some(65538));
        assert_eq!(decode_tiny("AgABAAAAAAA"), Some(65538));
        assert_eq!(decode_tiny("<script>"), None);
    }
}
