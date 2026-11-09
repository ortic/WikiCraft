//! SAML 2.0 against a real IdP (SimpleSAMLphp). Needs `--features saml` and the test IdP:
//!
//! ```sh
//! docker run -d --name wc-saml-idp -p 127.0.0.1:8180:8080 \
//!   -e SIMPLESAMLPHP_SP_ENTITY_ID=http://localhost:8790/api/auth/sso/okta/metadata \
//!   -e SIMPLESAMLPHP_SP_ASSERTION_CONSUMER_SERVICE=http://localhost:8790/api/auth/sso/okta/acs \
//!   kristophjunge/test-saml-idp
//! # the image signs with RSA-SHA1 by default, which WikiCraft refuses; switch it to SHA-256:
//! docker exec wc-saml-idp sed -i "s|//'signature.algorithm'|'signature.algorithm'|" \
//!   /var/www/simplesamlphp/metadata/saml20-idp-hosted.php
//! ```
//!
//! Skipped when the IdP isn't running.
#![cfg(feature = "saml")]

mod common;

use serde_json::json;

const METADATA: &str = "http://localhost:8180/simplesaml/saml2/idp/metadata.php";

/// Value of `name="…" value="…"` in an HTML form.
fn input(html: &str, name: &str) -> Option<String> {
    let at = html.find(&format!("name=\"{name}\""))?;
    let rest = &html[at..];
    let v = rest.find("value=\"")? + 7;
    let end = rest[v..].find('"')?;
    Some(rest[v..v + end].replace("&quot;", "\"").replace("&amp;", "&").replace("&#x3D;", "=").replace("&#x2B;", "+").replace("&#x2F;", "/"))
}

fn form_action(html: &str) -> Option<String> {
    let at = html.find("action=\"")? + 8;
    Some(html[at..at + html[at..].find('"')?].replace("&amp;", "&"))
}

/// Runs the browser part of the flow; returns the (SAMLResponse, RelayState) the IdP posts back.
async fn idp_login(app: &common::TestApp, user: &str, password: &str) -> (String, String) {
    let idp = reqwest::Client::builder().cookie_store(true).build().unwrap();
    let r = reqwest::Client::builder().redirect(reqwest::redirect::Policy::none()).build().unwrap().get(app.url("/auth/sso/okta/login")).send().await.unwrap();
    assert_eq!(r.status(), 303);
    let to_idp = r.headers()["location"].to_str().unwrap().to_string();
    assert!(to_idp.starts_with("http://localhost:8180/"), "{to_idp}");
    let login_page = idp.get(&to_idp).send().await.unwrap();
    let login_url = login_page.url().clone();
    let html = login_page.text().await.unwrap();
    let auth_state = input(&html, "AuthState").expect("IdP login form");
    let action = login_url.join(&form_action(&html).unwrap_or_default()).unwrap();
    let html =
        idp.post(action).form(&[("username", user), ("password", password), ("AuthState", auth_state.as_str())]).send().await.unwrap().text().await.unwrap();
    (input(&html, "SAMLResponse").expect("IdP posts a SAMLResponse"), input(&html, "RelayState").expect("RelayState"))
}

#[tokio::test(flavor = "multi_thread")]
async fn saml_login_against_simplesamlphp() {
    if reqwest::get(METADATA).await.is_err() {
        eprintln!("skipping: SAML test IdP not running");
        return;
    }
    let providers =
        json!([{"type": "saml", "id": "okta", "name": "Test IdP", "idpMetadataUrl": METADATA, "groupsAttribute": "eduPersonAffiliation", "syncGroups": true}]);
    // the IdP knows the SP as localhost:8790; requests still go to the test server's real port
    let app = common::spawn_with(|c| {
        c.base_url = "http://localhost:8790".into();
        c.auth_providers = Some(providers.to_string());
    })
    .await;
    app.setup().await;

    let md = reqwest::get(app.url("/auth/sso/okta/metadata")).await.unwrap().text().await.unwrap();
    assert!(md.contains(r#"entityID="http://localhost:8790/api/auth/sso/okta/metadata""#) && md.contains("/api/auth/sso/okta/acs"), "{md}");

    let acs =
        |b: &reqwest::Client, resp: &str, relay: &str| b.post(app.url("/auth/sso/okta/acs")).form(&[("SAMLResponse", resp), ("RelayState", relay)]).send();
    let browser = reqwest::Client::builder().cookie_store(true).redirect(reqwest::redirect::Policy::none()).build().unwrap();

    // happy path: account created, groups synced
    let (resp, relay) = idp_login(&app, "user1", "user1pass").await;
    let r = acs(&browser, &resp, &relay).await.unwrap();
    assert_eq!(r.status(), 303);
    assert_eq!(r.headers()["location"], "/", "signed in");
    let (_, me) = app.get(&browser, "/me").await;
    assert_eq!(me["user"]["email"], "user1@example.com");
    let groups: Vec<String> = sqlx::query_scalar(
        "SELECT g.name FROM group_members m JOIN groups g ON g.id = m.group_id JOIN users u ON u.id = m.user_id WHERE u.email = 'user1@example.com'",
    )
    .fetch_all(&app.state.db)
    .await
    .unwrap();
    assert_eq!(groups, vec!["group1".to_string()]);

    // replaying the same response is refused (single-use state)
    let r = acs(&reqwest::Client::new(), &resp, &relay).await.unwrap();
    assert!(r.url().as_str().contains("/login?error=") || r.headers().get("location").is_some_and(|l| l.to_str().unwrap().contains("error")));

    // a tampered assertion fails signature verification
    let (resp, relay) = idp_login(&app, "user2", "user2pass").await;
    use base64::Engine;
    let xml = String::from_utf8(base64::engine::general_purpose::STANDARD.decode(&resp).unwrap()).unwrap().replace("user2@example.com", "owner@acme.test");
    let forged = base64::engine::general_purpose::STANDARD.encode(xml);
    let fresh = reqwest::Client::builder().cookie_store(true).redirect(reqwest::redirect::Policy::none()).build().unwrap();
    let r = acs(&fresh, &forged, &relay).await.unwrap();
    let loc = r.headers()["location"].to_str().unwrap().to_string();
    assert!(loc.starts_with("/login?error=") && loc.contains("signed"), "{loc}");
    assert!(app.get(&fresh, "/me").await.1["user"].is_null(), "not signed in");

    // IdP-initiated responses (no RelayState) are refused
    let r = fresh.post(app.url("/auth/sso/okta/acs")).form(&[("SAMLResponse", resp.as_str())]).send().await.unwrap();
    let loc = r.headers()["location"].to_str().unwrap().to_string();
    assert!(loc.starts_with("/login?error=IdP"), "{loc}");
}
