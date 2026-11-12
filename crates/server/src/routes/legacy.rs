//! Redirects for old Confluence URLs, so bookmarks and links in tickets/chat keep working after
//! a migration:
//!
//! * `/display/{SPACE}/{Title}` (Server/DC)
//! * `/pages/viewpage.action?pageId=123` and `?spaceKey=KEY&title=Title`
//! * `/wiki/spaces/{KEY}/pages/{id}[/{slug}]` (Cloud) and `/spaces/{KEY}/pages/{id}[/{slug}]` (DC 8+)
//! * `/x/{tinyId}` and `/wiki/x/{tinyId}` (short links)

use std::collections::HashMap;

use axum::extract::{Path, Query, State};
use axum::http::Uri;
use axum::response::Redirect;
use uuid::Uuid;

use crate::app::AppState;
use crate::auth::MaybeUser;
use crate::authz::Principal;
use crate::error::ApiResult;

pub use wikicraft_confluence::urls::decode_tiny;

async fn workspace(state: &AppState) -> Option<Uuid> {
    sqlx::query_scalar("SELECT id FROM workspaces ORDER BY created_at LIMIT 1").fetch_optional(&state.db).await.ok().flatten()
}

/// Where a redirect for an old address goes when no page the caller may open matches: the sign-in
/// (then back here) for visitors, the "doesn't exist" page for people signed in. Visitors learn
/// nothing about pages they can't see, signed-in people nothing about restricted ones.
fn not_found(p: &Principal, original: &Uri, missing: String) -> Redirect {
    if p.user_id().is_none() {
        let back = original.path_and_query().map(|pq| pq.as_str()).unwrap_or("/");
        return Redirect::to(&format!("/login?next={}", urlencode(back)));
    }
    Redirect::to(&missing)
}

async fn viewable(state: &AppState, p: &Principal, id: Uuid) -> bool {
    crate::authz::page_access(state, p, id).await.is_ok_and(|(a, _)| a.view)
}

async fn by_source_id(state: &AppState, p: &Principal, original: &Uri, id: &str) -> ApiResult<Redirect> {
    let ws = workspace(state).await;
    let target: Option<Uuid> = sqlx::query_scalar(
        "SELECT target_id FROM import_mappings WHERE workspace_id = $1 AND source = 'confluence' AND source_type = 'page' AND source_id = $2",
    )
    .bind(ws)
    .bind(id)
    .fetch_optional(&state.db)
    .await?;
    Ok(match target {
        Some(t) if viewable(state, p, t).await => Redirect::to(&format!("/pages/{t}")),
        _ => not_found(p, original, format!("/missing?source=confluence%3A{}", urlencode(id))),
    })
}

async fn by_title(state: &AppState, p: &Principal, original: &Uri, space: &str, title: &str) -> ApiResult<Redirect> {
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
    let space_id = match (space_id, ws) {
        (Some(s), _) => Some(s),
        (None, Some(ws)) => crate::paths::space_by_key(&state.db, ws, space).await?,
        (None, None) => None,
    };
    let page: Option<Uuid> = match space_id {
        // pages renamed or moved since keep their old addresses; the first one the caller may open
        Some(sid) => crate::paths::resolve_for(state, p, sid, title).await?,
        None => None,
    };
    Ok(match page {
        Some(id) => Redirect::to(&format!("/pages/{id}")),
        None => not_found(p, original, format!("/missing?space={}&title={}", urlencode(space), urlencode(title))),
    })
}

fn urlencode(s: &str) -> String {
    s.bytes().map(|b| if b.is_ascii_alphanumeric() || b"-_.~".contains(&b) { (b as char).to_string() } else { format!("%{b:02X}") }).collect()
}

/// Confluence encodes spaces in `/display` titles as `+`.
fn plus_decode(s: &str) -> String {
    s.replace('+', " ")
}

pub async fn display(State(state): State<AppState>, MaybeUser(user): MaybeUser, uri: Uri, Path((space, title)): Path<(String, String)>) -> ApiResult<Redirect> {
    let p = Principal::load(&state, user).await?;
    by_title(&state, &p, &uri, &space, &plus_decode(&title)).await
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

pub async fn viewpage(State(state): State<AppState>, MaybeUser(user): MaybeUser, uri: Uri, Query(q): Query<HashMap<String, String>>) -> ApiResult<Redirect> {
    let p = Principal::load(&state, user).await?;
    if let Some(id) = q.get("pageId") {
        return by_source_id(&state, &p, &uri, id).await;
    }
    match (q.get("spaceKey"), q.get("title")) {
        (Some(s), Some(t)) => by_title(&state, &p, &uri, s, t).await,
        _ => Ok(Redirect::to("/")),
    }
}

pub async fn cloud_page(
    State(state): State<AppState>,
    MaybeUser(user): MaybeUser,
    uri: Uri,
    Path(params): Path<HashMap<String, String>>,
) -> ApiResult<Redirect> {
    let p = Principal::load(&state, user).await?;
    match params.get("id") {
        Some(id) => by_source_id(&state, &p, &uri, id).await,
        None => Ok(Redirect::to("/")),
    }
}

pub async fn tiny(State(state): State<AppState>, MaybeUser(user): MaybeUser, uri: Uri, Path(tiny): Path<String>) -> ApiResult<Redirect> {
    let p = Principal::load(&state, user).await?;
    match decode_tiny(&tiny) {
        Some(id) => by_source_id(&state, &p, &uri, &id.to_string()).await,
        None => Ok(Redirect::to("/")),
    }
}

/// `GET /opensearch.xml`: lets browsers add the wiki as a search engine (address-bar search).
pub async fn opensearch(State(state): State<AppState>) -> axum::response::Response {
    use axum::response::IntoResponse;
    let base = state.config.base_url.trim_end_matches('/').to_string();
    let ws = workspace(&state).await;
    let name: String = sqlx::query_scalar("SELECT name FROM workspaces WHERE id = $1")
        .bind(ws)
        .fetch_optional(&state.db)
        .await
        .ok()
        .flatten()
        .unwrap_or_else(|| "WikiCraft".into());
    let esc = wikicraft_doc_model::html::escape;
    // ShortName is limited to 16 characters
    let short: String = name.chars().take(16).collect();
    let xml = format!(
        "<?xml version=\"1.0\" encoding=\"UTF-8\"?>\n<OpenSearchDescription xmlns=\"http://a9.com/-/spec/opensearch/1.1/\" xmlns:moz=\"http://www.mozilla.org/2006/browser/search/\">\n  <ShortName>{}</ShortName>\n  <Description>Search {}</Description>\n  <InputEncoding>UTF-8</InputEncoding>\n  <Image width=\"16\" height=\"16\" type=\"image/svg+xml\">{}/favicon.svg</Image>\n  <Url type=\"text/html\" method=\"get\" template=\"{}/search?q={{searchTerms}}\"/>\n  <moz:SearchForm>{}/search</moz:SearchForm>\n</OpenSearchDescription>\n",
        esc(&short),
        esc(&name),
        esc(&base),
        esc(&base),
        esc(&base)
    );
    ([(axum::http::header::CONTENT_TYPE, "application/opensearchdescription+xml; charset=utf-8")], xml).into_response()
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
