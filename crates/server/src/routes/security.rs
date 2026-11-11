//! Account security: active sessions (list / sign out) and TOTP two-factor authentication.

use axum::Json;
use axum::extract::{Path, State};
use axum::http::{HeaderMap, StatusCode, header};
use axum::response::{IntoResponse, Response};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use uuid::Uuid;

use crate::app::AppState;
use crate::audit;
use crate::auth::{CurrentUser, create_session, require_admin, session_token, token_hash, verify_password};
use crate::error::{ApiError, ApiResult};
use crate::mfa;

#[derive(Serialize, sqlx::FromRow)]
#[serde(rename_all = "camelCase")]
pub struct SessionRow {
    id: Uuid,
    created_at: chrono::DateTime<chrono::Utc>,
    last_seen_at: chrono::DateTime<chrono::Utc>,
    expires_at: chrono::DateTime<chrono::Utc>,
    user_agent: Option<String>,
    ip: Option<String>,
    current: bool,
}

/// `GET /api/me/sessions`
pub async fn sessions(State(state): State<AppState>, CurrentUser(user): CurrentUser, headers: HeaderMap) -> ApiResult<Json<Vec<SessionRow>>> {
    let current = session_token(&headers).map(token_hash).unwrap_or_default();
    Ok(Json(
        sqlx::query_as::<_, SessionRow>(
            "SELECT id, created_at, last_seen_at, expires_at, user_agent, ip, token_hash = $2 AS current FROM sessions
             WHERE user_id = $1 AND expires_at > now() ORDER BY (token_hash = $2) DESC, last_seen_at DESC",
        )
        .bind(user.id)
        .bind(current)
        .fetch_all(&state.db)
        .await?,
    ))
}

/// `DELETE /api/me/sessions/{id}`
pub async fn revoke_session(State(state): State<AppState>, CurrentUser(user): CurrentUser, Path(id): Path<Uuid>) -> ApiResult<StatusCode> {
    let n = sqlx::query("DELETE FROM sessions WHERE id = $1 AND user_id = $2").bind(id).bind(user.id).execute(&state.db).await?.rows_affected();
    if n == 0 {
        return Err(ApiError::NotFound("session"));
    }
    Ok(StatusCode::NO_CONTENT)
}

/// `POST /api/me/sessions/revoke-others` — sign out everywhere else.
pub async fn revoke_others(State(state): State<AppState>, CurrentUser(user): CurrentUser, headers: HeaderMap) -> ApiResult<Json<Value>> {
    let current = session_token(&headers).map(token_hash).unwrap_or_default();
    let n = sqlx::query("DELETE FROM sessions WHERE user_id = $1 AND token_hash <> $2").bind(user.id).bind(current).execute(&state.db).await?.rows_affected();
    audit::log(&state, user.workspace_id, Some(user.id), "auth.sessions_revoked", Some("user"), Some(user.id), json!({"count": n})).await;
    Ok(Json(json!({ "revoked": n })))
}

// ---- two-factor ----------------------------------------------------------------------------

/// `GET /api/me/mfa`
pub async fn status(State(state): State<AppState>, CurrentUser(user): CurrentUser) -> ApiResult<Json<Value>> {
    let (enabled, codes, has_password): (Option<chrono::DateTime<chrono::Utc>>, i32, bool) =
        sqlx::query_as("SELECT totp_enabled_at, cardinality(recovery_codes), password_hash IS NOT NULL FROM users WHERE id = $1")
            .bind(user.id)
            .fetch_one(&state.db)
            .await?;
    let passkeys: i64 = sqlx::query_scalar("SELECT count(*) FROM passkeys WHERE user_id = $1").bind(user.id).fetch_one(&state.db).await?;
    Ok(Json(json!({ "enabled": enabled.is_some(), "enabledAt": enabled, "recoveryCodesLeft": codes, "hasPassword": has_password, "passkeys": passkeys })))
}

/// `POST /api/me/mfa/setup` — a new pending secret (not active until confirmed with a code).
pub async fn setup(State(state): State<AppState>, CurrentUser(user): CurrentUser) -> ApiResult<Json<Value>> {
    // replacing an active factor needs the password (disable) first: a stolen session alone
    // must not swap in another authenticator
    let totp_on: bool = sqlx::query_scalar("SELECT totp_enabled_at IS NOT NULL FROM users WHERE id = $1").bind(user.id).fetch_one(&state.db).await?;
    if totp_on {
        return Err(ApiError::bad("two-factor authentication is already on; turn it off first"));
    }
    let secret = mfa::new_secret();
    sqlx::query("UPDATE users SET totp_pending = $2 WHERE id = $1").bind(user.id).bind(&secret).execute(&state.db).await?;
    let account = user.email.clone().unwrap_or_else(|| user.name.clone());
    let uri = mfa::otpauth_uri(&account, &secret);
    let svg = mfa::qr_svg(&uri).map_err(ApiError::Internal)?;
    Ok(Json(json!({ "secret": secret, "uri": uri, "qrSvg": svg })))
}

#[derive(Deserialize)]
pub struct CodeRequest {
    code: String,
}

/// `POST /api/me/mfa/enable {code}` — returns the recovery codes (shown once).
pub async fn enable(State(state): State<AppState>, CurrentUser(user): CurrentUser, Json(req): Json<CodeRequest>) -> ApiResult<Json<Value>> {
    let pending: Option<String> = sqlx::query_scalar("SELECT totp_pending FROM users WHERE id = $1").bind(user.id).fetch_one(&state.db).await?;
    let pending = pending.ok_or_else(|| ApiError::bad("start the setup first"))?;
    let step =
        mfa::matching_step(&pending, &req.code).ok_or_else(|| ApiError::bad("that code doesn't match — check the time on your phone and try the next one"))?;
    let (codes, hashes) = mfa::recovery_codes();
    sqlx::query(
        "UPDATE users SET totp_secret = totp_pending, totp_pending = NULL, totp_enabled_at = now(), totp_last_step = $2, recovery_codes = $3 WHERE id = $1",
    )
    .bind(user.id)
    .bind(step as i64)
    .bind(&hashes)
    .execute(&state.db)
    .await?;
    audit::log(&state, user.workspace_id, Some(user.id), "auth.mfa_enabled", Some("user"), Some(user.id), json!({})).await;
    Ok(Json(json!({ "recoveryCodes": codes })))
}

#[derive(Deserialize)]
pub struct DisableRequest {
    password: Option<String>,
    code: Option<String>,
}

/// `POST /api/me/mfa/disable` — needs the password (or, for SSO-only accounts, a code).
pub async fn disable(State(state): State<AppState>, CurrentUser(user): CurrentUser, Json(req): Json<DisableRequest>) -> ApiResult<StatusCode> {
    let hash: Option<String> = sqlx::query_scalar("SELECT password_hash FROM users WHERE id = $1").bind(user.id).fetch_one(&state.db).await?;
    let ok = match (hash, &req.password, &req.code) {
        (Some(h), Some(pw), _) => verify_password(pw.clone(), h).await,
        (_, _, Some(code)) => mfa::verify(&state, user.id, code).await.map_err(mfa_err)?,
        _ => false,
    };
    if !ok {
        return Err(ApiError::bad("password or code is wrong"));
    }
    clear(&state, user.id).await?;
    audit::log(&state, user.workspace_id, Some(user.id), "auth.mfa_disabled", Some("user"), Some(user.id), json!({})).await;
    Ok(StatusCode::NO_CONTENT)
}

/// `POST /api/me/mfa/recovery-codes {code}` — replaces the recovery codes.
pub async fn new_recovery_codes(State(state): State<AppState>, CurrentUser(user): CurrentUser, Json(req): Json<CodeRequest>) -> ApiResult<Json<Value>> {
    if !mfa::verify(&state, user.id, &req.code).await.map_err(mfa_err)? {
        return Err(ApiError::bad("that code doesn't match"));
    }
    let (codes, hashes) = mfa::recovery_codes();
    sqlx::query("UPDATE users SET recovery_codes = $2 WHERE id = $1").bind(user.id).bind(&hashes).execute(&state.db).await?;
    Ok(Json(json!({ "recoveryCodes": codes })))
}

fn mfa_err(e: anyhow::Error) -> ApiError {
    if e.is::<mfa::TooManyAttempts>() { ApiError::bad(e.to_string()) } else { ApiError::Internal(e) }
}

async fn clear(state: &AppState, user: Uuid) -> ApiResult<()> {
    sqlx::query("UPDATE users SET totp_secret = NULL, totp_pending = NULL, totp_enabled_at = NULL, totp_last_step = NULL, recovery_codes = '{}' WHERE id = $1")
        .bind(user)
        .execute(&state.db)
        .await?;
    Ok(())
}

/// Removes a person's passkeys too (an admin reset after a lost device).
async fn clear_all(state: &AppState, user: Uuid) -> ApiResult<()> {
    clear(state, user).await?;
    sqlx::query("DELETE FROM passkeys WHERE user_id = $1").bind(user).execute(&state.db).await?;
    Ok(())
}

/// `POST /api/users/{id}/reset-mfa` (admins) — for people who lost their phone and codes.
pub async fn admin_reset(State(state): State<AppState>, CurrentUser(admin): CurrentUser, Path(id): Path<Uuid>) -> ApiResult<StatusCode> {
    require_admin(&admin)?;
    let target = crate::routes::auth::load_user(&state, admin.workspace_id, id).await?;
    // only the owner can reset admins (and the owner's own second factor is theirs to manage)
    if (target.role == "owner" || target.role == "admin") && admin.role != "owner" {
        return Err(ApiError::Forbidden);
    }
    clear_all(&state, id).await?;
    sqlx::query("DELETE FROM sessions WHERE user_id = $1").bind(id).execute(&state.db).await?;
    sqlx::query("DELETE FROM api_tokens WHERE user_id = $1").bind(id).execute(&state.db).await?;
    audit::log(&state, admin.workspace_id, Some(admin.id), "auth.mfa_reset", Some("user"), Some(id), json!({})).await;
    Ok(StatusCode::NO_CONTENT)
}

/// Whether a user must pass a second factor after the password (a code or a passkey).
pub async fn required(state: &AppState, user: Uuid) -> ApiResult<bool> {
    Ok(sqlx::query_scalar::<_, bool>("SELECT totp_enabled_at IS NOT NULL OR EXISTS (SELECT 1 FROM passkeys WHERE user_id = $1) FROM users WHERE id = $1")
        .bind(user)
        .fetch_one(&state.db)
        .await?)
}

/// The second factors a user can use: `code` (authenticator app or recovery code), `passkey`.
pub async fn methods(state: &AppState, user: Uuid) -> ApiResult<Vec<&'static str>> {
    let (totp, passkey): (bool, bool) =
        sqlx::query_as("SELECT totp_enabled_at IS NOT NULL, EXISTS (SELECT 1 FROM passkeys WHERE user_id = $1) FROM users WHERE id = $1")
            .bind(user)
            .fetch_one(&state.db)
            .await?;
    Ok([("code", totp), ("passkey", passkey)].into_iter().filter(|(_, on)| *on).map(|(m, _)| m).collect())
}

/// Starts the second step: a short-lived token naming the user who passed the password check.
pub async fn challenge(state: &AppState, user: Uuid, next: Option<&str>) -> ApiResult<String> {
    crate::sso::put_state(state, &json!({"mfaUser": user, "attempts": 0, "next": next}), None).await.map_err(ApiError::Internal)
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct MfaLogin {
    /// from the password login response; SSO logins and resets use the `wc_mfa` cookie instead
    mfa_token: Option<String>,
    /// a code from the authenticator app or a recovery code …
    #[serde(default)]
    code: String,
    /// … or the answer of a passkey (after `POST /auth/mfa/passkey`)
    #[serde(default)]
    passkey: Option<crate::routes::passkeys::Assertion>,
}

fn mfa_token(token: Option<&str>, headers: &HeaderMap) -> ApiResult<String> {
    token
        .filter(|t| !t.is_empty())
        .map(String::from)
        .or_else(|| crate::auth::cookie_value(headers, crate::sso::MFA_COOKIE).map(String::from))
        .ok_or_else(|| ApiError::bad("the sign-in expired, please log in again"))
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PasskeyChallenge {
    mfa_token: Option<String>,
}

/// `POST /api/auth/mfa/passkey {mfaToken?}` — the second step with a passkey: options for
/// `navigator.credentials.get()` (null when the person has none), plus which methods they have.
pub async fn passkey_challenge(State(state): State<AppState>, headers: HeaderMap, Json(req): Json<PasskeyChallenge>) -> ApiResult<Json<Value>> {
    let token = mfa_token(req.mfa_token.as_deref(), &headers)?;
    let key = hex::encode(token_hash(&token));
    let challenge = crate::webauthn::challenge();
    let data: Option<Value> = sqlx::query_scalar(
        "UPDATE auth_states SET data = jsonb_set(data, '{passkeyChallenge}', to_jsonb($2::text))
         WHERE state = $1 AND expires_at > now() AND data ? 'mfaUser' RETURNING data",
    )
    .bind(&key)
    .bind(crate::webauthn::b64(&challenge))
    .fetch_optional(&state.db)
    .await?;
    let data = data.ok_or_else(|| ApiError::bad("the sign-in expired, please log in again"))?;
    let user: Uuid = data.get("mfaUser").and_then(Value::as_str).and_then(|s| s.parse().ok()).ok_or_else(|| ApiError::bad("invalid sign-in"))?;
    let methods = methods(&state, user).await?;
    let options = if methods.contains(&"passkey") { Some(crate::routes::passkeys::request_options(&state, Some(user), &challenge).await?) } else { None };
    Ok(Json(json!({ "methods": methods, "publicKey": options })))
}

/// `POST /api/auth/mfa {mfaToken, code}` — completes a password login (5 tries per challenge,
/// counted atomically; plus the per-user limit in `mfa::verify`).
pub async fn login(State(state): State<AppState>, headers: HeaderMap, Json(req): Json<MfaLogin>) -> ApiResult<Response> {
    let token = mfa_token(req.mfa_token.as_deref(), &headers)?;
    let key = hex::encode(token_hash(&token));
    let data: Option<Value> = sqlx::query_scalar(
        "UPDATE auth_states SET data = jsonb_set(data, '{attempts}', to_jsonb(COALESCE((data->>'attempts')::int, 0) + 1))
         WHERE state = $1 AND expires_at > now() AND data ? 'mfaUser' RETURNING data",
    )
    .bind(&key)
    .fetch_optional(&state.db)
    .await?;
    let data = data.ok_or_else(|| ApiError::bad("the sign-in expired, please log in again"))?;
    let user: Uuid = data.get("mfaUser").and_then(Value::as_str).and_then(|s| s.parse().ok()).ok_or_else(|| ApiError::bad("invalid sign-in"))?;
    let attempts = data.get("attempts").and_then(Value::as_i64).unwrap_or(1);
    if attempts > 5 {
        sqlx::query("DELETE FROM auth_states WHERE state = $1").bind(&key).execute(&state.db).await?;
        return Err(ApiError::bad("too many wrong codes, please log in again"));
    }
    let ok = match &req.passkey {
        Some(answer) => {
            let challenge = data.get("passkeyChallenge").and_then(Value::as_str).and_then(|c| crate::webauthn::unb64(c).ok());
            let challenge = challenge.ok_or_else(|| ApiError::bad("start the passkey step first"))?;
            match crate::routes::passkeys::verify(&state, &challenge, answer, Some(user)).await {
                Ok(_) => true,
                Err(ApiError::BadRequest(m)) => {
                    tracing::info!(%user, "passkey second step refused: {m}");
                    false
                }
                Err(e) => return Err(e),
            }
        }
        None => match mfa::verify(&state, user, &req.code).await {
            Ok(ok) => ok,
            Err(e) if e.is::<mfa::TooManyAttempts>() => return Err(ApiError::bad(e.to_string())),
            Err(e) => return Err(ApiError::Internal(e)),
        },
    };
    if !ok {
        if attempts >= 5 {
            sqlx::query("DELETE FROM auth_states WHERE state = $1").bind(&key).execute(&state.db).await?;
            return Err(ApiError::bad("too many wrong codes, please log in again"));
        }
        return Err(ApiError::bad(if req.passkey.is_some() { "that passkey didn't work" } else { "that code doesn't match" }));
    }
    sqlx::query("DELETE FROM auth_states WHERE state = $1").bind(&key).execute(&state.db).await?;
    let ws: Uuid = sqlx::query_scalar("SELECT workspace_id FROM users WHERE id = $1 AND status = 'active'")
        .bind(user)
        .fetch_optional(&state.db)
        .await?
        .ok_or(ApiError::Forbidden)?;
    let next = data.get("next").and_then(Value::as_str).map(String::from);
    let cookie = create_session(&state, user, &headers).await?;
    audit::log(&state, ws, Some(user), "auth.login", None, None, json!({"mfa": true})).await;
    let mut resp = ([(header::SET_COOKIE, cookie)], Json(json!({ "ok": true, "next": next }))).into_response();
    resp.headers_mut().append(header::SET_COOKIE, crate::sso::mfa_cookie(&state, "", 0).parse().expect("cookie"));
    Ok(resp)
}
