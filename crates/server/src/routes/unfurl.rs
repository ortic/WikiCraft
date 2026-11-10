//! Link previews ("smart links"): title, description and site name of an external page.
//!
//! The server fetches the page with the webhook protections: http(s) only, public addresses
//! only (unless WIKICRAFT_WEBHOOK_ALLOW_PRIVATE), DNS pinned for the request, no proxy, every
//! redirect re-checked, at most 512 KB of HTML, short timeouts. Results (public metadata) are
//! cached for a day, failures for an hour. Images from the target are never fetched or shown.

use std::collections::{HashMap, VecDeque};
use std::sync::{LazyLock, Mutex};
use std::time::{Duration, Instant};

use axum::Json;
use axum::extract::{Query, State};
use serde::Deserialize;
use serde_json::{Value, json};
use uuid::Uuid;

use crate::app::AppState;
use crate::auth::CurrentUser;
use crate::error::{ApiError, ApiResult};
use crate::webhooks::{resolve, validate_url};

const MAX_BODY: usize = 512 * 1024;
const MAX_REDIRECTS: usize = 3;

static RECENT: LazyLock<Mutex<HashMap<Uuid, VecDeque<Instant>>>> = LazyLock::new(Default::default);

/// 60 previews per person per 10 minutes (per replica)
fn allow(user: Uuid) -> bool {
    let mut m = RECENT.lock().unwrap_or_else(|e| e.into_inner());
    let q = m.entry(user).or_default();
    let now = Instant::now();
    while q.front().is_some_and(|t| now.duration_since(*t) > Duration::from_secs(600)) {
        q.pop_front();
    }
    if q.len() >= 60 {
        return false;
    }
    q.push_back(now);
    true
}

#[derive(Deserialize)]
pub struct UnfurlQuery {
    url: String,
}

/// `GET /api/unfurl?url=` → `{url, title, description, siteName}` (fields may be null)
pub async fn unfurl(State(state): State<AppState>, CurrentUser(user): CurrentUser, Query(q): Query<UnfurlQuery>) -> ApiResult<Json<Value>> {
    let url = validate_url(&q.url).map_err(|e| ApiError::bad(format!("{e}")))?;
    let key = url.to_string();
    let cached: Option<Value> = sqlx::query_scalar(
        "SELECT data FROM link_previews WHERE url = $1
           AND fetched_at > now() - CASE WHEN data ? 'error' THEN interval '1 hour' ELSE interval '1 day' END",
    )
    .bind(&key)
    .fetch_optional(&state.db)
    .await?;
    if let Some(data) = cached {
        return Ok(Json(data));
    }
    if !allow(user.id) {
        return Err(ApiError::bad("too many link previews; try again in a few minutes"));
    }
    let data = match fetch(&state, url).await {
        Ok(html) => {
            let m = parse(&html);
            json!({ "url": key, "title": m.title, "description": m.description, "siteName": m.site })
        }
        Err(e) => {
            // the reason stays in the log: telling callers would map internal names and ports
            tracing::debug!(url = %key, "link preview failed: {e:#}");
            json!({ "url": key, "title": null, "description": null, "siteName": null, "error": "preview unavailable" })
        }
    };
    sqlx::query("INSERT INTO link_previews (url, data, fetched_at) VALUES ($1, $2, now()) ON CONFLICT (url) DO UPDATE SET data = $2, fetched_at = now()")
        .bind(&key)
        .bind(&data)
        .execute(&state.db)
        .await?;
    // keep the cache small: drop a batch of expired entries
    sqlx::query("DELETE FROM link_previews WHERE url IN (SELECT url FROM link_previews WHERE fetched_at < now() - interval '1 day' LIMIT 200)")
        .execute(&state.db)
        .await?;
    Ok(Json(data))
}

async fn fetch(state: &AppState, mut url: openidconnect::url::Url) -> anyhow::Result<String> {
    for _ in 0..=MAX_REDIRECTS {
        let addr = resolve(&url, state.config.unfurl_allow_private).await?;
        let client = openidconnect::reqwest::Client::builder()
            .redirect(openidconnect::reqwest::redirect::Policy::none())
            .no_proxy()
            .timeout(Duration::from_secs(6))
            .user_agent("WikiCraft-LinkPreview/1")
            .resolve(url.host_str().unwrap_or_default(), addr)
            .build()?;
        let mut resp = client.get(url.as_str()).header("accept", "text/html,application/xhtml+xml").send().await?;
        let status = resp.status();
        if status.is_redirection() {
            let loc = resp.headers().get("location").and_then(|v| v.to_str().ok()).ok_or_else(|| anyhow::anyhow!("redirect without a location"))?;
            // every hop is validated and resolved again (no redirect into the private network)
            url = validate_url(url.join(loc)?.as_str())?;
            continue;
        }
        anyhow::ensure!(status.is_success(), "HTTP {}", status.as_u16());
        let ctype = resp.headers().get("content-type").and_then(|v| v.to_str().ok()).unwrap_or("").to_ascii_lowercase();
        anyhow::ensure!(ctype.contains("text/html") || ctype.contains("xhtml"), "not an HTML page");
        let mut body: Vec<u8> = Vec::new();
        while let Some(chunk) = resp.chunk().await? {
            body.extend_from_slice(&chunk);
            if body.len() >= MAX_BODY {
                body.truncate(MAX_BODY);
                break;
            }
        }
        return Ok(String::from_utf8_lossy(&body).into_owned());
    }
    anyhow::bail!("too many redirects")
}

#[derive(Default, Debug, PartialEq)]
struct Meta {
    title: Option<String>,
    description: Option<String>,
    site: Option<String>,
}

fn decode(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    let mut rest = s;
    while let Some(i) = rest.find('&') {
        out.push_str(&rest[..i]);
        let after = &rest[i + 1..];
        match after.find(';').filter(|e| *e <= 10) {
            Some(end) => {
                let ent = &after[..end];
                let ch = match ent {
                    "amp" => Some('&'),
                    "lt" => Some('<'),
                    "gt" => Some('>'),
                    "quot" => Some('"'),
                    "apos" => Some('\''),
                    "nbsp" => Some(' '),
                    _ => ent
                        .strip_prefix("#x")
                        .or_else(|| ent.strip_prefix("#X"))
                        .and_then(|h| u32::from_str_radix(h, 16).ok())
                        .or_else(|| ent.strip_prefix('#').and_then(|d| d.parse().ok()))
                        .and_then(char::from_u32),
                };
                match ch {
                    Some(c) => {
                        out.push(c);
                        rest = &after[end + 1..];
                    }
                    None => {
                        out.push('&');
                        rest = after;
                    }
                }
            }
            None => {
                out.push('&');
                rest = after;
            }
        }
    }
    out.push_str(rest);
    out
}

fn clean(s: &str, max: usize) -> Option<String> {
    // twice: some sites escape their entities twice (`&amp;nbsp;`); React escapes the result
    let t = decode(&decode(s)).split_whitespace().collect::<Vec<_>>().join(" ");
    let t: String = t.chars().take(max).collect();
    (!t.is_empty()).then_some(t)
}

/// The value of attribute `name` in a tag's source (`name="…"`, `name='…'` or bare).
fn attr<'a>(tag: &'a str, lower: &str, name: &str) -> Option<&'a str> {
    let mut from = 0;
    while let Some(i) = lower[from..].find(name) {
        let at = from + i;
        from = at + name.len();
        let before_ok = at == 0 || lower.as_bytes()[at - 1].is_ascii_whitespace();
        let rest = lower[from..].trim_start();
        if !before_ok || !rest.starts_with('=') {
            continue;
        }
        let vstart = tag.len() - rest.len() + 1;
        let v = tag[vstart..].trim_start();
        let offset = tag.len() - v.len();
        return match v.chars().next() {
            Some(q @ ('"' | '\'')) => tag[offset + 1..].find(q).map(|e| &tag[offset + 1..offset + 1 + e]),
            Some(_) => Some(&tag[offset..offset + v.find(|c: char| c.is_whitespace() || c == '>' || c == '/').unwrap_or(v.len())]),
            None => None,
        };
    }
    None
}

fn parse(html: &str) -> Meta {
    // ASCII lower-casing keeps byte offsets
    let lower = html.to_ascii_lowercase();
    let mut metas: HashMap<String, String> = HashMap::new();
    let mut from = 0;
    while let Some(i) = lower[from..].find("<meta") {
        let start = from + i;
        let Some(end) = lower[start..].find('>') else { break };
        let (tag, ltag) = (&html[start..start + end], &lower[start..start + end]);
        from = start + end;
        let key = attr(tag, ltag, "property").or_else(|| attr(tag, ltag, "name")).map(|k| k.to_ascii_lowercase());
        if let (Some(k), Some(v)) = (key, attr(tag, ltag, "content")) {
            metas.entry(k).or_insert_with(|| v.to_string());
        }
        if metas.len() > 200 {
            break;
        }
    }
    let title_tag = lower.find("<title").and_then(|i| {
        let open = i + lower[i..].find('>')? + 1;
        let close = open + lower[open..].find("</title")?;
        Some(&html[open..close])
    });
    // the first non-empty candidate wins (sites ship `og:title=""` next to a real <title>)
    let get = |keys: &[&str], max: usize| keys.iter().find_map(|k| metas.get(*k).and_then(|v| clean(v, max)));
    Meta {
        title: get(&["og:title", "twitter:title"], 200).or_else(|| title_tag.and_then(|t| clean(t, 200))),
        description: get(&["og:description", "description", "twitter:description"], 300),
        site: get(&["og:site_name", "application-name"], 80),
    }
}

#[cfg(test)]
mod tests {
    #[test]
    fn page_metadata() {
        let html = r#"<html><head><title>  Fallback &amp; title </title>
            <meta property="og:title" content="Release notes &#8212; v2.1">
            <META NAME=description CONTENT='What&apos;s new in 2.1'>
            <meta property="og:site_name" content="Acme Docs" />
            </head></html>"#;
        let m = super::parse(html);
        assert_eq!(m.title.as_deref(), Some("Release notes \u{2014} v2.1"));
        assert_eq!(m.description.as_deref(), Some("What's new in 2.1"));
        assert_eq!(m.site.as_deref(), Some("Acme Docs"));
        let m =
            super::parse(r#"<meta property="og:title" content=""><title> Real title </title><meta property="og:description" content="Chapter&amp;nbsp;13">"#);
        assert_eq!(m.title.as_deref(), Some("Real title"), "empty og:title falls back to <title>");
        assert_eq!(m.description.as_deref(), Some("Chapter 13"), "double-escaped entities");
        let m = super::parse("<title>Only a <b>title</b></title>");
        assert_eq!(m.title.as_deref(), Some("Only a <b>title</b>"), "text only: React escapes it");
        assert_eq!(super::parse("<meta content=x>"), super::Meta::default());
    }
}
