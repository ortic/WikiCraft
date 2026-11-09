//! Multi-provider sign-in: several OIDC providers side by side (mock IdP on 127.0.0.1:8099) and
//! LDAP against a real directory (lldap on 127.0.0.1:3890 with user `dana` / `dana-secret-1` in
//! group `Engineering`, see WORKLOG.md). Each test is skipped when its server isn't running.

mod common;

use serde_json::{Value, json};

use common::client;

const ISSUER: &str = "http://127.0.0.1:8099/default";

fn browser() -> reqwest::Client {
    reqwest::Client::builder().cookie_store(true).build().unwrap()
}

#[tokio::test(flavor = "multi_thread")]
async fn several_oidc_providers_with_domain_rules() {
    if reqwest::get(format!("{ISSUER}/.well-known/openid-configuration")).await.is_err() {
        eprintln!("skipping: mock OIDC provider not running");
        return;
    }
    let providers = json!([
        {"type": "oidc", "id": "corp", "name": "Corp SSO", "issuer": ISSUER, "clientId": "wikicraft", "clientSecret": "secret", "allowedDomains": ["acme.test"]},
        {"type": "oidc", "id": "partner", "name": "Partner SSO", "issuer": ISSUER, "clientId": "wikicraft", "clientSecret": "secret", "allowedDomains": ["partner.test"]},
    ]);
    let app = common::spawn_with(|c| c.auth_providers = Some(providers.to_string())).await;
    app.setup().await;

    let (_, info) = app.get(&client(), "/auth/providers").await;
    let list = info["providers"].as_array().unwrap();
    assert_eq!(list.len(), 2);
    assert_eq!(list[0]["loginUrl"], "/api/auth/sso/corp/login");
    assert_eq!(info["passwordLogin"], true);

    // the mock IdP asserts sso.user@acme.test, which the partner provider doesn't accept
    let r = browser().get(app.url("/auth/sso/partner/login")).send().await.unwrap();
    assert!(r.url().as_str().contains("/login?error=") && r.url().as_str().contains("not%20allowed"), "{}", r.url());

    let b = browser();
    let r = b.get(app.url("/auth/sso/corp/login?next=/spaces")).send().await.unwrap();
    assert_eq!(r.url().path(), "/spaces");
    let (_, me) = app.get(&b, "/me").await;
    assert_eq!(me["user"]["email"], "sso.user@acme.test");

    // a login started at one provider can't be completed at another
    let manual = || reqwest::Client::builder().cookie_store(true).redirect(reqwest::redirect::Policy::none()).build().unwrap();
    let login_url = app.url("/auth/sso/corp/login");
    let start = |c: reqwest::Client| {
        let login_url = login_url.clone();
        async move {
            let r = c.get(login_url).send().await.unwrap();
            let location = r.headers()["location"].to_str().unwrap().to_string();
            reqwest::Url::parse(&location).unwrap().query_pairs().find(|(k, _)| k == "state").unwrap().1.to_string()
        }
    };
    let b = manual();
    let state = start(b.clone()).await;
    let r = b.get(app.url(&format!("/auth/sso/partner/callback?code=x&state={state}"))).send().await.unwrap();
    let loc = r.headers()["location"].to_str().unwrap().to_string();
    assert!(loc.contains("another%20provider"), "{loc}");

    // login CSRF: a login started in one browser can't be completed in another one
    let attacker = manual();
    let state = start(attacker.clone()).await;
    let victim = manual();
    let r = victim.get(app.url(&format!("/auth/sso/corp/callback?code=x&state={state}"))).send().await.unwrap();
    let loc = r.headers()["location"].to_str().unwrap().to_string();
    assert!(loc.contains("different%20browser"), "{loc}");

    // unknown providers
    let r = browser().get(app.url("/auth/sso/nope/login")).send().await.unwrap();
    assert!(r.url().as_str().contains("/login?error="));
}

fn ldap_providers() -> String {
    json!([{
        "type": "ldap",
        "id": "corp",
        "name": "ACME directory",
        "url": "ldap://127.0.0.1:3890",
        "bindDn": "uid=admin,ou=people,dc=acme,dc=test",
        "bindPassword": "adminpassword",
        "baseDn": "dc=acme,dc=test",
        "syncGroups": true,
    }])
    .to_string()
}

async fn ldap_running() -> bool {
    tokio::net::TcpStream::connect("127.0.0.1:3890").await.is_ok()
}

#[tokio::test(flavor = "multi_thread")]
async fn ldap_login_provisions_users_and_syncs_groups() {
    if !ldap_running().await {
        eprintln!("skipping: LDAP server not running");
        return;
    }
    let app = common::spawn_with(|c| c.auth_providers = Some(ldap_providers())).await;
    let owner = app.setup().await;
    let (_, info) = app.get(&client(), "/auth/providers").await;
    assert_eq!(info["providers"][0]["type"], "ldap");
    assert_eq!(info["providers"][0]["loginUrl"], Value::Null, "LDAP signs in through the password form");

    let dana = client();
    let (st, body) = app.post(&dana, "/auth/login", json!({"email": "dana", "password": "dana-secret-1"})).await;
    assert_eq!(st, 200, "{body}");
    let (_, me) = app.get(&dana, "/me").await;
    assert_eq!(me["user"]["email"], "dana@acme.test");
    assert_eq!(me["user"]["name"], "Dana Directory");
    assert_eq!(me["user"]["role"], "member");
    let dana_id = me["user"]["id"].as_str().unwrap().to_string();

    // directory groups are mirrored
    let members: Vec<String> =
        sqlx::query_scalar("SELECT g.name FROM group_members m JOIN groups g ON g.id = m.group_id WHERE m.user_id = $1::uuid ORDER BY 1")
            .bind(&dana_id)
            .fetch_all(&app.state.db)
            .await
            .unwrap();
    assert!(members.contains(&"Engineering".to_string()), "{members:?}");

    // e-mail works as login name too; the same account is used
    let again = client();
    let (st, _) = app.post(&again, "/auth/login", json!({"email": "dana@acme.test", "password": "dana-secret-1"})).await;
    assert_eq!(st, 200);
    let (_, me2) = app.get(&again, "/me").await;
    assert_eq!(me2["user"]["id"], dana_id.as_str());

    // wrong password, empty password, unknown user, filter injection
    for (user, pw) in [("dana", "wrong"), ("dana", ""), ("nobody", "x"), ("*", "dana-secret-1"), ("dana)(uid=*", "dana-secret-1")] {
        let (st, _) = app.post(&client(), "/auth/login", json!({"email": user, "password": pw})).await;
        assert_eq!(st, 400, "{user}/{pw}");
    }

    // local accounts still work alongside the directory
    let (st, _) = app.post(&client(), "/auth/login", json!({"email": "owner@acme.test", "password": "correct horse"})).await;
    assert_eq!(st, 200);

    // deactivated directory users are refused even with a valid directory password
    let (st, _) = app.patch(&owner, &format!("/users/{dana_id}"), json!({"status": "deactivated"})).await;
    assert_eq!(st, 204);
    let (st, body) = app.post(&client(), "/auth/login", json!({"email": "dana", "password": "dana-secret-1"})).await;
    assert_eq!(st, 400);
    assert!(body["message"].as_str().unwrap().contains("deactivated"), "{body}");
}

#[tokio::test(flavor = "multi_thread")]
async fn ldap_still_works_when_local_passwords_are_disabled() {
    if !ldap_running().await {
        eprintln!("skipping: LDAP server not running");
        return;
    }
    let app = common::spawn_with(|c| {
        c.auth_providers = Some(ldap_providers());
        c.disable_password_login = true;
    })
    .await;
    app.setup().await;
    let (st, _) = app.post(&client(), "/auth/login", json!({"email": "owner@acme.test", "password": "correct horse"})).await;
    assert_eq!(st, 400, "local password refused");
    let (st, _) = app.post(&client(), "/auth/login", json!({"email": "dana", "password": "dana-secret-1"})).await;
    assert_eq!(st, 200, "directory login allowed");
}

#[tokio::test(flavor = "multi_thread")]
async fn directory_logins_dont_take_over_admins_or_local_groups() {
    if !ldap_running().await {
        eprintln!("skipping: LDAP server not running");
        return;
    }
    let app = common::spawn_with(|c| c.auth_providers = Some(ldap_providers())).await;
    let owner = app.setup().await;
    // a local admin happens to have the directory user's e-mail
    let (_, admin_id) = app.user(&owner, "dana@acme.test", "admin").await;
    let (st, body) = app.post(&client(), "/auth/login", json!({"email": "dana", "password": "dana-secret-1"})).await;
    assert_eq!(st, 400);
    assert!(body["message"].as_str().unwrap().contains("administrator account"), "{body}");
    let linked: i64 = sqlx::query_scalar("SELECT count(*) FROM identities WHERE user_id = $1::uuid").bind(&admin_id).fetch_one(&app.state.db).await.unwrap();
    assert_eq!(linked, 0);

    // with an ordinary account: a local group named like a directory group isn't taken over
    let (_, _) = app.patch(&owner, &format!("/users/{admin_id}"), json!({"role": "member"})).await;
    let (_, g) = app.post(&owner, "/groups", json!({"name": "Engineering"})).await;
    let local = g["id"].as_str().unwrap().to_string();
    let dana = client();
    let (st, body) = app.post(&dana, "/auth/login", json!({"email": "dana", "password": "dana-secret-1"})).await;
    assert_eq!(st, 200, "{body}");
    let groups: Vec<(String, String)> =
        sqlx::query_as("SELECT g.id::text, g.name FROM group_members m JOIN groups g ON g.id = m.group_id WHERE m.user_id = $1::uuid ORDER BY g.name")
            .bind(&admin_id)
            .fetch_all(&app.state.db)
            .await
            .unwrap();
    assert!(!groups.iter().any(|(id, _)| *id == local), "local group untouched: {groups:?}");
    assert!(groups.iter().any(|(_, n)| n == "Engineering (corp)"), "{groups:?}");
}
