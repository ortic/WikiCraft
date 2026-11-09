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
    let ws: Uuid = sqlx::query_scalar("SELECT workspace_id FROM users WHERE id = $1").bind(id).fetch_one(&state.db).await?;
    audit::log(&state, ws, Some(id), "auth.password_reset", Some("user"), Some(id), json!({})).await;
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

// ---- preferences & admin -------------------------------------------------------------------

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Preferences {
    email_notifications: Option<String>,
}

/// `GET /api/me/preferences`
pub async fn get_preferences(State(state): State<AppState>, CurrentUser(user): CurrentUser) -> ApiResult<Json<Value>> {
    let pref: String = sqlx::query_scalar("SELECT email_notifications FROM users WHERE id = $1").bind(user.id).fetch_one(&state.db).await?;
    Ok(Json(json!({ "emailNotifications": pref, "mailEnabled": state.mailer.enabled() })))
}

/// `PUT /api/me/preferences`
pub async fn put_preferences(State(state): State<AppState>, CurrentUser(user): CurrentUser, Json(req): Json<Preferences>) -> ApiResult<Json<Value>> {
    if let Some(p) = &req.email_notifications {
        if !matches!(p.as_str(), "all" | "important" | "none") {
            return Err(ApiError::bad("emailNotifications must be all, important or none"));
        }
        sqlx::query("UPDATE users SET email_notifications = $2 WHERE id = $1").bind(user.id).bind(p).execute(&state.db).await?;
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
