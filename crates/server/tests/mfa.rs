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
