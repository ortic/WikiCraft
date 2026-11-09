//! E-mail: invitations, password reset and notification digests, delivered over SMTP to
//! mailpit (docker: `axllent/mailpit` with SMTP on 127.0.0.1:11025 and its API on
//! 127.0.0.1:18025). Skipped when mailpit isn't running.

mod common;

use serde_json::{Value, json};

use common::client;

const MAILPIT: &str = "http://127.0.0.1:18025";

/// Waits for the mail to `to` whose subject contains `subject`; returns its text body.
async fn mail_to(to: &str, subject: &str) -> String {
    for _ in 0..100 {
        let list: Value = reqwest::get(format!("{MAILPIT}/api/v1/search?query={}", urlencode(&format!("to:{to}")))).await.unwrap().json().await.unwrap();
        if let Some(m) = list["messages"].as_array().unwrap().iter().find(|m| m["Subject"].as_str().unwrap_or("").contains(subject)) {
            let full: Value = reqwest::get(format!("{MAILPIT}/api/v1/message/{}", m["ID"].as_str().unwrap())).await.unwrap().json().await.unwrap();
            return full["Text"].as_str().unwrap().to_string();
        }
        tokio::time::sleep(std::time::Duration::from_millis(200)).await;
    }
    panic!("no mail to {to} with subject {subject:?}");
}

async fn count_mails(to: &str, subject: &str) -> usize {
    let list: Value = reqwest::get(format!("{MAILPIT}/api/v1/search?query={}", urlencode(&format!("to:{to}")))).await.unwrap().json().await.unwrap();
    list["messages"].as_array().unwrap().iter().filter(|m| m["Subject"].as_str().unwrap_or("").contains(subject)).count()
}

fn urlencode(s: &str) -> String {
    s.bytes().map(|b| if b.is_ascii_alphanumeric() { (b as char).to_string() } else { format!("%{b:02X}") }).collect()
}

fn token_in(text: &str, path: &str) -> String {
    let at = text.find(&format!("{path}?token=")).unwrap_or_else(|| panic!("no {path} link in {text}")) + path.len() + 7;
    text[at..].chars().take_while(|c| c.is_ascii_alphanumeric() || *c == '-' || *c == '_').collect()
}

#[tokio::test(flavor = "multi_thread")]
async fn invitations_password_reset_and_notification_mails() {
    if reqwest::get(format!("{MAILPIT}/api/v1/info")).await.is_err() {
        eprintln!("skipping: mailpit not running");
        return;
    }
    let app = common::spawn_with(|c| c.smtp_url = Some("smtp://127.0.0.1:11025".into())).await;
    let owner = app.setup().await;
    let tag = uuid::Uuid::new_v4().simple().to_string()[..8].to_string();
    let invitee = format!("ivy-{tag}@acme.test");

    // admin: status + test mail
    let (_, status) = app.get(&owner, "/admin/mail").await;
    assert_eq!(status["enabled"], true);
    let (st, body) = app.post(&owner, "/admin/mail/test", json!({"to": format!("check-{tag}@acme.test")})).await;
    assert_eq!(st, 200, "{body}");
    mail_to(&format!("check-{tag}@acme.test"), "WikiCraft test e-mail").await;
    let (_, info) = app.get(&client(), "/auth/providers").await;
    assert_eq!(info["passwordReset"], true);

    // invitation → accept → signed in
    let (st, inv) = app.post(&owner, "/users/invite", json!({"email": invitee, "role": "member"})).await;
    assert_eq!(st, 201, "{inv}");
    assert_eq!(inv["emailed"], true);
    let (st, _) = app.post(&owner, "/users/invite", json!({"email": "owner@acme.test"})).await;
    assert_eq!(st, 409, "active accounts can't be invited");
    let text = mail_to(&invitee, "invited you").await;
    assert!(text.contains("Owner invited you to join Acme"), "{text}");
    let token = token_in(&text, "/accept-invite");
    let (st, info) = app.get(&client(), &format!("/auth/invite?token={token}")).await;
    assert_eq!(st, 200);
    assert_eq!(info["email"], invitee.as_str());
    assert_eq!(info["workspace"], "Acme");
    let (st, _) = app.post(&client(), "/auth/login", json!({"email": invitee, "password": "whatever-123"})).await;
    assert_eq!(st, 400, "invited accounts can't log in yet");
    let ivy = client();
    let (st, body) = app.post(&ivy, "/auth/accept-invite", json!({"token": token, "name": "Ivy Invitee", "password": "first-pass-1"})).await;
    assert_eq!(st, 200, "{body}");
    let (_, me) = app.get(&ivy, "/me").await;
    assert_eq!(me["user"]["name"], "Ivy Invitee");
    assert_eq!(me["user"]["status"], "active");
    let ivy_id = me["user"]["id"].as_str().unwrap().to_string();
    let (st, _) = app.post(&client(), "/auth/accept-invite", json!({"token": token, "name": "X", "password": "other-pass-1"})).await;
    assert_eq!(st, 400, "invitation links are single-use");

    // password reset
    let (st, _) = app.post(&client(), "/auth/forgot", json!({"email": format!("nobody-{tag}@acme.test")})).await;
    assert_eq!(st, 204, "no account enumeration");
    let (st, _) = app.post(&client(), "/auth/forgot", json!({"email": invitee.to_uppercase()})).await;
    assert_eq!(st, 204);
    let text = mail_to(&invitee, "Reset your WikiCraft password").await;
    let token = token_in(&text, "/reset-password");
    let (st, _) = app.post(&client(), "/auth/reset", json!({"token": token, "password": "short"})).await;
    assert_eq!(st, 400);
    let fresh = client();
    let (st, _) = app.post(&fresh, "/auth/reset", json!({"token": token, "password": "second-pass-2"})).await;
    assert_eq!(st, 200);
    let (_, me) = app.get(&fresh, "/me").await;
    assert_eq!(me["user"]["id"], ivy_id.as_str(), "signed in after the reset");
    let (_, me) = app.get(&ivy, "/me").await;
    assert!(me["user"].is_null(), "other sessions are signed out");
    let (st, _) = app.post(&client(), "/auth/reset", json!({"token": token, "password": "third-pass-3"})).await;
    assert_eq!(st, 400, "reset links are single-use");
    let (st, _) = app.post(&client(), "/auth/login", json!({"email": invitee, "password": "second-pass-2"})).await;
    assert_eq!(st, 200);
    // at most 3 reset mails per hour
    for _ in 0..4 {
        app.post(&client(), "/auth/forgot", json!({"email": invitee})).await;
    }
    tokio::time::sleep(std::time::Duration::from_secs(4)).await;
    assert_eq!(count_mails(&invitee, "Reset your WikiCraft password").await, 3);

    // notification digest: a mention that's still unread after the delay is mailed
    app.post(&owner, "/spaces", json!({"key": "MAIL", "name": "Mail"})).await;
    let mention = |title: &str| {
        json!({"spaceKey": "MAIL", "title": title, "content": {"type": "doc", "content": [{"type": "paragraph", "content": [
            {"type": "text", "text": "Please review, "},
            {"type": "mention", "attrs": {"id": ivy_id, "label": "Ivy Invitee"}}
        ]}]}})
    };
    let (st, _) = app.post(&owner, "/pages", mention("Quarterly plan")).await;
    assert_eq!(st, 201);
    let pending: i64 = sqlx::query_scalar("SELECT count(*) FROM notifications WHERE emailed_at IS NULL").fetch_one(&app.state.db).await.unwrap();
    assert_eq!(pending, 1);
    assert_eq!(wikicraft::mail::digests(&app.state).await.unwrap(), 0, "too fresh: the user may still see it in the app");
    sqlx::query("UPDATE notifications SET created_at = now() - interval '5 minutes'").execute(&app.state.db).await.unwrap();
    assert_eq!(wikicraft::mail::digests(&app.state).await.unwrap(), 1);
    let text = mail_to(&invitee, "mentioned you").await;
    assert!(text.contains("Owner mentioned you on “Quarterly plan”") && text.contains("/pages/"), "{text}");
    assert_eq!(wikicraft::mail::digests(&app.state).await.unwrap(), 0, "each notification is mailed once");

    // notifications read in the app, or with e-mails turned off, aren't mailed
    let (_, prefs) = app.get(&fresh, "/me/preferences").await;
    assert_eq!(prefs["emailNotifications"], "important");
    app.post(&owner, "/pages", mention("Read already")).await;
    sqlx::query("UPDATE notifications SET read_at = now(), created_at = now() - interval '5 minutes' WHERE emailed_at IS NULL")
        .execute(&app.state.db)
        .await
        .unwrap();
    assert_eq!(wikicraft::mail::digests(&app.state).await.unwrap(), 0);
    let (st, _) = app.put(&fresh, "/me/preferences", json!({"emailNotifications": "none"})).await;
    assert_eq!(st, 200);
    app.post(&owner, "/pages", mention("Muted")).await;
    sqlx::query("UPDATE notifications SET created_at = now() - interval '5 minutes' WHERE emailed_at IS NULL").execute(&app.state.db).await.unwrap();
    assert_eq!(wikicraft::mail::digests(&app.state).await.unwrap(), 0);
}

#[tokio::test(flavor = "multi_thread")]
async fn without_smtp_invites_still_return_a_link() {
    let app = common::spawn().await;
    let owner = app.setup().await;
    let (_, info) = app.get(&client(), "/auth/providers").await;
    assert_eq!(info["passwordReset"], false);
    let (st, inv) = app.post(&owner, "/users/invite", json!({"email": "nomail@acme.test"})).await;
    assert_eq!(st, 201);
    assert_eq!(inv["emailed"], false);
    let url = inv["inviteUrl"].as_str().unwrap();
    let token = url.split("token=").nth(1).unwrap();
    let c = client();
    let (st, _) = app.post(&c, "/auth/accept-invite", json!({"token": token, "name": "No Mail", "password": "password-123"})).await;
    assert_eq!(st, 200);
    let (st, body) = app.post(&owner, "/admin/mail/test", json!({})).await;
    assert_eq!(st, 400, "{body}");
}
