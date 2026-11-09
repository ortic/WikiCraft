//! Two-factor sign-in (TOTP + recovery codes) and session management.

mod common;

use serde_json::{Value, json};
use wikicraft::mfa;

use common::client;

fn code(secret: &str) -> String {
    mfa::code_at(&mfa::base32_decode(secret).unwrap(), mfa::current_step())
}

#[tokio::test(flavor = "multi_thread")]
async fn totp_login_recovery_codes_sessions_and_admin_reset() {
    let app = common::spawn().await;
    let owner = app.setup().await;
    let (mia, mia_id) = app.user(&owner, "mia@acme.test", "member").await;

    // enrol: setup → confirm with a code → recovery codes
    let (_, setup) = app.post(&mia, "/me/mfa/setup", json!({})).await;
    let secret = setup["secret"].as_str().unwrap().to_string();
    assert!(setup["uri"].as_str().unwrap().starts_with("otpauth://totp/WikiCraft:mia@acme.test?secret="));
    assert!(setup["qrSvg"].as_str().unwrap().contains("<svg"));
    let (st, _) = app.post(&mia, "/me/mfa/enable", json!({"code": "000000"})).await;
    assert_eq!(st, 400);
    let (st, en) = app.post(&mia, "/me/mfa/enable", json!({"code": code(&secret)})).await;
    assert_eq!(st, 200, "{en}");
    let recovery: Vec<String> = en["recoveryCodes"].as_array().unwrap().iter().map(|c| c.as_str().unwrap().to_string()).collect();
    assert_eq!(recovery.len(), 10);
    let (_, s) = app.get(&mia, "/me/mfa").await;
    assert_eq!(s["enabled"], true);
    assert_eq!(s["recoveryCodesLeft"], 10);

    // the password alone no longer signs in
    let login = |c: reqwest::Client| {
        let app = &app;
        async move {
            let (st, body) = app.post(&c, "/auth/login", json!({"email": "mia@acme.test", "password": "password123"})).await;
            assert_eq!(st, 200, "{body}");
            assert_eq!(body["mfaRequired"], true);
            let (_, me) = app.get(&c, "/me").await;
            assert!(me["user"].is_null(), "no session before the second factor");
            body["mfaToken"].as_str().unwrap().to_string()
        }
    };
    let c1 = client();
    let token = login(c1.clone()).await;
    let (st, _) = app.post(&c1, "/auth/mfa", json!({"mfaToken": token, "code": "123456x"})).await;
    assert_eq!(st, 400);
    // the code used to enable can't be replayed
    let (st, _) = app.post(&c1, "/auth/mfa", json!({"mfaToken": token, "code": code(&secret)})).await;
    assert_eq!(st, 400, "a TOTP code works once");
    // recovery code (any case, dashes optional), single use
    let rc = recovery[0].to_uppercase().replace('-', "");
    let (st, _) = app.post(&c1, "/auth/mfa", json!({"mfaToken": token, "code": rc})).await;
    assert_eq!(st, 200);
    let (_, me) = app.get(&c1, "/me").await;
    assert_eq!(me["user"]["id"], mia_id.as_str());
    let c2 = client();
    let token2 = login(c2.clone()).await;
    let (st, _) = app.post(&c2, "/auth/mfa", json!({"mfaToken": token2, "code": recovery[0]})).await;
    assert_eq!(st, 400, "recovery codes are single-use");
    let (st, _) = app.post(&c2, "/auth/mfa", json!({"mfaToken": token2, "code": recovery[1]})).await;
    assert_eq!(st, 200);

    // five wrong codes end the attempt
    let c3 = client();
    let token3 = login(c3.clone()).await;
    for _ in 0..4 {
        let (st, _) = app.post(&c3, "/auth/mfa", json!({"mfaToken": token3, "code": "999999"})).await;
        assert_eq!(st, 400);
    }
    let (_, body) = app.post(&c3, "/auth/mfa", json!({"mfaToken": token3, "code": "999998"})).await;
    assert!(body["message"].as_str().unwrap().contains("too many"), "{body}");
    let (_, body) = app.post(&c3, "/auth/mfa", json!({"mfaToken": token3, "code": recovery[2]})).await;
    assert!(body["message"].as_str().unwrap().contains("expired"), "{body}");

    // sessions: list, current marker, sign out everywhere else
    let (_, list) = app.get(&c2, "/me/sessions").await;
    let list = list.as_array().unwrap();
    assert!(list.len() >= 3, "{list:?}");
    assert_eq!(list.iter().filter(|s| s["current"] == true).count(), 1);
    assert_eq!(list[0]["current"], true);
    let (_, r) = app.post(&c2, "/me/sessions/revoke-others", json!({})).await;
    assert!(r["revoked"].as_u64().unwrap() >= 2);
    let (st, _) = app.get(&c1, "/recent").await;
    assert_eq!(st, 401, "other sessions are signed out");
    let (_, list) = app.get(&c2, "/me/sessions").await;
    assert_eq!(list.as_array().unwrap().len(), 1);

    // admins reset a lost second factor (and sign the person out)
    let _ = &mia; // signed out above by "revoke others"
    let (st, _) = app.post(&c2, &format!("/users/{mia_id}/reset-mfa"), json!({})).await;
    assert_eq!(st, 403, "members can't");
    let (st, _) = app.post(&owner, &format!("/users/{mia_id}/reset-mfa"), json!({})).await;
    assert_eq!(st, 204);
    let (st, _) = app.get(&c2, "/recent").await;
    assert_eq!(st, 401);
    let c4 = client();
    let (_, body) = app.post(&c4, "/auth/login", json!({"email": "mia@acme.test", "password": "password123"})).await;
    assert_eq!(body["ok"], true, "{body}");

    // enable again and disable with the password
    let (_, setup) = app.post(&c4, "/me/mfa/setup", json!({})).await;
    let secret = setup["secret"].as_str().unwrap().to_string();
    app.post(&c4, "/me/mfa/enable", json!({"code": code(&secret)})).await;
    let (st, _) = app.post(&c4, "/me/mfa/disable", json!({"password": "wrong"})).await;
    assert_eq!(st, 400);
    let (st, _) = app.post(&c4, "/me/mfa/disable", json!({"password": "password123"})).await;
    assert_eq!(st, 204);
    let (_, s): (u16, Value) = app.get(&c4, "/me/mfa").await;
    assert_eq!(s["enabled"], false);
}

#[tokio::test(flavor = "multi_thread")]
async fn idle_sessions_expire() {
    let app = common::spawn_with(|c| c.session_idle_minutes = 30).await;
    let owner = app.setup().await;
    assert_eq!(app.get(&owner, "/recent").await.0, 200);
    sqlx::query("UPDATE sessions SET last_seen_at = now() - interval '31 minutes'").execute(&app.state.db).await.unwrap();
    assert_eq!(app.get(&owner, "/recent").await.0, 401, "idle session signed out");
}

async fn enrol(app: &common::TestApp, c: &reqwest::Client) -> String {
    let (_, setup) = app.post(c, "/me/mfa/setup", json!({})).await;
    let secret = setup["secret"].as_str().unwrap().to_string();
    let (st, _) = app.post(c, "/me/mfa/enable", json!({"code": code(&secret)})).await;
    assert_eq!(st, 200);
    secret
}

#[tokio::test(flavor = "multi_thread")]
async fn codes_cant_be_brute_forced_across_new_logins() {
    let app = common::spawn().await;
    let owner = app.setup().await;
    let (mo, _) = app.user(&owner, "mo@acme.test", "member").await;
    let secret = enrol(&app, &mo).await;
    // a correct password gives a fresh challenge each time, but wrong codes add up per person
    for _ in 0..10 {
        let c = client();
        let (_, b) = app.post(&c, "/auth/login", json!({"email": "mo@acme.test", "password": "password123"})).await;
        let (st, _) = app.post(&c, "/auth/mfa", json!({"mfaToken": b["mfaToken"], "code": "000001"})).await;
        assert_eq!(st, 400);
    }
    let c = client();
    let (_, b) = app.post(&c, "/auth/login", json!({"email": "mo@acme.test", "password": "password123"})).await;
    let next = mfa::code_at(&mfa::base32_decode(&secret).unwrap(), mfa::current_step() + 1);
    let (st, body) = app.post(&c, "/auth/mfa", json!({"mfaToken": b["mfaToken"], "code": next})).await;
    assert_eq!(st, 400, "locked even with the right code");
    assert!(body["message"].as_str().unwrap().contains("too many"), "{body}");
}

#[tokio::test(flavor = "multi_thread")]
async fn password_reset_still_needs_the_second_factor() {
    let app = common::spawn().await;
    let owner = app.setup().await;
    let (rae, rae_id) = app.user(&owner, "rae@acme.test", "member").await;
    let secret = enrol(&app, &rae).await;
    // an API token the "intruder" made is revoked by the reset
    let (_, tok) = app.post(&rae, "/me/tokens", json!({"name": "intruder"})).await;
    let token = tok["token"].as_str().unwrap().to_string();
    let reset = "reset-token-for-test-0123456789abcdef";
    let hash = {
        use sha2::Digest;
        hex::encode(sha2::Sha256::digest(reset.as_bytes()))
    };
    sqlx::query("INSERT INTO user_tokens (token_hash, user_id, purpose, expires_at) VALUES ($1, $2::uuid, 'password_reset', now() + interval '1 hour')")
        .bind(&hash)
        .bind(&rae_id)
        .execute(&app.state.db)
        .await
        .unwrap();
    let c = client();
    let (st, body) = app.post(&c, "/auth/reset", json!({"token": reset, "password": "brand-new-pass"})).await;
    assert_eq!(st, 200);
    assert_eq!(body["mfaRequired"], true);
    assert!(app.get(&c, "/me").await.1["user"].is_null(), "no session from mailbox access alone");
    let r = reqwest::Client::new().get(app.url("/recent")).bearer_auth(&token).send().await.unwrap();
    assert_eq!(r.status(), 401, "API tokens revoked");
    // the challenge travels in a cookie; the code completes the sign-in
    let next = mfa::code_at(&mfa::base32_decode(&secret).unwrap(), mfa::current_step() + 1);
    let (st, body) = app.post(&c, "/auth/mfa", json!({"code": next})).await;
    assert_eq!(st, 200, "{body}");
    assert_eq!(app.get(&c, "/me").await.1["user"]["id"], rae_id.as_str());
}

#[tokio::test(flavor = "multi_thread")]
async fn an_active_factor_cant_be_replaced_with_just_a_session() {
    let app = common::spawn().await;
    let owner = app.setup().await;
    let (eve, _) = app.user(&owner, "eve@acme.test", "member").await;
    enrol(&app, &eve).await;
    let (st, body) = app.post(&eve, "/me/mfa/setup", json!({})).await;
    assert_eq!(st, 400, "{body}");
}
