pub mod admin;
pub mod attachments;
pub mod auth;
pub mod comments;
pub mod imports;
pub mod legacy;
pub mod notifications;
pub mod pages;
pub mod search;
pub mod spaces;
pub mod tasks;
pub mod templates;
pub mod users;

use axum::Router;
use axum::extract::DefaultBodyLimit;
use axum::routing::{delete, get, post, put};
use tower_http::services::{ServeDir, ServeFile};

use crate::app::AppState;

pub fn router(state: AppState) -> Router {
    let max_upload = (state.config.max_upload_mb as usize).saturating_mul(1024 * 1024);
    let api = Router::new()
        // auth & setup
        .route("/setup", get(auth::setup_status).post(auth::setup))
        .route("/auth/login", post(auth::login))
        .route("/auth/logout", post(auth::logout))
        .route("/auth/oidc", get(crate::oidc::info))
        .route("/auth/oidc/login", get(crate::oidc::login))
        .route("/auth/oidc/callback", get(crate::oidc::callback))
        .route("/me", get(auth::me))
        .route("/me/tokens", get(auth::list_tokens).post(auth::create_token))
        .route("/me/tokens/{id}", delete(auth::delete_token))
        // users & groups
        .route("/users", get(users::list).post(users::create))
        .route("/users/{id}", axum::routing::patch(users::update))
        .route("/groups", get(users::list_groups).post(users::create_group))
        .route("/groups/{id}", delete(users::delete_group))
        .route("/groups/{id}/members", post(users::add_member))
        .route("/groups/{id}/members/{user_id}", delete(users::remove_member))
        // spaces
        .route("/spaces", get(spaces::list).post(spaces::create))
        .route("/spaces/{key}", get(spaces::get).patch(spaces::update).delete(spaces::archive))
        .route("/spaces/{key}/tree", get(spaces::tree))
        .route("/spaces/{key}/permissions", get(spaces::get_permissions).put(spaces::put_permissions))
        .route("/spaces/{key}/trash", get(spaces::trash))
        .route("/spaces/{key}/blog", get(spaces::blog))
        // pages
        .route("/pages", post(pages::create))
        .route("/pages/{id}", get(pages::get).patch(pages::update).delete(pages::trash))
        .route("/pages/{id}/content", put(pages::put_content))
        .route("/pages/{id}/move", post(pages::move_page))
        .route("/pages/{id}/copy", post(pages::copy))
        .route("/pages/{id}/restore", post(pages::restore))
        .route("/pages/{id}/children", get(pages::children))
        .route("/pages/{id}/versions", get(pages::versions).post(pages::save_version))
        .route("/pages/{id}/versions/{version}", get(pages::get_version))
        .route("/pages/{id}/versions/{version}/restore", post(pages::restore_version))
        .route("/pages/{id}/restrictions", get(pages::get_restrictions).put(pages::put_restrictions))
        .route("/pages/{id}/labels", get(pages::get_labels).put(pages::put_labels))
        .route("/pages/{id}/favorite", put(pages::favorite).delete(pages::unfavorite))
        .route("/pages/{id}/watch", put(notifications::watch).delete(notifications::unwatch))
        .route("/notifications", get(notifications::list))
        .route("/tasks", get(tasks::mine))
        .route("/pages/{id}/tasks/{idx}/toggle", post(tasks::toggle))
        .route("/notifications/read", post(notifications::mark_read))
        .route("/templates", get(templates::list).post(templates::create))
        .route("/templates/{id}", delete(templates::delete))
        .route("/pages/{id}/export", get(pages::export))
        .route("/pages/{id}/collab", get(crate::collab::ws_handler))
        // comments
        .route("/pages/{id}/comments", get(comments::list).post(comments::create))
        .route("/comments/{id}", axum::routing::patch(comments::update).delete(comments::delete))
        // attachments
        .route("/pages/{id}/attachments", get(attachments::list).post(attachments::upload).layer(DefaultBodyLimit::max(max_upload)))
        .route("/attachments/{id}", delete(attachments::delete))
        .route("/attachments/{id}/content", get(attachments::download))
        // discovery
        .route("/search", get(search::search))
        .route("/recent", get(search::recent))
        .route("/favorites", get(search::favorites))
        .route("/labels/{name}", get(search::by_label))
        .route("/resolve", get(search::resolve))
        // imports & jobs
        .route("/imports/confluence", post(imports::upload_confluence).layer(DefaultBodyLimit::max(max_upload)))
        .route("/imports/confluence/analyze", post(imports::analyze_confluence).layer(DefaultBodyLimit::max(max_upload)))
        .route("/imports/markdown", post(imports::upload_markdown).layer(DefaultBodyLimit::max(max_upload)))
        .route("/jobs", get(imports::list_jobs))
        .route("/jobs/{id}", get(imports::get_job))
        // admin
        .route("/admin/audit", get(admin::audit))
        .route("/admin/stats", get(admin::stats))
        .route("/admin/reindex", post(admin::reindex))
        .route("/health", get(admin::health))
        .route("/ready", get(admin::ready))
        .route("/metrics", get(crate::metrics::render))
        // MCP server for AI assistants (bearer token auth inside the tools)
        .nest_service("/mcp", crate::mcp::service(state.clone()))
        // JSON bodies (pages can be big, but not gigabytes); uploads override this per route
        .layer(DefaultBodyLimit::max(16 * 1024 * 1024));

    let static_dir = state.config.static_dir.clone();
    let index = static_dir.join("index.html");
    let spa = ServeDir::new(&static_dir).fallback(ServeFile::new(index));

    // old Confluence URLs → imported pages
    let legacy = Router::new()
        .route("/display/{space}", get(legacy::display_space))
        .route("/display/{space}/{title}", get(legacy::display))
        .route("/wiki/display/{space}/{title}", get(legacy::display))
        .route("/pages/viewpage.action", get(legacy::viewpage))
        .route("/wiki/pages/viewpage.action", get(legacy::viewpage))
        .route("/wiki/spaces/{key}/pages/{id}", get(legacy::cloud_page))
        .route("/wiki/spaces/{key}/pages/{id}/{*rest}", get(legacy::cloud_page))
        .route("/spaces/{key}/pages/{id}", get(legacy::cloud_page))
        .route("/spaces/{key}/pages/{id}/{*rest}", get(legacy::cloud_page))
        .route("/x/{tiny}", get(legacy::tiny))
        .route("/wiki/x/{tiny}", get(legacy::tiny));

    Router::new()
        .nest("/api", api)
        .merge(legacy)
        .fallback_service(spa)
        .layer(tower_http::compression::CompressionLayer::new())
        .layer(axum::middleware::from_fn_with_state(state.clone(), crate::metrics::track))
        // request ids: accepted from / returned to the client and attached to every log line
        .layer(tower_http::request_id::PropagateRequestIdLayer::x_request_id())
        .layer(tower_http::trace::TraceLayer::new_for_http().make_span_with(|req: &axum::http::Request<_>| {
            let id = req.headers().get("x-request-id").and_then(|v| v.to_str().ok()).unwrap_or("-");
            tracing::info_span!("http", method = %req.method(), path = %req.uri().path(), request_id = %id)
        }))
        .layer(tower_http::request_id::SetRequestIdLayer::x_request_id(tower_http::request_id::MakeRequestUuid))
        .layer(tower_http::catch_panic::CatchPanicLayer::new())
        .layer(tower_http::set_header::SetResponseHeaderLayer::if_not_present(
            axum::http::header::X_CONTENT_TYPE_OPTIONS,
            axum::http::HeaderValue::from_static("nosniff"),
        ))
        .layer(tower_http::set_header::SetResponseHeaderLayer::if_not_present(
            axum::http::header::REFERRER_POLICY,
            axum::http::HeaderValue::from_static("strict-origin-when-cross-origin"),
        ))
        .layer(tower_http::set_header::SetResponseHeaderLayer::if_not_present(
            axum::http::header::X_FRAME_OPTIONS,
            axum::http::HeaderValue::from_static("SAMEORIGIN"),
        ))
        .with_state(state)
}
