//! Likes on pages and comments.

mod common;

use serde_json::json;

#[tokio::test(flavor = "multi_thread")]
async fn likes_pages_and_comments() {
    let app = common::spawn().await;
    let owner = app.setup().await;
    let (lea, _) = app.user(&owner, "lea@acme.test", "member").await;
    app.post(&owner, "/spaces", json!({"key": "LIKE", "name": "Likes"})).await;
    let (_, page) = app.post(&owner, "/pages", json!({"spaceKey": "LIKE", "title": "Launch post"})).await;
    let id = page["id"].as_str().unwrap().to_string();

    let (st, s) = app.put(&lea, &format!("/pages/{id}/like"), json!({})).await;
    assert_eq!(st, 200);
    assert_eq!(s["count"], 1);
    assert_eq!(s["liked"], true);
    assert_eq!(s["names"], json!(["lea"]));
    // idempotent
    let (_, s) = app.put(&lea, &format!("/pages/{id}/like"), json!({})).await;
    assert_eq!(s["count"], 1);
    let (_, p) = app.get(&owner, &format!("/pages/{id}")).await;
    assert_eq!(p["likes"]["count"], 1);
    assert_eq!(p["likes"]["liked"], false);
    // the author hears about it
    let (_, n) = app.get(&owner, "/notifications").await;
    assert!(n.to_string().contains("\"kind\":\"like\""), "{n}");

    let (_, c) = app.post(&owner, &format!("/pages/{id}/comments"), json!({"text": "Thanks!"})).await;
    let cid = c["id"].as_str().unwrap().to_string();
    assert_eq!(app.put(&lea, &format!("/comments/{cid}/like"), json!({})).await.0, 204);
    let (_, list) = app.get(&lea, &format!("/pages/{id}/comments")).await;
    assert_eq!(list[0]["likeCount"], 1);
    assert_eq!(list[0]["liked"], true);
    let (_, list) = app.get(&owner, &format!("/pages/{id}/comments")).await;
    assert_eq!(list[0]["liked"], false);

    let (_, s) = app.delete_json(&lea, &format!("/pages/{id}/like")).await;
    assert_eq!(s["count"], 0);

    // pages you can't see can't be liked
    let (_, me) = app.get(&owner, "/me").await;
    app.put(&owner, &format!("/pages/{id}/restrictions"), json!({"restrictions": [{"kind": "view", "subjectType": "user", "subjectId": me["user"]["id"]}]}))
        .await;
    assert_eq!(app.put(&lea, &format!("/pages/{id}/like"), json!({})).await.0, 404);
    assert_eq!(app.put(&lea, &format!("/comments/{cid}/like"), json!({})).await.0, 404);
}

#[tokio::test(flavor = "multi_thread")]
async fn comment_mentions_notify_only_people_who_can_see_the_page() {
    let app = common::spawn().await;
    let owner = app.setup().await;
    let (lea, lea_id) = app.user(&owner, "lea@acme.test", "member").await;
    let (_, guest_id) = app.user(&owner, "gus@acme.test", "guest").await;
    app.post(&owner, "/spaces", json!({"key": "MEN", "name": "Mentions"})).await;
    let (_, page) = app.post(&owner, "/pages", json!({"spaceKey": "MEN", "title": "Plan"})).await;
    let id = page["id"].as_str().unwrap().to_string();
    let text = format!("@[Lea](user:{lea_id}) and @[Gus](user:{guest_id}) please review");
    let (st, c) = app.post(&owner, &format!("/pages/{id}/comments"), json!({"text": text})).await;
    assert_eq!(st, 201);
    let s = c["content"].to_string();
    assert!(s.contains("\"type\":\"mention\"") && s.contains(&lea_id) && !s.contains("user:"), "{s}");
    let (_, n) = app.get(&lea, "/notifications").await;
    assert!(n.to_string().contains("\"kind\":\"mention\""), "{n}");
    // the guest has no access to the space: no notification (and no excerpt) for them
    let count: i64 = sqlx::query_scalar("SELECT count(*) FROM notifications WHERE user_id = $1::uuid").bind(&guest_id).fetch_one(&app.state.db).await.unwrap();
    assert_eq!(count, 0);
}
