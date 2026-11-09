mod common;

use common::peer::Peer;

use std::time::Duration;

use serde_json::json;
use wikicraft_doc_model::Node;
use yrs::sync::{Message, SyncMessage};

#[tokio::test(flavor = "multi_thread")]
async fn live_collaboration_roundtrip() {
    let app = common::spawn().await;
    let owner = app.setup().await;
    let (_, space) = app.post(&owner, "/spaces", json!({"key": "LIVE", "name": "Live", "defaultRole": "viewer"})).await;
    let page = space["homePageId"].as_str().unwrap().to_string();
    let (_, t) = app.post(&owner, "/me/tokens", json!({"name": "ws"})).await;
    let token = t["token"].as_str().unwrap().to_string();

    let mut a = Peer::connect(&app.base, &page, &token).await;
    let mut b = Peer::connect(&app.base, &page, &token).await;
    // server seeded the Yjs doc from the stored content (welcome text)
    assert!(a.text().contains("Welcome to the Live space"), "{}", a.text());
    assert_eq!(a.text(), b.text());

    a.append_paragraph("Hello from A — grüße 👋").await;
    b.pump_until(|m| matches!(m, Message::Sync(SyncMessage::Update(_)))).await; // the first update B sees must be A's edit
    assert!(b.text().contains("Hello from A — grüße 👋"));

    b.append_paragraph("Reply from B").await;
    a.pump_until(|m| matches!(m, Message::Sync(SyncMessage::Update(_)))).await;
    assert_eq!(a.text(), b.text());

    // a viewer can watch but not write
    let (viewer, _) = app.user(&owner, "viewer@acme.test", "member").await;
    let (_, vt) = app.post(&viewer, "/me/tokens", json!({"name": "ws"})).await;
    let mut v = Peer::connect(&app.base, &page, vt["token"].as_str().unwrap()).await;
    assert!(v.text().contains("Reply from B"));
    v.append_paragraph("viewer vandalism").await;
    tokio::time::sleep(Duration::from_millis(300)).await;

    // REST content is the live content while the room is open
    let (_, p) = app.get(&owner, &format!("/pages/{page}")).await;
    let content = p["content"].to_string();
    assert!(content.contains("Reply from B"));
    assert!(!content.contains("vandalism"), "read-only updates must be ignored");

    // everyone leaves → persisted to the DB with a new version
    drop(a);
    drop(b);
    drop(v);
    let stored = app
        .eventually(|| async {
            let row: (serde_json::Value, i32) =
                sqlx::query_as("SELECT content, version FROM pages WHERE id = $1::uuid").bind(&page).fetch_one(&app.state.db).await.unwrap();
            (row.0.to_string().contains("Reply from B") && row.1 >= 2).then_some(row)
        })
        .await;
    let doc = Node::from_json(&stored.0).unwrap();
    assert!(wikicraft_doc_model::text::plain_text(&doc).contains("grüße 👋"));

    // a REST update reaches a connected editor live
    let mut c = Peer::connect(&app.base, &page, &token).await;
    let (st, _) = app.put(&owner, &format!("/pages/{page}/content"), json!({"markdown": "Replaced via API"})).await;
    assert_eq!(st, 200);
    c.pump_until(|m| matches!(m, Message::Sync(SyncMessage::Update(_)))).await;
    assert_eq!(c.text(), "Replaced via API");
}
