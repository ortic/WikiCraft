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

    // the periodic flush stores the content while people are still connected (this used to make
    // the version on the last disconnect get skipped: nothing was dirty any more)
    wikicraft::collab::flush_all(&app.state).await;
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

/// Blocks carry stable ids: pages created through the API get them, pages from before block ids
/// get them when first opened (without counting as an edit), and content replaced through the
/// API keeps the ids of the blocks that didn't change.
#[tokio::test(flavor = "multi_thread")]
async fn block_ids() {
    use wikicraft_doc_model::{blocks, ydoc};
    let app = common::spawn().await;
    let owner = app.setup().await;
    app.post(&owner, "/spaces", json!({"key": "BID", "name": "Blocks", "defaultRole": "viewer"})).await;
    let md = "# Plan\n\nFirst paragraph.\n\n- one\n- two\n\nLast paragraph.";
    let (st, page) = app.post(&owner, "/pages", json!({"spaceKey": "BID", "title": "Plan", "markdown": md})).await;
    assert_eq!(st, 201, "{page}");
    let id = page["id"].as_str().unwrap().to_string();
    let (_, p) = app.get(&owner, &format!("/pages/{id}")).await;
    let mut created = Node::from_json(&p["content"]).unwrap();
    assert!(created.content.len() >= 4, "{}", p["content"]);
    assert_eq!(blocks::ensure_ids(&mut created), 0, "API-created pages have ids on every block: {}", p["content"]);

    // a page from before block ids: no ids in the content nor in the Yjs state
    let mut old = created.clone();
    blocks::strip_ids(&mut old);
    sqlx::query("UPDATE pages SET content = $2, ydoc = $3, updated_at = '2020-01-01T00:00:00Z' WHERE id = $1::uuid")
        .bind(&id)
        .bind(old.to_json())
        .bind(ydoc::pm_to_update(&old))
        .execute(&app.state.db)
        .await
        .unwrap();
    let versions = || async {
        let n: i64 = sqlx::query_scalar("SELECT count(*) FROM page_versions WHERE page_id = $1::uuid").bind(&id).fetch_one(&app.state.db).await.unwrap();
        n
    };
    let versions_before = versions().await;

    // a reader opens it: the server assigns the ids before the first sync
    let (viewer, _) = app.user(&owner, "reader@acme.test", "member").await;
    let (_, vt) = app.post(&viewer, "/me/tokens", json!({"name": "ws"})).await;
    let v = Peer::connect(&app.base, &id, vt["token"].as_str().unwrap()).await;
    let mut seen = ydoc::doc_to_pm(&v.doc);
    assert_eq!(blocks::ensure_ids(&mut seen), 0, "the reader got ids for every block");
    let (content, updated): (serde_json::Value, chrono::DateTime<chrono::Utc>) =
        sqlx::query_as("SELECT content, updated_at FROM pages WHERE id = $1::uuid").bind(&id).fetch_one(&app.state.db).await.unwrap();
    let mut stored = Node::from_json(&content).unwrap();
    assert_eq!(blocks::ensure_ids(&mut stored), 0, "stored right away");
    assert_eq!(stored, seen);
    drop(v);
    wikicraft::collab::flush_all(&app.state).await;
    tokio::time::sleep(Duration::from_millis(300)).await;
    let updated_after: chrono::DateTime<chrono::Utc> =
        sqlx::query_scalar("SELECT updated_at FROM pages WHERE id = $1::uuid").bind(&id).fetch_one(&app.state.db).await.unwrap();
    assert_eq!(updated.to_rfc3339(), "2020-01-01T00:00:00+00:00", "not an edit");
    assert_eq!(updated_after, updated, "not an edit after the flush either");
    assert_eq!(versions().await, versions_before, "no version for assigning ids");

    // the whole content replaced through the API (no ids): unchanged blocks keep theirs
    let id_of = |d: &Node, text: &str| {
        let mut out = None;
        d.walk(&mut |n| {
            if out.is_none() && n.kind != "text" && wikicraft_doc_model::text::plain_text(n) == text {
                out = n.attr_str(blocks::ATTR).map(str::to_string);
            }
        });
        out
    };
    let (st, _) =
        app.put(&owner, &format!("/pages/{id}/content"), json!({"markdown": "# Plan\n\nFirst paragraph, edited.\n\n- one\n- two\n\nLast paragraph."})).await;
    assert_eq!(st, 200);
    let (_, p) = app.get(&owner, &format!("/pages/{id}")).await;
    let mut replaced = Node::from_json(&p["content"]).unwrap();
    assert_eq!(id_of(&replaced, "Plan"), id_of(&stored, "Plan"));
    assert_eq!(id_of(&replaced, "Last paragraph."), id_of(&stored, "Last paragraph."));
    assert!(id_of(&replaced, "First paragraph, edited.").is_some());
    assert_ne!(id_of(&replaced, "First paragraph, edited."), id_of(&stored, "First paragraph."));
    assert_eq!(blocks::ensure_ids(&mut replaced), 0);

    // shared pages render the anchors
    let (st, link) = app.post(&owner, &format!("/pages/{id}/share-links"), json!({})).await;
    assert_eq!(st, 201, "{link}");
    let token = link["token"].as_str().unwrap();
    let shared: serde_json::Value = reqwest::get(app.url(&format!("/public/{token}"))).await.unwrap().json().await.unwrap();
    let html = shared["html"].as_str().unwrap();
    let block = id_of(&replaced, "Last paragraph.").unwrap();
    assert!(html.contains(&format!("<p id=\"block-{block}\">Last paragraph.</p>")), "{html}");
}
