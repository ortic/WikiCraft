//! External authentication providers: OpenID Connect (Microsoft Entra ID, Google, Okta,
//! Keycloak, Authentik, GitLab, …), GitHub (OAuth2), LDAP / Active Directory and SAML 2.0
//! (cargo feature `saml`).
//!
//! Providers are configured with `WIKICRAFT_AUTH_PROVIDERS` (JSON list) or
//! `WIKICRAFT_AUTH_PROVIDERS_FILE`, plus shortcuts (`WIKICRAFT_OIDC_*`, `WIKICRAFT_GOOGLE_*`,
//! `WIKICRAFT_MICROSOFT_*`). All of them end in [`sign_in`], which applies the same rules:
//! allowed e-mail domains, identity linking by (provider, subject), linking existing/imported
//! accounts by verified e-mail, just-in-time provisioning and optional group sync.

pub mod github;
pub mod ldap;
pub mod oidc;
#[cfg(feature = "saml")]
pub mod saml;

use std::collections::HashSet;

use axum::Json;
use axum::extract::State;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use uuid::Uuid;

use crate::app::{AppState, new_id};
use crate::audit;
use crate::config::Config;

/// Settings shared by all provider types.
#[derive(Debug, Clone, Deserialize, Serialize, Default)]
#[serde(rename_all = "camelCase")]
pub struct Common {
    pub id: String,
    #[serde(default)]
    pub name: Option<String>,
    /// Only these e-mail domains may sign in (empty = any).
    #[serde(default)]
    pub allowed_domains: Vec<String>,
    /// Create accounts for unknown users (default: true when the provider is private or
    /// `allowedDomains` restricts it, false for public providers like Google/GitHub).
    #[serde(default)]
    pub auto_create: Option<bool>,
    /// Mirror the provider's groups into WikiCraft groups.
    #[serde(default)]
    pub sync_groups: bool,
    /// Role of auto-created users: `member` (default) or `guest`.
    #[serde(default)]
    pub default_role: Option<String>,
    /// Trust e-mail addresses the provider doesn't explicitly mark as verified (default: yes
    /// for LDAP, SAML and single-tenant Microsoft, whose directories are administered; no for
    /// other OIDC providers and GitHub). `true` also allows linking to existing admin accounts.
    #[serde(default)]
    pub trust_email: Option<bool>,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct OidcConfig {
    #[serde(flatten)]
    pub common: Common,
    /// `google`, `microsoft`, `gitlab` or none (generic: give `issuer`)
    #[serde(default)]
    pub preset: Option<String>,
    #[serde(default)]
    pub issuer: Option<String>,
    /// Microsoft Entra tenant id (for preset `microsoft`)
    #[serde(default)]
    pub tenant: Option<String>,
    pub client_id: String,
    #[serde(default)]
    pub client_secret: Option<String>,
    #[serde(default)]
    pub scopes: Vec<String>,
    /// Claim holding group names (e.g. `groups`, Keycloak `groups`, Entra `groups`/`roles`).
    #[serde(default)]
    pub groups_claim: Option<String>,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct GithubConfig {
    #[serde(flatten)]
    pub common: Common,
    pub client_id: String,
    pub client_secret: String,
    /// GitHub Enterprise Server API base, default https://api.github.com
    #[serde(default)]
    pub api_url: Option<String>,
    #[serde(default)]
    pub web_url: Option<String>,
    /// Members of these organizations only (requires the read:org scope).
    #[serde(default)]
    pub allowed_orgs: Vec<String>,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct LdapConfig {
    #[serde(flatten)]
    pub common: Common,
    /// `ldap://host:389` or `ldaps://host:636`
    pub url: String,
    #[serde(default)]
    pub start_tls: bool,
    /// Accept self-signed certificates (testing only).
    #[serde(default)]
    pub insecure_skip_verify: bool,
    /// Service account used to search users (anonymous search when absent).
    #[serde(default)]
    pub bind_dn: Option<String>,
    #[serde(default)]
    pub bind_password: Option<String>,
    pub base_dn: String,
    /// `{username}` is replaced (escaped). Default matches uid, mail or sAMAccountName.
    #[serde(default)]
    pub user_filter: Option<String>,
    #[serde(default)]
    pub email_attribute: Option<String>,
    #[serde(default)]
    pub name_attribute: Option<String>,
    /// Stable id attribute (entryUUID / objectGUID); the DN is used when absent.
    #[serde(default)]
    pub id_attribute: Option<String>,
    #[serde(default)]
    pub group_attribute: Option<String>,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SamlConfig {
    #[serde(flatten)]
    pub common: Common,
    /// IdP metadata XML (inline) or URL to fetch it from.
    #[serde(default)]
    pub idp_metadata_url: Option<String>,
    #[serde(default)]
    pub idp_metadata_xml: Option<String>,
    /// SP entity id (default `{base_url}/api/auth/sso/{id}/metadata`).
    #[serde(default)]
    pub entity_id: Option<String>,
    #[serde(default)]
    pub email_attribute: Option<String>,
    #[serde(default)]
    pub name_attribute: Option<String>,
    #[serde(default)]
    pub groups_attribute: Option<String>,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(tag = "type", rename_all = "lowercase")]
pub enum Provider {
    Oidc(OidcConfig),
    Github(GithubConfig),
    Ldap(LdapConfig),
    Saml(SamlConfig),
}

impl Provider {
    pub fn common(&self) -> &Common {
        match self {
            Provider::Oidc(c) => &c.common,
            Provider::Github(c) => &c.common,
            Provider::Ldap(c) => &c.common,
            Provider::Saml(c) => &c.common,
        }
    }

    pub fn id(&self) -> &str {
        &self.common().id
    }

    pub fn kind(&self) -> &'static str {
        match self {
            Provider::Oidc(_) => "oidc",
            Provider::Github(_) => "github",
            Provider::Ldap(_) => "ldap",
            Provider::Saml(_) => "saml",
        }
    }

    pub fn display_name(&self) -> String {
        if let Some(n) = &self.common().name {
            return n.clone();
        }
        match self {
            Provider::Oidc(c) => match c.preset.as_deref() {
                Some("google") => "Google".into(),
                Some("microsoft") => "Microsoft".into(),
                Some("gitlab") => "GitLab".into(),
                _ => "Single sign-on".into(),
            },
            Provider::Github(_) => "GitHub".into(),
            Provider::Ldap(_) => "Directory (LDAP)".into(),
            Provider::Saml(_) => "SAML single sign-on".into(),
        }
    }

    /// Public identity providers anyone can register with.
    fn is_public(&self) -> bool {
        match self {
            Provider::Github(_) => true,
            Provider::Oidc(c) => {
                matches!(c.preset.as_deref(), Some("google") | Some("gitlab"))
                    || (c.preset.as_deref() == Some("microsoft") && matches!(c.tenant.as_deref(), None | Some("common" | "consumers")))
            }
            _ => false,
        }
    }

    /// Whether this sign-in's e-mail may be used to link, claim or restrict by domain.
    pub fn email_trusted(&self, verified: Option<bool>) -> bool {
        match verified {
            Some(v) => v,
            None => self.common().trust_email.unwrap_or(match self {
                Provider::Ldap(_) | Provider::Saml(_) => true,
                Provider::Oidc(c) => c.preset.as_deref() == Some("microsoft"),
                Provider::Github(_) => false,
            }),
        }
    }

    pub fn auto_create(&self) -> bool {
        let c = self.common();
        let restricted = !c.allowed_domains.is_empty() || matches!(self, Provider::Github(g) if !g.allowed_orgs.is_empty());
        c.auto_create.unwrap_or(!self.is_public() || restricted)
    }
}

fn env(name: &str) -> Option<String> {
    std::env::var(name).ok().filter(|v| !v.trim().is_empty())
}

fn split_list(s: Option<String>) -> Vec<String> {
    s.map(|v| v.split(',').map(|d| d.trim().to_lowercase()).filter(|d| !d.is_empty()).collect()).unwrap_or_default()
}

/// Builds the provider list from configuration. Fails on invalid configuration (fail fast).
pub fn load(config: &Config) -> anyhow::Result<Vec<Provider>> {
    let mut out: Vec<Provider> = Vec::new();
    let mut json = config.auth_providers.clone();
    if let Some(path) = &config.auth_providers_file {
        json = Some(std::fs::read_to_string(path).map_err(|e| anyhow::anyhow!("reading {}: {e}", path.display()))?);
    }
    if let Some(j) = json.filter(|j| !j.trim().is_empty()) {
        out.extend(serde_json::from_str::<Vec<Provider>>(&j).map_err(|e| anyhow::anyhow!("invalid WIKICRAFT_AUTH_PROVIDERS: {e}"))?);
    }
    // legacy single OIDC provider
    if let (Some(issuer), Some(client_id)) = (&config.oidc_issuer, &config.oidc_client_id) {
        out.push(Provider::Oidc(OidcConfig {
            common: Common { id: "oidc".into(), name: Some(config.oidc_name.clone()), auto_create: Some(config.oidc_auto_create), ..Default::default() },
            preset: None,
            issuer: Some(issuer.clone()),
            tenant: None,
            client_id: client_id.clone(),
            client_secret: config.oidc_client_secret.clone(),
            scopes: vec![],
            groups_claim: None,
        }));
    }
    // shortcuts
    if let (Some(id), Some(secret)) = (env("WIKICRAFT_GOOGLE_CLIENT_ID"), env("WIKICRAFT_GOOGLE_CLIENT_SECRET")) {
        out.push(Provider::Oidc(OidcConfig {
            common: Common { id: "google".into(), allowed_domains: split_list(env("WIKICRAFT_GOOGLE_ALLOWED_DOMAINS")), ..Default::default() },
            preset: Some("google".into()),
            issuer: None,
            tenant: None,
            client_id: id,
            client_secret: Some(secret),
            scopes: vec![],
            groups_claim: None,
        }));
    }
    if let (Some(id), Some(secret)) = (env("WIKICRAFT_MICROSOFT_CLIENT_ID"), env("WIKICRAFT_MICROSOFT_CLIENT_SECRET")) {
        out.push(Provider::Oidc(OidcConfig {
            common: Common { id: "microsoft".into(), allowed_domains: split_list(env("WIKICRAFT_MICROSOFT_ALLOWED_DOMAINS")), ..Default::default() },
            preset: Some("microsoft".into()),
            issuer: None,
            tenant: env("WIKICRAFT_MICROSOFT_TENANT"),
            client_id: id,
            client_secret: Some(secret),
            scopes: vec![],
            groups_claim: Some("groups".into()),
        }));
    }
    let mut seen = HashSet::new();
    for p in &out {
        let id = p.id();
        if id.is_empty() || !id.chars().all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_') {
            anyhow::bail!("auth provider id {id:?} must be non-empty and use letters, digits, '-' or '_'");
        }
        if !seen.insert(id.to_string()) {
            anyhow::bail!("duplicate auth provider id {id:?}");
        }
        if let Provider::Oidc(c) = p {
            if c.issuer.is_none() && c.preset.is_none() {
                anyhow::bail!("OIDC provider {id:?} needs an issuer or a preset");
            }
            if c.preset.as_deref() == Some("microsoft") && matches!(c.tenant.as_deref(), None | Some("common" | "organizations" | "consumers")) {
                anyhow::bail!("Microsoft provider {id:?} needs a tenant id (multi-tenant 'common' isn't supported)");
            }
        }
        #[cfg(not(feature = "saml"))]
        if matches!(p, Provider::Saml(_)) {
            anyhow::bail!("SAML provider {id:?} configured but this build has no SAML support (cargo feature `saml`)");
        }
    }
    Ok(out)
}

pub fn find<'a>(state: &'a AppState, id: &str) -> Option<&'a Provider> {
    state.providers.iter().find(|p| p.id() == id)
}

/// `GET /api/auth/providers` — what the login page shows.
pub async fn list(State(state): State<AppState>) -> Json<Value> {
    let providers: Vec<Value> = state
        .providers
        .iter()
        .map(|p| {
            let redirect = matches!(p, Provider::Oidc(_) | Provider::Github(_) | Provider::Saml(_));
            let preset = match p {
                Provider::Oidc(c) => c.preset.as_deref(),
                _ => None,
            };
            json!({ "id": p.id(), "name": p.display_name(), "type": p.kind(), "preset": preset, "loginUrl": redirect.then(|| format!("/api/auth/sso/{}/login", p.id())) })
        })
        .collect();
    Json(json!({
        "passwordLogin": !state.config.disable_password_login,
        // signing in with a passkey alone (as the second step they work either way)
        "passkeyLogin": !state.config.disable_password_login,
        "passwordReset": !state.config.disable_password_login && state.mailer.enabled(),
        "providers": providers,
    }))
}

/// An identity asserted by an external provider.
#[derive(Debug, Clone)]
pub struct External {
    /// Stable key of the identity provider (issuer, `ldap:<id>`, `github`, SAML entity id).
    pub provider_key: String,
    pub subject: String,
    pub email: Option<String>,
    /// `Some(false)` when the provider says the e-mail is unverified.
    pub email_verified: Option<bool>,
    pub name: Option<String>,
    pub groups: Vec<String>,
}

#[derive(Debug, thiserror::Error)]
pub enum SignInError {
    #[error("{0}")]
    Denied(String),
    #[error(transparent)]
    Other(#[from] anyhow::Error),
}

impl From<sqlx::Error> for SignInError {
    fn from(e: sqlx::Error) -> Self {
        SignInError::Other(e.into())
    }
}

/// Maps an external identity to a WikiCraft user (link, claim or create) and syncs groups.
pub async fn sign_in(state: &AppState, provider: &Provider, ext: External) -> Result<Uuid, SignInError> {
    let common = provider.common();
    let email = ext.email.as_deref().map(|e| e.trim().to_lowercase()).filter(|e| e.contains('@'));
    // unverified addresses are only used when the provider is trusted for them (fail closed)
    let trusted = provider.email_trusted(ext.email_verified);
    if !common.allowed_domains.is_empty() {
        let domain = email.as_deref().and_then(|e| e.rsplit('@').next()).unwrap_or("");
        let ok = trusted && common.allowed_domains.iter().any(|d| d.eq_ignore_ascii_case(domain));
        if !ok {
            return Err(SignInError::Denied(format!("{} accounts from this e-mail domain are not allowed", provider.display_name())));
        }
    }
    let ws: Uuid = sqlx::query_scalar("SELECT id FROM workspaces ORDER BY created_at LIMIT 1").fetch_one(&state.db).await?;
    let name = ext.name.clone().filter(|n| !n.trim().is_empty()).or_else(|| email.clone()).unwrap_or_else(|| "SSO user".into());
    let linked: Option<(Uuid, String)> =
        sqlx::query_as("SELECT u.id, u.status FROM identities i JOIN users u ON u.id = i.user_id WHERE i.provider = $1 AND i.subject = $2")
            .bind(&ext.provider_key)
            .bind(&ext.subject)
            .fetch_optional(&state.db)
            .await?;
    let user_id = match linked {
        Some((_, status)) if status == "deactivated" => return Err(SignInError::Denied("your account is deactivated".into())),
        Some((id, _)) => id,
        None => {
            let by_email: Option<(Uuid, String, String)> = match &email {
                Some(e) if trusted => {
                    sqlx::query_as("SELECT id, status, role FROM users WHERE workspace_id = $1 AND lower(email) = $2")
                        .bind(ws)
                        .bind(e)
                        .fetch_optional(&state.db)
                        .await?
                }
                _ => None,
            };
            let id = match by_email {
                Some((_, status, _)) if status == "deactivated" => return Err(SignInError::Denied("your account is deactivated".into())),
                // admin accounts are only linked on an explicit trust signal
                Some((_, _, role)) if (role == "owner" || role == "admin") && ext.email_verified != Some(true) && common.trust_email != Some(true) => {
                    return Err(SignInError::Denied(format!(
                        "{} matches an administrator account; sign in with your password and ask for the identity to be linked (or set trustEmail for this provider)",
                        email.as_deref().unwrap_or("this e-mail")
                    )));
                }
                Some((id, status, _)) => {
                    if status == "placeholder" || status == "invited" {
                        // imported Confluence author claims their content / invitation accepted via SSO
                        sqlx::query("UPDATE users SET status = 'active', name = $2 WHERE id = $1").bind(id).bind(&name).execute(&state.db).await?;
                    }
                    id
                }
                None => {
                    if !provider.auto_create() {
                        return Err(SignInError::Denied(format!(
                            "no WikiCraft account for {}; ask an administrator to invite you",
                            email.as_deref().unwrap_or("this identity")
                        )));
                    }
                    let id = new_id();
                    let role = match common.default_role.as_deref() {
                        Some("guest") => "guest",
                        _ => "member",
                    };
                    sqlx::query("INSERT INTO users (id, workspace_id, email, name, role, status) VALUES ($1, $2, $3, $4, $5, 'active')")
                        .bind(id)
                        .bind(ws)
                        .bind(email.as_deref().filter(|_| trusted))
                        .bind(&name)
                        .bind(role)
                        .execute(&state.db)
                        .await?;
                    audit::log(state, ws, Some(id), "user.sso_provisioned", Some("user"), Some(id), json!({"provider": provider.id()})).await;
                    id
                }
            };
            sqlx::query("INSERT INTO identities (provider, subject, user_id, email) VALUES ($1, $2, $3, $4) ON CONFLICT DO NOTHING")
                .bind(&ext.provider_key)
                .bind(&ext.subject)
                .bind(id)
                .bind(&email)
                .execute(&state.db)
                .await?;
            id
        }
    };
    sqlx::query("UPDATE identities SET last_login = now() WHERE provider = $1 AND subject = $2")
        .bind(&ext.provider_key)
        .bind(&ext.subject)
        .execute(&state.db)
        .await?;
    if common.sync_groups {
        sync_groups(state, ws, provider.id(), user_id, &ext.groups).await?;
    }
    audit::log(state, ws, Some(user_id), "auth.sso_login", None, None, json!({"provider": provider.id()})).await;
    Ok(user_id)
}

/// Sets the user's membership in the groups managed by this provider to exactly `groups`.
async fn sync_groups(state: &AppState, ws: Uuid, provider_id: &str, user: Uuid, groups: &[String]) -> sqlx::Result<()> {
    let prefix = format!("sso:{provider_id}:");
    let mut wanted: Vec<Uuid> = Vec::new();
    for g in groups.iter().map(|g| g.trim()).filter(|g| !g.is_empty() && g.chars().count() <= 200).take(500) {
        let ext_ref = format!("{prefix}{g}");
        // only groups this provider created are reused; an existing local group (or another
        // provider's) with the same name is never taken over — ours gets a suffix instead
        let mut id: Option<Uuid> =
            sqlx::query_scalar("SELECT id FROM groups WHERE workspace_id = $1 AND external_ref = $2").bind(ws).bind(&ext_ref).fetch_optional(&state.db).await?;
        for name in [g.to_string(), format!("{g} ({provider_id})")] {
            if id.is_some() {
                break;
            }
            id = sqlx::query_scalar(
                "INSERT INTO groups (id, workspace_id, name, external_ref) VALUES ($1, $2, $3, $4) ON CONFLICT (workspace_id, lower(name)) DO NOTHING RETURNING id",
            )
            .bind(new_id())
            .bind(ws)
            .bind(&name)
            .bind(&ext_ref)
            .fetch_optional(&state.db)
            .await?;
        }
        if let Some(id) = id {
            wanted.push(id);
        }
    }
    sqlx::query(
        "DELETE FROM group_members m USING groups g WHERE m.group_id = g.id AND m.user_id = $1 AND g.workspace_id = $2 AND starts_with(g.external_ref, $3) AND NOT (g.id = ANY($4))",
    )
    .bind(user)
    .bind(ws)
    .bind(&prefix)
    .bind(&wanted)
    .execute(&state.db)
    .await?;
    for g in wanted {
        sqlx::query("INSERT INTO group_members (group_id, user_id) VALUES ($1, $2) ON CONFLICT DO NOTHING").bind(g).bind(user).execute(&state.db).await?;
    }
    Ok(())
}

/// Cookie holding a random value that ties a pending login to the browser that started it
/// (login-CSRF protection: an attacker can't complete their own login in a victim's browser).
pub const BROWSER_COOKIE: &str = "wc_sso";

/// `SameSite=None` is needed when the IdP returns with a cross-site POST (SAML); browsers only
/// accept that together with `Secure`.
pub fn browser_cookie(value: &str, cross_site_post: bool, secure: bool) -> String {
    let same_site = if cross_site_post { "None" } else { "Lax" };
    format!("{BROWSER_COOKIE}={value}; Path=/api/auth; HttpOnly; SameSite={same_site}; Max-Age=600{}", if secure { "; Secure" } else { "" })
}

/// Cookie carrying a pending two-factor challenge from an SSO login or password reset to the
/// login page's code step (keeps the token out of URLs).
pub const MFA_COOKIE: &str = "wc_mfa";

/// After a successful sign-in: `(Set-Cookie, redirect)` for a session — or, when the person
/// has two-factor authentication on, for a challenge completed on `/login?mfa=1`.
pub async fn session_or_challenge(state: &AppState, user: Uuid, headers: &axum::http::HeaderMap, next: &str) -> Result<(String, String), SignInError> {
    if crate::routes::security::required(state, user).await.map_err(|e| anyhow::anyhow!("{e}"))? {
        let token = crate::routes::security::challenge(state, user, Some(next)).await.map_err(|e| anyhow::anyhow!("{e}"))?;
        return Ok((mfa_cookie(state, &token, 600), format!("/login?mfa=1&next={}", urlencode(next))));
    }
    let cookie = crate::auth::create_session(state, user, headers).await.map_err(|e| anyhow::anyhow!("{e}"))?;
    Ok((cookie, next.to_string()))
}

pub fn mfa_cookie(state: &AppState, token: &str, max_age: i64) -> String {
    format!("{MFA_COOKIE}={token}; Path=/api/auth; HttpOnly; SameSite=Lax; Max-Age={max_age}{}", if state.config.secure_cookies { "; Secure" } else { "" })
}

/// Stores short-lived login state (shared by all replicas); returns the opaque state key.
/// With `browser`, completing the login requires the matching [`BROWSER_COOKIE`].
pub async fn put_state(state: &AppState, data: &Value, browser: Option<&str>) -> anyhow::Result<String> {
    let key = crate::auth::random_token();
    let mut data = data.clone();
    if let (Some(b), Some(obj)) = (browser, data.as_object_mut()) {
        obj.insert("_browser".into(), Value::String(hex::encode(crate::auth::token_hash(b))));
    }
    sqlx::query("DELETE FROM auth_states WHERE expires_at < now()").execute(&state.db).await?;
    sqlx::query("INSERT INTO auth_states (state, data, expires_at) VALUES ($1, $2, now() + interval '10 minutes')")
        .bind(hex::encode(crate::auth::token_hash(&key)))
        .bind(&data)
        .execute(&state.db)
        .await?;
    Ok(key)
}

/// Consumes login state (one-time use), checking the browser binding.
pub async fn take_state(state: &AppState, key: &str, headers: &axum::http::HeaderMap) -> anyhow::Result<Value> {
    let data: Option<Value> = sqlx::query_scalar("DELETE FROM auth_states WHERE state = $1 AND expires_at > now() RETURNING data")
        .bind(hex::encode(crate::auth::token_hash(key)))
        .fetch_optional(&state.db)
        .await?;
    let data = data.ok_or_else(|| anyhow::anyhow!("login expired, please try again"))?;
    if let Some(expected) = data.get("_browser").and_then(Value::as_str) {
        let actual = crate::auth::cookie_value(headers, BROWSER_COOKIE).map(|v| hex::encode(crate::auth::token_hash(v)));
        anyhow::ensure!(actual.as_deref() == Some(expected), "this sign-in was started in a different browser, please try again");
    }
    Ok(data)
}

/// Same-site paths only: no `//` or `/\` prefix, and no whitespace, control characters or
/// backslashes anywhere (browsers strip tabs, turning `/\t/evil.com` into `//evil.com`).
pub fn safe_next(next: Option<String>) -> String {
    next.filter(|n| n.starts_with('/') && !n.starts_with("//") && !n.chars().any(|c| c.is_whitespace() || c.is_control() || c == '\\'))
        .unwrap_or_else(|| "/".into())
}

pub fn urlencode(s: &str) -> String {
    s.bytes().map(|b| if b.is_ascii_alphanumeric() { (b as char).to_string() } else { format!("%{b:02X}") }).collect()
}

/// Redirect to the login page with an error message.
pub fn login_error(msg: &str) -> axum::response::Response {
    use axum::response::IntoResponse;
    axum::response::Redirect::to(&format!("/login?error={}", urlencode(msg))).into_response()
}

#[cfg(test)]
mod next_tests {
    #[test]
    fn redirects_stay_on_site() {
        for bad in ["//evil.com", "/\\evil.com", "/\t/evil.com", "/\n/evil.com", "https://evil.com", "/ok\\..\\x"] {
            assert_eq!(super::safe_next(Some(bad.into())), "/", "{bad:?}");
        }
        assert_eq!(super::safe_next(Some("/pages/1?x=1#y".into())), "/pages/1?x=1#y");
    }
}
