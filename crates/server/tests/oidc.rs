//! SSO against a mock OpenID Connect provider (docker: ghcr.io/navikt/mock-oauth2-server on
//! 127.0.0.1:8099, non-interactive). Skipped when the provider isn't running.

mod common;

use serde_json::json;

const ISSUER: &str = "http://127.0.0.1:8099/default";

#[tokio::test(flavor = "multi_thread")]
async fn sso_login_provisions_and_links_accounts() {
    if reqwest::get(format!("{ISSUER}/.well-known/openid-configuration")).await.is_err() {
        eprintln!("skipping: mock OIDC provider not running");
        return;
    }
    let app = common::spawn_with(|c| {
        c.oidc_issuer = Some(ISSUER.into());
        c.oidc_client_id = Some("wikicraft".into());
        c.oidc_client_secret = Some("secret".into());
    })
    .await;
    let owner = app.setup().await;
    let (_, info) = app.get(&owner, "/auth/oidc").await;
    assert_eq!(info["enabled"], true);
    // an imported Confluence author with the same e-mail gets claimed by the SSO login
    let ws: uuid::Uuid = sqlx::query_scalar("SELECT id FROM workspaces").fetch_one(&app.state.db).await.unwrap();
    let placeholder = uuid::Uuid::now_v7();
    sqlx::query("INSERT INTO users (id, workspace_id, email, name, role, status, external_ref) VALUES ($1, $2, 'sso.user@acme.test', 'Imported Author', 'member', 'placeholder', 'confluence:abc')")
        .bind(placeholder)
        .bind(ws)
        .execute(&app.state.db)
        .await
        .unwrap();

    let browser = reqwest::Client::builder().cookie_store(true).build().unwrap();
    let r = browser.get(app.url("/auth/oidc/login?next=/spaces")).send().await.unwrap();
    assert!(r.url().path() == "/spaces", "ended at {}", r.url());
    let (_, me) = app.get(&browser, "/me").await;
    assert_eq!(me["user"]["email"], "sso.user@acme.test");
    assert_eq!(me["user"]["id"], placeholder.to_string(), "placeholder account claimed");
    assert_eq!(me["user"]["status"], "active");
    assert_eq!(me["user"]["name"], "Sso User");

    // second login maps to the same account via (issuer, subject)
    let browser2 = reqwest::Client::builder().cookie_store(true).build().unwrap();
    browser2.get(app.url("/auth/oidc/login")).send().await.unwrap();
    let (_, me2) = app.get(&browser2, "/me").await;
    assert_eq!(me2["user"]["id"], placeholder.to_string());
    let identities: i64 = sqlx::query_scalar("SELECT count(*) FROM identities").fetch_one(&app.state.db).await.unwrap();
    assert_eq!(identities, 1);

    // forged state is rejected
    let r = browser2.get(app.url("/auth/oidc/callback?code=x&state=forged")).send().await.unwrap();
    assert!(r.url().as_str().contains("/login?error="));

    // deactivated users can't sign in via SSO
    let (st, _) = app.patch(&owner, &format!("/users/{placeholder}"), json!({"status": "deactivated"})).await;
    assert_eq!(st, 204);
    let browser3 = reqwest::Client::builder().cookie_store(true).build().unwrap();
    let r = browser3.get(app.url("/auth/oidc/login")).send().await.unwrap();
    assert!(r.url().as_str().contains("deactivated"), "{}", r.url());
}
