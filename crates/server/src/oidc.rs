//! Single sign-on with OpenID Connect (authorization code flow + PKCE + nonce).
//!
//! Users are matched by (issuer, subject); on first login an existing account with the same
//! (verified) e-mail is linked — this also claims imported Confluence placeholder accounts —
//! otherwise a member account is created (unless `WIKICRAFT_OIDC_AUTO_CREATE=false`).

use std::time::Duration;

use axum::Json;
use axum::extract::{Query, State};
use axum::http::{HeaderMap, header};
use axum::response::{IntoResponse, Redirect, Response};
use openidconnect::core::{CoreAuthenticationFlow, CoreClient, CoreProviderMetadata};
use openidconnect::{
    AuthorizationCode, ClientId, ClientSecret, CsrfToken, IssuerUrl, Nonce, PkceCodeChallenge, PkceCodeVerifier, RedirectUrl, Scope, TokenResponse,
};
use serde::Deserialize;
use serde_json::{Value, json};
use tokio::sync::OnceCell;
use uuid::Uuid;

use crate::app::{AppState, new_id};
use crate::audit;
use crate::auth::create_session;
use crate::error::{ApiError, ApiResult};

#[derive(serde::Serialize, Deserialize)]
struct Pending {
    verifier: String,
    nonce: String,
    next: String,
}

static METADATA: OnceCell<CoreProviderMetadata> = OnceCell::const_new();

fn http() -> openidconnect::reqwest::Client {
    // following redirects would open the client up to SSRF
    openidconnect::reqwest::ClientBuilder::new()
        .redirect(openidconnect::reqwest::redirect::Policy::none())
        .timeout(Duration::from_secs(15))
        .build()
        .expect("http client")
}

pub fn enabled(state: &AppState) -> bool {
    state.config.oidc_issuer.is_some() && state.config.oidc_client_id.is_some()
}

async fn metadata(state: &AppState) -> anyhow::Result<CoreProviderMetadata> {
    let issuer = state.config.oidc_issuer.clone().ok_or_else(|| anyhow::anyhow!("OIDC not configured"))?;
    Ok(METADATA
        .get_or_try_init(|| async move {
            CoreProviderMetadata::discover_async(IssuerUrl::new(issuer)?, &http()).await.map_err(|e| anyhow::anyhow!("OIDC discovery failed: {e}"))
        })
        .await?
        .clone())
}

fn redirect_uri(state: &AppState) -> String {
    format!("{}/api/auth/oidc/callback", state.config.base_url.trim_end_matches('/'))
}

/// `GET /api/auth/oidc` — whether SSO is available (login page).
pub async fn info(State(state): State<AppState>) -> Json<Value> {
    Json(json!({ "enabled": enabled(&state), "name": state.config.oidc_name, "passwordLogin": !state.config.disable_password_login }))
}

#[derive(Deserialize)]
pub struct LoginQuery {
    next: Option<String>,
}

/// `GET /api/auth/oidc/login` — redirects to the identity provider.
pub async fn login(State(state): State<AppState>, Query(q): Query<LoginQuery>) -> ApiResult<Redirect> {
    if !enabled(&state) {
        return Err(ApiError::NotFound("SSO"));
    }
    let md = metadata(&state).await.map_err(ApiError::Internal)?;
    let client = CoreClient::from_provider_metadata(
        md,
        ClientId::new(state.config.oidc_client_id.clone().unwrap_or_default()),
        state.config.oidc_client_secret.clone().map(ClientSecret::new),
    )
    .set_redirect_uri(RedirectUrl::new(redirect_uri(&state)).map_err(|e| ApiError::Internal(e.into()))?);
    let (challenge, verifier) = PkceCodeChallenge::new_random_sha256();
    let (url, csrf, nonce) = client
        .authorize_url(CoreAuthenticationFlow::AuthorizationCode, CsrfToken::new_random, Nonce::new_random)
        .add_scope(Scope::new("email".into()))
        .add_scope(Scope::new("profile".into()))
        .set_pkce_challenge(challenge)
        .url();
    let next = q.next.filter(|n| n.starts_with('/') && !n.starts_with("//")).unwrap_or_else(|| "/".into());
    // stored in the database so the callback may land on any replica
    let pending = Pending { verifier: verifier.secret().clone(), nonce: nonce.secret().clone(), next };
    sqlx::query("DELETE FROM auth_states WHERE expires_at < now()").execute(&state.db).await?;
    sqlx::query("INSERT INTO auth_states (state, data, expires_at) VALUES ($1, $2, now() + interval '10 minutes')")
        .bind(hex::encode(crate::auth::token_hash(csrf.secret())))
        .bind(serde_json::to_value(&pending)?)
        .execute(&state.db)
        .await?;
    Ok(Redirect::to(url.as_str()))
}

#[derive(Deserialize)]
pub struct Callback {
    code: Option<String>,
    state: Option<String>,
    error: Option<String>,
}

/// `GET /api/auth/oidc/callback` — completes the login, creates a session.
pub async fn callback(State(state): State<AppState>, headers: HeaderMap, Query(cb): Query<Callback>) -> Response {
    match complete(&state, &headers, cb).await {
        Ok((cookie, next)) => ([(header::SET_COOKIE, cookie)], Redirect::to(&next)).into_response(),
        Err(e) => {
            tracing::warn!("SSO login failed: {e}");
            Redirect::to(&format!("/login?error={}", urlencode(&e.to_string()))).into_response()
        }
    }
}

fn urlencode(s: &str) -> String {
    s.bytes().map(|b| if b.is_ascii_alphanumeric() { (b as char).to_string() } else { format!("%{b:02X}") }).collect()
}

async fn complete(state: &AppState, headers: &HeaderMap, cb: Callback) -> anyhow::Result<(String, String)> {
    if let Some(e) = cb.error {
        anyhow::bail!("identity provider error: {e}");
    }
    let (Some(code), Some(csrf)) = (cb.code, cb.state) else { anyhow::bail!("missing code/state") };
    let data: Option<Value> = sqlx::query_scalar("DELETE FROM auth_states WHERE state = $1 AND expires_at > now() RETURNING data")
        .bind(hex::encode(crate::auth::token_hash(&csrf)))
        .fetch_optional(&state.db)
        .await?;
    let pending: Pending = serde_json::from_value(data.ok_or_else(|| anyhow::anyhow!("login expired, please try again"))?)?;
    let md = metadata(state).await?;
    let client = CoreClient::from_provider_metadata(
        md,
        ClientId::new(state.config.oidc_client_id.clone().unwrap_or_default()),
        state.config.oidc_client_secret.clone().map(ClientSecret::new),
    )
    .set_redirect_uri(RedirectUrl::new(redirect_uri(state))?);
    let token = client
        .exchange_code(AuthorizationCode::new(code))?
        .set_pkce_verifier(PkceCodeVerifier::new(pending.verifier))
        .request_async(&http())
        .await
        .map_err(|e| anyhow::anyhow!("token exchange failed: {e}"))?;
    let id_token = token.id_token().ok_or_else(|| anyhow::anyhow!("no ID token"))?;
    let claims = id_token.claims(&client.id_token_verifier(), &Nonce::new(pending.nonce))?;
    let issuer = claims.issuer().to_string();
    let subject = claims.subject().to_string();
    let email = claims.email().map(|e| e.to_string().to_lowercase());
    let email_verified = claims.email_verified();
    let name = claims
        .name()
        .and_then(|n| n.get(None))
        .map(|n| n.to_string())
        .or_else(|| claims.preferred_username().map(|u| u.to_string()))
        .or_else(|| email.clone())
        .unwrap_or_else(|| "SSO user".into());

    let ws: Uuid = sqlx::query_scalar("SELECT id FROM workspaces ORDER BY created_at LIMIT 1").fetch_one(&state.db).await?;
    let linked: Option<(Uuid, String)> =
        sqlx::query_as("SELECT u.id, u.status FROM identities i JOIN users u ON u.id = i.user_id WHERE i.provider = $1 AND i.subject = $2")
            .bind(&issuer)
            .bind(&subject)
            .fetch_optional(&state.db)
            .await?;
    let user_id = match linked {
        Some((id, status)) => {
            if status == "deactivated" {
                anyhow::bail!("your account is deactivated");
            }
            id
        }
        None => {
            // link by e-mail unless the IdP explicitly says it's unverified
            let by_email: Option<(Uuid, String)> = match (&email, email_verified) {
                (Some(e), v) if v != Some(false) => {
                    sqlx::query_as("SELECT id, status FROM users WHERE workspace_id = $1 AND lower(email) = $2")
                        .bind(ws)
                        .bind(e)
                        .fetch_optional(&state.db)
                        .await?
                }
                _ => None,
            };
            let id = match by_email {
                Some((_, status)) if status == "deactivated" => anyhow::bail!("your account is deactivated"),
                Some((id, status)) => {
                    if status == "placeholder" {
                        sqlx::query("UPDATE users SET status = 'active', name = $2 WHERE id = $1").bind(id).bind(&name).execute(&state.db).await?;
                    }
                    id
                }
                None => {
                    if !state.config.oidc_auto_create {
                        anyhow::bail!("no account for {}; ask an administrator to invite you", email.as_deref().unwrap_or("this identity"));
                    }
                    let id = new_id();
                    sqlx::query("INSERT INTO users (id, workspace_id, email, name, role, status) VALUES ($1, $2, $3, $4, 'member', 'active')")
                        .bind(id)
                        .bind(ws)
                        .bind(email.as_deref().filter(|_| email_verified != Some(false)))
                        .bind(&name)
                        .execute(&state.db)
                        .await?;
                    audit::log(state, ws, Some(id), "user.sso_provisioned", Some("user"), Some(id), json!({"issuer": issuer})).await;
                    id
                }
            };
            sqlx::query("INSERT INTO identities (provider, subject, user_id, email) VALUES ($1, $2, $3, $4) ON CONFLICT DO NOTHING")
                .bind(&issuer)
                .bind(&subject)
                .bind(id)
                .bind(&email)
                .execute(&state.db)
                .await?;
            id
        }
    };
    sqlx::query("UPDATE identities SET last_login = now() WHERE provider = $1 AND subject = $2").bind(&issuer).bind(&subject).execute(&state.db).await?;
    let cookie = create_session(state, user_id, headers).await.map_err(|e| anyhow::anyhow!("{e}"))?;
    audit::log(state, ws, Some(user_id), "auth.sso_login", None, None, json!({"issuer": issuer})).await;
    Ok((cookie, pending.next))
}
