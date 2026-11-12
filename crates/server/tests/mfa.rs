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

/// A workspace can require two-factor authentication: until it's set up, a session can only
/// enrol (or sign out); API tokens are separate credentials.
#[tokio::test(flavor = "multi_thread")]
async fn required_two_factor() {
    let app = common::spawn().await;
    let owner = app.setup().await;
    let (member, _) = app.user(&owner, "mem@acme.test", "member").await;
    let (_, t) = app.post(&member, "/me/tokens", json!({"name": "ci"})).await;
    let token = t["token"].as_str().unwrap().to_string();
    assert_eq!(app.put(&owner, "/admin/settings", json!({"requireTwoFactor": "sometimes"})).await.0, 400);
    let (st, s) = app.put(&owner, "/admin/settings", json!({"requireTwoFactor": "admins"})).await;
    assert_eq!((st, &s["requireTwoFactor"]), (200, &json!("admins")), "{s}");
    // from now on the owner can't change settings (e.g. switch it off again) before enrolling
    assert_eq!(app.put(&owner, "/admin/settings", json!({"requireTwoFactor": "off"})).await.0, 403);

    // admins: blocked until enrolled; members aren't affected yet
    let (st, err) = app.get(&owner, "/spaces").await;
    assert_eq!(st, 403);
    assert!(err["message"].as_str().unwrap().contains("two-factor"));
    let (_, me) = app.get(&owner, "/me").await;
    assert_eq!(me["mustSetUpTwoFactor"], true);
    assert_eq!(app.get(&member, "/spaces").await.0, 200);
    enrol(&app, &owner).await;
    assert_eq!(app.get(&owner, "/spaces").await.0, 200);
    let (_, me) = app.get(&owner, "/me").await;
    assert_eq!(me["mustSetUpTwoFactor"], false);

    // everyone
    let (st, _) = app.put(&owner, "/admin/settings", json!({"requireTwoFactor": "everyone"})).await;
    assert_eq!(st, 200);
    assert_eq!(app.get(&member, "/spaces").await.0, 403);
    assert_eq!(app.post(&member, "/pages", json!({"spaceKey": "X", "title": "y"})).await.0, 403);
    let r = reqwest::Client::new().get(app.url("/spaces")).bearer_auth(&token).send().await.unwrap();
    assert_eq!(r.status(), 200, "API tokens are separate credentials");
    enrol(&app, &member).await;
    assert_eq!(app.get(&member, "/spaces").await.0, 200);
}

/// Registers a passkey for the signed-in client with a software authenticator.
async fn add_passkey(app: &common::TestApp, c: &reqwest::Client, ed25519: bool, password: Option<&str>) -> wikicraft::webauthn::testing::Authenticator {
    use wikicraft::webauthn::{RelyingParty, b64, testing::Authenticator, unb64};
    let rp = RelyingParty::from_base_url(&app.base).unwrap();
    let (st, opts) = app.post(c, "/me/passkeys/options", json!({"password": password})).await;
    assert_eq!(st, 200, "{opts}");
    let auth = Authenticator::new(rp, ed25519);
    let challenge = unb64(opts["publicKey"]["challenge"].as_str().unwrap()).unwrap();
    let (client_data, attestation) = auth.register(&challenge);
    let (st, row) = app
        .post(
            c,
            "/me/passkeys",
            json!({"token": opts["token"], "name": "Laptop", "credential": {
                "id": b64(&auth.credential_id), "rawId": b64(&auth.credential_id), "type": "public-key",
                "response": {"clientDataJSON": b64(&client_data), "attestationObject": b64(&attestation), "transports": ["internal"]}
            }}),
        )
        .await;
    assert_eq!(st, 201, "{row}");
    auth
}

fn assertion(auth: &mut wikicraft::webauthn::testing::Authenticator, options: &Value, user: Option<&str>) -> Value {
    use wikicraft::webauthn::{b64, unb64};
    let challenge = unb64(options["challenge"].as_str().unwrap()).unwrap();
    let (client_data, data, sig) = auth.sign(&challenge);
    json!({"id": b64(&auth.credential_id), "rawId": b64(&auth.credential_id), "type": "public-key", "response": {
        "clientDataJSON": b64(&client_data), "authenticatorData": b64(&data), "signature": b64(&sig), "userHandle": user
    }})
}

#[tokio::test(flavor = "multi_thread")]
async fn passkeys_as_second_step_and_without_a_password() {
    use wikicraft::webauthn::b64;
    let app = common::spawn().await;
    let owner = app.setup().await;
    let (mia, mia_id) = app.user(&owner, "mia@acme.test", "member").await;
    let uuid: uuid::Uuid = mia_id.parse().unwrap();
    let handle = b64(uuid.as_bytes());

    // the options name this wiki and the person (their id as the user handle)
    let (_, opts) = app.post(&mia, "/me/passkeys/options", json!({})).await;
    assert_eq!(opts["publicKey"]["rp"]["id"], "127.0.0.1");
    assert_eq!(opts["publicKey"]["user"]["id"], handle.as_str());
    let mut key = add_passkey(&app, &mia, false, None).await;
    let (_, list) = app.get(&mia, "/me/passkeys").await;
    assert_eq!(list[0]["name"], "Laptop");
    let (_, s) = app.get(&mia, "/me/mfa").await;
    assert_eq!(s["passkeys"], 1);
    // the same authenticator can't be registered twice; the options leave it out
    let (_, opts) = app.post(&mia, "/me/passkeys/options", json!({})).await;
    assert_eq!(opts["publicKey"]["excludeCredentials"][0]["id"], b64(&key.credential_id).as_str());

    // a sign-in older than 15 minutes needs the password to add one
    sqlx::query("UPDATE sessions SET created_at = now() - interval '1 hour' WHERE user_id = $1::uuid").bind(&mia_id).execute(&app.state.db).await.unwrap();
    let (st, err) = app.post(&mia, "/me/passkeys/options", json!({})).await;
    assert_eq!(st, 403, "{err}");
    assert_eq!(app.post(&mia, "/me/passkeys/options", json!({"password": "wrong"})).await.0, 403);
    let mut phone = add_passkey(&app, &mia, true, Some("password123")).await;
    // … and so does adding an authenticator app next to the passkeys
    assert_eq!(app.post(&mia, "/me/mfa/setup", json!({})).await.0, 403);
    assert_eq!(app.post(&mia, "/me/mfa/setup", json!({"password": "password123"})).await.0, 200);
    // an API token can't touch sign-in methods at all
    let (_, tok) = app.post(&mia, "/me/tokens", json!({"name": "script"})).await;
    let bearer = |path: &'static str| {
        let token = tok["token"].as_str().unwrap().to_string();
        let url = app.url(path);
        async move { reqwest::Client::new().post(url).bearer_auth(token).json(&json!({"password": "password123"})).send().await.unwrap().status().as_u16() }
    };
    assert_eq!(bearer("/me/passkeys/options").await, 403);
    assert_eq!(bearer("/me/mfa/setup").await, 403);

    // the password is no longer enough: the second step takes the passkey
    let c = client();
    let (_, login) = app.post(&c, "/auth/login", json!({"email": "mia@acme.test", "password": "password123"})).await;
    assert_eq!(login["mfaRequired"], true);
    assert_eq!(login["methods"], json!(["passkey"]));
    let token = login["mfaToken"].as_str().unwrap();
    assert_eq!(
        app.post(&c, "/auth/mfa", json!({"mfaToken": token, "passkey": assertion(&mut key, &json!({"challenge": b64(&[0; 32])}), None)})).await.0,
        400,
        "the passkey step must be started"
    );
    let (_, step) = app.post(&c, "/auth/mfa/passkey", json!({"mfaToken": token})).await;
    assert_eq!(step["methods"], json!(["passkey"]));
    assert_eq!(step["publicKey"]["allowCredentials"].as_array().unwrap().len(), 2);
    let answer = assertion(&mut phone, &step["publicKey"], Some(&handle));
    let (st, done) = app.post(&c, "/auth/mfa", json!({"mfaToken": token, "passkey": answer})).await;
    assert_eq!(st, 200, "{done}");
    assert_eq!(app.get(&c, "/me").await.1["user"]["id"], mia_id.as_str());
    // the same answer again (a replay) is refused
    let c2 = client();
    let (_, login) = app.post(&c2, "/auth/login", json!({"email": "mia@acme.test", "password": "password123"})).await;
    let token = login["mfaToken"].as_str().unwrap();
    app.post(&c2, "/auth/mfa/passkey", json!({"mfaToken": token})).await;
    assert_eq!(app.post(&c2, "/auth/mfa", json!({"mfaToken": token, "passkey": answer})).await.0, 400);
    // someone else's passkey doesn't do
    let (other, _) = app.user(&owner, "ola@acme.test", "member").await;
    let mut olas = add_passkey(&app, &other, false, None).await;
    let (_, step) = app.post(&c2, "/auth/mfa/passkey", json!({"mfaToken": token})).await;
    assert_eq!(app.post(&c2, "/auth/mfa", json!({"mfaToken": token, "passkey": assertion(&mut olas, &step["publicKey"], None)})).await.0, 400);

    // without a password: the authenticator verified the person
    let c3 = client();
    let (_, opts) = app.post(&c3, "/auth/passkey/options", json!({})).await;
    assert_eq!(opts["publicKey"]["userVerification"], "required");
    let (st, r) = app.post(&c3, "/auth/passkey", json!({"token": opts["token"], "credential": assertion(&mut key, &opts["publicKey"], Some(&handle))})).await;
    assert_eq!(st, 200, "{r}");
    assert_eq!(app.get(&c3, "/me").await.1["user"]["id"], mia_id.as_str());
    // … not when it only saw someone touch it
    key.user_verified = false;
    let c4 = client();
    let (_, opts) = app.post(&c4, "/auth/passkey/options", json!({})).await;
    let (st, r) = app.post(&c4, "/auth/passkey", json!({"token": opts["token"], "credential": assertion(&mut key, &opts["publicKey"], None)})).await;
    assert_eq!(st, 400, "{r}");
    assert_eq!(app.get(&c4, "/me").await.1["user"], Value::Null);

    // a passkey counts as two-factor when the workspace requires it
    let (newbie, _) = app.user(&owner, "neo@acme.test", "member").await;
    assert_eq!(app.put(&owner, "/admin/settings", json!({"requireTwoFactor": "everyone"})).await.0, 200);
    assert_eq!(app.get(&newbie, "/me").await.1["mustSetUpTwoFactor"], true);
    assert_eq!(app.get(&newbie, "/spaces").await.0, 403);
    add_passkey(&app, &newbie, false, None).await;
    assert_eq!(app.get(&newbie, "/me").await.1["mustSetUpTwoFactor"], false);
    assert_eq!(app.get(&newbie, "/spaces").await.0, 200);

    // rename, remove; an admin reset removes them all
    let (_, list) = app.get(&mia, "/me/passkeys").await;
    let first = list[0]["id"].as_str().unwrap().to_string();
    assert_eq!(app.patch(&mia, &format!("/me/passkeys/{first}"), json!({"name": "Work laptop"})).await.1["name"], "Work laptop");
    // removing one also needs a recent sign-in or the password: a stolen session can't take it away
    assert_eq!(app.delete(&mia, &format!("/me/passkeys/{first}")).await, 403);
    let r = mia.delete(app.url(&format!("/me/passkeys/{first}"))).json(&json!({"password": "password123"})).send().await.unwrap();
    assert_eq!(r.status(), 204);
    assert_eq!(app.delete(&other, &format!("/me/passkeys/{}", list[1]["id"].as_str().unwrap())).await, 404, "not theirs");
    enrol(&app, &owner).await;
    assert_eq!(app.post(&owner, &format!("/users/{mia_id}/reset-mfa"), json!({})).await.0, 204);
    let n: i64 = sqlx::query_scalar("SELECT count(*) FROM passkeys WHERE user_id = $1::uuid").bind(&mia_id).fetch_one(&app.state.db).await.unwrap();
    assert_eq!(n, 0);
}

#[tokio::test(flavor = "multi_thread")]
async fn passkeys_alone_dont_sign_in_when_single_sign_on_is_enforced() {
    let app = common::spawn_with(|c| c.disable_password_login = true).await;
    let owner = app.setup().await;
    let mut key = add_passkey(&app, &owner, false, None).await;
    let (_, providers) = app.get(&client(), "/auth/providers").await;
    assert_eq!(providers["passkeyLogin"], false);
    let c = client();
    assert_eq!(app.post(&c, "/auth/passkey/options", json!({})).await.0, 400);
    // an answer to options from before the switch doesn't get in either
    let options = json!({"challenge": wikicraft::webauthn::b64(&[7; 32])});
    let (st, _) = app.post(&c, "/auth/passkey", json!({"token": "x", "credential": assertion(&mut key, &options, None)})).await;
    assert_eq!(st, 400);
    assert_eq!(app.get(&c, "/me").await.1["user"], Value::Null);
}
