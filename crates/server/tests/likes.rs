//! Likes on pages and comments, and images in comments.

mod common;

use serde_json::{Value, json};

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

#[tokio::test(flavor = "multi_thread")]
async fn images_in_comments() {
    let app = common::spawn().await;
    let owner = app.setup().await;
    let (commenter, commenter_id) = app.user(&owner, "pic@acme.test", "member").await;
    let (viewer, viewer_id) = app.user(&owner, "look@acme.test", "member").await;
    app.post(&owner, "/spaces", json!({"key": "PIC", "name": "Pictures", "defaultRole": "none"})).await;
    app.put(
        &owner,
        "/spaces/PIC/permissions",
        json!({"defaultRole": "none", "grants": [
            {"subjectType": "user", "subjectId": commenter_id, "role": "commenter"},
            {"subjectType": "user", "subjectId": viewer_id, "role": "viewer"}]}),
    )
    .await;
    let (_, page) = app.post(&owner, "/pages", json!({"spaceKey": "PIC", "title": "Mockups", "markdown": "Have a look"})).await;
    let pid = page["id"].as_str().unwrap().to_string();
    let upload = |c: &reqwest::Client, bytes: Vec<u8>, name: &str| {
        let form = reqwest::multipart::Form::new().part("file", reqwest::multipart::Part::bytes(bytes).file_name(name.to_string()));
        c.post(app.url(&format!("/pages/{pid}/comment-images"))).multipart(form).send()
    };
    let png = b"\x89PNG\r\n\x1a\n\0\0\0\rIHDR not really a picture".to_vec();
    assert_eq!(upload(&viewer, png.clone(), "shot.png").await.unwrap().status(), 403, "viewers can't comment");
    assert_eq!(upload(&commenter, b"<svg onload=alert(1)>".to_vec(), "x.png").await.unwrap().status(), 400, "images only, by content");
    let r = upload(&commenter, png.clone(), "image.png").await.unwrap();
    assert_eq!(r.status(), 201);
    let img: Value = r.json().await.unwrap();
    let url = img["url"].as_str().unwrap().to_string();
    // a second paste never replaces the first
    let r2: Value = upload(&commenter, png.clone(), "image.png").await.unwrap().json().await.unwrap();
    assert_ne!(r2["id"], img["id"]);

    let (st, c) = app.post(&commenter, &format!("/pages/{pid}/comments"), json!({"text": format!("The header is off:\n\n![header]({url})")})).await;
    assert_eq!(st, 201, "{c}");
    fn images(n: &Value, out: &mut Vec<Value>) {
        if n["type"] == "image" {
            out.push(n.clone());
        }
        for c in n["content"].as_array().into_iter().flatten() {
            images(c, out);
        }
    }
    let mut found = Vec::new();
    images(&c["content"], &mut found);
    let images = found;
    assert_eq!(images.len(), 1, "{}", c["content"]);
    assert_eq!(images[0]["attrs"]["src"], url.as_str());
    // an image alone is a comment
    assert_eq!(app.post(&commenter, &format!("/pages/{pid}/comments"), json!({"text": format!("![]({url})")})).await.0, 201);
    // readers of the page see it
    let r = viewer.get(app.url(&url.replacen("/api", "", 1))).send().await.unwrap();
    assert_eq!(r.status(), 200);
    assert_eq!(r.headers()["content-type"], "image/png");
}
