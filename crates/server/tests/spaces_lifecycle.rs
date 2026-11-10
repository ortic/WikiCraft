//! Archiving, unarchiving and deleting spaces.

mod common;

use serde_json::{Value, json};

#[tokio::test(flavor = "multi_thread")]
async fn archive_is_read_only_and_delete_needs_archive_and_confirmation() {
    let app = common::spawn().await;
    let owner = app.setup().await;
    let (ed, _) = app.user(&owner, "ed@acme.test", "member").await;
    app.post(&owner, "/spaces", json!({"key": "OLD", "name": "Old project", "defaultRole": "editor"})).await;
    let (_, page) = app.post(&owner, "/pages", json!({"spaceKey": "OLD", "title": "Retro notes", "markdown": "zanzibar lessons"})).await;
    let id = page["id"].as_str().unwrap().to_string();
    app.eventually(|| async { (app.get(&ed, "/search?q=zanzibar").await.1.as_array().unwrap().len() == 1).then_some(()) }).await;
    assert_eq!(app.put(&ed, &format!("/pages/{id}/content"), json!({"markdown": "edit ok"})).await.0, 200);

    // delete needs an archived space
    let r = owner.delete(app.url("/admin/spaces/OLD")).json(&json!({"confirm": "OLD"})).send().await.unwrap();
    assert_eq!(r.status(), 400);

    assert_eq!(app.delete(&owner, "/spaces/OLD").await, 204, "archive");
    // read-only for everyone, out of search, still readable
    assert_eq!(app.get(&ed, &format!("/pages/{id}")).await.0, 200);
    assert_eq!(app.put(&ed, &format!("/pages/{id}/content"), json!({"markdown": "nope"})).await.0, 403);
    assert_eq!(app.post(&ed, "/pages", json!({"spaceKey": "OLD", "title": "New"})).await.0, 403);
    assert!(app.get(&ed, "/search?q=zanzibar").await.1.as_array().unwrap().is_empty());
    let (_, spaces) = app.get(&owner, "/admin/spaces").await;
    let old = spaces.as_array().unwrap().iter().find(|s| s["key"] == "OLD").unwrap().clone();
    assert!(old["archivedAt"].is_string());
    assert_eq!(old["pages"], 2, "home + page");
    assert_eq!(app.get(&ed, "/admin/spaces").await.0, 403);

    // unarchive restores editing
    assert_eq!(app.post(&owner, "/admin/spaces/OLD/unarchive", json!({})).await.0, 204);
    assert_eq!(app.put(&ed, &format!("/pages/{id}/content"), json!({"markdown": "edit again"})).await.0, 200);

    // permanent delete: archived + typed key
    app.delete(&owner, "/spaces/OLD").await;
    let r = owner.delete(app.url("/admin/spaces/OLD")).header("x-wikicraft", "1").json(&json!({"confirm": "WRONG"})).send().await.unwrap();
    assert_eq!(r.status(), 400);
    let r = owner.delete(app.url("/admin/spaces/OLD")).header("x-wikicraft", "1").json(&json!({"confirm": "old"})).send().await.unwrap();
    assert_eq!(r.status(), 204);
    assert_eq!(app.get(&owner, "/spaces/OLD").await.0, 404);
    assert_eq!(app.get(&owner, &format!("/pages/{id}")).await.0, 404);
}

#[tokio::test(flavor = "multi_thread")]
async fn renaming_a_space_key_keeps_old_links() {
    let app = common::spawn().await;
    let owner = app.setup().await;
    let (member, _) = app.user(&owner, "mo@acme.test", "member").await;
    let (_, sp) = app.post(&owner, "/spaces", json!({"key": "OPS", "name": "Operations"})).await;
    let home = sp["homePageId"].as_str().unwrap().to_string();
    app.post(&owner, "/spaces", json!({"key": "HR", "name": "People"})).await;

    // only space admins rename; keys are validated and unique (also against former keys)
    assert_eq!(app.patch(&member, "/spaces/OPS", json!({"key": "PLATFORM"})).await.0, 403);
    assert_eq!(app.patch(&owner, "/spaces/OPS", json!({"key": "bad key!"})).await.0, 400);
    assert_eq!(app.patch(&owner, "/spaces/OPS", json!({"key": "hr"})).await.0, 409);
    let (st, s) = app.patch(&owner, "/spaces/OPS", json!({"key": "platform"})).await;
    assert_eq!(st, 200, "{s}");
    assert_eq!(s["key"], "PLATFORM");

    // the old key still resolves (API and Confluence-style links), to the new space
    let (st, s) = app.get(&member, "/spaces/ops").await;
    assert_eq!(st, 200);
    assert_eq!(s["key"], "PLATFORM");
    let (_, p) = app.get(&member, &format!("/pages/{home}")).await;
    assert_eq!(p["spaceKey"], "PLATFORM");
    let (st, _) = app.post(&owner, "/pages", json!({"spaceKey": "OPS", "title": "Via the old key"})).await;
    assert_eq!(st, 201);
    // a new space can't take the old key; renaming back is fine
    let (st, _) = app.post(&owner, "/spaces", json!({"key": "OPS", "name": "Impostor"})).await;
    assert_eq!(st, 409);
    let (st, s) = app.patch(&owner, "/spaces/PLATFORM", json!({"key": "OPS"})).await;
    assert_eq!(st, 200);
    assert_eq!(s["key"], "OPS");
    let (st, s) = app.get(&member, "/spaces/platform").await;
    assert_eq!((st, s["key"].as_str()), (200, Some("OPS")));
}

#[tokio::test(flavor = "multi_thread")]
async fn archived_pages_are_read_only_and_hidden() {
    let app = common::spawn().await;
    let owner = app.setup().await;
    let (member, _) = app.user(&owner, "ed@acme.test", "member").await;
    let (_, sp) = app.post(&owner, "/spaces", json!({"key": "ARC", "name": "Archive test"})).await;
    let home = sp["homePageId"].as_str().unwrap().to_string();
    let (_, a) = app.post(&owner, "/pages", json!({"spaceKey": "ARC", "parentId": home, "title": "Old project", "markdown": "legacy pumpernickel"})).await;
    let a = a["id"].as_str().unwrap().to_string();
    let (_, a1) = app.post(&owner, "/pages", json!({"spaceKey": "ARC", "parentId": a, "title": "Old notes", "markdown": "more pumpernickel"})).await;
    let a1 = a1["id"].as_str().unwrap().to_string();
    app.eventually(|| async {
        let (_, r) = app.get(&member, "/search?q=pumpernickel").await;
        (r.as_array().unwrap().len() == 2).then_some(())
    })
    .await;

    assert_eq!(app.post(&member, &format!("/pages/{home}/archive"), json!({})).await.0, 400, "home page");
    let (st, _) = app.post(&member, &format!("/pages/{a}/archive"), json!({})).await;
    assert_eq!(st, 204);
    assert_eq!(app.post(&member, &format!("/pages/{a}/archive"), json!({})).await.0, 409);

    // gone from the tree and search, still readable by link, but read-only
    let (_, tree) = app.get(&member, "/spaces/ARC/tree").await;
    let titles: Vec<&str> = tree["nodes"].as_array().unwrap().iter().map(|n| n["title"].as_str().unwrap()).collect();
    assert!(!titles.contains(&"Old project") && !titles.contains(&"Old notes"), "{titles:?}");
    let (_, r) = app.get(&member, "/search?q=pumpernickel").await;
    assert_eq!(r.as_array().unwrap().len(), 0);
    let (st, p) = app.get(&member, &format!("/pages/{a1}")).await;
    assert_eq!(st, 200);
    assert_eq!(p["access"]["edit"], false);
    assert_eq!(p["access"]["archivedPage"], a.as_str());
    assert_eq!(app.put(&member, &format!("/pages/{a1}/content"), json!({"markdown": "edit"})).await.0, 403);
    assert_eq!(app.post(&member, &format!("/pages/{a1}/comments"), json!({"text": "hi"})).await.0, 403);
    assert_eq!(app.post(&member, &format!("/pages/{a1}/unarchive"), json!({})).await.0, 400, "restore the archived ancestor instead");

    // the space lists it; restoring brings everything back
    let (_, list) = app.get(&member, "/spaces/ARC/archived").await;
    assert_eq!(list.as_array().unwrap().len(), 1);
    assert_eq!(list[0]["title"], "Old project");
    assert_eq!(list[0]["descendants"], 1);
    assert_eq!(list[0]["canRestore"], true);
    assert_eq!(app.post(&member, &format!("/pages/{a}/unarchive"), json!({})).await.0, 204);
    let (_, p) = app.get(&member, &format!("/pages/{a1}")).await;
    assert_eq!(p["access"]["edit"], true);
    assert!(p["access"]["archivedPage"].is_null());
    let (_, tree) = app.get(&member, "/spaces/ARC/tree").await;
    assert_eq!(tree["nodes"].as_array().unwrap().len(), 3);
}

#[tokio::test(flavor = "multi_thread")]
async fn spaces_from_templates() {
    let app = common::spawn().await;
    let owner = app.setup().await;
    let (_, list) = app.get(&owner, "/space-templates").await;
    let ids: Vec<&str> = list.as_array().unwrap().iter().map(|t| t["id"].as_str().unwrap()).collect();
    assert_eq!(ids, vec!["team", "project", "knowledge", "engineering"]);
    assert_eq!(app.post(&owner, "/spaces", json!({"key": "X", "name": "X", "template": "nope"})).await.0, 400);

    let (st, sp) = app.post(&owner, "/spaces", json!({"key": "APOLLO", "name": "Apollo", "template": "project"})).await;
    assert_eq!(st, 201, "{sp}");
    let (_, tree) = app.get(&owner, "/spaces/APOLLO/tree").await;
    let nodes = tree["nodes"].as_array().unwrap();
    let title_of = |id: &Value| nodes.iter().find(|n| n["id"] == *id).map(|n| n["title"].as_str().unwrap().to_string());
    let titles: Vec<&str> = nodes.iter().map(|n| n["title"].as_str().unwrap()).collect();
    for t in ["Apollo", "Project plan", "Requirements", "Decisions", "Decision: scope of the first release", "Status reports", "Status report", "Retrospective"]
    {
        assert!(titles.contains(&t), "{t} missing: {titles:?}");
    }
    let decision = nodes.iter().find(|n| n["title"] == "Decision: scope of the first release").unwrap();
    assert_eq!(title_of(&decision["parentId"]).as_deref(), Some("Decisions"));
    // the home page lists its children and keeps the template's content
    let (_, home) = app.get(&owner, &format!("/pages/{}", sp["homePageId"].as_str().unwrap())).await;
    let content = home["content"].to_string();
    assert!(content.contains("childPages") && content.contains("Owner"), "{content}");
    let plan = nodes.iter().find(|n| n["title"] == "Project plan").unwrap();
    let (_, plan) = app.get(&owner, &format!("/pages/{}", plan["id"].as_str().unwrap())).await;
    assert!(plan["content"].to_string().contains("Milestones") || plan["content"]["content"].as_array().unwrap().len() > 2, "{plan}");
}

#[tokio::test(flavor = "multi_thread")]
async fn personal_spaces_are_private() {
    let app = common::spawn().await;
    let owner = app.setup().await;
    let (ann, _) = app.user(&owner, "ann.lee@acme.test", "member").await;
    let (bob, _) = app.user(&owner, "bob@acme.test", "member").await;
    let (_, none) = app.get(&ann, "/me/space").await;
    assert!(none["key"].is_null());
    let (st, sp) = app.post(&ann, "/me/space", json!({})).await;
    assert_eq!(st, 201, "{sp}");
    assert_eq!(sp["key"], "~ANNLEE");
    let (st, again) = app.post(&ann, "/me/space", json!({})).await;
    assert_eq!((st, again["id"].clone()), (200, sp["id"].clone()), "one per person");
    let (_, mine) = app.get(&ann, "/me/space").await;
    assert_eq!(mine["key"], "~ANNLEE");
    // Ann administers it; Bob neither sees nor finds it
    let (st, s) = app.get(&ann, "/spaces/~annlee").await;
    assert_eq!((st, s["role"].as_str()), (200, Some("admin")));
    let (st, _) = app.post(&ann, "/pages", json!({"spaceKey": "~ANNLEE", "title": "Draft idea", "markdown": "quokka plan"})).await;
    assert_eq!(st, 201);
    assert_eq!(app.get(&bob, "/spaces/~ANNLEE").await.0, 404);
    let (_, list) = app.get(&bob, "/spaces").await;
    assert!(!list.to_string().contains("~ANNLEE"));
    app.eventually(|| async {
        let (_, r) = app.get(&ann, "/search?q=quokka").await;
        (r.as_array().unwrap().len() == 1).then_some(())
    })
    .await;
    let (_, r) = app.get(&bob, "/search?q=quokka").await;
    assert_eq!(r.as_array().unwrap().len(), 0);
}

/// Trash for good: one page (with its trashed sub-pages and files), the whole trash, and the
/// retention period. A file another page still uses stays.
#[tokio::test(flavor = "multi_thread")]
async fn trash_purge_and_retention() {
    let app = common::spawn().await;
    let owner = app.setup().await;
    let (editor, editor_id) = app.user(&owner, "ed@acme.test", "member").await;
    let (_, space) = app.post(&owner, "/spaces", json!({"key": "PUR", "name": "Purge"})).await;
    let space_id = space["id"].as_str().unwrap().to_string();
    app.put(&owner, "/spaces/PUR/permissions", json!({"defaultRole": "viewer", "grants": [{"subjectType": "user", "subjectId": editor_id, "role": "editor"}]}))
        .await;
    let page = |title: &'static str, parent: Option<String>| {
        let (app, owner) = (&app, &owner);
        async move { app.post(owner, "/pages", json!({"spaceKey": "PUR", "title": title, "parentId": parent})).await.1["id"].as_str().unwrap().to_string() }
    };
    let upload = |page: String, bytes: &'static [u8]| {
        let (app, owner) = (&app, &owner);
        async move {
            let form = reqwest::multipart::Form::new().part("file", reqwest::multipart::Part::bytes(bytes.to_vec()).file_name("notes.txt"));
            let r = owner.post(app.url(&format!("/pages/{page}/attachments"))).multipart(form).send().await.unwrap();
            assert_eq!(r.status(), 201);
        }
    };
    let (a, keep) = (page("Old plan", None).await, page("Live page", None).await);
    let b = page("Old plan details", Some(a.clone())).await;
    upload(b.clone(), b"only in the old plan").await;
    upload(a.clone(), b"shared file").await;
    upload(keep.clone(), b"shared file").await;
    let key_of = |s: &str| wikicraft::blobs::BlobStore::key_for(s.as_bytes());
    let blob_exists = |k: String| {
        let db = app.state.db.clone();
        async move { sqlx::query_scalar::<_, bool>("SELECT EXISTS (SELECT 1 FROM blobs WHERE key = $1)").bind(k).fetch_one(&db).await.unwrap() }
    };
    assert_eq!(app.delete(&owner, &format!("/pages/{a}")).await, 204);

    // space admins only, and only for trashed pages
    assert_eq!(app.post(&editor, &format!("/pages/{a}/purge"), json!({})).await.0, 403);
    assert_eq!(app.post(&owner, &format!("/pages/{keep}/purge"), json!({})).await.0, 404);
    let (st, r) = app.post(&owner, &format!("/pages/{a}/purge"), json!({})).await;
    assert_eq!(st, 200, "{r}");
    assert_eq!(r["purged"], 2, "the trashed sub-page goes too");
    let left: i64 = sqlx::query_scalar("SELECT count(*) FROM pages WHERE id IN ($1::uuid, $2::uuid)").bind(&a).bind(&b).fetch_one(&app.state.db).await.unwrap();
    assert_eq!(left, 0);
    assert!(!blob_exists(key_of("only in the old plan")).await, "its own file is gone");
    assert!(app.state.blobs.get(&key_of("only in the old plan")).await.is_err(), "from storage too");
    assert!(blob_exists(key_of("shared file")).await, "a file another page uses stays");
    let (_, att) = app.get(&owner, &format!("/pages/{keep}/attachments")).await;
    let att_id = att[0]["id"].as_str().unwrap();
    let r = owner.get(app.url(&format!("/attachments/{att_id}/content"))).send().await.unwrap();
    assert_eq!(r.bytes().await.unwrap().as_ref(), b"shared file");

    // retention: only what has been in the trash longer than the period
    let (old, recent) = (page("Ancient", None).await, page("Recent", None).await);
    app.delete(&owner, &format!("/pages/{old}")).await;
    app.delete(&owner, &format!("/pages/{recent}")).await;
    sqlx::query("UPDATE pages SET deleted_at = now() - interval '40 days' WHERE id = $1::uuid").bind(&old).execute(&app.state.db).await.unwrap();
    assert_eq!(wikicraft::purge::purge_expired(&app.state).await.unwrap(), 0, "kept forever by default");
    let (st, s) = app.put(&owner, "/admin/settings", json!({"trashRetentionDays": 30})).await;
    assert_eq!((st, &s["trashRetentionDays"]), (200, &json!(30)), "{s}");
    assert_eq!(app.put(&owner, "/admin/settings", json!({"trashRetentionDays": -1})).await.0, 400);
    assert_eq!(wikicraft::purge::purge_expired(&app.state).await.unwrap(), 1);
    let (_, trash) = app.get(&owner, "/spaces/PUR/trash").await;
    let titles: Vec<&str> = trash.as_array().unwrap().iter().map(|t| t["title"].as_str().unwrap()).collect();
    assert_eq!(titles, ["Recent"]);

    // empty the trash
    assert_eq!(app.post(&editor, "/spaces/PUR/trash/empty", json!({})).await.0, 403);
    let (_, r) = app.post(&owner, "/spaces/PUR/trash/empty", json!({})).await;
    assert_eq!(r, json!({"purged": 1, "kept": 0}));
    let (_, trash) = app.get(&owner, "/spaces/PUR/trash").await;
    assert_eq!(trash, json!([]));
    let _ = space_id;
}

#[tokio::test(flavor = "multi_thread")]
async fn starred_spaces() {
    let app = common::spawn().await;
    let owner = app.setup().await;
    let (member, _) = app.user(&owner, "star@acme.test", "member").await;
    app.post(&owner, "/spaces", json!({"key": "AAA", "name": "Alpha"})).await;
    app.post(&owner, "/spaces", json!({"key": "ZZZ", "name": "Zulu"})).await;
    let starred = |c: &reqwest::Client| {
        let (app, c) = (&app, c.clone());
        async move {
            let (_, list) = app.get(&c, "/spaces").await;
            list.as_array().unwrap().iter().filter(|s| s["starred"] == true).map(|s| s["key"].as_str().unwrap().to_string()).collect::<Vec<_>>()
        }
    };
    assert_eq!(app.put(&member, "/spaces/ZZZ/star", json!({})).await.0, 204);
    assert_eq!(starred(&member).await, ["ZZZ"]);
    assert!(starred(&owner).await.is_empty(), "stars are personal");
    let (_, s) = app.get(&member, "/spaces/ZZZ").await;
    assert_eq!(s["starred"], true);
    assert_eq!(app.delete(&member, "/spaces/ZZZ/star").await, 204);
    assert!(starred(&member).await.is_empty());
}

#[tokio::test(flavor = "multi_thread")]
async fn space_owner_handover() {
    let app = common::spawn().await;
    let owner = app.setup().await;
    let (rita, rita_id) = app.user(&owner, "rita@acme.test", "member").await;
    let (_, gus_id) = app.user(&owner, "gus@acme.test", "guest").await;
    app.post(&owner, "/spaces", json!({"key": "OPS", "name": "Operations", "defaultRole": "editor"})).await;
    let (_, me) = app.get(&owner, "/me").await;
    let owner_id = me["user"]["id"].as_str().unwrap().to_string();

    // the creator owns a new space
    let (_, s) = app.get(&rita, "/spaces/OPS").await;
    assert_eq!(s["ownerId"], owner_id.as_str());
    assert_eq!(s["ownerName"], "Owner");
    assert_eq!(s["role"], "editor");

    // only space admins hand it over, to an active non-guest account
    assert_eq!(app.put(&rita, "/spaces/OPS/owner", json!({"userId": rita_id})).await.0, 403);
    assert_eq!(app.put(&owner, "/spaces/OPS/owner", json!({"userId": gus_id})).await.0, 400, "guests can't own spaces");
    assert_eq!(app.put(&owner, "/spaces/OPS/owner", json!({"userId": uuid::Uuid::nil()})).await.0, 400);
    let (st, s) = app.put(&owner, "/spaces/OPS/owner", json!({"userId": rita_id})).await;
    assert_eq!(st, 200, "{s}");
    assert_eq!(s["ownerName"], "rita");
    // the new owner is a space admin now and was told
    let (_, s) = app.get(&rita, "/spaces/OPS").await;
    assert_eq!(s["role"], "admin");
    let (_, n) = app.get(&rita, "/notifications").await;
    let item = n["items"].as_array().unwrap().iter().find(|i| i["kind"] == "space_owner").cloned().unwrap();
    assert_eq!(item["data"]["spaceKey"], "OPS");
    assert_eq!(item["actorName"], "Owner");

    // Admin → Spaces shows owners and flags spaces whose owner left
    let owner_row = |list: &Value| list.as_array().unwrap().iter().find(|s| s["key"] == "OPS").cloned().unwrap();
    let (_, list) = app.get(&owner, "/admin/spaces").await;
    assert_eq!(owner_row(&list)["ownerName"], "rita");
    assert_eq!(owner_row(&list)["ownerActive"], true);
    app.patch(&owner, &format!("/users/{rita_id}"), json!({"status": "deactivated"})).await;
    let (_, list) = app.get(&owner, "/admin/spaces").await;
    assert_eq!(owner_row(&list)["ownerActive"], false);
    assert_eq!(app.put(&owner, "/spaces/OPS/owner", json!({"userId": rita_id})).await.0, 400, "no active account");
    assert_eq!(app.put(&owner, "/spaces/OPS/owner", json!({"userId": owner_id})).await.0, 200, "taken back");

    // personal spaces stay with their person
    let (st, ps) = app.post(&owner, "/me/space", json!({})).await;
    assert_eq!(st, 201, "{ps}");
    let key = ps["key"].as_str().unwrap();
    assert_eq!(app.get(&owner, &format!("/spaces/{key}")).await.1["ownerId"], owner_id.as_str());
    assert_eq!(app.put(&owner, &format!("/spaces/{key}/owner"), json!({"userId": owner_id})).await.0, 400);
}

#[tokio::test(flavor = "multi_thread")]
async fn copying_a_whole_space() {
    let app = common::spawn().await;
    let owner = app.setup().await;
    let (rita, rita_id) = app.user(&owner, "rita@acme.test", "member").await;
    let (_, sp) = app.post(&owner, "/spaces", json!({"key": "SRC", "name": "Source", "defaultRole": "viewer"})).await;
    let home = sp["homePageId"].as_str().unwrap().to_string();
    app.put(&owner, "/spaces/SRC/permissions", json!({"grants": [{"subjectType": "user", "subjectId": rita_id, "role": "editor"}]})).await;
    let (_, me) = app.get(&owner, "/me").await;
    let owner_id = me["user"]["id"].as_str().unwrap().to_string();

    let mk = |title: &str, parent: &str| json!({"spaceKey": "SRC", "title": title, "parentId": parent});
    let (_, guide) = app.post(&owner, "/pages", mk("Guide", &home)).await;
    let guide = guide["id"].as_str().unwrap().to_string();
    let (_, setup) = app.post(&owner, "/pages", mk("Setup", &guide)).await;
    let setup = setup["id"].as_str().unwrap().to_string();
    let form = reqwest::multipart::Form::new().part("file", reqwest::multipart::Part::bytes(b"\x89PNG\r\n\x1a\nimg".to_vec()).file_name("shot.png"));
    let rows: Value = owner.post(app.url(&format!("/pages/{setup}/attachments"))).multipart(form).send().await.unwrap().json().await.unwrap();
    let att = rows[0]["id"].as_str().unwrap().to_string();
    app.put(&owner, &format!("/pages/{setup}/content"), json!({"markdown": format!("![shot](/api/attachments/{att}/content)")})).await;
    app.put(&owner, &format!("/pages/{guide}/content"), json!({"markdown": format!("Start with [the setup](/pages/{setup}).")})).await;
    app.put(&owner, &format!("/pages/{guide}/labels"), json!({"labels": ["howto"]})).await;
    app.put(&owner, &format!("/pages/{guide}/properties"), json!({"properties": [{"key": "Next", "type": "page", "value": setup}]})).await;
    let (_, secret) = app.post(&owner, "/pages", mk("Secret", &home)).await;
    let secret = secret["id"].as_str().unwrap().to_string();
    app.put(&owner, &format!("/pages/{secret}/restrictions"), json!({"restrictions": [{"kind": "view", "subjectType": "user", "subjectId": owner_id}]})).await;
    let (_, old) = app.post(&owner, "/pages", mk("Old stuff", &home)).await;
    app.post(&owner, &format!("/pages/{}/archive", old["id"].as_str().unwrap()), json!({})).await;
    let (st, _) = app.post(&rita, "/pages", json!({"spaceKey": "SRC", "title": "Rita's draft", "parentId": home, "draft": true})).await;
    assert_eq!(st, 201);
    let (st, tpl) = app.post(&owner, "/templates", json!({"name": "Ops checklist", "pageId": setup, "spaceKey": "SRC"})).await;
    assert_eq!(st, 201, "{tpl}");
    let tpl = tpl["id"].as_str().unwrap().to_string();
    app.patch(&owner, "/spaces/SRC", json!({"defaultTemplate": tpl})).await;
    // sidebar shortcuts: web addresses or wiki paths, checked
    for bad in [json!([{"title": "x", "url": "javascript:alert(1)"}]), json!([{"title": "", "url": "/pages"}]), json!([{"title": "x", "url": "//evil.test"}])] {
        assert_eq!(app.patch(&owner, "/spaces/SRC", json!({ "shortcuts": bad })).await.0, 400, "{bad}");
    }
    let too_many: Vec<Value> = (0..21).map(|i| json!({"title": format!("L{i}"), "url": "/spaces"})).collect();
    assert_eq!(app.patch(&owner, "/spaces/SRC", json!({ "shortcuts": too_many })).await.0, 400);
    assert_eq!(app.patch(&rita, "/spaces/SRC", json!({"shortcuts": []})).await.0, 403, "space admins only");
    let (st, s) = app
        .patch(
            &owner,
            "/spaces/SRC",
            json!({"shortcuts": [{"title": "  Setup  guide ", "url": format!("/pages/{setup}")}, {"title": "Status page", "url": "https://status.acme.test"}]}),
        )
        .await;
    assert_eq!(st, 200, "{s}");
    assert_eq!(s["shortcuts"][0], json!({"title": "Setup guide", "url": format!("/pages/{setup}")}));
    // the space's own look: an accent colour and a sidebar without the blog
    assert_eq!(s["look"], json!({}));
    for bad in [json!({"accentColor": "teal"}), json!({"hideBlog": "yes"}), json!({"font": "Comic"}), json!("dark")] {
        assert_eq!(app.patch(&owner, "/spaces/SRC", json!({ "look": bad })).await.0, 400, "{bad}");
    }
    assert_eq!(app.patch(&rita, "/spaces/SRC", json!({"look": {}})).await.0, 403);
    let (_, s) = app.patch(&owner, "/spaces/SRC", json!({"look": {"accentColor": "#0E7C66", "hideBlog": true}})).await;
    assert_eq!(s["look"], json!({"accentColor": "#0e7c66", "hideBlog": true}));
    let (_, s) = app.patch(&owner, "/spaces/SRC", json!({"look": {"accentColor": "", "hideBlog": false}})).await;
    assert_eq!(s["look"], json!({}), "empty and false go back to the defaults");
    app.patch(&owner, "/spaces/SRC", json!({"look": {"accentColor": "#0e7c66"}})).await;

    // only space admins copy a space
    assert_eq!(app.post(&rita, "/spaces/SRC/copy", json!({"key": "DST", "name": "Copy"})).await.0, 403);
    let (st, r) = app.post(&owner, "/spaces/SRC/copy", json!({"key": "DST", "name": "Copy"})).await;
    assert_eq!(st, 201, "{r}");
    assert_eq!(r["pages"], 4, "home, guide, setup, secret: no drafts, nothing archived");
    assert_eq!(app.post(&owner, "/spaces/SRC/copy", json!({"key": "DST", "name": "Again"})).await.0, 409);

    let (_, tree) = app.get(&owner, "/spaces/DST/tree").await;
    let nodes = tree["nodes"].as_array().unwrap();
    let id_of = |t: &str| nodes.iter().find(|n| n["title"] == t).map(|n| n["id"].as_str().unwrap().to_string()).unwrap_or_else(|| panic!("{t} missing"));
    let mut titles: Vec<&str> = nodes.iter().map(|n| n["title"].as_str().unwrap()).collect();
    titles.sort();
    assert_eq!(titles, vec!["Guide", "Secret", "Setup", "Source"]);
    assert_eq!(tree["homePageId"].as_str().unwrap(), id_of("Source"));
    // links, attachments and page properties point at the copies
    let (new_guide, new_setup) = (id_of("Guide"), id_of("Setup"));
    let (_, g) = app.get(&owner, &format!("/pages/{new_guide}")).await;
    assert!(g["content"].to_string().contains(&format!("/pages/{new_setup}")), "{}", g["content"]);
    assert_eq!(g["labels"], json!(["howto"]));
    assert_eq!(g["properties"][0]["value"], new_setup.as_str());
    let (_, s) = app.get(&owner, &format!("/pages/{new_setup}")).await;
    let (_, atts) = app.get(&owner, &format!("/pages/{new_setup}/attachments")).await;
    let new_att = atts[0]["id"].as_str().unwrap();
    assert_ne!(new_att, att);
    assert!(s["content"].to_string().contains(&format!("/api/attachments/{new_att}/")), "{}", s["content"]);
    // the original is untouched
    let (_, orig) = app.get(&owner, &format!("/pages/{guide}")).await;
    assert!(orig["content"].to_string().contains(&format!("/pages/{setup}")));
    // people, restrictions, settings and templates came along
    let (_, d) = app.get(&rita, "/spaces/DST").await;
    assert_eq!(d["role"], "editor");
    assert_eq!(d["defaultRole"], "viewer");
    assert_eq!(d["ownerId"], owner_id.as_str());
    assert_eq!(app.get(&rita, &format!("/pages/{}", id_of("Secret"))).await.0, 404, "restrictions are copied");
    let (_, templates) = app.get(&owner, "/templates?spaceKey=DST").await;
    let copied_tpl = templates.as_array().unwrap().iter().find(|t| t["name"] == "Ops checklist").cloned().unwrap();
    assert_ne!(copied_tpl["id"], tpl.as_str());
    assert_eq!(d["defaultTemplate"], copied_tpl["id"]);
    assert_eq!(
        d["shortcuts"],
        json!([{"title": "Setup guide", "url": format!("/pages/{new_setup}")}, {"title": "Status page", "url": "https://status.acme.test"}]),
        "shortcuts follow the copied pages"
    );
    assert_eq!(d["look"], json!({"accentColor": "#0e7c66"}));
}
