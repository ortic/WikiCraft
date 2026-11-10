//! E-mail based account flows: password reset, invitations, notification preferences and the
//! admin mail check.

use axum::Json;
use axum::extract::{Query, State};
use axum::http::{HeaderMap, StatusCode, header};
use axum::response::{IntoResponse, Response};
use serde::Deserialize;
use serde_json::{Value, json};
use uuid::Uuid;

use crate::app::{AppState, new_id};
use crate::audit;
use crate::auth::{CurrentUser, create_session, hash_password, random_token, require_admin, token_hash};
use crate::error::{ApiError, ApiResult};
use crate::routes::auth::{validate_email, validate_password};

const RESET_HOURS: i32 = 1;
const INVITE_DAYS: i32 = 7;

fn base(state: &AppState) -> String {
    state.config.base_url.trim_end_matches('/').to_string()
}

/// Creates a single-use link token; previous unused tokens of the same purpose are revoked.
async fn issue_token(state: &AppState, user: Uuid, purpose: &str, created_by: Option<Uuid>, valid_hours: i32) -> ApiResult<String> {
    sqlx::query("DELETE FROM user_tokens WHERE expires_at < now() - interval '30 days'").execute(&state.db).await?;
    sqlx::query("UPDATE user_tokens SET used_at = now() WHERE user_id = $1 AND purpose = $2 AND used_at IS NULL")
        .bind(user)
        .bind(purpose)
        .execute(&state.db)
        .await?;
    let token = random_token();
    sqlx::query("INSERT INTO user_tokens (token_hash, user_id, purpose, created_by, expires_at) VALUES ($1, $2, $3, $4, now() + make_interval(hours => $5))")
        .bind(hex::encode(token_hash(&token)))
        .bind(user)
        .bind(purpose)
        .bind(created_by)
        .bind(valid_hours)
        .execute(&state.db)
        .await?;
    Ok(token)
}

/// Looks up a valid token: (user id, e-mail, name, status, workspace name).
async fn find_token(state: &AppState, token: &str, purpose: &str) -> ApiResult<(Uuid, Option<String>, String, String, String)> {
    sqlx::query_as(
        "SELECT u.id, u.email, u.name, u.status, w.name FROM user_tokens t JOIN users u ON u.id = t.user_id JOIN workspaces w ON w.id = u.workspace_id
         WHERE t.token_hash = $1 AND t.purpose = $2 AND t.used_at IS NULL AND t.expires_at > now()",
    )
    .bind(hex::encode(token_hash(token)))
    .bind(purpose)
    .fetch_optional(&state.db)
    .await?
    .ok_or_else(|| ApiError::bad("this link is invalid or has expired"))
}

async fn consume_token(state: &AppState, token: &str) -> ApiResult<()> {
    sqlx::query("UPDATE user_tokens SET used_at = now() WHERE token_hash = $1").bind(hex::encode(token_hash(token))).execute(&state.db).await?;
    Ok(())
}

// ---- password reset ------------------------------------------------------------------------

#[derive(Deserialize)]
pub struct ForgotRequest {
    email: String,
}

/// `POST /api/auth/forgot` — always 204 (no account enumeration); mails a reset link to
/// active accounts with a local password, at most 3 per hour.
pub async fn forgot(State(state): State<AppState>, Json(req): Json<ForgotRequest>) -> ApiResult<StatusCode> {
    if state.config.disable_password_login {
        return Err(ApiError::bad("password login is disabled; use single sign-on"));
    }
    let Ok(email) = validate_email(&req.email) else { return Ok(StatusCode::NO_CONTENT) };
    let user: Option<(Uuid, String)> = sqlx::query_as("SELECT id, name FROM users WHERE lower(email) = $1 AND status = 'active' AND password_hash IS NOT NULL")
        .bind(&email)
        .fetch_optional(&state.db)
        .await?;
    let Some((id, name)) = user else { return Ok(StatusCode::NO_CONTENT) };
    let recent: i64 =
        sqlx::query_scalar("SELECT count(*) FROM user_tokens WHERE user_id = $1 AND purpose = 'password_reset' AND created_at > now() - interval '1 hour'")
            .bind(id)
            .fetch_one(&state.db)
            .await?;
    if recent >= 3 {
        tracing::info!(user = %id, "password reset throttled");
        return Ok(StatusCode::NO_CONTENT);
    }
    let token = issue_token(&state, id, "password_reset", None, RESET_HOURS).await?;
    let url = format!("{}/reset-password?token={token}", base(&state));
    let (text, html) = crate::mail::action_mail(
        &format!("Hi {name},"),
        "Someone (hopefully you) asked to reset the password of your WikiCraft account.",
        "Choose a new password",
        &url,
        "The link is valid for one hour and can be used once. If you didn't ask for this, ignore this e-mail — your password stays the same.",
    );
    crate::mail::queue(&state, &email, "Reset your WikiCraft password", &text, Some(&html)).await.map_err(ApiError::Internal)?;
    Ok(StatusCode::NO_CONTENT)
}

#[derive(Deserialize)]
pub struct ResetRequest {
    token: String,
    password: String,
}

/// `POST /api/auth/reset` — sets the new password, signs out all sessions and signs in.
pub async fn reset(State(state): State<AppState>, headers: HeaderMap, Json(req): Json<ResetRequest>) -> ApiResult<Response> {
    validate_password(&req.password)?;
    let (id, email, _, status, _) = find_token(&state, &req.token, "password_reset").await?;
    if status != "active" {
        return Err(ApiError::bad("this account is not active"));
    }
    let hash = hash_password(req.password).await?;
    consume_token(&state, &req.token).await?;
    sqlx::query("UPDATE users SET password_hash = $2 WHERE id = $1").bind(id).bind(&hash).execute(&state.db).await?;
    sqlx::query("DELETE FROM sessions WHERE user_id = $1").bind(id).execute(&state.db).await?;
    if let Some(e) = &email {
        crate::auth::clear_login_failures(&state, e).await;
    }
    // API tokens an intruder may have created go too (account recovery)
    sqlx::query("DELETE FROM api_tokens WHERE user_id = $1").bind(id).execute(&state.db).await?;
    let ws: Uuid = sqlx::query_scalar("SELECT workspace_id FROM users WHERE id = $1").bind(id).fetch_one(&state.db).await?;
    audit::log(&state, ws, Some(id), "auth.password_reset", Some("user"), Some(id), json!({})).await;
    // with two-factor on, mailbox access alone must not sign in
    if crate::routes::security::required(&state, id).await? {
        let token = crate::routes::security::challenge(&state, id, Some("/")).await?;
        return Ok(([(header::SET_COOKIE, crate::sso::mfa_cookie(&state, &token, 600))], Json(json!({ "ok": true, "mfaRequired": true }))).into_response());
    }
    let cookie = create_session(&state, id, &headers).await?;
    Ok(([(header::SET_COOKIE, cookie)], Json(json!({ "ok": true }))).into_response())
}

// ---- invitations ---------------------------------------------------------------------------

#[derive(Deserialize)]
pub struct InviteRequest {
    email: String,
    #[serde(default)]
    name: Option<String>,
    #[serde(default)]
    role: Option<String>,
}

/// `POST /api/users/invite` (admins) — creates the account as `invited` (or re-invites) and
/// mails a link to choose a name and password. The link is also returned, for setups without
/// e-mail.
pub async fn invite(State(state): State<AppState>, CurrentUser(admin): CurrentUser, Json(req): Json<InviteRequest>) -> ApiResult<(StatusCode, Json<Value>)> {
    require_admin(&admin)?;
    let email = validate_email(&req.email)?;
    let role = req.role.unwrap_or_else(|| "member".into());
    if !matches!(role.as_str(), "admin" | "member" | "guest") {
        return Err(ApiError::bad("role must be admin, member or guest"));
    }
    let name =
        req.name.as_deref().map(str::trim).filter(|n| !n.is_empty()).map(String::from).unwrap_or_else(|| email.split('@').next().unwrap_or(&email).to_string());
    let existing: Option<(Uuid, String)> = sqlx::query_as("SELECT id, status FROM users WHERE workspace_id = $1 AND lower(email) = $2")
        .bind(admin.workspace_id)
        .bind(&email)
        .fetch_optional(&state.db)
        .await?;
    let id = match existing {
        Some((_, s)) if s == "active" => return Err(ApiError::Conflict(format!("{email} already has an account"))),
        Some((_, s)) if s == "deactivated" => return Err(ApiError::Conflict(format!("{email} is deactivated; reactivate the account instead"))),
        // imported authors (placeholders) and pending invitations
        Some((id, _)) => {
            sqlx::query("UPDATE users SET status = 'invited', role = $2 WHERE id = $1").bind(id).bind(&role).execute(&state.db).await?;
            id
        }
        None => {
            let id = new_id();
            sqlx::query("INSERT INTO users (id, workspace_id, email, name, role, status) VALUES ($1, $2, $3, $4, $5, 'invited')")
                .bind(id)
                .bind(admin.workspace_id)
                .bind(&email)
                .bind(&name)
                .bind(&role)
                .execute(&state.db)
                .await?;
            id
        }
    };
    let token = issue_token(&state, id, "invite", Some(admin.id), INVITE_DAYS * 24).await?;
    let url = format!("{}/accept-invite?token={token}", base(&state));
    let workspace: String = sqlx::query_scalar("SELECT name FROM workspaces WHERE id = $1").bind(admin.workspace_id).fetch_one(&state.db).await?;
    let (text, html) = crate::mail::action_mail(
        "Hi,",
        &format!("{} invited you to join {workspace} on WikiCraft, the team wiki.", admin.name),
        "Accept the invitation",
        &url,
        &format!("The invitation is valid for {INVITE_DAYS} days."),
    );
    let emailed = crate::mail::queue(&state, &email, &format!("{} invited you to {workspace} on WikiCraft", admin.name), &text, Some(&html))
        .await
        .map_err(ApiError::Internal)?;
    audit::log(&state, admin.workspace_id, Some(admin.id), "user.invited", Some("user"), Some(id), json!({"email": email, "role": role})).await;
    Ok((StatusCode::CREATED, Json(json!({ "id": id, "inviteUrl": url, "emailed": emailed }))))
}

#[derive(Deserialize)]
pub struct TokenQuery {
    token: String,
}

/// `GET /api/auth/invite?token=` — what the accept page shows.
pub async fn invite_info(State(state): State<AppState>, Query(q): Query<TokenQuery>) -> ApiResult<Json<Value>> {
    let (_, email, name, status, workspace) = find_token(&state, &q.token, "invite").await?;
    if status != "invited" {
        return Err(ApiError::bad("this invitation was already used"));
    }
    Ok(Json(json!({ "email": email, "name": name, "workspace": workspace, "passwordLogin": !state.config.disable_password_login })))
}

#[derive(Deserialize)]
pub struct AcceptRequest {
    token: String,
    name: String,
    password: String,
}

/// `POST /api/auth/accept-invite` — activates the account and signs in.
pub async fn accept_invite(State(state): State<AppState>, headers: HeaderMap, Json(req): Json<AcceptRequest>) -> ApiResult<Response> {
    if state.config.disable_password_login {
        return Err(ApiError::bad("password login is disabled; sign in with single sign-on instead"));
    }
    validate_password(&req.password)?;
    let name = req.name.trim();
    if name.is_empty() || name.chars().count() > 200 {
        return Err(ApiError::bad("please enter your name"));
    }
    let (id, _, _, status, _) = find_token(&state, &req.token, "invite").await?;
    if status != "invited" {
        return Err(ApiError::bad("this invitation was already used"));
    }
    let hash = hash_password(req.password).await?;
    consume_token(&state, &req.token).await?;
    sqlx::query("UPDATE users SET name = $2, password_hash = $3, status = 'active' WHERE id = $1 AND status = 'invited'")
        .bind(id)
        .bind(name)
        .bind(&hash)
        .execute(&state.db)
        .await?;
    let ws: Uuid = sqlx::query_scalar("SELECT workspace_id FROM users WHERE id = $1").bind(id).fetch_one(&state.db).await?;
    audit::log(&state, ws, Some(id), "user.invite_accepted", Some("user"), Some(id), json!({})).await;
    let cookie = create_session(&state, id, &headers).await?;
    Ok(([(header::SET_COOKIE, cookie)], Json(json!({ "ok": true }))).into_response())
}

// ---- open sign-up for allowed e-mail domains ----------------------------------------------

const SIGNUP_HOURS: i32 = 48;

/// The workspace people sign up to (self-hosted installs have one) and its allowed domains.
async fn signup_workspace(state: &AppState) -> ApiResult<Option<(Uuid, String, Vec<String>)>> {
    Ok(sqlx::query_as("SELECT id, name, signup_domains FROM workspaces ORDER BY created_at LIMIT 1").fetch_optional(&state.db).await?)
}

fn signup_possible(state: &AppState) -> bool {
    // the domain check means nothing without proving the address, so sign-up needs e-mail
    state.config.smtp_url.is_some() && !state.config.disable_password_login
}

/// `GET /api/auth/signup` — whether sign-up is open, and for which domains.
pub async fn signup_info(State(state): State<AppState>) -> ApiResult<Json<Value>> {
    let domains = signup_workspace(&state).await?.map(|w| w.2).unwrap_or_default();
    let enabled = signup_possible(&state) && !domains.is_empty();
    Ok(Json(json!({ "enabled": enabled, "domains": if enabled { domains } else { vec![] } })))
}

#[derive(Deserialize)]
pub struct SignupRequest {
    email: String,
}

/// `POST /api/auth/signup {email}` — mails a confirmation link (the invitation page) to new
/// addresses at an allowed domain. Answers the same for every address.
pub async fn signup(State(state): State<AppState>, Json(req): Json<SignupRequest>) -> ApiResult<StatusCode> {
    let Some((ws, workspace, domains)) = signup_workspace(&state).await? else { return Err(ApiError::bad("sign-up is not open")) };
    if !signup_possible(&state) || domains.is_empty() {
        return Err(ApiError::bad("sign-up is not open; ask an admin for an invitation"));
    }
    let email = validate_email(&req.email)?;
    let domain = email.rsplit('@').next().unwrap_or("").to_lowercase();
    if !domains.iter().any(|d| d.eq_ignore_ascii_case(&domain)) {
        return Err(ApiError::bad(format!("sign-up is open for {} addresses only", domains.iter().map(|d| format!("@{d}")).collect::<Vec<_>>().join(", "))));
    }
    let existing: Option<(Uuid, String)> =
        sqlx::query_as("SELECT id, status FROM users WHERE workspace_id = $1 AND lower(email) = $2").bind(ws).bind(&email).fetch_optional(&state.db).await?;
    let id = match existing {
        // existing accounts get nothing (no enumeration: the answer is the same)
        Some((_, s)) if s == "active" || s == "deactivated" => return Ok(StatusCode::NO_CONTENT),
        // pending invitations and imported authors (placeholders) can be claimed
        Some((id, s)) => {
            if s != "invited" {
                sqlx::query("UPDATE users SET status = 'invited' WHERE id = $1").bind(id).execute(&state.db).await?;
            }
            id
        }
        None => {
            let id = new_id();
            sqlx::query("INSERT INTO users (id, workspace_id, email, name, role, status) VALUES ($1, $2, $3, $4, 'member', 'invited')")
                .bind(id)
                .bind(ws)
                .bind(&email)
                .bind(email.split('@').next().unwrap_or(&email))
                .execute(&state.db)
                .await?;
            id
        }
    };
    // at most three confirmation mails per address and hour
    let recent: i64 =
        sqlx::query_scalar("SELECT count(*) FROM user_tokens WHERE user_id = $1 AND purpose = 'invite' AND created_at > now() - interval '1 hour'")
            .bind(id)
            .fetch_one(&state.db)
            .await?;
    if recent >= 3 {
        return Ok(StatusCode::NO_CONTENT);
    }
    let token = issue_token(&state, id, "invite", None, SIGNUP_HOURS).await?;
    let url = format!("{}/accept-invite?token={token}", base(&state));
    let (text, html) = crate::mail::action_mail(
        "Hi,",
        &format!("someone (hopefully you) asked to join {workspace} on WikiCraft with this address. Confirm it to choose your name and password."),
        "Confirm your e-mail",
        &url,
        &format!("The link is valid for {SIGNUP_HOURS} hours. If you didn't ask for this, ignore this e-mail."),
    );
    crate::mail::queue(&state, &email, &format!("Confirm your e-mail to join {workspace}"), &text, Some(&html)).await.map_err(ApiError::Internal)?;
    audit::log(&state, ws, None, "user.signup_requested", Some("user"), Some(id), json!({"email": email})).await;
    Ok(StatusCode::NO_CONTENT)
}

#[derive(Deserialize)]
pub struct SignupSettings {
    domains: Vec<String>,
}

/// `GET/PUT /api/admin/signup` — the domains open for sign-up (admins).
pub async fn get_signup_settings(State(state): State<AppState>, CurrentUser(admin): CurrentUser) -> ApiResult<Json<Value>> {
    require_admin(&admin)?;
    let domains: Vec<String> = sqlx::query_scalar("SELECT signup_domains FROM workspaces WHERE id = $1").bind(admin.workspace_id).fetch_one(&state.db).await?;
    Ok(Json(json!({ "domains": domains, "mailConfigured": state.config.smtp_url.is_some() })))
}

pub async fn put_signup_settings(State(state): State<AppState>, CurrentUser(admin): CurrentUser, Json(req): Json<SignupSettings>) -> ApiResult<Json<Value>> {
    require_admin(&admin)?;
    let mut domains: Vec<String> = Vec::new();
    for d in req.domains.iter().map(|d| d.trim().trim_start_matches('@').to_lowercase()).filter(|d| !d.is_empty()) {
        let ok = d.len() <= 253
            && d.contains('.')
            && d.split('.').all(|l| !l.is_empty() && l.len() <= 63 && l.chars().all(|c| c.is_ascii_alphanumeric() || c == '-'));
        if !ok {
            return Err(ApiError::bad(format!("{d} is not a domain name")));
        }
        if !domains.contains(&d) {
            domains.push(d);
        }
    }
    if domains.len() > 50 {
        return Err(ApiError::bad("at most 50 domains"));
    }
    sqlx::query("UPDATE workspaces SET signup_domains = $2 WHERE id = $1").bind(admin.workspace_id).bind(&domains).execute(&state.db).await?;
    audit::log(&state, admin.workspace_id, Some(admin.id), "workspace.signup_domains", None, None, json!({"domains": domains})).await;
    Ok(Json(json!({ "domains": domains, "mailConfigured": state.config.smtp_url.is_some() })))
}

// ---- preferences & admin -------------------------------------------------------------------

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Preferences {
    email_notifications: Option<String>,
    /// notification kinds to drop entirely
    muted: Option<Vec<String>>,
}

/// The notification kinds people can mute.
pub const NOTIFICATION_KINDS: &[&str] =
    &["comment", "reply", "mention", "page_mention", "page_updated", "like", "share", "verification_request", "access_request"];

/// `GET /api/me/preferences`
pub async fn get_preferences(State(state): State<AppState>, CurrentUser(user): CurrentUser) -> ApiResult<Json<Value>> {
    let (pref, muted): (String, Vec<String>) =
        sqlx::query_as("SELECT email_notifications, muted_notifications FROM users WHERE id = $1").bind(user.id).fetch_one(&state.db).await?;
    Ok(Json(json!({ "emailNotifications": pref, "muted": muted, "kinds": NOTIFICATION_KINDS, "mailEnabled": state.mailer.enabled() })))
}

/// `PUT /api/me/preferences`
pub async fn put_preferences(State(state): State<AppState>, CurrentUser(user): CurrentUser, Json(req): Json<Preferences>) -> ApiResult<Json<Value>> {
    if let Some(p) = &req.email_notifications {
        if !matches!(p.as_str(), "all" | "important" | "none") {
            return Err(ApiError::bad("emailNotifications must be all, important or none"));
        }
        sqlx::query("UPDATE users SET email_notifications = $2 WHERE id = $1").bind(user.id).bind(p).execute(&state.db).await?;
    }
    if let Some(m) = &req.muted {
        if let Some(bad) = m.iter().find(|k| !NOTIFICATION_KINDS.contains(&k.as_str())) {
            return Err(ApiError::bad(format!("unknown notification kind {bad}")));
        }
        let mut m = m.clone();
        m.sort();
        m.dedup();
        sqlx::query("UPDATE users SET muted_notifications = $2 WHERE id = $1").bind(user.id).bind(&m).execute(&state.db).await?;
    }
    get_preferences(State(state), CurrentUser(user)).await
}

/// `GET /api/admin/mail` — mail configuration and outbox health.
pub async fn mail_status(State(state): State<AppState>, CurrentUser(user): CurrentUser) -> ApiResult<Json<Value>> {
    require_admin(&user)?;
    let (pending, failed, sent): (i64, i64, i64) = sqlx::query_as(
        "SELECT count(*) FILTER (WHERE sent_at IS NULL AND attempts < 8), count(*) FILTER (WHERE sent_at IS NULL AND attempts >= 8), count(*) FILTER (WHERE sent_at > now() - interval '1 day') FROM outbox",
    )
    .fetch_one(&state.db)
    .await?;
    let last_error: Option<String> = sqlx::query_scalar("SELECT last_error FROM outbox WHERE last_error IS NOT NULL ORDER BY created_at DESC LIMIT 1")
        .fetch_optional(&state.db)
        .await?
        .flatten();
    Ok(Json(
        json!({ "enabled": state.mailer.enabled(), "from": state.mailer.from_address(), "pending": pending, "failed": failed, "sentLastDay": sent, "lastError": last_error }),
    ))
}

#[derive(Deserialize)]
pub struct TestMail {
    to: Option<String>,
}

/// `POST /api/admin/mail/test` — sends a test e-mail right away and reports SMTP errors.
pub async fn mail_test(State(state): State<AppState>, CurrentUser(user): CurrentUser, Json(req): Json<TestMail>) -> ApiResult<Json<Value>> {
    require_admin(&user)?;
    let to = match req.to.filter(|t| !t.trim().is_empty()) {
        Some(t) => validate_email(&t)?,
        None => user.email.clone().ok_or_else(|| ApiError::bad("your account has no e-mail address"))?,
    };
    let html = crate::mail::layout("Test e-mail", "<p>E-mail delivery from WikiCraft works.</p>", "Sent from the admin console.");
    state
        .mailer
        .send_now(&to, "WikiCraft test e-mail", "E-mail delivery from WikiCraft works.\n", Some(&html))
        .await
        .map_err(|e| ApiError::bad(format!("sending failed: {e:#}")))?;
    Ok(Json(json!({ "ok": true, "to": to })))
}
