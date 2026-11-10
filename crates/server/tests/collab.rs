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

/// An imported author (placeholder) merged into the real person: authorship, likes,
/// permissions, mentions (stored and in an open editing session) and the Confluence account
/// mapping move over; the placeholder disappears.
#[tokio::test(flavor = "multi_thread")]
async fn merging_a_placeholder_user() {
    use wikicraft_doc_model::ydoc;
    let app = common::spawn().await;
    let owner = app.setup().await;
    let db = &app.state.db;
    let (_, space) = app.post(&owner, "/spaces", json!({"key": "MRG", "name": "Merge", "defaultRole": "viewer"})).await;
    let space_id = space["id"].as_str().unwrap().to_string();
    let (ada, ada_id) = app.user(&owner, "ada@acme.test", "member").await;
    let ws: uuid::Uuid = sqlx::query_scalar("SELECT workspace_id FROM spaces WHERE id = $1::uuid").bind(&space_id).fetch_one(db).await.unwrap();
    let ph = uuid::Uuid::now_v7();
    sqlx::query("INSERT INTO users (id, workspace_id, name, role, status, external_ref) VALUES ($1, $2, 'Confluence user 9f3a', 'member', 'placeholder', 'confluence:9f3a')")
        .bind(ph)
        .bind(ws)
        .execute(db)
        .await
        .unwrap();
    let mention = json!({"type": "mention", "attrs": {"id": ph.to_string(), "label": "Confluence user 9f3a"}});
    let content = json!({"type": "doc", "content": [{"type": "paragraph", "content": [{"type": "text", "text": "Ask "}, mention]}]});
    let (st, page) = app.post(&owner, "/pages", json!({"spaceKey": "MRG", "title": "Runbook", "content": content})).await;
    assert_eq!(st, 201, "{page}");
    let pid = page["id"].as_str().unwrap().to_string();
    // the placeholder wrote the page, liked it (as Ada did), and had a space role
    sqlx::query("UPDATE pages SET created_by = $2, updated_by = $2 WHERE id = $1::uuid").bind(&pid).bind(ph).execute(db).await.unwrap();
    sqlx::query("INSERT INTO page_likes (page_id, user_id) VALUES ($1::uuid, $2)").bind(&pid).bind(ph).execute(db).await.unwrap();
    app.put(&ada, &format!("/pages/{pid}/like"), json!({})).await;
    sqlx::query("INSERT INTO space_permissions (space_id, subject_type, subject_id, role) VALUES ($1::uuid, 'user', $2, 'editor')")
        .bind(&space_id)
        .bind(ph)
        .execute(db)
        .await
        .unwrap();

    // someone has the page open while the merge happens
    let (_, t) = app.post(&owner, "/me/tokens", json!({"name": "ws"})).await;
    let mut peer = Peer::connect(&app.base, &pid, t["token"].as_str().unwrap()).await;
    assert!(peer.text().contains("@Confluence user 9f3a") || peer.text().contains("Confluence user 9f3a"), "{}", peer.text());

    // only admins, only placeholders, only into real accounts
    let (st, _) = app.post(&ada, &format!("/users/{ph}/merge"), json!({"intoUserId": ada_id})).await;
    assert_eq!(st, 403);
    let (st, _) = app.post(&owner, &format!("/users/{ada_id}/merge"), json!({"intoUserId": ph})).await;
    assert_eq!(st, 400);
    let (st, res) = app.post(&owner, &format!("/users/{ph}/merge"), json!({"intoUserId": ada_id})).await;
    assert_eq!(st, 200, "{res}");
    assert_eq!(res["moved"]["pages.mentions"], 1, "{res}");

    let gone: bool = sqlx::query_scalar("SELECT NOT EXISTS (SELECT 1 FROM users WHERE id = $1)").bind(ph).fetch_one(db).await.unwrap();
    assert!(gone);
    let (created_by, ext): (uuid::Uuid, Option<String>) =
        sqlx::query_as("SELECT p.created_by, u.external_ref FROM pages p JOIN users u ON u.id = p.created_by WHERE p.id = $1::uuid")
            .bind(&pid)
            .fetch_one(db)
            .await
            .unwrap();
    assert_eq!(created_by.to_string(), ada_id);
    assert_eq!(ext.as_deref(), Some("confluence:9f3a"), "re-imports map the Confluence account to Ada");
    let likes: i64 = sqlx::query_scalar("SELECT count(*) FROM page_likes WHERE page_id = $1::uuid").bind(&pid).fetch_one(db).await.unwrap();
    assert_eq!(likes, 1, "the duplicate like is dropped");
    let role: String = sqlx::query_scalar("SELECT role FROM space_permissions WHERE space_id = $1::uuid AND subject_id = $2::uuid")
        .bind(&space_id)
        .bind(&ada_id)
        .fetch_one(db)
        .await
        .unwrap();
    assert_eq!(role, "editor");

    // the open session sees the mention change, and what it saves keeps it
    peer.wait_for_text("ada").await;
    let live = ydoc::doc_to_pm(&peer.doc).to_json().to_string();
    assert!(live.contains(&ada_id) && !live.contains(&ph.to_string()), "{live}");
    let updated_at: chrono::DateTime<chrono::Utc> =
        sqlx::query_scalar("SELECT updated_at FROM pages WHERE id = $1::uuid").bind(&pid).fetch_one(db).await.unwrap();
    drop(peer);
    wikicraft::collab::flush_all(&app.state).await;
    let (_, p) = app.get(&owner, &format!("/pages/{pid}")).await;
    let stored = p["content"].to_string();
    assert!(stored.contains(&ada_id) && stored.contains("\"label\":\"ada\""), "{stored}");
    let after: chrono::DateTime<chrono::Utc> = sqlx::query_scalar("SELECT updated_at FROM pages WHERE id = $1::uuid").bind(&pid).fetch_one(db).await.unwrap();
    assert_eq!(after, updated_at, "a merge isn't an edit");
}

/// Pages stored in an older document format are upgraded in the background (here: block ids
/// for pages from before them), with or without Yjs state, without counting as an edit.
#[tokio::test(flavor = "multi_thread")]
async fn old_pages_are_upgraded_in_the_background() {
    use wikicraft_doc_model::{blocks, schema, ydoc};
    let app = common::spawn().await;
    let owner = app.setup().await;
    app.post(&owner, "/spaces", json!({"key": "UPG", "name": "Upgrade"})).await;
    let db = &app.state.db;
    let mut ids = Vec::new();
    for (title, with_ydoc) in [("Opened before", true), ("Never opened", false)] {
        let (_, p) = app.post(&owner, "/pages", json!({"spaceKey": "UPG", "title": title, "markdown": "# Notes\n\n- one\n- two\n\nText."})).await;
        let id = p["id"].as_str().unwrap().to_string();
        let (_, page) = app.get(&owner, &format!("/pages/{id}")).await;
        let mut old = Node::from_json(&page["content"]).unwrap();
        blocks::strip_ids(&mut old);
        let y = with_ydoc.then(|| ydoc::pm_to_update(&old));
        sqlx::query("UPDATE pages SET content = $2, ydoc = $3, schema_version = 1, updated_at = '2021-05-05T00:00:00Z' WHERE id = $1::uuid")
            .bind(&id)
            .bind(old.to_json())
            .bind(y)
            .execute(db)
            .await
            .unwrap();
        ids.push(id);
    }
    let handled = wikicraft::schema_upgrade::upgrade_batch(&app.state, 50).await.unwrap();
    assert!(handled >= 2, "{handled}");
    assert_eq!(wikicraft::schema_upgrade::upgrade_batch(&app.state, 50).await.unwrap(), 0, "all done");
    for id in &ids {
        let (content, y, version, updated): (serde_json::Value, Option<Vec<u8>>, i32, chrono::DateTime<chrono::Utc>) =
            sqlx::query_as("SELECT content, ydoc, schema_version, updated_at FROM pages WHERE id = $1::uuid").bind(id).fetch_one(db).await.unwrap();
        assert_eq!(version, schema::VERSION);
        assert_eq!(updated.to_rfc3339(), "2021-05-05T00:00:00+00:00", "not an edit");
        let mut doc = Node::from_json(&content).unwrap();
        assert_eq!(blocks::ensure_ids(&mut doc), 0, "every block has an id: {content}");
        if let Some(y) = y {
            assert_eq!(ydoc::update_to_pm(&y).unwrap(), Node::from_json(&content).unwrap(), "JSON and Yjs state agree");
        }
    }
}

/// Minor changes: when everybody who edited in a session marked it minor, the session's version
/// says so and watchers aren't notified; the flag ends with the session.
#[tokio::test(flavor = "multi_thread")]
async fn minor_edits_dont_notify_watchers() {
    let app = common::spawn().await;
    let owner = app.setup().await;
    let (_, space) = app.post(&owner, "/spaces", json!({"key": "MIN", "name": "Minor"})).await;
    let page = space["homePageId"].as_str().unwrap().to_string();
    let (bob, _) = app.user(&owner, "bob@acme.test", "member").await;
    let (st, _) = app.put(&bob, &format!("/pages/{page}/watch"), json!({})).await;
    assert!(st < 300);
    let (_, t) = app.post(&owner, "/me/tokens", json!({"name": "ws"})).await;
    let token = t["token"].as_str().unwrap().to_string();
    // nobody's editing: nothing to mark
    let (st, _) = app.put(&owner, &format!("/pages/{page}/minor-edit"), json!({"minor": true})).await;
    assert_eq!(st, 409);

    // a minor session
    let mut a = Peer::connect(&app.base, &page, &token).await;
    let (st, r) = app.put(&owner, &format!("/pages/{page}/minor-edit"), json!({"minor": true})).await;
    assert_eq!((st, r["minor"].as_bool()), (200, Some(true)));
    a.append_paragraph("typo fix").await;
    tokio::time::sleep(Duration::from_millis(200)).await;
    drop(a);
    let (v, minor) = latest_version(&app, &page, 2).await;
    assert!(minor, "v{v} is minor");
    tokio::time::sleep(Duration::from_millis(300)).await;
    assert_eq!(updates_for(&app, &bob).await, 0, "watchers aren't told about minor changes");
    let (_, versions) = app.get(&owner, &format!("/pages/{page}/versions")).await;
    assert_eq!(versions[0]["minor"], true);

    // the next session is a normal one again
    let mut a = Peer::connect(&app.base, &page, &token).await;
    let (_, r) = app.get(&owner, &format!("/pages/{page}/minor-edit")).await;
    assert_eq!(r["minor"], false);
    a.append_paragraph("a real change").await;
    tokio::time::sleep(Duration::from_millis(200)).await;
    drop(a);
    let (_, minor) = latest_version(&app, &page, v + 1).await;
    assert!(!minor);
    app.eventually(|| async { (updates_for(&app, &bob).await == 1).then_some(()) }).await;

    // a named version can be minor too
    app.put(&owner, &format!("/pages/{page}/content"), json!({"markdown": "Rewritten"})).await;
    let (st, saved) = app.post(&owner, &format!("/pages/{page}/versions"), json!({"message": "cleanup", "minor": true})).await;
    assert_eq!(st, 200, "{saved}");
    let (_, versions) = app.get(&owner, &format!("/pages/{page}/versions")).await;
    assert!(versions.as_array().unwrap().iter().any(|v| v["message"] == "cleanup" && v["minor"] == true), "{versions}");
}

async fn updates_for(app: &common::TestApp, who: &reqwest::Client) -> usize {
    let (_, n) = app.get(who, "/notifications").await;
    n["items"].as_array().unwrap().iter().filter(|n| n["kind"] == "page_updated").count()
}

/// The newest version of a page once it's at least `min`: (version, minor).
async fn latest_version(app: &common::TestApp, page: &str, min: i32) -> (i32, bool) {
    app.eventually(|| async {
        let row: Option<(i32, bool)> = sqlx::query_as("SELECT version, minor FROM page_versions WHERE page_id = $1::uuid ORDER BY version DESC LIMIT 1")
            .bind(page)
            .fetch_optional(&app.state.db)
            .await
            .unwrap();
        row.filter(|r| r.0 >= min)
    })
    .await
}
