//! Built-in MCP (Model Context Protocol) server at `/api/mcp` (streamable HTTP).
//!
//! AI assistants authenticate with a personal API token (`Authorization: Bearer wct_…`) and act
//! with exactly that user's permissions: every tool calls the regular route handlers, so all
//! authorization rules (space roles, page restrictions, …) apply unchanged.

use axum::extract::{FromRequestParts, Json as AxJson, Path, Query, State};
use rmcp::handler::server::tool::Extension;
use rmcp::handler::server::wrapper::Parameters;
use rmcp::model::{CallToolResult, ContentBlock, Implementation, ServerCapabilities, ServerConfig};
use rmcp::transport::streamable_http_server::session::local::LocalSessionManager;
use rmcp::transport::streamable_http_server::{StreamableHttpServerConfig, StreamableHttpService};
use rmcp::{ErrorData, ServerHandler, tool, tool_handler, tool_router};
use schemars::JsonSchema;
use serde::Deserialize;
use serde_json::{Value, json};
use uuid::Uuid;
use wikicraft_doc_model::{Node, markdown};

use crate::app::AppState;
use crate::auth::{CurrentUser, MaybeUser};
use crate::error::ApiError;

#[derive(Clone)]
pub struct WikiMcp {
    state: AppState,
}

type ToolResult = Result<CallToolResult, ErrorData>;

fn ok_json(v: Value) -> ToolResult {
    Ok(CallToolResult::success(vec![ContentBlock::text(serde_json::to_string_pretty(&v).unwrap_or_default())]))
}

fn ok_text(s: impl Into<String>) -> ToolResult {
    Ok(CallToolResult::success(vec![ContentBlock::text(s)]))
}

fn api_err(e: ApiError) -> ToolResult {
    Ok(CallToolResult::error(vec![ContentBlock::text(e.to_string())]))
}

fn strip_tags(s: &str) -> String {
    s.replace("<b>", "").replace("</b>", "").replace("&quot;", "\"").replace("&#39;", "'").replace("&lt;", "<").replace("&gt;", ">").replace("&amp;", "&")
}

#[derive(Deserialize, JsonSchema)]
pub struct SearchArgs {
    /// Full-text query
    pub query: String,
    /// Restrict to one space (space key)
    pub space_key: Option<String>,
    /// Max results (default 10)
    pub limit: Option<usize>,
}

#[derive(Deserialize, JsonSchema)]
pub struct PageArgs {
    /// Page id (UUID)
    pub page_id: String,
}

#[derive(Deserialize, JsonSchema)]
pub struct ChildrenArgs {
    /// Parent page id; omit and give `space_key` to list the space's top-level pages
    pub page_id: Option<String>,
    pub space_key: Option<String>,
}

#[derive(Deserialize, JsonSchema)]
pub struct CreatePageArgs {
    pub space_key: String,
    pub title: String,
    /// Page body in Markdown (GFM: tables, task lists, `> [!NOTE]` callouts)
    pub markdown: String,
    /// Optional parent page id
    pub parent_id: Option<String>,
}

#[derive(Deserialize, JsonSchema)]
pub struct UpdatePageArgs {
    pub page_id: String,
    /// New page body in Markdown (replaces the content)
    pub markdown: String,
    /// Version comment
    pub message: Option<String>,
}

#[derive(Deserialize, JsonSchema)]
pub struct AppendArgs {
    pub page_id: String,
    /// Markdown to append at the end of the page
    pub markdown: String,
}

#[derive(Deserialize, JsonSchema)]
pub struct CommentArgs {
    pub page_id: String,
    /// Comment text (Markdown)
    pub text: String,
}

#[derive(Deserialize, JsonSchema)]
pub struct RetrieveArgs {
    /// The question or topic, in natural language (e.g. "How do we rotate the VPN certificates?")
    pub query: String,
    /// Only these spaces (space keys)
    pub space_keys: Option<Vec<String>>,
    /// Only pages with one of these labels
    pub labels: Option<Vec<String>>,
    /// Max passages (default 8, at most 30)
    pub limit: Option<usize>,
}

#[derive(Deserialize, JsonSchema)]
pub struct QueryArgs {
    /// A CQL query, e.g. `space = ENG and label = runbook and lastModified > now("-30d") order by title`.
    /// Fields: space, space.title, title, text, label, type (page, blogpost, folder), creator,
    /// contributor, mention, favourite, watcher, created, lastModified, id, parent, ancestor,
    /// status, property[Name]; operators = != ~ !~ < <= > >= in, not in; and/or/not; functions
    /// currentUser(), now("-7d"), startOfWeek(), startOfMonth("-1").
    pub cql: String,
    /// Max results (default 25, at most 100)
    pub limit: Option<usize>,
}

#[derive(Deserialize, JsonSchema)]
pub struct RecentArgs {
    /// Max results (default 20)
    pub limit: Option<usize>,
}

fn parse_id(s: &str) -> Result<Uuid, ApiError> {
    s.trim().parse().map_err(|_| ApiError::bad("invalid page id"))
}

impl WikiMcp {
    async fn authenticate(&self, parts: &axum::http::request::Parts) -> Result<(CurrentUser, crate::auth::TokenScope), ApiError> {
        let mut parts = parts.clone();
        parts.extensions.clear();
        // bearer tokens only: cookie sessions would need the CSRF header
        if !parts.headers.get(axum::http::header::AUTHORIZATION).and_then(|v| v.to_str().ok()).is_some_and(|v| v.starts_with("Bearer ")) {
            return Err(ApiError::Unauthorized);
        }
        // every MCP call is a POST: the tools check the token's scope themselves
        parts.extensions.insert(crate::auth::ScopeCheckedByCaller);
        let user = MaybeUser::from_request_parts(&mut parts, &self.state).await?.0.ok_or(ApiError::Unauthorized)?;
        let scope = parts.extensions.get::<crate::auth::TokenScope>().copied().unwrap_or(crate::auth::TokenScope::Write);
        Ok((CurrentUser(user), scope))
    }

    /// For the reading tools.
    async fn user(&self, parts: &axum::http::request::Parts) -> Result<CurrentUser, ApiError> {
        Ok(self.authenticate(parts).await?.0)
    }

    /// For the tools that change something: not with a read-only token.
    async fn writer(&self, parts: &axum::http::request::Parts) -> Result<CurrentUser, ApiError> {
        match self.authenticate(parts).await? {
            (_, crate::auth::TokenScope::Read) => Err(ApiError::Denied("this API token is read-only".into())),
            (u, _) => Ok(u),
        }
    }

    fn url(&self, page: &str) -> String {
        format!("{}/pages/{page}", self.state.config.base_url.trim_end_matches('/'))
    }
}

#[tool_router]
impl WikiMcp {
    pub fn new(state: AppState) -> Self {
        Self { state }
    }

    #[tool(description = "List the wiki spaces you can access (key, name, description, number of pages).")]
    async fn list_spaces(&self, Extension(parts): Extension<axum::http::request::Parts>) -> ToolResult {
        let user = match self.user(&parts).await {
            Ok(u) => u,
            Err(e) => return api_err(e),
        };
        match crate::routes::spaces::list(State(self.state.clone()), MaybeUser(Some(user.0)), axum::extract::Query(crate::routes::spaces::ListQuery::default()))
            .await
        {
            Ok(AxJson(spaces)) => {
                let v = serde_json::to_value(spaces).unwrap_or_default();
                let list: Vec<Value> = v
                    .as_array()
                    .into_iter()
                    .flatten()
                    .map(|s| json!({"key": s["key"], "name": s["name"], "description": s["description"], "pages": s["pageCount"], "yourRole": s["role"]}))
                    .collect();
                ok_json(json!(list))
            }
            Err(e) => api_err(e),
        }
    }

    #[tool(description = "Full-text search across wiki pages. Returns page ids, titles, spaces and matching snippets.")]
    async fn search(&self, Extension(parts): Extension<axum::http::request::Parts>, Parameters(a): Parameters<SearchArgs>) -> ToolResult {
        let user = match self.user(&parts).await {
            Ok(u) => u,
            Err(e) => return api_err(e),
        };
        let q = crate::routes::search::SearchQuery {
            q: a.query,
            space: a.space_key,
            limit: Some(a.limit.unwrap_or(10).min(50)),
            kind: None,
            label: None,
            contributor: None,
            days: None,
        };
        match crate::routes::search::search(State(self.state.clone()), MaybeUser(Some(user.0)), Query(q)).await {
            Ok(AxJson(hits)) => {
                let v = serde_json::to_value(hits).unwrap_or_default();
                let list: Vec<Value> = v
                    .as_array()
                    .into_iter()
                    .flatten()
                    .map(|h| json!({"pageId": h["pageId"], "title": h["title"], "space": h["spaceKey"], "snippet": strip_tags(h["snippet"].as_str().unwrap_or("")), "url": self.url(h["pageId"].as_str().unwrap_or(""))}))
                    .collect();
                ok_json(json!(list))
            }
            Err(e) => api_err(e),
        }
    }

    #[tool(
        description = "Find the passages of the wiki that best answer a question (semantic + keyword retrieval over page sections and attached files). Returns the best-matching sections, each with its page, heading path and a URL to the exact section — the best first step to answer a question from the wiki; cite the URLs. Use get_page to read a whole page."
    )]
    async fn retrieve(&self, Extension(parts): Extension<axum::http::request::Parts>, Parameters(a): Parameters<RetrieveArgs>) -> ToolResult {
        let user = match self.user(&parts).await {
            Ok(u) => u,
            Err(e) => return api_err(e),
        };
        let p = match crate::authz::Principal::load(&self.state, Some(user.0)).await {
            Ok(p) => p,
            Err(e) => return api_err(e),
        };
        let opts = crate::retrieval::Options {
            query: a.query,
            spaces: a.space_keys.unwrap_or_default(),
            labels: a.labels.unwrap_or_default(),
            limit: a.limit.unwrap_or(8).clamp(1, 30),
            per_page: 3,
            mode: crate::retrieval::Mode::Auto,
            max_chars: Some(40_000),
        };
        match crate::retrieval::retrieve(&self.state, &p, &opts).await {
            Ok(r) if r.passages.is_empty() => ok_text("No passages match. Try other words, or search / query_pages to browse."),
            Ok(r) => ok_text(crate::retrieval::to_markdown(&r.passages)),
            Err(e) => api_err(e),
        }
    }

    #[tool(
        description = "Find pages with a structured CQL query (space, label, type, author, dates, properties, ancestors…), Confluence's query language. Returns page ids, titles, spaces, labels and last update."
    )]
    async fn query_pages(&self, Extension(parts): Extension<axum::http::request::Parts>, Parameters(a): Parameters<QueryArgs>) -> ToolResult {
        let user = match self.user(&parts).await {
            Ok(u) => u,
            Err(e) => return api_err(e),
        };
        let p = match crate::authz::Principal::load(&self.state, Some(user.0)).await {
            Ok(p) => p,
            Err(e) => return api_err(e),
        };
        match crate::cql::search(&self.state, &p, &a.cql, None, a.limit.unwrap_or(25).clamp(1, 100)).await {
            Ok(rows) => ok_json(json!(rows
                .iter()
                .map(|r| json!({"pageId": r.id, "title": r.title, "space": r.space_key, "labels": r.labels, "updatedAt": r.updated_at, "url": self.url(&r.id.to_string())}))
                .collect::<Vec<_>>())),
            Err(e) => api_err(e),
        }
    }

    #[tool(description = "Read a page: title, space, breadcrumbs, labels, last update and the full content as Markdown.")]
    async fn get_page(&self, Extension(parts): Extension<axum::http::request::Parts>, Parameters(a): Parameters<PageArgs>) -> ToolResult {
        let user = match self.user(&parts).await {
            Ok(u) => u,
            Err(e) => return api_err(e),
        };
        let id = match parse_id(&a.page_id) {
            Ok(i) => i,
            Err(e) => return api_err(e),
        };
        match crate::routes::pages::get(State(self.state.clone()), MaybeUser(Some(user.0)), Path(id)).await {
            Ok(AxJson(p)) => {
                let doc = Node::from_json(&p["content"]).unwrap_or_else(|_| Node::empty_doc());
                let crumbs: Vec<&str> = p["breadcrumbs"].as_array().into_iter().flatten().filter_map(|c| c["title"].as_str()).collect();
                let md = format!(
                    "# {}\n\nSpace: {} · Path: {} · Labels: {} · Updated {} by {} · Version {} · Can edit: {}\nURL: {}\n\n---\n\n{}",
                    p["title"].as_str().unwrap_or(""),
                    p["spaceKey"].as_str().unwrap_or(""),
                    crumbs.join(" / "),
                    p["labels"].as_array().map(|l| l.iter().filter_map(|x| x.as_str()).collect::<Vec<_>>().join(", ")).unwrap_or_default(),
                    p["updatedAt"].as_str().unwrap_or(""),
                    p["updatedByName"].as_str().unwrap_or("?"),
                    p["version"],
                    p["access"]["edit"],
                    self.url(&id.to_string()),
                    markdown::pm_to_markdown(&doc)
                );
                ok_text(md)
            }
            Err(e) => api_err(e),
        }
    }

    #[tool(description = "List child pages of a page, or the top-level pages of a space.")]
    async fn list_children(&self, Extension(parts): Extension<axum::http::request::Parts>, Parameters(a): Parameters<ChildrenArgs>) -> ToolResult {
        let user = match self.user(&parts).await {
            Ok(u) => u,
            Err(e) => return api_err(e),
        };
        if let Some(pid) = &a.page_id {
            let id = match parse_id(pid) {
                Ok(i) => i,
                Err(e) => return api_err(e),
            };
            let q: crate::routes::pages::ChildrenQuery = serde_json::from_value(json!({"depth": 1})).expect("valid");
            return match crate::routes::pages::children(State(self.state.clone()), MaybeUser(Some(user.0)), Path(id), Query(q)).await {
                Ok(AxJson(rows)) => ok_json(serde_json::to_value(rows).unwrap_or_default()),
                Err(e) => api_err(e),
            };
        }
        let Some(key) = a.space_key else { return api_err(ApiError::bad("page_id or space_key required")) };
        match crate::routes::spaces::tree(State(self.state.clone()), MaybeUser(Some(user.0)), Path(key)).await {
            Ok(AxJson(t)) => {
                let roots: Vec<Value> = t["nodes"]
                    .as_array()
                    .into_iter()
                    .flatten()
                    .filter(|n| n["parentId"].is_null())
                    .map(|n| json!({"id": n["id"], "title": n["title"]}))
                    .collect();
                ok_json(json!({"homePageId": t["homePageId"], "topLevelPages": roots}))
            }
            Err(e) => api_err(e),
        }
    }

    #[tool(description = "Recently updated pages you can see.")]
    async fn recent_changes(&self, Extension(parts): Extension<axum::http::request::Parts>, Parameters(a): Parameters<RecentArgs>) -> ToolResult {
        let user = match self.user(&parts).await {
            Ok(u) => u,
            Err(e) => return api_err(e),
        };
        match crate::routes::search::recent(State(self.state.clone()), user).await {
            Ok(AxJson(v)) => {
                let mut list = v["updated"].as_array().cloned().unwrap_or_default();
                list.truncate(a.limit.unwrap_or(20));
                ok_json(json!(list))
            }
            Err(e) => api_err(e),
        }
    }

    #[tool(description = "Create a new page from Markdown. Returns the new page id and URL.")]
    async fn create_page(&self, Extension(parts): Extension<axum::http::request::Parts>, Parameters(a): Parameters<CreatePageArgs>) -> ToolResult {
        let user = match self.writer(&parts).await {
            Ok(u) => u,
            Err(e) => return api_err(e),
        };
        let req = serde_json::from_value(json!({"spaceKey": a.space_key, "title": a.title, "markdown": a.markdown, "parentId": a.parent_id}))
            .map_err(|e| ErrorData::invalid_params(e.to_string(), None))?;
        match crate::routes::pages::create(State(self.state.clone()), user, AxJson(req)).await {
            Ok((_, AxJson(v))) => {
                let id = v["id"].as_str().unwrap_or("").to_string();
                ok_json(json!({"pageId": id, "url": self.url(&id)}))
            }
            Err(e) => api_err(e),
        }
    }

    #[tool(description = "Replace the content of a page with Markdown (creates a new version; people editing live see the change).")]
    async fn update_page(&self, Extension(parts): Extension<axum::http::request::Parts>, Parameters(a): Parameters<UpdatePageArgs>) -> ToolResult {
        let user = match self.writer(&parts).await {
            Ok(u) => u,
            Err(e) => return api_err(e),
        };
        let id = match parse_id(&a.page_id) {
            Ok(i) => i,
            Err(e) => return api_err(e),
        };
        let req = serde_json::from_value(json!({"markdown": a.markdown, "message": a.message.or(Some("Updated via MCP".into()))}))
            .map_err(|e| ErrorData::invalid_params(e.to_string(), None))?;
        match crate::routes::pages::put_content(State(self.state.clone()), user, Path(id), AxJson(req)).await {
            Ok(AxJson(v)) => ok_json(json!({"pageId": id, "version": v["version"], "url": self.url(&id.to_string())})),
            Err(e) => api_err(e),
        }
    }

    #[tool(description = "Append Markdown to the end of a page (keeps existing content).")]
    async fn append_to_page(&self, Extension(parts): Extension<axum::http::request::Parts>, Parameters(a): Parameters<AppendArgs>) -> ToolResult {
        let user = match self.writer(&parts).await {
            Ok(u) => u,
            Err(e) => return api_err(e),
        };
        let id = match parse_id(&a.page_id) {
            Ok(i) => i,
            Err(e) => return api_err(e),
        };
        let current = match crate::routes::pages::get(State(self.state.clone()), MaybeUser(Some(user.0.clone())), Path(id)).await {
            Ok(AxJson(p)) => Node::from_json(&p["content"]).unwrap_or_else(|_| Node::empty_doc()),
            Err(e) => return api_err(e),
        };
        let mut doc = current;
        doc.content.extend(markdown::markdown_to_pm(&a.markdown).doc.content);
        let req = serde_json::from_value(json!({"content": doc.to_json(), "message": "Appended via MCP"}))
            .map_err(|e| ErrorData::invalid_params(e.to_string(), None))?;
        match crate::routes::pages::put_content(State(self.state.clone()), user, Path(id), AxJson(req)).await {
            Ok(AxJson(v)) => ok_json(json!({"pageId": id, "version": v["version"]})),
            Err(e) => api_err(e),
        }
    }

    #[tool(description = "Add a comment to a page.")]
    async fn add_comment(&self, Extension(parts): Extension<axum::http::request::Parts>, Parameters(a): Parameters<CommentArgs>) -> ToolResult {
        let user = match self.writer(&parts).await {
            Ok(u) => u,
            Err(e) => return api_err(e),
        };
        let id = match parse_id(&a.page_id) {
            Ok(i) => i,
            Err(e) => return api_err(e),
        };
        let req = serde_json::from_value(json!({"text": a.text})).map_err(|e| ErrorData::invalid_params(e.to_string(), None))?;
        match crate::routes::comments::create(State(self.state.clone()), user, Path(id), AxJson(req)).await {
            Ok((_, AxJson(c))) => ok_json(json!({"commentId": serde_json::to_value(c).unwrap_or_default()["id"]})),
            Err(e) => api_err(e),
        }
    }
}

#[tool_handler]
impl ServerHandler for WikiMcp {
    fn get_info(&self) -> ServerConfig {
        ServerConfig::new(ServerCapabilities::builder().enable_tools().build())
            .with_server_info(Implementation::new("wikicraft", env!("CARGO_PKG_VERSION")))
            .with_instructions("WikiCraft team wiki. To answer a question, start with `retrieve` (the best-matching sections, with URLs to cite); use `search` or `query_pages` (CQL) to find pages, `get_page` to read them (Markdown), and `create_page`/`update_page`/`append_to_page`/`add_comment` to write. You act with the permissions of the API token's user.")
    }
}

/// The tower service mounted at `/api/mcp`.
pub fn service(state: AppState) -> StreamableHttpService<WikiMcp, LocalSessionManager> {
    let mut hosts = vec!["localhost".to_string(), "127.0.0.1".to_string(), "::1".to_string()];
    if let Some(h) = state.config.base_url.split("://").nth(1).map(|r| r.split('/').next().unwrap_or("").to_string()) {
        let host_only = h.split(':').next().unwrap_or("").to_string();
        hosts.push(h);
        hosts.push(host_only);
    }
    let config = StreamableHttpServerConfig::default().with_allowed_hosts(hosts);
    StreamableHttpService::new(move || Ok(WikiMcp::new(state.clone())), LocalSessionManager::default().into(), config)
}
