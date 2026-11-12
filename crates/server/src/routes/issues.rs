//! Issue links: key, title and state of a GitHub issue or pull request, a GitLab issue or merge
//! request, or a Jira issue, for the issue chips in pages (and Confluence's Jira macro).
//!
//! Fetched like link previews (public addresses only, DNS pinned, no proxy, short timeouts),
//! except that the trackers an admin configured may be private hosts and get the configured
//! credentials. Answers are cached for ten minutes. Guests can't look issues up: with a token,
//! the answer is what that account sees, not what the reader may see.

use axum::Json;
use axum::extract::{Query, State};
use base64::Engine;
use openidconnect::url::Url;
use serde::Deserialize;
use serde_json::{Value, json};

use super::unfurl::{Extra, allow, fetch_as};
use crate::app::AppState;
use crate::auth::CurrentUser;
use crate::config::Config;
use crate::error::{ApiError, ApiResult};

/// What an issue link points at.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum IssueRef {
    GitHub { owner: String, repo: String, number: u64, pull: bool },
    GitLab { host: String, project: String, number: u64, merge_request: bool },
    Jira { key: String },
}

impl IssueRef {
    pub fn tracker(&self) -> &'static str {
        match self {
            IssueRef::GitHub { .. } => "github",
            IssueRef::GitLab { .. } => "gitlab",
            IssueRef::Jira { .. } => "jira",
        }
    }

    /// The short form people know: `owner/repo#12`, `group/project!7`, `OPS-42`.
    pub fn key(&self) -> String {
        match self {
            IssueRef::GitHub { owner, repo, number, .. } => format!("{owner}/{repo}#{number}"),
            IssueRef::GitLab { project, number, merge_request, .. } => format!("{project}{}{number}", if *merge_request { "!" } else { "#" }),
            IssueRef::Jira { key } => key.clone(),
        }
    }
}

fn host_of(base: &str) -> Option<String> {
    Url::parse(base.trim()).ok().and_then(|u| u.host_str().map(str::to_ascii_lowercase))
}

/// `OPS-42`: a project key (a letter, then letters, digits or `_`) and a number.
pub fn is_jira_key(s: &str) -> bool {
    let Some((project, number)) = s.split_once('-') else { return false };
    project.len() >= 2
        && project.len() <= 30
        && project.starts_with(|c: char| c.is_ascii_uppercase())
        && project.bytes().all(|b| b.is_ascii_uppercase() || b.is_ascii_digit() || b == b'_')
        && !number.is_empty()
        && number.len() <= 9
        && number.bytes().all(|b| b.is_ascii_digit())
}

fn number(s: &str) -> Option<u64> {
    (!s.is_empty() && s.len() <= 9 && s.bytes().all(|b| b.is_ascii_digit())).then(|| s.parse().ok()).flatten()
}

fn name_ok(s: &str) -> bool {
    !s.is_empty() && s.len() <= 100 && s.bytes().all(|b| b.is_ascii_alphanumeric() || b"-_.".contains(&b)) && s != "." && s != ".."
}

/// An issue address (or a bare Jira key) the wiki can look up.
pub fn parse_ref(config: &Config, s: &str) -> Option<IssueRef> {
    let s = s.trim();
    if is_jira_key(s) {
        return Some(IssueRef::Jira { key: s.to_string() });
    }
    let u = Url::parse(s).ok()?;
    if !matches!(u.scheme(), "http" | "https") {
        return None;
    }
    let host = u.host_str()?.to_ascii_lowercase();
    let parts: Vec<&str> = u.path_segments()?.filter(|p| !p.is_empty()).collect();
    // (one host may serve several trackers: each shape is tried in turn)
    if host_of(&config.github_url).as_deref() == Some(host.as_str())
        && let [owner, repo, kind @ ("issues" | "pull" | "pulls"), n, ..] = parts.as_slice()
        && name_ok(owner)
        && name_ok(repo)
        && let Some(number) = number(n)
    {
        return Some(IssueRef::GitHub { owner: owner.to_string(), repo: repo.to_string(), number, pull: *kind != "issues" });
    }
    if config.jira_url.as_deref().and_then(host_of).as_deref() == Some(host.as_str())
        && let Some(i) = parts.iter().position(|p| *p == "browse")
        && let Some(key) = parts.get(i + 1).filter(|k| is_jira_key(k))
    {
        return Some(IssueRef::Jira { key: key.to_string() });
    }
    if host == "gitlab.com" || host_of(&config.gitlab_url).as_deref() == Some(host.as_str()) {
        let dash = parts.iter().position(|p| *p == "-")?;
        let project = &parts[..dash];
        if project.len() < 2 || project.len() > 20 || !project.iter().all(|p| name_ok(p)) {
            return None;
        }
        let (kind, n) = (parts.get(dash + 1)?, parts.get(dash + 2)?);
        let merge_request = match *kind {
            "issues" | "work_items" => false,
            "merge_requests" => true,
            _ => return None,
        };
        return Some(IssueRef::GitLab { host, project: project.join("/"), number: number(n)?, merge_request });
    }
    None
}

#[derive(Deserialize)]
pub struct IssueQuery {
    #[serde(rename = "ref")]
    reference: String,
}

/// `color` of the state: green open, blue in progress, purple done or merged, red closed
/// unmerged, grey anything else.
fn github_state(v: &Value, pull: bool) -> (&'static str, &'static str) {
    let closed = v["state"] == "closed";
    if pull || v["pull_request"].is_object() {
        if v["pull_request"]["merged_at"].is_string() {
            return ("Merged", "purple");
        }
        if closed {
            return ("Closed", "red");
        }
        return if v["draft"] == true { ("Draft", "grey") } else { ("Open", "green") };
    }
    match (closed, v["state_reason"].as_str()) {
        (false, _) => ("Open", "green"),
        (true, Some("not_planned")) => ("Not planned", "grey"),
        (true, _) => ("Closed", "purple"),
    }
}

fn gitlab_state(v: &Value, merge_request: bool) -> (&'static str, &'static str) {
    match v["state"].as_str() {
        Some("merged") => ("Merged", "purple"),
        Some("closed") if merge_request => ("Closed", "red"),
        Some("closed") => ("Closed", "purple"),
        Some("locked") => ("Locked", "grey"),
        _ if v["draft"] == true || v["work_in_progress"] == true => ("Draft", "grey"),
        _ => ("Open", "green"),
    }
}

fn jira_color(category: Option<&str>) -> &'static str {
    match category {
        Some("done") => "green",
        Some("indeterminate") => "blue",
        _ => "grey",
    }
}

fn base(url: &str) -> String {
    url.trim().trim_end_matches('/').to_string()
}

const NOT_CONNECTED: &str = "Jira isn't connected";

/// The configured Jira's address and what a request to it carries (credentials, and that it
/// may be a private host).
fn jira_connection(cfg: &Config) -> Option<(String, Extra)> {
    let jira = cfg.jira_url.as_deref().map(base)?;
    let mut extra = Extra { private_host: host_of(&jira), ..Default::default() };
    match (&cfg.jira_user, &cfg.jira_token) {
        (Some(u), Some(t)) => extra.headers.push(("authorization", format!("Basic {}", base64::engine::general_purpose::STANDARD.encode(format!("{u}:{t}"))))),
        (None, Some(t)) => extra.headers.push(("authorization", format!("Bearer {t}"))),
        _ => {}
    }
    Some((jira, extra))
}

/// `GET /api/unfurl/issue?ref=<address or Jira key>` → `{tracker, key, url, title, kind, status,
/// color}`; `error` instead of title and status when the tracker didn't answer (or, for Jira,
/// isn't configured).
pub async fn issue(State(state): State<AppState>, CurrentUser(user): CurrentUser, Query(q): Query<IssueQuery>) -> ApiResult<Json<Value>> {
    if user.role == "guest" {
        return Err(ApiError::Denied("guests can't look up issues".into()));
    }
    let r = parse_ref(&state.config, &q.reference).ok_or_else(|| ApiError::bad("not an issue address or Jira key"))?;
    let cfg = &state.config;
    let key = r.key();
    let cache_key = format!(
        "issue:{}:{}",
        r.tracker(),
        match &r {
            IssueRef::GitLab { host, .. } => format!("{host}/{key}"),
            _ => key.clone(),
        }
    );
    let (api, link, extra, accept) = match &r {
        IssueRef::GitHub { owner, repo, number, pull } => {
            let web = base(&cfg.github_url);
            let public = host_of(&web).as_deref() == Some("github.com");
            let api = if public { "https://api.github.com".to_string() } else { format!("{web}/api/v3") };
            let mut extra = Extra { private_host: (!public).then(|| host_of(&web)).flatten(), ..Default::default() };
            if let Some(t) = &cfg.github_token {
                extra.headers.push(("authorization", format!("Bearer {t}")));
            }
            let link = format!("{web}/{owner}/{repo}/{}/{number}", if *pull { "pull" } else { "issues" });
            (format!("{api}/repos/{owner}/{repo}/issues/{number}"), link, extra, "application/vnd.github+json")
        }
        IssueRef::GitLab { host, project, number, merge_request } => {
            let configured = host_of(&cfg.gitlab_url).as_deref() == Some(host.as_str());
            let web = if configured { base(&cfg.gitlab_url) } else { "https://gitlab.com".to_string() };
            let mut extra = Extra { private_host: (configured && host != "gitlab.com").then(|| host.clone()), ..Default::default() };
            if let (true, Some(t)) = (configured, &cfg.gitlab_token) {
                extra.headers.push(("private-token", t.clone()));
            }
            let kind = if *merge_request { "merge_requests" } else { "issues" };
            let id: String = project.bytes().map(|b| if b == b'/' { "%2F".to_string() } else { (b as char).to_string() }).collect();
            (format!("{web}/api/v4/projects/{id}/{kind}/{number}"), format!("{web}/{project}/-/{kind}/{number}"), extra, "application/json")
        }
        IssueRef::Jira { key } => {
            let Some((jira, extra)) = jira_connection(cfg) else {
                return Ok(Json(
                    json!({"tracker": "jira", "key": key, "url": null, "title": null, "kind": null, "status": null, "color": null, "error": NOT_CONNECTED}),
                ));
            };
            (format!("{jira}/rest/api/2/issue/{key}?fields=summary,status,issuetype"), format!("{jira}/browse/{key}"), extra, "application/json")
        }
    };

    let cached: Option<Value> = sqlx::query_scalar("SELECT data FROM link_previews WHERE url = $1 AND fetched_at > now() - interval '10 minutes'")
        .bind(&cache_key)
        .fetch_optional(&state.db)
        .await?;
    if let Some(data) = cached {
        return Ok(Json(data));
    }
    if !allow(user.id) {
        return Err(ApiError::bad("too many link previews; try again in a few minutes"));
    }
    let base_answer = json!({"tracker": r.tracker(), "key": key, "url": link});
    let fetched = match Url::parse(&api) {
        Ok(u) => fetch_as(&state, u, accept, |t| t.contains("json"), &extra).await.and_then(|b| Ok(serde_json::from_str::<Value>(&b)?)),
        Err(e) => Err(e.into()),
    };
    let mut data = base_answer;
    match fetched {
        Ok(v) => {
            let (title, kind, status, color) = match &r {
                IssueRef::GitHub { pull, .. } => {
                    let (s, c) = github_state(&v, *pull);
                    let kind = if *pull || v["pull_request"].is_object() { "pull request" } else { "issue" };
                    (v["title"].as_str().map(String::from), kind.to_string(), s.to_string(), c)
                }
                IssueRef::GitLab { merge_request, .. } => {
                    let (s, c) = gitlab_state(&v, *merge_request);
                    let kind = if *merge_request { "merge request" } else { "issue" };
                    (v["title"].as_str().map(String::from), kind.to_string(), s.to_string(), c)
                }
                IssueRef::Jira { .. } => {
                    let f = &v["fields"];
                    let status = f["status"]["name"].as_str().unwrap_or("").to_string();
                    let kind = f["issuetype"]["name"].as_str().unwrap_or("issue").to_string();
                    (f["summary"].as_str().map(String::from), kind, status, jira_color(f["status"]["statusCategory"]["key"].as_str()))
                }
            };
            let title: Option<String> = title.map(|t| t.split_whitespace().collect::<Vec<_>>().join(" ").chars().take(300).collect());
            let status: String = status.chars().take(40).collect();
            data["title"] = json!(title);
            data["kind"] = json!(kind.chars().take(40).collect::<String>());
            data["status"] = json!(status);
            data["color"] = json!(color);
            if title.is_none() {
                data["error"] = json!("issue unavailable");
            }
        }
        Err(e) => {
            // the reason stays in the log (it would map internal hosts and the token's reach)
            tracing::debug!(issue = %cache_key, "issue lookup failed: {e:#}");
            data["title"] = Value::Null;
            data["status"] = Value::Null;
            data["error"] = json!("issue unavailable");
        }
    }
    sqlx::query("INSERT INTO link_previews (url, data, fetched_at) VALUES ($1, $2, now()) ON CONFLICT (url) DO UPDATE SET data = $2, fetched_at = now()")
        .bind(&cache_key)
        .bind(&data)
        .execute(&state.db)
        .await?;
    Ok(Json(data))
}

/// Confluence's Jira macro columns, as Jira fields (`key` comes with every issue).
fn jira_field(column: &str) -> Option<String> {
    let c = column.trim().to_ascii_lowercase();
    let f = match c.as_str() {
        "key" | "" => return None,
        "type" | "issuetype" => "issuetype",
        "due" | "duedate" => "duedate",
        "resolved" | "resolutiondate" => "resolutiondate",
        "fixversion" | "fixversions" => "fixVersions",
        "version" | "versions" | "affectsversions" => "versions",
        "component" | "components" => "components",
        other if other.len() <= 64 && other.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'_') => other,
        _ => return None,
    };
    Some(f.to_string())
}

/// A field's value as text: names of people, statuses, versions …; lists joined.
fn display(v: &Value) -> Value {
    match v {
        Value::Null => Value::Null,
        Value::String(s) => Value::String(s.chars().take(300).collect()),
        Value::Number(_) | Value::Bool(_) => Value::String(v.to_string()),
        Value::Array(a) => {
            let parts: Vec<String> = a.iter().take(20).filter_map(|x| display(x).as_str().map(String::from)).collect();
            if parts.is_empty() { Value::Null } else { Value::String(parts.join(", ")) }
        }
        Value::Object(o) => ["displayName", "name", "value", "key"]
            .iter()
            .find_map(|k| o.get(*k).and_then(Value::as_str))
            .map(|s| Value::String(s.chars().take(300).collect()))
            .unwrap_or(Value::Null),
    }
}

#[derive(Deserialize)]
pub struct JiraSearchQuery {
    jql: String,
    #[serde(default)]
    columns: Option<String>,
    #[serde(default)]
    max: Option<u32>,
}

/// Columns of Confluence's Jira issues macro when it doesn't name any.
const DEFAULT_COLUMNS: &str = "key,summary,type,created,updated,due,assignee,reporter,priority,status,resolution";
/// issues per table at most
const MAX_ISSUES: u32 = 100;

/// `GET /api/unfurl/jira-search?jql=&columns=&max=` → `{searchUrl, columns, issues: [{key, url,
/// cells, statusColor}], more}`: the issues a JQL query finds in the connected Jira (Confluence's
/// Jira issues macro), as the configured account sees them; members only, cached five minutes.
pub async fn jira_search(State(state): State<AppState>, CurrentUser(user): CurrentUser, Query(q): Query<JiraSearchQuery>) -> ApiResult<Json<Value>> {
    if user.role == "guest" {
        return Err(ApiError::Denied("guests can't look up issues".into()));
    }
    let jql = q.jql.trim();
    if jql.is_empty() || jql.len() > 4000 {
        return Err(ApiError::bad("a JQL query of up to 4000 characters"));
    }
    let columns: Vec<String> = {
        let raw = q.columns.as_deref().map(str::trim).filter(|c| !c.is_empty()).unwrap_or(DEFAULT_COLUMNS);
        let mut cols: Vec<String> = raw.split(',').map(|c| c.trim().to_ascii_lowercase()).filter(|c| !c.is_empty()).take(20).collect();
        if !cols.iter().any(|c| c == "key") {
            cols.insert(0, "key".into());
        }
        cols
    };
    let max = q.max.unwrap_or(20).clamp(1, MAX_ISSUES);
    let Some((jira, extra)) = jira_connection(&state.config) else {
        return Ok(Json(json!({"columns": columns, "issues": [], "searchUrl": null, "more": false, "error": NOT_CONNECTED})));
    };
    let mut search = Url::parse(&format!("{jira}/issues/")).map_err(|e| ApiError::Internal(e.into()))?;
    search.query_pairs_mut().append_pair("jql", jql);
    let cache_key = format!("jira-search:{max}:{}:{jql}", columns.join(","));
    let cached: Option<Value> = sqlx::query_scalar("SELECT data FROM link_previews WHERE url = $1 AND fetched_at > now() - interval '5 minutes'")
        .bind(&cache_key)
        .fetch_optional(&state.db)
        .await?;
    if let Some(data) = cached {
        return Ok(Json(data));
    }
    if !allow(user.id) {
        return Err(ApiError::bad("too many link previews; try again in a few minutes"));
    }
    let mut fields: Vec<String> = columns.iter().filter_map(|c| jira_field(c)).collect();
    if !fields.iter().any(|f| f == "status") {
        fields.push("status".into());
    }
    // Jira Cloud searches at /rest/api/3/search/jql (its old search is gone); Data Center at /rest/api/2/search
    let cloud = host_of(&jira).is_some_and(|h| h.ends_with(".atlassian.net"));
    let path = if cloud { "/rest/api/3/search/jql" } else { "/rest/api/2/search" };
    let mut api = Url::parse(&format!("{jira}{path}")).map_err(|e| ApiError::Internal(e.into()))?;
    api.query_pairs_mut().append_pair("jql", jql).append_pair("fields", &fields.join(",")).append_pair("maxResults", &max.to_string());
    let fetched = fetch_as(&state, api, "application/json", |t| t.contains("json"), &extra).await.and_then(|b| Ok(serde_json::from_str::<Value>(&b)?));
    let data = match fetched {
        Ok(v) if v["issues"].is_array() => {
            let issues: Vec<Value> = v["issues"]
                .as_array()
                .into_iter()
                .flatten()
                .take(max as usize)
                .map(|i| {
                    let key = i["key"].as_str().unwrap_or("").to_string();
                    let f = &i["fields"];
                    let mut cells = serde_json::Map::new();
                    for c in &columns {
                        if let Some(field) = jira_field(c) {
                            cells.insert(c.clone(), display(&f[field.as_str()]));
                        }
                    }
                    json!({
                        "key": key,
                        "url": format!("{jira}/browse/{key}"),
                        "cells": cells,
                        "statusColor": jira_color(f["status"]["statusCategory"]["key"].as_str()),
                    })
                })
                .collect();
            let more = v["total"].as_u64().is_some_and(|t| t > issues.len() as u64) || v["nextPageToken"].is_string() || v["isLast"] == false;
            json!({"columns": columns, "issues": issues, "searchUrl": search.as_str(), "more": more, "total": v["total"]})
        }
        other => {
            if let Err(e) = &other {
                tracing::debug!("Jira search failed: {e:#}");
            }
            json!({"columns": columns, "issues": [], "searchUrl": search.as_str(), "more": false, "error": "Jira didn't answer this query"})
        }
    };
    sqlx::query("INSERT INTO link_previews (url, data, fetched_at) VALUES ($1, $2, now()) ON CONFLICT (url) DO UPDATE SET data = $2, fetched_at = now()")
        .bind(&cache_key)
        .bind(&data)
        .execute(&state.db)
        .await?;
    Ok(Json(data))
}

#[cfg(test)]
mod tests {
    use super::*;
    use clap::Parser;

    #[derive(Parser)]
    struct Cli {
        #[command(flatten)]
        config: Config,
    }

    fn config(extra: &[&str]) -> Config {
        let mut args = vec!["wikicraft"];
        args.extend_from_slice(extra);
        Cli::try_parse_from(args).unwrap().config
    }

    #[test]
    fn recognises_issue_addresses_and_keys() {
        let c = config(&["--jira-url", "https://jira.acme.internal/", "--gitlab-url", "https://git.acme.test"]);
        let gh = |o: &str, r: &str, n, pull| Some(IssueRef::GitHub { owner: o.into(), repo: r.into(), number: n, pull });
        assert_eq!(parse_ref(&c, "https://github.com/rust-lang/rust/issues/42"), gh("rust-lang", "rust", 42, false));
        assert_eq!(parse_ref(&c, "https://github.com/tokio-rs/axum/pull/7#discussion"), gh("tokio-rs", "axum", 7, true));
        assert_eq!(parse_ref(&c, "https://github.com/a/b/blob/main/x.rs"), None);
        assert_eq!(parse_ref(&c, "https://github.com/../b/issues/1"), None);
        assert_eq!(
            parse_ref(&c, "https://gitlab.com/gitlab-org/gitlab/-/merge_requests/123"),
            Some(IssueRef::GitLab { host: "gitlab.com".into(), project: "gitlab-org/gitlab".into(), number: 123, merge_request: true })
        );
        assert_eq!(
            parse_ref(&c, "https://git.acme.test/ops/infra/tools/-/issues/9"),
            Some(IssueRef::GitLab { host: "git.acme.test".into(), project: "ops/infra/tools".into(), number: 9, merge_request: false })
        );
        assert_eq!(parse_ref(&c, "https://elsewhere.test/a/b/-/issues/9"), None, "only gitlab.com and the configured GitLab");
        assert_eq!(parse_ref(&c, "OPS-42"), Some(IssueRef::Jira { key: "OPS-42".into() }));
        assert_eq!(parse_ref(&c, "https://jira.acme.internal/browse/OPS_2-7?focused=1"), Some(IssueRef::Jira { key: "OPS_2-7".into() }));
        for no in ["ops-42", "O-1", "OPS-", "OPS-4a", "1OPS-3", "file:///etc/passwd", "https://jira.other/browse/OPS-1"] {
            assert_eq!(parse_ref(&c, no), None, "{no}");
        }
        assert_eq!(parse_ref(&c, "https://github.com/a/b/issues/1").unwrap().key(), "a/b#1");
        assert_eq!(parse_ref(&c, "https://gitlab.com/g/p/-/merge_requests/3").unwrap().key(), "g/p!3");
    }

    #[test]
    fn jira_columns_and_values() {
        assert_eq!(jira_field("type").as_deref(), Some("issuetype"));
        assert_eq!(jira_field("Due").as_deref(), Some("duedate"));
        assert_eq!(jira_field("customfield_10020").as_deref(), Some("customfield_10020"));
        assert_eq!(jira_field("key"), None);
        assert_eq!(jira_field("a,b"), None);
        assert_eq!(display(&json!({"displayName": "Ada", "name": "ada"})), json!("Ada"));
        assert_eq!(display(&json!([{"name": "1.0"}, {"name": "1.1"}])), json!("1.0, 1.1"));
        assert_eq!(display(&json!(3)), json!("3"));
        assert_eq!(display(&json!(null)), Value::Null);
    }

    #[test]
    fn states_and_colours() {
        assert_eq!(github_state(&json!({"state": "open"}), false), ("Open", "green"));
        assert_eq!(github_state(&json!({"state": "closed", "state_reason": "not_planned"}), false), ("Not planned", "grey"));
        assert_eq!(github_state(&json!({"state": "closed", "pull_request": {"merged_at": "2026-01-01T00:00:00Z"}}), true), ("Merged", "purple"));
        assert_eq!(github_state(&json!({"state": "open", "draft": true, "pull_request": {}}), true), ("Draft", "grey"));
        assert_eq!(gitlab_state(&json!({"state": "opened"}), false), ("Open", "green"));
        assert_eq!(gitlab_state(&json!({"state": "closed"}), true), ("Closed", "red"));
        assert_eq!(jira_color(Some("indeterminate")), "blue");
    }
}
