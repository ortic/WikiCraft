pub mod access_request;
pub mod account;
pub mod activity;
pub mod admin;
pub mod ai;
pub mod announcement;
pub mod archive;
pub mod attachments;
pub mod auth;
pub mod branding;
pub mod comments;
pub mod exports;
pub mod imports;
pub mod inspect;
pub mod issues;
pub mod legacy;
pub mod likes;
pub mod links;
pub mod notifications;
pub mod openapi;
pub mod pages;
pub mod passkeys;
pub mod people;
pub mod properties;
pub mod reports;
pub mod retrieve;
pub mod search;
pub mod security;
pub mod share;
pub mod space_templates;
pub mod spaces;
pub mod tasks;
pub mod templates;
pub mod unfurl;
pub mod uploads;
pub mod users;
pub mod verification;
pub mod webhooks;

use axum::Router;
use axum::extract::DefaultBodyLimit;
use axum::routing::{delete, get, post, put};

use crate::app::AppState;

/// Content-Security-Policy for the app (attachments set a stricter sandbox of their own).
pub const CSP: &str = "default-src 'self'; script-src 'self'; style-src 'self' 'unsafe-inline'; img-src 'self' data: blob: https: http:; font-src 'self' data:; \
                       connect-src 'self'; worker-src 'self' blob:; media-src 'self' blob: https:; frame-src 'self' https:; object-src 'none'; base-uri 'self'; form-action 'self'; frame-ancestors 'self'";

pub fn router(state: AppState) -> Router {
    let max_upload = (state.config.max_upload_mb as usize).saturating_mul(1024 * 1024);
    let api = Router::new()
        // auth & setup
        .route("/setup", get(auth::setup_status).post(auth::setup))
        .route("/auth/login", post(auth::login))
        .route("/auth/logout", post(auth::logout))
        .route("/auth/providers", get(crate::sso::list))
        .route("/auth/forgot", post(account::forgot))
        .route("/auth/mfa", post(security::login))
        .route("/auth/mfa/passkey", post(security::passkey_challenge))
        .route("/auth/passkey/options", post(passkeys::login_options))
        .route("/auth/passkey", post(passkeys::login))
        .route("/me/passkeys", get(passkeys::list).post(passkeys::add))
        .route("/me/passkeys/options", post(passkeys::add_options))
        .route("/me/passkeys/{id}", axum::routing::patch(passkeys::rename).delete(passkeys::delete))
        .route("/me/sessions", get(security::sessions))
        .route("/me/sessions/{id}", delete(security::revoke_session))
        .route("/me/sessions/revoke-others", post(security::revoke_others))
        .route("/me/mfa", get(security::status))
        .route("/me/mfa/setup", post(security::setup))
        .route("/me/mfa/enable", post(security::enable))
        .route("/me/mfa/disable", post(security::disable))
        .route("/me/mfa/recovery-codes", post(security::new_recovery_codes))
        .route("/users/{id}/reset-mfa", post(security::admin_reset))
        .route("/auth/reset", post(account::reset))
        .route("/auth/invite", get(account::invite_info))
        .route("/auth/accept-invite", post(account::accept_invite))
        .route("/auth/signup", get(account::signup_info).post(account::signup))
        .route("/admin/signup", get(account::get_signup_settings).put(account::put_signup_settings))
        .route("/announcement", get(announcement::get))
        .route("/features", get(share::features))
        .route("/activity", get(activity::feed))
        .route("/admin/workspace", put(branding::rename))
        .route("/admin/workspace/logo", post(branding::upload_logo).delete(branding::delete_logo))
        .route("/workspace/logo", get(branding::logo))
        .route("/admin/announcement", put(announcement::put))
        .route("/me/preferences", get(account::get_preferences).put(account::put_preferences))
        .route("/me/space", get(spaces::my_space).post(spaces::create_my_space))
        .route("/users/invite", post(account::invite))
        .route("/admin/mail", get(account::mail_status))
        .route("/admin/mail/test", post(account::mail_test))
        .route("/auth/sso/{id}/login", get(crate::sso::oidc::sso_login))
        .route("/auth/sso/{id}/callback", get(crate::sso::oidc::sso_callback))
        .route("/auth/oidc", get(crate::sso::oidc::legacy_info))
        .route("/auth/oidc/login", get(crate::sso::oidc::legacy_login))
        .route("/auth/oidc/callback", get(crate::sso::oidc::legacy_callback))
        .route("/me", get(auth::me))
        .route("/me/tokens", get(auth::list_tokens).post(auth::create_token))
        .route("/me/tokens/{id}", delete(auth::delete_token))
        // users & groups
        .route("/users", get(users::list).post(users::create))
        .route("/people", get(people::directory))
        .route("/people/avatars", get(people::avatars))
        .route("/me/avatar", post(people::upload_avatar).delete(people::delete_avatar).layer(DefaultBodyLimit::max(people::MAX_AVATAR_UPLOAD + (64 << 10))))
        .route("/users/{id}/avatar", get(people::avatar))
        .route("/people/{id}", get(people::profile))
        .route("/people/{id}/follow", put(people::follow).delete(people::unfollow))
        .route("/me/following", get(people::following))
        .route("/me/profile", put(people::update_mine))
        .route("/users/{id}", axum::routing::patch(users::update))
        .route("/users/{id}/merge", post(users::merge))
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
        .route("/spaces/{key}/link-report", get(links::report))
        .route("/spaces/{key}/trash/empty", post(spaces::empty_trash))
        .route("/spaces/{key}/blog", get(spaces::blog))
        .route("/spaces/{key}/properties", get(properties::space_keys))
        .route("/spaces/{key}/blog.atom", get(spaces::blog_feed))
        // pages
        .route("/pages", post(pages::create))
        .route("/pages/{id}", get(pages::get).patch(pages::update).delete(pages::trash))
        .route("/pages/{id}/content", put(pages::put_content))
        .route("/pages/{id}/move", post(pages::move_page))
        .route("/pages/{id}/copy", post(pages::copy))
        .route("/pages/{id}/restore", post(pages::restore))
        .route("/pages/{id}/purge", post(pages::purge))
        .route("/pages/{id}/viewers", get(pages::viewers))
        .route("/pages/{id}/backlinks", get(links::backlinks))
        .route("/pages/{id}/publish", post(pages::publish))
        .route("/me/drafts", get(pages::my_drafts))
        .route("/pages/{id}/children", get(pages::children))
        .route("/pages/{id}/versions", get(pages::versions).post(pages::save_version))
        .route("/pages/{id}/minor-edit", get(pages::get_minor_edit).put(pages::set_minor_edit))
        .route("/pages/{id}/publish-schedule", put(pages::schedule_publish).delete(pages::unschedule_publish))
        .route("/pages/{id}/versions/{version}", get(pages::get_version))
        .route("/pages/{id}/versions/{version}/restore", post(pages::restore_version))
        .route("/pages/{id}/restrictions", get(pages::get_restrictions).put(pages::put_restrictions))
        .route("/pages/{id}/labels", get(pages::get_labels).put(pages::put_labels))
        .route("/pages/{id}/properties", get(properties::get).put(properties::put))
        .route("/pages/{id}/favorite", put(pages::favorite).delete(pages::unfavorite))
        .route("/pages/{id}/watch", put(notifications::watch).delete(notifications::unwatch))
        .route("/notifications", get(notifications::list))
        .route("/tasks", get(tasks::mine))
        .route("/pages/{id}/tasks/{idx}/toggle", post(tasks::toggle))
        .route("/notifications/read", post(notifications::mark_read))
        .route("/templates", get(templates::list).post(templates::create))
        .route("/templates/{id}", delete(templates::delete).patch(templates::update))
        .route("/templates/{id}/preview", get(templates::preview))
        .route("/pages/{id}/export", get(pages::export))
        .route("/pages/{id}/collab", get(crate::collab::ws_handler))
        // comments
        .route("/pages/{id}/comments", get(comments::list).post(comments::create))
        .route("/comments/{id}", axum::routing::patch(comments::update).delete(comments::delete))
        // attachments
        .route("/pages/{id}/attachments", get(attachments::list).post(attachments::upload).layer(DefaultBodyLimit::max(max_upload)))
        .route("/pages/{id}/comment-images", post(attachments::upload_comment_image).layer(DefaultBodyLimit::max(max_upload)))
        .route("/attachments/{id}", delete(attachments::delete))
        .route("/attachments/{id}/content", get(attachments::download))
        .route("/attachments/{id}/versions", get(attachments::versions))
        .route("/attachments/{id}/versions/{version}/content", get(attachments::download_version))
        .route("/attachments/{id}/versions/{version}/restore", post(attachments::restore_version))
        // discovery
        .route("/search", get(search::search))
        .route("/cql", get(search::cql))
        .route("/retrieve", get(retrieve::get).post(retrieve::post))
        .route("/recent", get(search::recent))
        .route("/favorites", get(search::favorites))
        .route("/popular", get(search::popular))
        .route("/me/searches", get(search::saved_searches).post(search::save_search))
        .route("/me/searches/{id}", delete(search::delete_search))
        .route("/space-templates", get(space_templates::list))
        .route("/markdown", post(pages::markdown_to_doc))
        .route("/unfurl", get(unfurl::unfurl))
        .route("/unfurl/gist", get(unfurl::gist))
        .route("/unfurl/issue", get(issues::issue))
        .route("/labels", get(search::suggest_labels))
        .route("/labels/{name}", get(search::by_label))
        .route("/labels/{name}/attachments", get(attachments::by_label))
        .route("/attachments/{id}/labels", put(attachments::put_labels))
        .route("/pages/{id}/access/{who}", get(inspect::explain))
        .route("/pages/{id}/like", put(likes::like_page).delete(likes::unlike_page))
        .route("/pages/{id}/verification", get(verification::get).put(verification::verify).delete(verification::unverify))
        .route("/pages/{id}/verification/request", post(verification::request_review))
        .route("/pages/{id}/access-request", post(access_request::request))
        .route("/pages/{id}/access-grant", post(access_request::grant))
        .route("/pages/{id}/access-dismiss", post(access_request::dismiss))
        .route("/pages/{id}/owner", put(pages::set_owner))
        .route("/pages/{id}/archive", post(archive::archive))
        .route("/pages/{id}/unarchive", post(archive::unarchive))
        .route("/spaces/{key}/archived", get(archive::list))
        .route("/spaces/{key}/watch", put(spaces::watch).delete(spaces::unwatch))
        .route("/spaces/{key}/owner", put(spaces::set_owner))
        .route("/spaces/{key}/copy", post(spaces::copy))
        .route("/spaces/{key}/default-template", get(pages::get_default_template))
        .route("/spaces/{key}/star", put(spaces::star).delete(spaces::unstar))
        .route("/reports/verification", get(verification::report))
        .route("/ai/status", get(ai::status))
        .route("/ai/ask", post(ai::ask))
        .route("/comments/{id}/like", put(likes::like_comment).delete(likes::unlike_comment))
        .route("/pages/{id}/share-links", get(share::list).post(share::create))
        .route("/share-links/{id}", delete(share::revoke))
        .route("/admin/settings", get(share::get_settings).put(share::put_settings))
        .route("/admin/spaces", get(admin::spaces))
        .route("/admin/spaces/{key}", delete(admin::delete_space))
        .route("/admin/spaces/{key}/unarchive", post(admin::unarchive))
        .route("/admin/webhooks", get(webhooks::list).post(webhooks::create))
        .route("/admin/webhooks/{id}", axum::routing::patch(webhooks::update).delete(webhooks::delete))
        .route("/admin/webhooks/{id}/test", post(webhooks::test))
        .route("/admin/webhooks/{id}/deliveries", get(webhooks::deliveries))
        .route("/public/{token}", get(share::view))
        .route("/public/{token}/unlock", post(share::unlock))
        .route("/public/{token}/pages/{id}", get(share::view_page))
        .route("/public/{token}/attachments/{id}", get(share::attachment))
        .route("/reports/pages", get(reports::pages_report))
        .route("/reports/properties", get(reports::properties_report))
        .route("/reports/tasks", get(reports::tasks_report))
        .route("/reports/decisions", get(reports::decisions_report))
        .route("/resolve", get(search::resolve))
        // imports & jobs
        .route("/imports/confluence", post(imports::upload_confluence).layer(DefaultBodyLimit::max(max_upload)))
        .route("/imports/confluence/analyze", post(imports::analyze_confluence).layer(DefaultBodyLimit::max(max_upload)))
        .route("/imports/markdown", post(imports::upload_markdown).layer(DefaultBodyLimit::max(max_upload)))
        .route("/imports/backup", post(exports::restore).layer(DefaultBodyLimit::max(max_upload)))
        .route("/uploads", post(uploads::create))
        .route("/uploads/{id}", get(uploads::get).delete(uploads::delete))
        .route("/uploads/{id}/parts/{n}", put(uploads::put_part).layer(DefaultBodyLimit::max(uploads::PART_SIZE + (64 << 10))))
        .route("/uploads/{id}/analyze-confluence", post(uploads::analyze_confluence))
        .route("/uploads/{id}/import-confluence", post(uploads::import_confluence))
        .route("/jobs", get(imports::list_jobs))
        .route("/jobs/{id}", get(imports::get_job))
        .route("/jobs/{id}/cancel", post(imports::cancel_job))
        .route("/jobs/{id}/retry", post(imports::retry_job))
        .route("/jobs/{id}/download", get(exports::download))
        .route("/spaces/{key}/export", post(exports::start))
        // admin
        .route("/admin/audit", get(admin::audit))
        .route("/admin/stats", get(admin::stats))
        .route("/admin/system", get(admin::system))
        .route("/admin/reindex", post(admin::reindex))
        .route("/health", get(admin::health))
        .route("/openapi.yaml", get(openapi::spec))
        .route("/ready", get(admin::ready))
        .route("/metrics", get(crate::metrics::render))
        // MCP server for AI assistants (bearer token auth inside the tools)
        .nest_service("/mcp", crate::mcp::service(state.clone()))
        // JSON bodies (pages can be big, but not gigabytes); uploads override this per route
        .layer(DefaultBodyLimit::max(16 * 1024 * 1024));

    let spa = crate::web_assets::service(&state.config.static_dir);
    // HTTPS deployments: browsers stay on HTTPS (one year; no includeSubDomains, other hosts of
    // the domain are none of our business)
    let hsts = state.config.base_url.starts_with("https://").then(|| {
        tower_http::set_header::SetResponseHeaderLayer::if_not_present(
            axum::http::header::STRICT_TRANSPORT_SECURITY,
            axum::http::HeaderValue::from_static("max-age=31536000"),
        )
    });

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
        .route("/opensearch.xml", get(legacy::opensearch))
        .route("/x/{tiny}", get(legacy::tiny))
        .route("/wiki/x/{tiny}", get(legacy::tiny));

    let app = Router::new()
        // /api/v1 is the stable, versioned path for integrations; /api is what the web app uses
        .nest("/api/v1", saml_routes(api.clone()))
        .nest("/api", saml_routes(api))
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
        // scripts only from our own bundle: injected markup can't run inline handlers
        .layer(tower_http::set_header::SetResponseHeaderLayer::if_not_present(
            axum::http::header::CONTENT_SECURITY_POLICY,
            axum::http::HeaderValue::from_static(CSP),
        ));
    match hsts {
        Some(h) => app.layer(h),
        None => app,
    }
    .with_state(state)
}

/// SAML endpoints (assertion consumer service + SP metadata) when built with `--features saml`.
#[cfg(feature = "saml")]
fn saml_routes(api: Router<AppState>) -> Router<AppState> {
    api.route("/auth/sso/{id}/acs", post(crate::sso::saml::acs)).route("/auth/sso/{id}/metadata", get(crate::sso::saml::metadata))
}

#[cfg(not(feature = "saml"))]
fn saml_routes(api: Router<AppState>) -> Router<AppState> {
    api
}
