//! OpenID Connect providers (authorization code flow + PKCE + nonce). Presets for Google,
//! Microsoft Entra ID and GitLab; any other OIDC issuer (Okta, Keycloak, Authentik, Auth0, …)
//! via `issuer`.

use std::sync::LazyLock;
use std::time::Duration;

use axum::extract::{Path, Query, State};
use axum::http::{HeaderMap, header};
use axum::response::{IntoResponse, Redirect, Response};
use base64::Engine;
use dashmap::DashMap;
use openidconnect::core::{CoreAuthenticationFlow, CoreClient, CoreProviderMetadata};
use openidconnect::{AuthorizationCode, ClientId, ClientSecret, CsrfToken, IssuerUrl, Nonce, PkceCodeChallenge, PkceCodeVerifier, RedirectUrl, Scope, TokenResponse};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

use super::{External, OidcConfig, Provider, SignInError};
use crate::app::AppState;
use crate::auth::create_session;

static METADATA: LazyLock<DashMap<String, CoreProviderMetadata>> = LazyLock::new(DashMap::new);

#[derive(Serialize, Deserialize)]
struct Pending {
    provider: String,
    verifier: String,
    nonce: String,
    next: String,
}

fn http() -> openidconnect::reqwest::Client {
    // following redirects would open the client up to SSRF
    openidconnect::reqwest::ClientBuilder::new().redirect(openidconnect::reqwest::redirect::Policy::none()).timeout(Duration::from_secs(15)).build().expect("http client")
}

fn issuer(c: &OidcConfig) -> String {
    if let Some(i) = &c.issuer {
        return i.clone();
    }
    match c.preset.as_deref() {
        Some("google") => "https://accounts.google.com".into(),
        Some("microsoft") => format!("https://login.microsoftonline.com/{}/v2.0", c.tenant.as_deref().unwrap_or("organizations")),
        Some("gitlab") => "https://gitlab.com".into(),
        _ => String::new(),
    }
}

async fn metadata(c: &OidcConfig) -> anyhow::Result<CoreProviderMetadata> {
    let iss = issuer(c);
    if let Some(m) = METADATA.get(&iss) {
        return Ok(m.clone());
    }
    let m = CoreProviderMetadata::discover_async(IssuerUrl::new(iss.clone())?, &http()).await.map_err(|e| anyhow::anyhow!("OIDC discovery for {iss} failed: {e}"))?;
    METADATA.insert(iss, m.clone());
    Ok(m)
}

pub fn redirect_uri(state: &AppState, provider_id: &str) -> String {
    let base = state.config.base_url.trim_end_matches('/');
    if provider_id == "oidc" {
        // the single `WIKICRAFT_OIDC_*` provider keeps its original, already registered URI
        return format!("{base}/api/auth/oidc/callback");
    }
    format!("{base}/api/auth/sso/{provider_id}/callback")
}

macro_rules! oidc_client {
    ($state:expr, $c:expr, $md:expr) => {
        CoreClient::from_provider_metadata($md, ClientId::new($c.client_id.clone()), $c.client_secret.clone().map(ClientSecret::new))
            .set_redirect_uri(RedirectUrl::new(redirect_uri($state, &$c.common.id)).map_err(anyhow::Error::from)?)
    };
}

/// Starts the login: redirect to the identity provider.
pub async fn login(state: &AppState, c: &OidcConfig, next: String) -> anyhow::Result<Response> {
    let md = metadata(c).await?;
    let client = oidc_client!(state, c, md);
    let (challenge, verifier) = PkceCodeChallenge::new_random_sha256();
    let pending_nonce = Nonce::new_random();
    let n2 = pending_nonce.clone();
    // the CSRF token is our state key (stored in the DB so the callback may hit any replica)
    let key = super::put_state(state, &json!({})).await?;
    let k2 = key.clone();
    let mut req = client.authorize_url(CoreAuthenticationFlow::AuthorizationCode, move || CsrfToken::new(k2.clone()), move || n2.clone()).set_pkce_challenge(challenge);
    let mut scopes: Vec<String> = vec!["email".into(), "profile".into()];
    scopes.extend(c.scopes.iter().cloned());
    for s in scopes {
        req = req.add_scope(Scope::new(s));
    }
    let (url, _, _) = req.url();
    let pending = Pending { provider: c.common.id.clone(), verifier: verifier.secret().clone(), nonce: pending_nonce.secret().clone(), next };
    sqlx::query("UPDATE auth_states SET data = $2 WHERE state = $1").bind(hex::encode(crate::auth::token_hash(&key))).bind(serde_json::to_value(&pending)?).execute(&state.db).await?;
    Ok(Redirect::to(url.as_str()).into_response())
}

#[derive(Deserialize)]
pub struct Callback {
    code: Option<String>,
    state: Option<String>,
    error: Option<String>,
    error_description: Option<String>,
}

pub async fn callback(state: &AppState, provider: &Provider, c: &OidcConfig, headers: &HeaderMap, cb: Callback) -> Response {
    match complete(state, provider, c, headers, cb).await {
        Ok((cookie, next)) => ([(header::SET_COOKIE, cookie)], Redirect::to(&next)).into_response(),
        Err(SignInError::Denied(msg)) => super::login_error(&msg),
        Err(SignInError::Other(e)) => {
            tracing::warn!(provider = %c.common.id, "SSO login failed: {e:#}");
            super::login_error(&format!("{e}"))
        }
    }
}

/// Reads extra claims (e.g. `groups`) from an ID token whose signature was already verified.
fn raw_claims(id_token: &str) -> Value {
    id_token.split('.').nth(1).and_then(|p| base64::engine::general_purpose::URL_SAFE_NO_PAD.decode(p).ok()).and_then(|b| serde_json::from_slice(&b).ok()).unwrap_or(Value::Null)
}

async fn complete(state: &AppState, provider: &Provider, c: &OidcConfig, headers: &HeaderMap, cb: Callback) -> Result<(String, String), SignInError> {
    if let Some(e) = cb.error {
        return Err(SignInError::Denied(format!("identity provider error: {e} {}", cb.error_description.unwrap_or_default())));
    }
    let (Some(code), Some(key)) = (cb.code, cb.state) else { return Err(SignInError::Denied("missing code/state".into())) };
    let pending: Pending = serde_json::from_value(super::take_state(state, &key).await.map_err(|e| SignInError::Denied(e.to_string()))?).map_err(|_| SignInError::Denied("login expired, please try again".into()))?;
    if pending.provider != c.common.id {
        return Err(SignInError::Denied("login state belongs to another provider".into()));
    }
    let md = metadata(c).await?;
    let client = oidc_client!(state, c, md);
    let token = client
        .exchange_code(AuthorizationCode::new(code))
        .map_err(|e| anyhow::anyhow!("{e}"))?
        .set_pkce_verifier(PkceCodeVerifier::new(pending.verifier))
        .request_async(&http())
        .await
        .map_err(|e| anyhow::anyhow!("token exchange failed: {e}"))?;
    let id_token = token.id_token().ok_or_else(|| anyhow::anyhow!("no ID token"))?;
    let claims = id_token.claims(&client.id_token_verifier(), &Nonce::new(pending.nonce)).map_err(|e| anyhow::anyhow!("invalid ID token: {e}"))?;
    let raw = raw_claims(&id_token.to_string());
    let groups: Vec<String> = c
        .groups_claim
        .as_deref()
        .and_then(|g| raw.get(g))
        .and_then(Value::as_array)
        .map(|a| a.iter().filter_map(|v| v.as_str().map(String::from)).collect())
        .unwrap_or_default();
    let email = claims.email().map(|e| e.to_string()).or_else(|| raw.get("preferred_username").and_then(Value::as_str).filter(|u| u.contains('@')).map(String::from));
    let name = claims.name().and_then(|n| n.get(None)).map(|n| n.to_string()).or_else(|| claims.preferred_username().map(|u| u.to_string()));
    let ext = External {
        provider_key: claims.issuer().to_string(),
        subject: claims.subject().to_string(),
        email,
        // Entra ID doesn't send email_verified; its `email` comes from the tenant directory
        email_verified: claims.email_verified(),
        name,
        groups,
    };
    let user = super::sign_in(state, provider, ext).await?;
    let cookie = create_session(state, user, headers).await.map_err(|e| anyhow::anyhow!("{e}"))?;
    Ok((cookie, pending.next))
}

// ---- routes -------------------------------------------------------------------------------

#[derive(Deserialize)]
pub struct LoginQuery {
    next: Option<String>,
}

/// `GET /api/auth/sso/{id}/login`
pub async fn sso_login(State(state): State<AppState>, Path(id): Path<String>, Query(q): Query<LoginQuery>) -> Response {
    let next = super::safe_next(q.next);
    let Some(p) = super::find(&state, &id).cloned() else { return super::login_error("unknown sign-in provider") };
    let res = match &p {
        Provider::Oidc(c) => login(&state, c, next).await,
        Provider::Github(c) => super::github::login(&state, c, next).await,
        #[cfg(feature = "saml")]
        Provider::Saml(c) => super::saml::login(&state, c, next).await,
        _ => return super::login_error("this provider signs in with username and password"),
    };
    res.unwrap_or_else(|e| {
        tracing::warn!(provider = %id, "SSO login start failed: {e:#}");
        super::login_error(&format!("{e}"))
    })
}

/// `GET /api/auth/sso/{id}/callback`
pub async fn sso_callback(State(state): State<AppState>, Path(id): Path<String>, headers: HeaderMap, Query(cb): Query<Callback>) -> Response {
    let Some(p) = super::find(&state, &id).cloned() else { return super::login_error("unknown sign-in provider") };
    match &p {
        Provider::Oidc(c) => callback(&state, &p, c, &headers, cb).await,
        Provider::Github(c) => super::github::callback(&state, &p, c, &headers, cb.code, cb.state, cb.error).await,
        _ => super::login_error("unsupported callback"),
    }
}

/// Legacy routes for the single `WIKICRAFT_OIDC_*` provider (id `oidc`).
pub async fn legacy_info(State(state): State<AppState>) -> axum::Json<Value> {
    let p = super::find(&state, "oidc");
    axum::Json(json!({ "enabled": p.is_some(), "name": p.map(|p| p.display_name()), "passwordLogin": !state.config.disable_password_login }))
}

pub async fn legacy_login(state: State<AppState>, q: Query<LoginQuery>) -> Response {
    sso_login(state, Path("oidc".into()), q).await
}

pub async fn legacy_callback(state: State<AppState>, headers: HeaderMap, cb: Query<Callback>) -> Response {
    sso_callback(state, Path("oidc".into()), headers, cb).await
}
