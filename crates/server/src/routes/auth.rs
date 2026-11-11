use axum::Json;
use axum::extract::{Path, State};
use axum::http::{HeaderMap, StatusCode, header};
use axum::response::{IntoResponse, Response};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use uuid::Uuid;

use crate::app::{AppState, new_id};
use crate::audit;
use crate::auth::{
    API_TOKEN_PREFIX, CurrentUser, MaybeUser, USER_COLUMNS, User, create_session, hash_password, random_token, session_cookie, session_token, token_hash,
    verify_password,
};
use crate::error::{ApiError, ApiResult};

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SetupStatus {
    needs_setup: bool,
}

pub async fn setup_status(State(state): State<AppState>) -> ApiResult<Json<SetupStatus>> {
    let n: i64 = sqlx::query_scalar("SELECT count(*) FROM workspaces").fetch_one(&state.db).await?;
    Ok(Json(SetupStatus { needs_setup: n == 0 }))
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SetupRequest {
    workspace_name: String,
    name: String,
    email: String,
    password: String,
}

pub fn validate_password(p: &str) -> ApiResult<()> {
    if p.chars().count() < 8 {
        return Err(ApiError::bad("password must be at least 8 characters"));
    }
    Ok(())
}

pub fn validate_email(e: &str) -> ApiResult<String> {
    let e = e.trim().to_lowercase();
    if e.len() > 254 || !e.contains('@') || e.starts_with('@') || e.ends_with('@') || e.contains(char::is_whitespace) {
        return Err(ApiError::bad("invalid e-mail address"));
    }
    Ok(e)
}

/// First-run setup: creates the workspace and its owner. Only works once.
pub async fn setup(State(state): State<AppState>, headers: HeaderMap, Json(req): Json<SetupRequest>) -> ApiResult<Response> {
    validate_password(&req.password)?;
    let email = validate_email(&req.email)?;
    if req.workspace_name.trim().is_empty() || req.name.trim().is_empty() {
        return Err(ApiError::bad("workspace name and name are required"));
    }
    // cheap check before the expensive hash (the authoritative check happens under the lock)
    let n: i64 = sqlx::query_scalar("SELECT count(*) FROM workspaces").fetch_one(&state.db).await?;
    if n > 0 {
        return Err(ApiError::Conflict("already set up".into()));
    }
    let hash = hash_password(req.password).await?;
    let mut tx = state.db.begin().await?;
    // serialize concurrent setup attempts
    sqlx::query("LOCK TABLE workspaces IN EXCLUSIVE MODE").execute(&mut *tx).await?;
    let n: i64 = sqlx::query_scalar("SELECT count(*) FROM workspaces").fetch_one(&mut *tx).await?;
    if n > 0 {
        return Err(ApiError::Conflict("already set up".into()));
    }
    let ws = new_id();
    sqlx::query("INSERT INTO workspaces (id, name) VALUES ($1, $2)").bind(ws).bind(req.workspace_name.trim()).execute(&mut *tx).await?;
    let uid = new_id();
    sqlx::query("INSERT INTO users (id, workspace_id, email, name, password_hash, role) VALUES ($1, $2, $3, $4, $5, 'owner')")
        .bind(uid)
        .bind(ws)
        .bind(&email)
        .bind(req.name.trim())
        .bind(hash)
        .execute(&mut *tx)
        .await?;
    tx.commit().await?;
    audit::log(&state, ws, Some(uid), "workspace.setup", None, None, json!({})).await;
    let cookie = create_session(&state, uid, &headers).await?;
    Ok(([(header::SET_COOKIE, cookie)], Json(json!({ "ok": true }))).into_response())
}

#[derive(Deserialize)]
pub struct LoginRequest {
    email: String,
    password: String,
}

pub async fn login(State(state): State<AppState>, headers: HeaderMap, Json(req): Json<LoginRequest>) -> ApiResult<Response> {
    // `disable_password_login` turns off local passwords; directory (LDAP) accounts still sign in here
    let has_directory = state.providers.iter().any(|p| matches!(p, crate::sso::Provider::Ldap(_)));
    if state.config.disable_password_login && !has_directory {
        return Err(ApiError::BadRequest("password login is disabled; use single sign-on".into()));
    }
    let email = req.email.trim().to_lowercase();
    let req_email_raw = req.email.clone();
    let req_password_raw = req.password.clone();
    if !crate::auth::login_allowed(&state, &email).await {
        return Err(ApiError::BadRequest("too many failed attempts, try again in 15 minutes".into()));
    }
    let row: Option<(Uuid, Uuid, Option<String>, String)> =
        sqlx::query_as("SELECT id, workspace_id, password_hash, status FROM users WHERE lower(email) = $1").bind(&email).fetch_optional(&state.db).await?;
    // constant-ish time: always run a verification (against a real hash for unknown users)
    static DUMMY: tokio::sync::OnceCell<String> = tokio::sync::OnceCell::const_new();
    let dummy = DUMMY.get_or_try_init(|| hash_password("wikicraft-timing-dummy".into())).await?.clone();
    let (ok, user) = match row {
        Some((id, ws, Some(hash), status)) if status == "active" && !state.config.disable_password_login => {
            (verify_password(req.password, hash).await, Some((id, ws)))
        }
        _ => {
            let _ = verify_password(req.password, dummy).await;
            (false, None)
        }
    };
    // directory accounts (LDAP / Active Directory)
    let (ok, user) = if ok {
        (ok, user)
    } else {
        match ldap_login(&state, &req_email_raw, &req_password_raw).await? {
            Some((id, ws)) => (true, Some((id, ws))),
            None => (false, user),
        }
    };
    match (ok, user) {
        (true, Some((id, ws))) => {
            crate::auth::clear_login_failures(&state, &email).await;
            // second factor: no session yet, just a short-lived challenge
            if crate::routes::security::required(&state, id).await? {
                let token = crate::routes::security::challenge(&state, id, None).await?;
                let methods = crate::routes::security::methods(&state, id).await?;
                return Ok(Json(json!({ "mfaRequired": true, "mfaToken": token, "methods": methods })).into_response());
            }
            let cookie = create_session(&state, id, &headers).await?;
            audit::log(&state, ws, Some(id), "auth.login", None, None, json!({})).await;
            Ok(([(header::SET_COOKIE, cookie)], Json(json!({ "ok": true }))).into_response())
        }
        _ => {
            crate::auth::record_login_failure(&state, &email).await;
            Err(ApiError::BadRequest("invalid e-mail or password".into()))
        }
    }
}

pub async fn logout(State(state): State<AppState>, headers: HeaderMap) -> ApiResult<Response> {
    if let Some(tok) = session_token(&headers) {
        sqlx::query("DELETE FROM sessions WHERE token_hash = $1").bind(token_hash(tok)).execute(&state.db).await?;
    }
    Ok(([(header::SET_COOKIE, session_cookie("", 0, state.config.secure_cookies))], StatusCode::NO_CONTENT).into_response())
}

pub async fn me(
    State(state): State<AppState>,
    MaybeUser(user): MaybeUser,
    enroll: Option<axum::Extension<crate::auth::MustEnrollTwoFactor>>,
) -> ApiResult<Json<Value>> {
    let ws: Option<(Uuid, String)> = match &user {
        Some(u) => sqlx::query_as("SELECT id, name FROM workspaces WHERE id = $1").bind(u.workspace_id).fetch_optional(&state.db).await?,
        None => sqlx::query_as("SELECT id, name FROM workspaces ORDER BY created_at LIMIT 1").fetch_optional(&state.db).await?,
    };
    let workspace = match &ws {
        Some((id, name)) => {
            let mut w = crate::routes::branding::look(&state, *id).await?;
            w.insert("id".into(), json!(id));
            w.insert("name".into(), json!(name));
            w.insert("logoVersion".into(), json!(crate::routes::branding::logo_version(&state, *id).await?));
            Some(Value::Object(w))
        }
        None => None,
    };
    // the reader's own time zone and language (profile) win over the workspace's
    let (time_zone, language): (Option<String>, Option<String>) = match &user {
        Some(u) => sqlx::query_as("SELECT timezone, language FROM users WHERE id = $1").bind(u.id).fetch_optional(&state.db).await?.unwrap_or_default(),
        None => (None, None),
    };
    Ok(Json(json!({
        "user": user,
        "workspace": workspace,
        "timeZone": time_zone,
        "language": language,
        // the workspace requires two-factor authentication and this person hasn't set it up
        "mustSetUpTwoFactor": enroll.is_some(),
    })))
}

#[derive(Serialize, sqlx::FromRow)]
#[serde(rename_all = "camelCase")]
pub struct TokenRow {
    id: Uuid,
    name: String,
    created_at: chrono::DateTime<chrono::Utc>,
    expires_at: Option<chrono::DateTime<chrono::Utc>>,
    last_used_at: Option<chrono::DateTime<chrono::Utc>>,
    /// `read` or `write`
    scope: String,
}

pub async fn list_tokens(State(state): State<AppState>, CurrentUser(user): CurrentUser) -> ApiResult<Json<Vec<TokenRow>>> {
    let rows = sqlx::query_as::<_, TokenRow>(
        "SELECT id, name, created_at, expires_at, last_used_at, scope FROM api_tokens WHERE user_id = $1 ORDER BY created_at DESC",
    )
    .bind(user.id)
    .fetch_all(&state.db)
    .await?;
    Ok(Json(rows))
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CreateToken {
    name: String,
    expires_in_days: Option<i32>,
    /// `write` (default: everything you may do) or `read` (no changes)
    scope: Option<String>,
}

/// Creates a personal API token. The plaintext token is returned once.
pub async fn create_token(State(state): State<AppState>, CurrentUser(user): CurrentUser, Json(req): Json<CreateToken>) -> ApiResult<Json<Value>> {
    let scope = req.scope.as_deref().unwrap_or("write");
    if !matches!(scope, "read" | "write") {
        return Err(ApiError::bad("scope must be read or write"));
    }
    let token = format!("{API_TOKEN_PREFIX}{}", random_token());
    let id = new_id();
    sqlx::query("INSERT INTO api_tokens (id, user_id, name, token_hash, expires_at, scope) VALUES ($1, $2, $3, $4, CASE WHEN $5::int IS NULL THEN NULL ELSE now() + make_interval(days => $5::int) END, $6)")
        .bind(id)
        .bind(user.id)
        .bind(req.name.trim())
        .bind(token_hash(&token))
        .bind(req.expires_in_days)
        .bind(scope)
        .execute(&state.db)
        .await?;
    audit::log(&state, user.workspace_id, Some(user.id), "token.create", Some("token"), Some(id), json!({"name": req.name, "scope": scope})).await;
    Ok(Json(json!({ "id": id, "token": token, "scope": scope })))
}

pub async fn delete_token(State(state): State<AppState>, CurrentUser(user): CurrentUser, Path(id): Path<Uuid>) -> ApiResult<StatusCode> {
    sqlx::query("DELETE FROM api_tokens WHERE id = $1 AND user_id = $2").bind(id).bind(user.id).execute(&state.db).await?;
    Ok(StatusCode::NO_CONTENT)
}

/// Loads a user by id within a workspace.
pub async fn load_user(state: &AppState, ws: Uuid, id: Uuid) -> ApiResult<User> {
    sqlx::query_as::<_, User>(sqlx::AssertSqlSafe(format!("SELECT {USER_COLUMNS} FROM users WHERE id = $1 AND workspace_id = $2")))
        .bind(id)
        .bind(ws)
        .fetch_optional(&state.db)
        .await?
        .ok_or(ApiError::NotFound("user"))
}

/// Tries the configured LDAP providers; returns (user id, workspace) on success.
async fn ldap_login(state: &AppState, username: &str, password: &str) -> ApiResult<Option<(Uuid, Uuid)>> {
    for p in state.providers.iter() {
        let crate::sso::Provider::Ldap(c) = p else { continue };
        match crate::sso::ldap::authenticate(c, username, password).await {
            Ok(Some(ext)) => {
                return match crate::sso::sign_in(state, p, ext).await {
                    Ok(id) => {
                        let ws: Uuid = sqlx::query_scalar("SELECT workspace_id FROM users WHERE id = $1").bind(id).fetch_one(&state.db).await?;
                        Ok(Some((id, ws)))
                    }
                    Err(crate::sso::SignInError::Denied(m)) => Err(ApiError::BadRequest(m)),
                    Err(crate::sso::SignInError::Other(e)) => Err(ApiError::Internal(e)),
                };
            }
            Ok(None) => {}
            Err(e) => tracing::warn!(provider = %c.common.id, "LDAP login failed: {e:#}"),
        }
    }
    Ok(None)
}
