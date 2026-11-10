mod common;

use serde_json::{Value, json};

use common::spawn;

#[tokio::test(flavor = "multi_thread")]
async fn templates_create_pages() {
    let app = spawn().await;
    let c = app.setup().await;
    app.post(&c, "/spaces", json!({"key": "TPL", "name": "Templates"})).await;
    let (_, list) = app.get(&c, "/templates?spaceKey=TPL").await;
    assert!(list.as_array().unwrap().iter().any(|t| t["id"] == "builtin:meeting-notes"));
    let (st, page) = app.post(&c, "/pages", json!({"spaceKey": "TPL", "templateId": "builtin:meeting-notes"})).await;
    assert_eq!(st, 201);
    let (_, p) = app.get(&c, &format!("/pages/{}", page["id"].as_str().unwrap())).await;
    assert!(p["title"].as_str().unwrap().starts_with("Meeting notes 20"));
    assert!(p["content"].to_string().contains("Action items"));
    assert!(p["content"].to_string().contains("taskList"));
    // save a page as a space template and use it
    let (st, t) = app.post(&c, "/templates", json!({"name": "Incident review", "pageId": p["id"], "spaceKey": "TPL"})).await;
    assert_eq!(st, 201);
    let (_, page2) = app.post(&c, "/pages", json!({"spaceKey": "TPL", "title": "Incident 42", "templateId": t["id"]})).await;
    let (_, p2) = app.get(&c, &format!("/pages/{}", page2["id"].as_str().unwrap())).await;
    assert_eq!(p2["title"], "Incident 42");
    assert!(p2["content"].to_string().contains("Action items"));
}

#[tokio::test(flavor = "multi_thread")]
async fn notifications_for_mentions_and_watched_pages() {
    let app = spawn().await;
    let owner = app.setup().await;
    let (bob, bob_id) = app.user(&owner, "bob@acme.test", "member").await;
    let (_, sp) = app.post(&owner, "/spaces", json!({"key": "NT", "name": "Notify"})).await;
    let home = sp["homePageId"].as_str().unwrap().to_string();
    // mention in a comment
    let mention = json!({"type": "doc", "content": [{"type": "paragraph", "content": [{"type": "text", "text": "hey "}, {"type": "mention", "attrs": {"id": bob_id, "label": "bob"}}]}]});
    app.post(&owner, &format!("/pages/{home}/comments"), json!({"content": mention})).await;
    // mention in page content (persisted through the collaboration room)
    let (st, _) = app.put(&owner, &format!("/pages/{home}/content"), json!({"content": mention})).await;
    assert_eq!(st, 200);
    let n = app
        .eventually(|| async {
            let (_, n) = app.get(&bob, "/notifications").await;
            let kinds: Vec<String> = n["items"].as_array().unwrap().iter().map(|i| i["kind"].as_str().unwrap().to_string()).collect();
            (kinds.contains(&"mention".into()) && kinds.contains(&"page_mention".into())).then_some(n)
        })
        .await;
    assert!(n["unread"].as_u64().unwrap() >= 2);
    // bob now watches the page: an update by the owner notifies him
    let (_, page) = app.get(&bob, &format!("/pages/{home}")).await;
    assert_eq!(page["watching"], true);
    app.put(&owner, &format!("/pages/{home}/content"), json!({"markdown": "changed", "message": "update"})).await;
    let (_, n) = app.get(&bob, "/notifications").await;
    assert!(n["items"].as_array().unwrap().iter().any(|i| i["kind"] == "page_updated"));
    let r = bob.post(app.url("/notifications/read")).json(&json!({})).send().await.unwrap();
    assert_eq!(r.status(), 204);
    let (_, n) = app.get(&bob, "/notifications").await;
    assert_eq!(n["unread"], 0);
    // unwatch
    assert_eq!(app.delete(&bob, &format!("/pages/{home}/watch")).await, 204);
}

#[tokio::test(flavor = "multi_thread")]
async fn attachment_text_is_searchable_and_blog_and_views() {
    use std::io::Write;
    let app = spawn().await;
    let c = app.setup().await;
    let (_, sp) = app.post(&c, "/spaces", json!({"key": "DOCS", "name": "Docs"})).await;
    let home = sp["homePageId"].as_str().unwrap().to_string();
    let mut buf = std::io::Cursor::new(Vec::new());
    {
        let mut z = zip::ZipWriter::new(&mut buf);
        z.start_file("word/document.xml", zip::write::SimpleFileOptions::default()).unwrap();
        z.write_all(br#"<w:document><w:body><w:p><w:r><w:t>The xylophone procurement policy</w:t></w:r></w:p></w:body></w:document>"#).unwrap();
        z.finish().unwrap();
    }
    let form = reqwest::multipart::Form::new().part("file", reqwest::multipart::Part::bytes(buf.into_inner()).file_name("policy.docx"));
    let r = c.post(app.url(&format!("/pages/{home}/attachments"))).multipart(form).send().await.unwrap();
    assert_eq!(r.status(), 201);
    let hits = app
        .eventually(|| async {
            let (_, h) = app.get(&c, "/search?q=xylophone").await;
            (!h.as_array().unwrap().is_empty()).then_some(h)
        })
        .await;
    assert_eq!(hits[0]["pageId"], home.as_str());

    // blog posts are listed per space, not in the page tree
    let (_, b) = app.post(&c, "/pages", json!({"spaceKey": "DOCS", "title": "Release 1.0 is out", "kind": "blog", "markdown": "We shipped!"})).await;
    let (_, blog) = app.get(&c, "/spaces/DOCS/blog").await;
    assert_eq!(blog[0]["id"], b["id"]);
    assert_eq!(blog[0]["excerpt"], "We shipped!");
    let (_, tree) = app.get(&c, "/spaces/DOCS/tree").await;
    assert!(tree["nodes"].as_array().unwrap().iter().all(|n| n["id"] != b["id"]));

    // views are counted
    app.get(&c, &format!("/pages/{home}")).await;
    let (_, p) = app.get(&c, &format!("/pages/{home}")).await;
    assert!(p["views"].as_i64().unwrap() >= 2);
    assert_eq!(p["viewers"], 1);
}

#[tokio::test(flavor = "multi_thread")]
async fn opensearch_description() {
    let app = common::spawn().await;
    let _owner = app.setup().await;
    let r = common::client().get(format!("{}/opensearch.xml", app.base)).send().await.unwrap();
    assert_eq!(r.status(), 200);
    assert!(r.headers()["content-type"].to_str().unwrap().starts_with("application/opensearchdescription+xml"));
    let body = r.text().await.unwrap();
    assert!(body.contains(&format!("template=\"{}/search?q={{searchTerms}}\"", app.base)), "{body}");
    assert!(body.contains("<ShortName>"));
}

#[tokio::test(flavor = "multi_thread")]
async fn changed_since_last_visit() {
    let app = common::spawn().await;
    let owner = app.setup().await;
    let (member, _) = app.user(&owner, "mia@acme.test", "member").await;
    app.post(&owner, "/spaces", json!({"key": "CHG", "name": "Changes"})).await;
    let (_, pg) = app.post(&owner, "/pages", json!({"spaceKey": "CHG", "title": "Policy", "markdown": "v1"})).await;
    let pg = pg["id"].as_str().unwrap().to_string();
    // first visit: nothing to report
    let (_, p) = app.get(&member, &format!("/pages/{pg}")).await;
    assert!(p["changedSince"].is_null());
    let v1 = p["version"].as_i64().unwrap();
    // the owner edits twice
    for text in ["v2", "v3"] {
        let (st, _) = app.put(&owner, &format!("/pages/{pg}/content"), json!({"markdown": text})).await;
        assert_eq!(st, 200);
    }
    let (_, p) = app.get(&member, &format!("/pages/{pg}")).await;
    let c = &p["changedSince"];
    assert_eq!(c["version"].as_i64().unwrap(), v1, "{p}");
    assert_eq!(c["versions"], 2);
    assert_eq!(c["editors"], json!(["Owner"]));
    // reported once; the editor's own changes are never reported to them
    let (_, p) = app.get(&member, &format!("/pages/{pg}")).await;
    assert!(p["changedSince"].is_null());
    let (_, p) = app.get(&owner, &format!("/pages/{pg}")).await;
    assert!(p["changedSince"].is_null());
}

#[tokio::test(flavor = "multi_thread")]
async fn popular_pages() {
    let app = common::spawn().await;
    let owner = app.setup().await;
    let (member, _) = app.user(&owner, "pop@acme.test", "member").await;
    app.post(&owner, "/spaces", json!({"key": "POP", "name": "Popular"})).await;
    let mk = |t: &'static str| {
        let (app, owner) = (&app, &owner);
        async move { app.post(owner, "/pages", json!({"spaceKey": "POP", "title": t})).await.1["id"].as_str().unwrap().to_string() }
    };
    let (hot, warm, secret) = (mk("Hot page").await, mk("Warm page").await, mk("Secret page").await);
    for (c, id) in [(&owner, &hot), (&member, &hot), (&owner, &warm), (&owner, &secret), (&member, &warm), (&member, &warm)] {
        app.get(c, &format!("/pages/{id}")).await;
    }
    let (_, me) = app.get(&owner, "/me").await;
    let owner_id = me["user"]["id"].as_str().unwrap().to_string();
    app.put(&owner, &format!("/pages/{secret}/restrictions"), json!({"restrictions": [{"kind": "view", "subjectType": "user", "subjectId": owner_id}]})).await;
    let (_, list) = app.get(&member, "/popular").await;
    let titles: Vec<&str> = list.as_array().unwrap().iter().map(|r| r["title"].as_str().unwrap()).collect();
    assert_eq!(titles[..2], ["Warm page", "Hot page"], "by viewers, then views: {list}");
    assert!(!titles.contains(&"Secret page"));
    assert_eq!(list[0]["viewers"], 2);
    assert_eq!(list[0]["views"], 3);
}

#[tokio::test(flavor = "multi_thread")]
async fn search_filters_and_comments() {
    let app = common::spawn().await;
    let owner = app.setup().await;
    let (member, member_id) = app.user(&owner, "fil@acme.test", "member").await;
    app.post(&owner, "/spaces", json!({"key": "FLT", "name": "Filters"})).await;
    let (_, a) = app.post(&owner, "/pages", json!({"spaceKey": "FLT", "title": "Gateway timeouts", "markdown": "zircon gateway"})).await;
    let a = a["id"].as_str().unwrap().to_string();
    let (_, b) = app.post(&member, "/pages", json!({"spaceKey": "FLT", "title": "Gateway announcement", "kind": "blog", "markdown": "zircon release"})).await;
    let b = b["id"].as_str().unwrap().to_string();
    app.put(&owner, &format!("/pages/{a}/labels"), json!({"labels": ["incident"]})).await;
    // comments are searchable as part of their page
    app.post(&member, &format!("/pages/{a}/comments"), json!({"text": "seen again with obsidianwolf traffic"})).await;
    let hits = |q: String| {
        let (app, owner) = (&app, &owner);
        async move {
            let (_, r) = app.get(owner, &format!("/search?q={q}")).await;
            r.as_array().unwrap().iter().map(|h| h["title"].as_str().unwrap().to_string()).collect::<Vec<_>>()
        }
    };
    app.eventually(|| async { (hits("obsidianwolf".into()).await == vec!["Gateway timeouts"]).then_some(()) }).await;
    app.eventually(|| async { (hits("zircon".into()).await.len() == 2).then_some(()) }).await;
    assert_eq!(hits("zircon&kind=blog".into()).await, vec!["Gateway announcement"]);
    assert_eq!(hits("zircon&label=Incident".into()).await, vec!["Gateway timeouts"]);
    assert_eq!(hits(format!("zircon&contributor={member_id}")).await, vec!["Gateway announcement"]);
    assert_eq!(hits("zircon&days=7".into()).await.len(), 2);
    sqlx::query("UPDATE pages SET updated_at = now() - interval '30 days' WHERE id = $1::uuid").bind(&b).execute(&app.state.db).await.unwrap();
    assert_eq!(hits("zircon&days=7".into()).await, vec!["Gateway timeouts"]);
}

#[tokio::test(flavor = "multi_thread")]
async fn markdown_conversion_for_paste() {
    let app = common::spawn().await;
    let owner = app.setup().await;
    let (st, r) = app.post(&owner, "/markdown", json!({"markdown": "## Title\n\n- [ ] task\n\n| a | b |\n|---|---|\n| 1 | 2 |\n"})).await;
    assert_eq!(st, 200);
    let kinds: Vec<&str> = r["content"]["content"].as_array().unwrap().iter().map(|n| n["type"].as_str().unwrap()).collect();
    assert_eq!(kinds, vec!["heading", "taskList", "table"]);
    assert_ne!(app.post(&common::client(), "/markdown", json!({"markdown": "x"})).await.0, 200, "signed-in only");
}

#[tokio::test(flavor = "multi_thread")]
async fn versioned_api_path() {
    let app = common::spawn().await;
    let owner = app.setup().await;
    let r = owner.get(format!("{}/api/v1/me", app.base)).send().await.unwrap();
    assert_eq!(r.status(), 200);
    let me: serde_json::Value = r.json().await.unwrap();
    assert_eq!(me["user"]["email"], "owner@acme.test");
    let spec = reqwest::get(format!("{}/api/v1/openapi.yaml", app.base)).await.unwrap().text().await.unwrap();
    assert!(spec.contains("url: /api/v1"));
}

#[tokio::test(flavor = "multi_thread")]
async fn muted_notification_kinds() {
    let app = common::spawn().await;
    let owner = app.setup().await;
    let (member, _) = app.user(&owner, "mute@acme.test", "member").await;
    app.post(&owner, "/spaces", json!({"key": "MUTE", "name": "Mute"})).await;
    let (_, pg) = app.post(&member, "/pages", json!({"spaceKey": "MUTE", "title": "Watched"})).await;
    let pg = pg["id"].as_str().unwrap().to_string();
    app.put(&member, &format!("/pages/{pg}/like"), json!({})).await;
    let (_, prefs) = app.get(&member, "/me/preferences").await;
    assert!(prefs["kinds"].as_array().unwrap().iter().any(|k| k == "comment"));
    assert_eq!(app.put(&member, "/me/preferences", json!({"muted": ["bogus"]})).await.0, 400);
    let (st, prefs) = app.put(&member, "/me/preferences", json!({"muted": ["comment", "like"]})).await;
    assert_eq!(st, 200);
    assert_eq!(prefs["muted"], json!(["comment", "like"]));
    // the creator watches their page: comments would notify them
    app.post(&owner, &format!("/pages/{pg}/comments"), json!({"text": "Looks good"})).await;
    app.put(&owner, &format!("/pages/{pg}/like"), json!({})).await;
    let kinds = |v: &Value| v["items"].as_array().unwrap().iter().map(|i| i["kind"].as_str().unwrap().to_string()).collect::<Vec<_>>();
    let (_, n) = app.get(&member, "/notifications").await;
    assert!(!kinds(&n).contains(&"comment".to_string()) && !kinds(&n).contains(&"like".to_string()), "{n}");
    app.put(&member, "/me/preferences", json!({"muted": []})).await;
    let (_, n) = app.get(&member, "/notifications").await;
    assert!(kinds(&n).contains(&"comment".to_string()), "unmuting shows them again: {n}");
}

#[tokio::test(flavor = "multi_thread")]
async fn saved_searches() {
    let app = common::spawn().await;
    let owner = app.setup().await;
    let (other, _) = app.user(&owner, "ss@acme.test", "member").await;
    assert_eq!(app.post(&owner, "/me/searches", json!({"name": "x", "params": "space=ENG"})).await.0, 400, "needs a query");
    let (st, s) = app.post(&owner, "/me/searches", json!({"name": "Recent runbooks", "params": "?q=runbook&days=30&evil=1&label=ops"})).await;
    assert_eq!(st, 201);
    assert_eq!(s["params"], "q=runbook&days=30&label=ops", "unknown parameters are dropped");
    let (_, list) = app.get(&owner, "/me/searches").await;
    assert_eq!(list.as_array().unwrap().len(), 1);
    let (_, theirs) = app.get(&other, "/me/searches").await;
    assert_eq!(theirs.as_array().unwrap().len(), 0, "searches are personal");
    let id = s["id"].as_str().unwrap();
    assert_eq!(app.delete(&other, &format!("/me/searches/{id}")).await, 204);
    assert_eq!(app.get(&owner, "/me/searches").await.1.as_array().unwrap().len(), 1, "others can't delete it");
    assert_eq!(app.delete(&owner, &format!("/me/searches/{id}")).await, 204);
    assert_eq!(app.get(&owner, "/me/searches").await.1.as_array().unwrap().len(), 0);
}

#[tokio::test(flavor = "multi_thread")]
async fn watching_a_space() {
    let app = common::spawn().await;
    let owner = app.setup().await;
    let (fan, _) = app.user(&owner, "fan@acme.test", "member").await;
    app.post(&owner, "/spaces", json!({"key": "NEWS", "name": "News"})).await;
    let (_, s) = app.get(&fan, "/spaces/NEWS").await;
    assert_eq!(s["watching"], false);
    assert_eq!(app.put(&fan, "/spaces/NEWS/watch", json!({})).await.0, 204);
    let (_, s) = app.get(&fan, "/spaces/NEWS").await;
    assert_eq!(s["watching"], true);
    let (_, pg) = app.post(&owner, "/pages", json!({"spaceKey": "NEWS", "title": "Launch day"})).await;
    let pg = pg["id"].as_str().unwrap().to_string();
    app.put(&owner, &format!("/pages/{pg}/content"), json!({"markdown": "It's live."})).await;
    let (_, n) = app.get(&fan, "/notifications").await;
    let kinds: Vec<&str> = n["items"].as_array().unwrap().iter().map(|i| i["kind"].as_str().unwrap()).collect();
    assert!(kinds.contains(&"page_created") && kinds.contains(&"page_updated"), "{n}");
    // restricted pages stay invisible to space watchers
    let (_, me) = app.get(&owner, "/me").await;
    let owner_id = me["user"]["id"].as_str().unwrap().to_string();
    let (_, secret) = app.post(&owner, "/pages", json!({"spaceKey": "NEWS", "title": "Embargoed"})).await;
    let secret = secret["id"].as_str().unwrap().to_string();
    app.put(&owner, &format!("/pages/{secret}/restrictions"), json!({"restrictions": [{"kind": "view", "subjectType": "user", "subjectId": owner_id}]})).await;
    let (_, n) = app.get(&fan, "/notifications").await;
    assert!(!n.to_string().contains("Embargoed"));
    assert_eq!(app.delete(&fan, "/spaces/NEWS/watch").await, 204);
    let (_, s) = app.get(&fan, "/spaces/NEWS").await;
    assert_eq!(s["watching"], false);
}

#[tokio::test(flavor = "multi_thread")]
async fn space_default_template() {
    let app = common::spawn().await;
    let owner = app.setup().await;
    app.post(&owner, "/spaces", json!({"key": "MTG", "name": "Meetings"})).await;
    assert_eq!(app.patch(&owner, "/spaces/MTG", json!({"defaultTemplate": "builtin:nope"})).await.0, 400);
    let (st, s) = app.patch(&owner, "/spaces/MTG", json!({"defaultTemplate": "builtin:meeting-notes"})).await;
    assert_eq!(st, 200, "{s}");
    assert_eq!(s["defaultTemplate"], "builtin:meeting-notes");
    let (_, pg) = app.post(&owner, "/pages", json!({"spaceKey": "MTG", "title": "Sync"})).await;
    let (_, p) = app.get(&owner, &format!("/pages/{}", pg["id"].as_str().unwrap())).await;
    assert!(p["content"]["content"].as_array().unwrap().len() > 2, "starts from the template: {p}");
    let (_, pg) = app.post(&owner, "/pages", json!({"spaceKey": "MTG", "title": "Empty", "blank": true})).await;
    let (_, p) = app.get(&owner, &format!("/pages/{}", pg["id"].as_str().unwrap())).await;
    assert!(p["content"]["content"].as_array().unwrap().len() <= 1, "blank stays blank: {p}");
    app.patch(&owner, "/spaces/MTG", json!({"defaultTemplate": ""})).await;
    let (_, s) = app.get(&owner, "/spaces/MTG").await;
    assert!(s["defaultTemplate"].is_null());
}

#[tokio::test(flavor = "multi_thread")]
async fn page_ownership() {
    let app = common::spawn().await;
    let owner = app.setup().await;
    let (_, ed_id) = app.user(&owner, "ed@acme.test", "member").await;
    let (_, vi_id) = app.user(&owner, "vi@acme.test", "member").await;
    let (_, sp) = app.post(&owner, "/spaces", json!({"key": "OWN", "name": "Own", "defaultRole": "none"})).await;
    let _ = sp;
    app.put(
        &owner,
        "/spaces/OWN/permissions",
        json!({"grants": [{"subjectType": "user", "subjectId": ed_id, "role": "editor"}, {"subjectType": "user", "subjectId": vi_id, "role": "viewer"}]}),
    )
    .await;
    let (_, pg) = app.post(&owner, "/pages", json!({"spaceKey": "OWN", "title": "Policy"})).await;
    let pg = pg["id"].as_str().unwrap().to_string();
    let (_, p) = app.get(&owner, &format!("/pages/{pg}")).await;
    assert_eq!(p["ownerName"], "Owner", "the creator owns it at first");
    assert_eq!(app.put(&owner, &format!("/pages/{pg}/owner"), json!({"userId": vi_id})).await.0, 400, "viewers can't own it");
    let (st, r) = app.put(&owner, &format!("/pages/{pg}/owner"), json!({"userId": ed_id})).await;
    assert_eq!(st, 200, "{r}");
    let (_, p) = app.get(&owner, &format!("/pages/{pg}")).await;
    assert_eq!(p["ownerId"], ed_id.as_str());
    assert_eq!(p["ownerName"], "ed");
}
