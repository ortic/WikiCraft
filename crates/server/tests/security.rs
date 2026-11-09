//! Regression tests for the findings of the security review (WORKLOG.md).

mod common;

use serde_json::{Value, json};

use common::{client, spawn};

async fn space_with_restricted_child(app: &common::TestApp, owner: &reqwest::Client, key: &str) -> (String, String, String) {
    let (_, sp) = app.post(owner, "/spaces", json!({"key": key, "name": key, "defaultRole": "editor"})).await;
    let home = sp["homePageId"].as_str().unwrap().to_string();
    let (_, parent) = app.post(owner, "/pages", json!({"spaceKey": key, "parentId": home, "title": "Public parent"})).await;
    let parent = parent["id"].as_str().unwrap().to_string();
    let (_, secret) = app.post(owner, "/pages", json!({"spaceKey": key, "parentId": parent, "title": "Secret child", "markdown": "classified kumquat"})).await;
    let secret = secret["id"].as_str().unwrap().to_string();
    let (_, me) = app.get(owner, "/me").await;
    let owner_id = me["user"]["id"].as_str().unwrap();
    let (st, _) = app
        .put(owner, &format!("/pages/{secret}/restrictions"), json!({"restrictions": [{"kind": "view", "subjectType": "user", "subjectId": owner_id}]}))
        .await;
    assert_eq!(st, 204);
    (home, parent, secret)
}

#[tokio::test(flavor = "multi_thread")]
async fn copy_and_move_do_not_leak_restricted_children() {
    let app = spawn().await;
    let owner = app.setup().await;
    let (mallory, _) = app.user(&owner, "mallory@acme.test", "member").await;
    let (_, parent, secret) = space_with_restricted_child(&app, &owner, "SEC").await;

    // copy with children: the restricted child is not copied
    let (st, copied) = app.post(&mallory, &format!("/pages/{parent}/copy"), json!({"includeChildren": true})).await;
    assert_eq!(st, 201);
    assert_eq!(copied["copied"], 1);
    let leaked: i64 = sqlx::query_scalar("SELECT count(*) FROM pages WHERE title = 'Secret child'").fetch_one(&app.state.db).await.unwrap();
    assert_eq!(leaked, 1, "only the original exists");

    // moving the subtree into a space mallory administers is refused
    app.post(&mallory, "/spaces", json!({"key": "MINE", "name": "Mine"})).await;
    let (st, _) = app.post(&mallory, &format!("/pages/{parent}/move"), json!({"spaceKey": "MINE"})).await;
    assert_eq!(st, 403);
    let (st, _) = app.get(&mallory, &format!("/pages/{secret}")).await;
    assert_eq!(st, 404);

    // trashing a subtree with invisible pages is refused; trash listing hides restricted titles
    let (st, _) = app.post(&mallory, &format!("/pages/{parent}/move"), json!({"parentId": null})).await;
    assert_eq!(st, 204, "same-space moves are fine");
    assert_eq!(app.delete(&mallory, &format!("/pages/{parent}")).await, 409);
    assert_eq!(app.delete(&owner, &format!("/pages/{secret}")).await, 204);
    let (_, trash) = app.get(&mallory, "/spaces/SEC/trash").await;
    assert!(trash.as_array().unwrap().iter().all(|t| t["title"] != "Secret child"));
    let (st, _) = app.post(&mallory, &format!("/pages/{secret}/restore"), json!({})).await;
    assert_eq!(st, 404, "can't restore what you can't see");
}

#[tokio::test(flavor = "multi_thread")]
async fn deep_nesting_does_not_crash_the_server() {
    let app = spawn().await;
    let c = app.setup().await;
    let (_, sp) = app.post(&c, "/spaces", json!({"key": "DEEP", "name": "Deep"})).await;
    let home = sp["homePageId"].as_str().unwrap();
    let text = format!("{} x", ">".repeat(20_000));
    let (st, _) = app.post(&c, &format!("/pages/{home}/comments"), json!({"text": text})).await;
    assert_eq!(st, 201);
    let (st, _) = app.put(&c, &format!("/pages/{home}/content"), json!({"markdown": format!("{}deep", "- ".repeat(5000))})).await;
    assert_eq!(st, 200);
    let (st, _) = app.get(&c, "/health").await;
    assert_eq!(st, 200, "server still alive");
}

#[tokio::test(flavor = "multi_thread")]
async fn account_takeover_and_brute_force_protection() {
    let app = spawn().await;
    let owner = app.setup().await;
    let (admin, _) = app.user(&owner, "adam@acme.test", "admin").await;
    let (_, me) = app.get(&owner, "/me").await;
    let owner_id = me["user"]["id"].as_str().unwrap();
    let (st, _) = app.patch(&admin, &format!("/users/{owner_id}"), json!({"password": "hijacked-pass"})).await;
    assert_eq!(st, 403, "admins can't reset the owner's password");

    // members' passwords can be reset by admins, and their sessions are revoked
    let (member, member_id) = app.user(&owner, "mo@acme.test", "member").await;
    let (st, _) = app.patch(&admin, &format!("/users/{member_id}"), json!({"password": "new-password-1"})).await;
    assert_eq!(st, 204);
    let (st, _) = app.get(&member, "/recent").await;
    assert_eq!(st, 401);

    // login throttling
    for _ in 0..10 {
        let (st, _) = app.post(&client(), "/auth/login", json!({"email": "owner@acme.test", "password": "wrong-password"})).await;
        assert_eq!(st, 400);
    }
    let (st, body) = app.post(&client(), "/auth/login", json!({"email": "owner@acme.test", "password": "correct horse"})).await;
    assert_eq!(st, 400);
    assert!(body["message"].as_str().unwrap().contains("too many"));

    // guests don't get the member directory
    let (guest, _) = app.user(&owner, "gail@acme.test", "guest").await;
    let (_, users) = app.get(&guest, "/users").await;
    assert!(users.as_array().unwrap().is_empty());
}

#[tokio::test(flavor = "multi_thread")]
async fn body_limits_and_public_spaces() {
    let app = spawn().await;
    let owner = app.setup().await;
    // a 20 MiB JSON body is rejected before it's buffered into a handler
    let big = "x".repeat(20 * 1024 * 1024);
    let r = owner.post(app.url("/auth/login")).json(&json!({"email": big, "password": "x"})).send().await.unwrap();
    assert_eq!(r.status(), 413);
    // only workspace admins can make a space public
    let (member, _) = app.user(&owner, "sam@acme.test", "member").await;
    app.post(&member, "/spaces", json!({"key": "SAM", "name": "Sam"})).await;
    let (st, _) = app.patch(&member, "/spaces/SAM", json!({"anonymousAccess": true})).await;
    assert_eq!(st, 400);
    let (st, _) = app.patch(&owner, "/spaces/SAM", json!({"anonymousAccess": true})).await;
    assert_eq!(st, 200);
}

#[tokio::test(flavor = "multi_thread")]
async fn search_follows_moves_and_trash() {
    let app = spawn().await;
    let owner = app.setup().await;
    let (member, _) = app.user(&owner, "pat@acme.test", "member").await;
    app.post(&owner, "/spaces", json!({"key": "PUB", "name": "Public", "defaultRole": "viewer"})).await;
    app.post(&owner, "/spaces", json!({"key": "PRIV", "name": "Private", "defaultRole": "none"})).await;
    let (_, page) = app.post(&owner, "/pages", json!({"spaceKey": "PUB", "title": "Roadmap", "markdown": "secret pineapple plans"})).await;
    let id = page["id"].as_str().unwrap().to_string();
    app.eventually(|| async {
        let (_, h) = app.get(&member, "/search?q=pineapple").await;
        (h.as_array().unwrap().len() == 1).then_some(())
    })
    .await;
    let (st, _) = app.post(&owner, &format!("/pages/{id}/move"), json!({"spaceKey": "PRIV"})).await;
    assert_eq!(st, 204);
    let (_, h) = app.get(&member, "/search?q=pineapple").await;
    assert!(h.as_array().unwrap().is_empty(), "moved page must not leak through a stale index: {h}");
    let (_, h): (u16, Value) = app.get(&owner, "/search?q=pineapple").await;
    assert_eq!(h.as_array().unwrap().len(), 1);
}
