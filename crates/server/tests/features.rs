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
    let (_, pg) = app.post(&owner, "/pages", json!({"spaceKey": "CHG", "title": "Policy", "markdown": "v1\n\nStays the same"})).await;
    let pg = pg["id"].as_str().unwrap().to_string();
    // first visit: nothing to report
    let (_, p) = app.get(&member, &format!("/pages/{pg}")).await;
    assert!(p["changedSince"].is_null());
    let v1 = p["version"].as_i64().unwrap();
    // the owner edits twice
    for text in ["v2\n\nStays the same", "v3\n\nStays the same\n\nA new rule"] {
        let (st, _) = app.put(&owner, &format!("/pages/{pg}/content"), json!({"markdown": text})).await;
        assert_eq!(st, 200);
    }
    let (_, p) = app.get(&member, &format!("/pages/{pg}")).await;
    let c = &p["changedSince"];
    assert_eq!(c["version"].as_i64().unwrap(), v1, "{p}");
    assert_eq!(c["versions"], 2);
    assert_eq!(c["editors"], json!(["Owner"]));
    // the blocks to highlight: the changed and the new paragraph, not the unchanged one
    let texts: Vec<String> = c["blocks"]
        .as_array()
        .unwrap()
        .iter()
        .map(|id| {
            let block = p["content"]["content"].as_array().unwrap().iter().find(|b| b["attrs"]["blockId"] == *id).unwrap_or_else(|| panic!("{id} not in {p}"));
            block["content"][0]["text"].as_str().unwrap().to_string()
        })
        .collect();
    assert_eq!(texts, vec!["v3", "A new rule"]);
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

    // label suggestions: most used first, counted over visible pages only
    app.put(&owner, &format!("/pages/{hot}/labels"), json!({"labels": ["runbook", "release"]})).await;
    app.put(&owner, &format!("/pages/{warm}/labels"), json!({"labels": ["runbook"]})).await;
    app.put(&owner, &format!("/pages/{secret}/labels"), json!({"labels": ["reorg-2027", "runbook"]})).await;
    let (_, mine) = app.get(&owner, "/labels?q=r").await;
    assert_eq!(mine, json!([{"name": "runbook", "count": 3}, {"name": "release", "count": 1}, {"name": "reorg-2027", "count": 1}]));
    let (_, theirs) = app.get(&member, "/labels?q=R").await;
    assert_eq!(theirs, json!([{"name": "runbook", "count": 2}, {"name": "release", "count": 1}]), "the restricted page's label stays hidden");
    let (_, none) = app.get(&member, "/labels?q=%25").await;
    assert_eq!(none.as_array().unwrap().len(), 2, "a % is no wildcard: {none}");
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
async fn template_for_pages_below_a_page() {
    let app = common::spawn().await;
    let owner = app.setup().await;
    let (viewer, _) = app.user(&owner, "vi@acme.test", "member").await;
    app.post(&owner, "/spaces", json!({"key": "ENG", "name": "Engineering", "defaultRole": "viewer"})).await;
    app.patch(&owner, "/spaces/ENG", json!({"defaultTemplate": "builtin:how-to"})).await;
    let (_, inc) = app.post(&owner, "/pages", json!({"spaceKey": "ENG", "title": "Incidents", "blank": true})).await;
    let inc = inc["id"].as_str().unwrap().to_string();
    let (_, y) = app.post(&owner, "/pages", json!({"spaceKey": "ENG", "title": "2026", "parentId": inc, "blank": true})).await;
    let year = y["id"].as_str().unwrap().to_string();

    // only editors set it, to a template that exists
    assert_ne!(app.patch(&viewer, &format!("/pages/{inc}"), json!({"childTemplate": "builtin:postmortem"})).await.0, 204);
    assert_eq!(app.patch(&owner, &format!("/pages/{inc}"), json!({"childTemplate": "builtin:nope"})).await.0, 400);
    assert_eq!(app.patch(&owner, &format!("/pages/{inc}"), json!({"childTemplate": "builtin:postmortem"})).await.0, 204);
    assert_eq!(app.get(&owner, &format!("/pages/{inc}")).await.1["childTemplate"], "builtin:postmortem");

    // the nearest page above decides, then the space
    let (_, d) = app.get(&owner, &format!("/spaces/ENG/default-template?parentId={year}")).await;
    assert_eq!(d["templateId"], "builtin:postmortem");
    assert_eq!(d["from"]["title"], "Incidents");
    let (_, d) = app.get(&owner, "/spaces/ENG/default-template").await;
    assert_eq!(d, json!({"templateId": "builtin:how-to", "from": "space"}));
    let text = |v: &Value| v["content"].to_string();
    let (_, pg) = app.post(&owner, "/pages", json!({"spaceKey": "ENG", "title": "Outage", "parentId": year})).await;
    let (_, p) = app.get(&owner, &format!("/pages/{}", pg["id"].as_str().unwrap())).await;
    let heading = "Root cause".to_string();
    assert!(text(&p).contains(&heading), "starts from the postmortem template: {p}");
    let (_, pg) = app.post(&owner, "/pages", json!({"spaceKey": "ENG", "title": "Blank one", "parentId": year, "blank": true})).await;
    let (_, p) = app.get(&owner, &format!("/pages/{}", pg["id"].as_str().unwrap())).await;
    assert!(!text(&p).contains(&heading), "blank stays blank");
    // copies keep it; clearing falls back to the space
    let (_, c) = app.post(&owner, &format!("/pages/{inc}/copy"), json!({})).await;
    assert_eq!(app.get(&owner, &format!("/pages/{}", c["id"].as_str().unwrap())).await.1["childTemplate"], "builtin:postmortem");
    app.patch(&owner, &format!("/pages/{inc}"), json!({"childTemplate": ""})).await;
    let (_, d) = app.get(&owner, &format!("/spaces/ENG/default-template?parentId={year}")).await;
    assert_eq!(d["templateId"], "builtin:how-to");
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

#[tokio::test(flavor = "multi_thread")]
async fn link_previews() {
    // a little site: an HTML page, a redirect to it, and a binary file
    let site = axum::Router::new()
        .route(
            "/post",
            axum::routing::get(|| async {
                (
                    [("content-type", "text/html; charset=utf-8")],
                    "<html><head><title>x</title><meta property=\"og:title\" content=\"Quarterly results\"><meta name=\"description\" content=\"Revenue up 12%\"></head></html>",
                )
            }),
        )
        .route("/old", axum::routing::get(|| async { (axum::http::StatusCode::MOVED_PERMANENTLY, [("location", "/post")]) }))
        .route("/file", axum::routing::get(|| async { ([("content-type", "application/zip")], vec![0u8; 10]) }));
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let base = format!("http://{}", listener.local_addr().unwrap());
    tokio::spawn(async move { axum::serve(listener, site).await.unwrap() });

    let app = common::spawn().await; // tests allow private targets
    let owner = app.setup().await;
    let (st, r) = app.get(&owner, &format!("/unfurl?url={base}/old")).await;
    assert_eq!(st, 200, "{r}");
    assert_eq!(r["title"], "Quarterly results");
    assert_eq!(r["description"], "Revenue up 12%");
    let (_, r) = app.get(&owner, &format!("/unfurl?url={base}/file")).await;
    assert!(r["title"].is_null() && r["error"].is_string());
    assert_eq!(app.get(&owner, "/unfurl?url=file:///etc/passwd").await.0, 400);
    assert_ne!(app.get(&common::client(), &format!("/unfurl?url={base}/post")).await.0, 200, "signed-in only");

    // with the default (no private targets) the local site is refused, even when webhooks may
    // reach private addresses, and the reason isn't disclosed
    let strict = common::spawn_with(|c| {
        c.unfurl_allow_private = false;
        c.webhook_allow_private = true;
    })
    .await;
    let o = strict.setup().await;
    let (_, r) = strict.get(&o, &format!("/unfurl?url={base}/post")).await;
    assert!(r["title"].is_null() && r["error"] == "preview unavailable", "{r}");
}

#[tokio::test]
async fn announcement_banner() {
    let app = common::spawn().await;
    let owner = app.setup().await;
    let (member, _) = app.user(&owner, "ann@acme.test", "member").await;
    assert_eq!(app.get(&member, "/announcement").await.1, Value::Null);

    let (st, a) = app.put(&owner, "/admin/announcement", json!({"text": "  Read-only on Saturday,\n see https://status.acme.test ", "level": "warning"})).await;
    assert_eq!(st, 200, "{a}");
    assert_eq!(a["text"], "Read-only on Saturday, see https://status.acme.test");
    let (_, seen) = app.get(&member, "/announcement").await;
    assert_eq!(seen, a, "everyone sees it");

    assert_eq!(app.put(&member, "/admin/announcement", json!({"text": "hi"})).await.0, 403, "admins only");
    assert_eq!(app.put(&owner, "/admin/announcement", json!({"text": "x", "level": "party"})).await.0, 400);
    assert_eq!(app.put(&owner, "/admin/announcement", json!({"text": "y".repeat(501)})).await.0, 400);
    assert_ne!(app.get(&common::client(), "/announcement").await.0, 200, "signed-in only");

    // a new text gets a new id (dismissals reset), an empty one removes it
    tokio::time::sleep(std::time::Duration::from_millis(5)).await;
    let (_, b) = app.put(&owner, "/admin/announcement", json!({"text": "Back to normal"})).await;
    assert_eq!(b["level"], "info");
    assert_ne!(b["id"], a["id"]);
    assert_eq!(app.put(&owner, "/admin/announcement", json!({"text": ""})).await.1, Value::Null);
    assert_eq!(app.get(&member, "/announcement").await.1, Value::Null);
}

#[tokio::test(flavor = "multi_thread")]
async fn feature_switches() {
    let app = common::spawn().await;
    let owner = app.setup().await;
    let (member, _) = app.user(&owner, "fs@acme.test", "member").await;
    app.post(&owner, "/spaces", json!({"key": "FS", "name": "Switches", "defaultRole": "editor"})).await;
    let (_, page) = app.post(&owner, "/pages", json!({"spaceKey": "FS", "title": "Plan", "markdown": "Ship it"})).await;
    let export = format!("/pages/{}/export?format=md", page["id"].as_str().unwrap());

    let (_, f) = app.get(&member, "/features").await;
    assert_eq!(
        f,
        json!({"publicLinks": true, "exports": true, "ai": false, "personalSpaces": true, "viewerList": false, "drawioUrl": "https://embed.diagrams.net", "maxFileMb": 512}),
        "on by default (the viewer list off); ai needs a key"
    );
    assert_eq!(app.get(&member, &export).await.0, 200);
    // exports are in the audit log
    let (_, audit) = app.get(&owner, "/admin/audit").await;
    let ev = audit.as_array().unwrap().iter().find(|e| e["action"] == "page.exported").cloned().unwrap_or_else(|| panic!("{audit}"));
    assert_eq!(ev["actorName"], "fs");
    assert_eq!(ev["data"]["format"], "md");

    assert_eq!(app.put(&member, "/admin/settings", json!({"exports": false})).await.0, 403);
    assert_eq!(app.put(&owner, "/admin/settings", json!({"export": false})).await.0, 400, "unknown switch");
    // an export started before the switch
    let (st, job) = app.post(&member, "/spaces/FS/export", json!({"format": "markdown"})).await;
    assert_eq!(st, 202, "{job}");
    let job = job["jobId"].as_str().unwrap().to_string();
    app.eventually(|| async { (app.get(&member, &format!("/jobs/{job}")).await.1["status"] == "succeeded").then_some(()) }).await;
    let (st, s) = app.put(&owner, "/admin/settings", json!({"exports": false, "personalSpaces": false})).await;
    assert_eq!(st, 200, "{s}");
    assert_eq!(
        s,
        json!({"publicLinks": true, "exports": false, "ai": true, "personalSpaces": false, "trashRetentionDays": 0, "viewerList": false, "requireTwoFactor": "off", "drawioUrl": "https://embed.diagrams.net",
            "attachments": {"maxFileMb": 512, "blockedTypes": [], "allowedTypes": [], "spaceQuotaMb": 0, "workspaceQuotaMb": 0},
            "serverMaxUploadMb": 512, "attachmentStorageBytes": 0})
    );

    // ... can't be downloaded after it; members can't export any more, admins still can
    assert_eq!(app.get(&member, &format!("/jobs/{job}/download")).await.0, 400);
    assert_eq!(app.get(&member, &export).await.0, 400);
    assert_eq!(app.post(&member, "/spaces/FS/export", json!({"format": "markdown"})).await.0, 400);
    assert_eq!(app.get(&owner, &export).await.0, 200);
    assert_eq!(app.get(&member, "/features").await.1["exports"], false);
    assert_eq!(app.get(&owner, "/features").await.1["exports"], true);
    // no new personal spaces
    assert_eq!(app.post(&member, "/me/space", json!({})).await.0, 400);

    app.put(&owner, "/admin/settings", json!({"exports": true, "personalSpaces": true})).await;
    assert_eq!(app.get(&member, &export).await.0, 200);
    assert_eq!(app.post(&member, "/me/space", json!({})).await.0, 201);
}

#[tokio::test]
async fn space_categories() {
    let app = common::spawn().await;
    let owner = app.setup().await;
    app.post(&owner, "/spaces", json!({"key": "CAT", "name": "Platform team"})).await;
    let (st, s) = app.patch(&owner, "/spaces/CAT", json!({"category": "  Teams "})).await;
    assert_eq!(st, 200, "{s}");
    assert_eq!(s["category"], "Teams");
    let (_, list) = app.get(&owner, "/spaces").await;
    assert_eq!(list.as_array().unwrap().iter().find(|s| s["key"] == "CAT").unwrap()["category"], "Teams");
    assert_eq!(app.patch(&owner, "/spaces/CAT", json!({"category": "x".repeat(61)})).await.0, 400);
    // other updates leave it alone; "" removes it
    assert_eq!(app.patch(&owner, "/spaces/CAT", json!({"name": "Platform"})).await.1["category"], "Teams");
    assert!(app.patch(&owner, "/spaces/CAT", json!({"category": ""})).await.1["category"].is_null());
}

#[tokio::test]
async fn workspace_name_and_logo() {
    let app = common::spawn().await;
    let owner = app.setup().await;
    let (member, _) = app.user(&owner, "wl@acme.test", "member").await;
    assert_eq!(app.put(&member, "/admin/workspace", json!({"name": "Hijack"})).await.0, 403);
    assert_eq!(app.put(&owner, "/admin/workspace", json!({"name": "  "})).await.0, 400);
    assert_eq!(app.put(&owner, "/admin/workspace", json!({"name": " Acme  Corp "})).await.1["name"], "Acme Corp");
    let (_, me) = app.get(&member, "/me").await;
    assert_eq!(me["workspace"]["name"], "Acme Corp");
    assert!(me["workspace"]["logoVersion"].is_null());
    assert_eq!(app.get(&common::client(), "/workspace/logo").await.0, 404);

    let upload = |c: &reqwest::Client, bytes: Vec<u8>, name: &str| {
        let form = reqwest::multipart::Form::new().part("file", reqwest::multipart::Part::bytes(bytes).file_name(name.to_string()));
        c.post(app.url("/admin/workspace/logo")).multipart(form).send()
    };
    let png = b"\x89PNG\r\n\x1a\n\0\0\0\rIHDR fake but typed".to_vec();
    let svg = b"<svg xmlns='http://www.w3.org/2000/svg'><script>alert(1)</script></svg>".to_vec();
    assert_eq!(upload(&member, png.clone(), "logo.png").await.unwrap().status(), 403);
    assert_eq!(upload(&owner, svg, "logo.svg").await.unwrap().status(), 400, "no SVG");
    assert_eq!(upload(&owner, png.clone(), "logo.png").await.unwrap().status(), 200);
    let (_, me) = app.get(&member, "/me").await;
    assert!(me["workspace"]["logoVersion"].is_string());
    // public, typed and not sniffable
    let r = common::client().get(app.url("/workspace/logo")).send().await.unwrap();
    assert_eq!(r.status(), 200);
    assert_eq!(r.headers()["content-type"], "image/png");
    assert_eq!(r.headers()["x-content-type-options"], "nosniff");
    assert_eq!(r.bytes().await.unwrap().to_vec(), png);
    assert_eq!(app.delete(&owner, "/admin/workspace/logo").await, 204);
    assert_eq!(app.get(&common::client(), "/workspace/logo").await.0, 404);
}

#[tokio::test]
async fn activity_feed() {
    let app = common::spawn().await;
    let owner = app.setup().await;
    let (member, _) = app.user(&owner, "act@acme.test", "member").await;
    app.post(&owner, "/spaces", json!({"key": "ACT", "name": "Activity", "defaultRole": "editor"})).await;
    let (_, a) = app.post(&owner, "/pages", json!({"spaceKey": "ACT", "title": "Roadmap", "markdown": "v1"})).await;
    let a = a["id"].as_str().unwrap().to_string();
    for text in ["v2", "v3"] {
        assert_eq!(app.put(&owner, &format!("/pages/{a}/content"), json!({"markdown": text})).await.0, 200);
    }
    let (st, c) = app.post(&member, &format!("/pages/{a}/comments"), json!({"text": "Looks good to me"})).await;
    assert_eq!(st, 201, "{c}");
    let (_, secret) = app.post(&owner, "/pages", json!({"spaceKey": "ACT", "title": "Salaries", "markdown": "x"})).await;
    let (_, me) = app.get(&owner, "/me").await;
    let owner_id = me["user"]["id"].as_str().unwrap();
    app.put(
        &owner,
        &format!("/pages/{}/restrictions", secret["id"].as_str().unwrap()),
        json!({"restrictions": [{"kind": "view", "subjectType": "user", "subjectId": owner_id}]}),
    )
    .await;

    let (st, feed) = app.get(&member, "/activity").await;
    assert_eq!(st, 200, "{feed}");
    let kinds: Vec<(String, String, i64)> = feed
        .as_array()
        .unwrap()
        .iter()
        .map(|e| (e["kind"].as_str().unwrap().into(), e["pageTitle"].as_str().unwrap().into(), e["count"].as_i64().unwrap()))
        .collect();
    assert_eq!(
        kinds,
        vec![
            ("commented".into(), "Roadmap".into(), 1),
            ("edited".into(), "Roadmap".into(), 2),
            ("created".into(), "Roadmap".into(), 1),
            ("created".into(), "Activity".into(), 1), // the space's home page
        ],
        "two edits folded, restricted page hidden"
    );
    assert_eq!(feed[0]["detail"], "Looks good to me");
    // watching: nothing until the member watches the page
    assert_eq!(app.get(&member, "/activity?scope=watching").await.1, json!([]));
    app.put(&member, &format!("/pages/{a}/watch"), json!({})).await;
    assert_eq!(app.get(&member, "/activity?scope=watching").await.1.as_array().unwrap().len(), 3);
    assert_ne!(app.get(&common::client(), "/activity").await.0, 200);
}

#[tokio::test]
async fn links_in_the_page_tree() {
    let app = common::spawn().await;
    let owner = app.setup().await;
    app.post(&owner, "/spaces", json!({"key": "LNK", "name": "Links"})).await;
    let (st, r) = app.post(&owner, "/pages", json!({"spaceKey": "LNK", "kind": "link", "url": "https://www.example.com/status"})).await;
    assert_eq!(st, 201, "{r}");
    let id = r["id"].as_str().unwrap();
    assert_eq!(app.post(&owner, "/pages", json!({"spaceKey": "LNK", "kind": "link", "url": "javascript:alert(1)"})).await.0, 400);
    assert_eq!(app.post(&owner, "/pages", json!({"spaceKey": "LNK", "kind": "link"})).await.0, 400);
    let (_, tree) = app.get(&owner, "/spaces/LNK/tree").await;
    let item = tree["nodes"].as_array().or(tree.as_array()).unwrap().iter().find(|n| n["id"] == id).cloned().unwrap_or_else(|| panic!("{tree}"));
    assert_eq!(item["kind"], "link");
    assert_eq!(item["title"], "example.com", "named after the site");
    assert_eq!(item["url"], "https://www.example.com/status");
    let (_, page) = app.get(&owner, &format!("/pages/{id}")).await;
    assert_eq!(page["content"]["content"][0]["type"], "embed", "the page shows a link preview");
}

/// Who viewed a page: off until an admin turns it on, then for the page's editors only.
#[tokio::test(flavor = "multi_thread")]
async fn page_viewer_list() {
    let app = common::spawn().await;
    let owner = app.setup().await;
    let (reader, _) = app.user(&owner, "reader@acme.test", "member").await;
    app.post(&owner, "/spaces", json!({"key": "VW", "name": "Views", "defaultRole": "viewer"})).await;
    let (_, page) = app.post(&owner, "/pages", json!({"spaceKey": "VW", "title": "Holiday policy"})).await;
    let id = page["id"].as_str().unwrap().to_string();
    app.get(&reader, &format!("/pages/{id}")).await;
    app.get(&reader, &format!("/pages/{id}")).await;
    app.get(&owner, &format!("/pages/{id}")).await;
    assert_eq!(app.get(&owner, &format!("/pages/{id}/viewers")).await.0, 403, "off by default");
    let (_, f) = app.get(&owner, "/features").await;
    assert_eq!(f["viewerList"], false);
    let (st, s) = app.put(&owner, "/admin/settings", json!({"viewerList": true})).await;
    assert_eq!((st, &s["viewerList"]), (200, &json!(true)));
    let (st, list) = app.get(&owner, &format!("/pages/{id}/viewers")).await;
    assert_eq!(st, 200, "{list}");
    let rows: Vec<(String, i64)> = list.as_array().unwrap().iter().map(|r| (r["name"].as_str().unwrap().to_string(), r["views"].as_i64().unwrap())).collect();
    assert!(rows.contains(&("reader".to_string(), 2)), "{list}");
    assert_eq!(rows.len(), 2);
    assert_eq!(app.get(&reader, &format!("/pages/{id}/viewers")).await.0, 403, "readers don't see who else read it");
}

/// Unpublished drafts: only the creator sees them (and what's below), space admins included;
/// nobody is notified until they're published.
#[tokio::test(flavor = "multi_thread")]
async fn unpublished_drafts() {
    let app = common::spawn().await;
    let owner = app.setup().await;
    let (_, me) = app.get(&owner, "/me").await;
    let owner_id = me["user"]["id"].as_str().unwrap().to_string();
    let (writer, _) = app.user(&owner, "writer@acme.test", "member").await;
    app.post(&owner, "/spaces", json!({"key": "DRF", "name": "Drafts", "defaultRole": "editor"})).await;
    let content = json!({"type": "doc", "content": [{"type": "paragraph", "content": [
        {"type": "text", "text": "zephyrine plan for "}, {"type": "mention", "attrs": {"id": owner_id, "label": "Owner"}}]}]});
    let (st, d) = app.post(&writer, "/pages", json!({"spaceKey": "DRF", "title": "Reorg proposal", "content": content, "draft": true})).await;
    assert_eq!(st, 201, "{d}");
    let draft = d["id"].as_str().unwrap().to_string();
    let (_, child) = app.post(&writer, "/pages", json!({"spaceKey": "DRF", "title": "Reorg appendix", "parentId": draft})).await;
    let child = child["id"].as_str().unwrap().to_string();

    // the writer sees it, marked as a draft
    let (_, p) = app.get(&writer, &format!("/pages/{draft}")).await;
    assert_eq!(p["draft"], true);
    let (_, tree) = app.get(&writer, "/spaces/DRF/tree").await;
    let mine: Vec<(String, bool)> = tree["nodes"].as_array().unwrap().iter().map(|n| (n["title"].as_str().unwrap().to_string(), n["draft"] == true)).collect();
    assert!(mine.contains(&("Reorg proposal".into(), true)) && mine.contains(&("Reorg appendix".into(), false)), "{tree}");
    let (_, drafts) = app.get(&writer, "/me/drafts").await;
    assert_eq!(drafts.as_array().unwrap().len(), 1);

    // nobody else does, not even the workspace owner; no notification yet
    for id in [&draft, &child] {
        assert_eq!(app.get(&owner, &format!("/pages/{id}")).await.0, 404);
    }
    let (_, tree) = app.get(&owner, "/spaces/DRF/tree").await;
    assert!(!tree.to_string().contains("Reorg"), "{tree}");
    app.eventually(|| async { (app.get(&writer, "/search?q=zephyrine").await.1.as_array().map(Vec::len) == Some(1)).then_some(()) }).await;
    let (_, hits) = app.get(&owner, "/search?q=zephyrine").await;
    assert_eq!(hits, json!([]), "not in anyone else's search");
    let mentions: i64 = sqlx::query_scalar("SELECT count(*) FROM notifications WHERE kind = 'page_mention'").fetch_one(&app.state.db).await.unwrap();
    assert_eq!(mentions, 0);
    assert_eq!(app.post(&owner, &format!("/pages/{draft}/publish"), json!({})).await.0, 404, "only its creator can publish it");

    // public links: a draft can't be shared, and a shared page's sub-pages skip drafts
    assert_eq!(app.post(&writer, &format!("/pages/{draft}/share-links"), json!({})).await.0, 400);
    let (_, parent) = app.post(&writer, "/pages", json!({"spaceKey": "DRF", "title": "Team page"})).await;
    let parent = parent["id"].as_str().unwrap().to_string();
    let (_, hidden) = app.post(&writer, "/pages", json!({"spaceKey": "DRF", "title": "Salary bands", "parentId": parent, "draft": true})).await;
    let hidden = hidden["id"].as_str().unwrap().to_string();
    let (st, link) = app.post(&writer, &format!("/pages/{parent}/share-links"), json!({"includeChildren": true})).await;
    assert_eq!(st, 201, "{link}");
    let token = link["token"].as_str().unwrap();
    let anon = reqwest::Client::new();
    assert_eq!(anon.get(app.url(&format!("/public/{token}/pages/{hidden}"))).send().await.unwrap().status(), 404);
    let shared = anon.get(app.url(&format!("/public/{token}"))).send().await.unwrap().text().await.unwrap();
    assert!(!shared.contains("Salary bands"), "{shared}");

    // published: everyone sees it and what's below, and the mention arrives
    assert_eq!(app.post(&writer, &format!("/pages/{draft}/publish"), json!({})).await.0, 204);
    assert_eq!(app.get(&owner, &format!("/pages/{draft}")).await.0, 200);
    assert_eq!(app.get(&owner, &format!("/pages/{child}")).await.0, 200);
    let mentions: i64 = sqlx::query_scalar("SELECT count(*) FROM notifications WHERE kind = 'page_mention'").fetch_one(&app.state.db).await.unwrap();
    assert_eq!(mentions, 1);
    let (_, versions) = app.get(&writer, &format!("/pages/{draft}/versions")).await;
    assert!(versions.to_string().contains("Published"), "{versions}");
    let (_, drafts) = app.get(&writer, "/me/drafts").await;
    let titles: Vec<&str> = drafts.as_array().unwrap().iter().map(|d| d["title"].as_str().unwrap()).collect();
    assert_eq!(titles, ["Salary bands"], "only the other draft is left");
    assert_eq!(app.post(&writer, &format!("/pages/{draft}/publish"), json!({})).await.0, 400, "already published");
}

/// Template previews; a space's templates are only for people who can see the space.
#[tokio::test(flavor = "multi_thread")]
async fn template_preview_and_space_templates() {
    let app = common::spawn().await;
    let owner = app.setup().await;
    let (outsider, _) = app.user(&owner, "out@acme.test", "member").await;
    app.post(&owner, "/spaces", json!({"key": "SEC", "name": "Secret", "defaultRole": "none"})).await;
    app.post(&outsider, "/spaces", json!({"key": "OPEN", "name": "Open"})).await;
    let (_, p) = app.post(&owner, "/pages", json!({"spaceKey": "SEC", "title": "Merger checklist", "markdown": "# Merger\n\nconfidential-term-sheet"})).await;
    let (st, t) = app.post(&owner, "/templates", json!({"name": "Merger checklist", "pageId": p["id"], "spaceKey": "SEC"})).await;
    assert_eq!(st, 201, "{t}");
    let tid = t["id"].as_str().unwrap().to_string();

    let (st, pv) = app.get(&owner, &format!("/templates/{tid}/preview")).await;
    assert_eq!(st, 200, "{pv}");
    assert!(pv["html"].as_str().unwrap().contains("confidential-term-sheet"));
    let (_, builtin) = app.get(&outsider, "/templates/builtin:meeting-notes/preview").await;
    assert!(builtin["html"].as_str().unwrap_or_default().contains("<h2"), "{builtin}");

    // the outsider can't list, preview or use it
    let (_, list) = app.get(&outsider, "/templates?spaceKey=SEC").await;
    assert!(!list.to_string().contains("Merger"), "{list}");
    assert_eq!(app.get(&outsider, &format!("/templates/{tid}/preview")).await.0, 404);
    let (st, _) = app.post(&outsider, "/pages", json!({"spaceKey": "OPEN", "title": "Copy", "templateId": tid})).await;
    assert_eq!(st, 404, "a template from a space you can't see");
}

/// Editing a page means watching it, unless you stopped watching it on purpose.
#[tokio::test(flavor = "multi_thread")]
async fn editors_watch_what_they_edit() {
    let app = common::spawn().await;
    let owner = app.setup().await;
    let (editor, _) = app.user(&owner, "ed@acme.test", "member").await;
    app.post(&owner, "/spaces", json!({"key": "AW", "name": "Auto watch", "defaultRole": "editor"})).await;
    let (_, page) = app.post(&owner, "/pages", json!({"spaceKey": "AW", "title": "Runbook", "markdown": "v1"})).await;
    let id = page["id"].as_str().unwrap().to_string();
    let watching = |c: &reqwest::Client| {
        let (app, c, id) = (&app, c.clone(), id.clone());
        async move { app.get(&c, &format!("/pages/{id}")).await.1["watching"] == true }
    };
    assert!(!watching(&editor).await);
    assert_eq!(app.put(&editor, &format!("/pages/{id}/content"), json!({"markdown": "v2"})).await.0, 200);
    assert!(watching(&editor).await, "editing signs you up");
    assert_eq!(app.delete(&editor, &format!("/pages/{id}/watch")).await, 204);
    app.put(&editor, &format!("/pages/{id}/content"), json!({"markdown": "v3"})).await;
    assert!(!watching(&editor).await, "stopping on purpose sticks");
    app.put(&editor, &format!("/pages/{id}/watch"), json!({})).await;
    assert!(watching(&editor).await);
}

#[tokio::test(flavor = "multi_thread")]
async fn watching_a_page_and_the_pages_below() {
    let app = common::spawn().await;
    let owner = app.setup().await;
    app.post(&owner, "/spaces", json!({"key": "SUB", "name": "Subtree"})).await;
    let (bob, _) = app.user(&owner, "bob@acme.test", "member").await;
    let page = |title: &'static str, parent: Option<String>| {
        let owner = owner.clone();
        let app = &app;
        async move {
            let (_, p) = app.post(&owner, "/pages", json!({"spaceKey": "SUB", "title": title, "parentId": parent})).await;
            p["id"].as_str().unwrap().to_string()
        }
    };
    let guide = page("Guide", None).await;
    let chapter = page("Chapter", Some(guide.clone())).await;
    let section = page("Section", Some(chapter.clone())).await;
    let elsewhere = page("Elsewhere", None).await;
    let bobs = |kind: &'static str| {
        let bob = bob.clone();
        let app = &app;
        async move {
            let (_, n) = app.get(&bob, "/notifications").await;
            n["items"].as_array().unwrap().iter().filter(|n| n["kind"] == kind).map(|n| n["pageId"].as_str().unwrap().to_string()).collect::<Vec<_>>()
        }
    };

    // a plain watch (no body) is just the page
    let (st, _) = app.put(&bob, &format!("/pages/{guide}/watch"), json!({})).await;
    assert_eq!(st, 204);
    app.put(&owner, &format!("/pages/{section}/content"), json!({"markdown": "one"})).await;
    assert!(bobs("page_updated").await.is_empty());

    // with the pages below: changes anywhere below and new pages there
    let (st, _) = app.put(&bob, &format!("/pages/{guide}/watch"), json!({"includeChildren": true})).await;
    assert_eq!(st, 204);
    let (_, s) = app.get(&bob, &format!("/pages/{section}")).await;
    assert_eq!((s["watching"].as_bool(), s["watchedThrough"]["id"].as_str()), (Some(false), Some(guide.as_str())));
    let (_, g) = app.get(&bob, &format!("/pages/{guide}")).await;
    assert_eq!(g["watchingChildren"], true);
    app.put(&owner, &format!("/pages/{section}/content"), json!({"markdown": "two"})).await;
    app.put(&owner, &format!("/pages/{elsewhere}/content"), json!({"markdown": "not watched"})).await;
    assert_eq!(bobs("page_updated").await, std::slice::from_ref(&section));
    let appendix = page("Appendix", Some(chapter.clone())).await;
    assert_eq!(bobs("page_created").await, std::slice::from_ref(&appendix));

    // stopping to watch one page below opts out of just that page
    app.delete(&bob, &format!("/pages/{section}/watch")).await;
    app.put(&owner, &format!("/pages/{section}/content"), json!({"markdown": "three"})).await;
    app.put(&owner, &format!("/pages/{chapter}/content"), json!({"markdown": "chapter"})).await;
    assert_eq!(bobs("page_updated").await.iter().filter(|p| **p == section).count(), 1);
    assert!(bobs("page_updated").await.contains(&chapter));

    // a minor change through the API notifies nobody
    let before = bobs("page_updated").await.len();
    app.put(&owner, &format!("/pages/{chapter}/content"), json!({"markdown": "chapter, typo fixed", "minor": true})).await;
    assert_eq!(bobs("page_updated").await.len(), before);
}

#[tokio::test(flavor = "multi_thread")]
async fn people_directory_and_profiles() {
    let app = common::spawn().await;
    let owner = app.setup().await;
    let (bob, bob_id) = app.user(&owner, "bob@acme.test", "member").await;
    let (guest, _) = app.user(&owner, "gus@acme.test", "guest").await;
    let (_, me) = app.get(&owner, "/me").await;
    let owner_id = me["user"]["id"].as_str().unwrap().to_string();

    let (st, p) = app
        .put(
            &owner,
            "/me/profile",
            json!({"jobTitle": "CTO", "department": "Engineering", "location": "Zurich", "timezone": "Europe/Zurich", "bio": "Builds wikis."}),
        )
        .await;
    assert_eq!(st, 200, "{p}");
    assert_eq!(p["jobTitle"], "CTO");
    app.put(&bob, "/me/profile", json!({"jobTitle": "Developer", "department": "engineering"})).await;
    for bad in [json!({"timezone": "not a zone"}), json!({"jobTitle": "x".repeat(101)})] {
        let (st, _) = app.put(&bob, "/me/profile", bad).await;
        assert_eq!(st, 400);
    }
    // empty clears a field, fields left out stay
    app.put(&bob, "/me/profile", json!({"location": "Bern"})).await;
    let (_, p) = app.put(&bob, "/me/profile", json!({"location": ""})).await;
    assert_eq!((p["location"].clone(), p["jobTitle"].clone()), (serde_json::Value::Null, json!("Developer")));

    // the directory: search, departments (case-insensitive), no e-mail addresses
    let (_, d) = app.get(&bob, "/people").await;
    let names: Vec<&str> = d["people"].as_array().unwrap().iter().map(|p| p["name"].as_str().unwrap()).collect();
    assert!(names.contains(&"Owner") && names.contains(&"bob"), "{names:?}");
    assert!(d["people"][0].get("email").is_none());
    assert_eq!(d["departments"], json!([{"name": "Engineering", "count": 2}]));
    let (_, d) = app.get(&bob, "/people?q=zurich").await;
    assert_eq!(d["people"].as_array().unwrap().len(), 1);
    let (_, d) = app.get(&bob, "/people?department=ENGINEERING").await;
    assert_eq!(d["people"].as_array().unwrap().len(), 2);
    let (_, d) = app.get(&guest, "/people").await;
    assert!(d["people"].as_array().unwrap().is_empty(), "guests don't get the directory");

    // a profile lists what the person edited lately, only what the viewer can see
    app.post(&owner, "/spaces", json!({"key": "PPL", "name": "People"})).await;
    let (_, open) = app.post(&owner, "/pages", json!({"spaceKey": "PPL", "title": "Open notes"})).await;
    let (_, secret) = app.post(&owner, "/pages", json!({"spaceKey": "PPL", "title": "Salary review"})).await;
    let secret = secret["id"].as_str().unwrap().to_string();
    app.put(&owner, &format!("/pages/{secret}/restrictions"), json!({"restrictions": [{"kind": "view", "subjectType": "user", "subjectId": owner_id}]})).await;
    let (st, prof) = app.get(&bob, &format!("/people/{owner_id}")).await;
    assert_eq!(st, 200);
    assert_eq!(prof["bio"], "Builds wikis.");
    assert!(prof["email"].is_null(), "only admins and the person see the address");
    let titles: Vec<&str> = prof["recentPages"].as_array().unwrap().iter().map(|p| p["title"].as_str().unwrap()).collect();
    assert!(titles.contains(&"Open notes") && !titles.contains(&"Salary review"), "{titles:?}");
    assert_eq!(prof["recentPages"][0]["spaceKey"], "PPL");
    let _ = open;
    let (_, own) = app.get(&bob, &format!("/people/{bob_id}")).await;
    assert_eq!(own["email"], "bob@acme.test");
    let (st, _) = app.get(&guest, &format!("/people/{owner_id}")).await;
    assert_eq!(st, 404);
}

#[tokio::test(flavor = "multi_thread")]
async fn scheduled_publishing() {
    let app = common::spawn().await;
    let owner = app.setup().await;
    app.post(&owner, "/spaces", json!({"key": "PUB", "name": "Publishing", "defaultRole": "editor"})).await;
    let (bob, _) = app.user(&owner, "bob@acme.test", "member").await;
    let (_, d) = app.post(&owner, "/pages", json!({"spaceKey": "PUB", "title": "Launch post", "draft": true})).await;
    let draft = d["id"].as_str().unwrap().to_string();
    let soon = |secs: i64| (chrono::Utc::now() + chrono::Duration::seconds(secs)).to_rfc3339();

    for at in [soon(-60), soon(400 * 24 * 3600)] {
        let (st, _) = app.put(&owner, &format!("/pages/{draft}/publish-schedule"), json!({ "at": at })).await;
        assert_eq!(st, 400, "{at}");
    }
    let (st, _) = app.put(&bob, &format!("/pages/{draft}/publish-schedule"), json!({ "at": soon(3600) })).await;
    assert!(st >= 400, "someone else's draft");
    let (st, r) = app.put(&owner, &format!("/pages/{draft}/publish-schedule"), json!({ "at": soon(3600) })).await;
    assert_eq!(st, 200, "{r}");
    let (_, p) = app.get(&owner, &format!("/pages/{draft}")).await;
    assert!(p["publishAt"].is_string() && p["draft"] == true);
    // not due yet: nothing happens
    assert_eq!(wikicraft::routes::pages::publish_scheduled(&app.state).await.unwrap(), 0);

    // its time has come
    sqlx::query("UPDATE pages SET publish_at = now() - interval '1 second' WHERE id = $1::uuid").bind(&draft).execute(&app.state.db).await.unwrap();
    assert_eq!(wikicraft::routes::pages::publish_scheduled(&app.state).await.unwrap(), 1);
    let (st, p) = app.get(&bob, &format!("/pages/{draft}")).await;
    assert_eq!(st, 200, "everybody sees it now");
    assert_eq!((p["draft"].as_bool(), p["publishAt"].is_null()), (Some(false), true));
    let (_, versions) = app.get(&owner, &format!("/pages/{draft}/versions")).await;
    assert!(versions.as_array().unwrap().iter().any(|v| v["message"] == "Published"));

    // cancelled schedules stay drafts
    let (_, d2) = app.post(&owner, "/pages", json!({"spaceKey": "PUB", "title": "Maybe later", "draft": true})).await;
    let d2 = d2["id"].as_str().unwrap().to_string();
    app.put(&owner, &format!("/pages/{d2}/publish-schedule"), json!({ "at": soon(3600) })).await;
    assert_eq!(app.delete(&owner, &format!("/pages/{d2}/publish-schedule")).await, 204);
    let (_, p) = app.get(&owner, &format!("/pages/{d2}")).await;
    assert!(p["publishAt"].is_null() && p["draft"] == true);

    // an author who can't edit there any more doesn't publish
    let (_, d3) = app.post(&bob, "/pages", json!({"spaceKey": "PUB", "title": "Bob's post", "draft": true})).await;
    let d3 = d3["id"].as_str().unwrap().to_string();
    let (st, _) = app.put(&bob, &format!("/pages/{d3}/publish-schedule"), json!({ "at": soon(3600) })).await;
    assert_eq!(st, 200);
    app.patch(&owner, "/spaces/PUB", json!({"defaultRole": "viewer"})).await;
    sqlx::query("UPDATE pages SET publish_at = now() - interval '1 second' WHERE id = $1::uuid").bind(&d3).execute(&app.state.db).await.unwrap();
    assert_eq!(wikicraft::routes::pages::publish_scheduled(&app.state).await.unwrap(), 0);
    let draft_still: bool = sqlx::query_scalar("SELECT draft FROM pages WHERE id = $1::uuid").bind(&d3).fetch_one(&app.state.db).await.unwrap();
    assert!(draft_still);
}

#[tokio::test(flavor = "multi_thread")]
async fn profile_pictures() {
    let app = common::spawn().await;
    let owner = app.setup().await;
    let (bob, _) = app.user(&owner, "bob@acme.test", "member").await;
    let (_, me) = app.get(&owner, "/me").await;
    let owner_id = me["user"]["id"].as_str().unwrap().to_string();
    let upload = |bytes: Vec<u8>, name: &'static str| {
        let (app, owner) = (&app, &owner);
        async move {
            let form = reqwest::multipart::Form::new().part("file", reqwest::multipart::Part::bytes(bytes).file_name(name));
            let r = owner.post(app.url("/me/avatar")).multipart(form).send().await.unwrap();
            (r.status().as_u16(), r.json::<serde_json::Value>().await.unwrap_or_default())
        }
    };
    // a 600×400 photo becomes a 256×256 PNG
    let img = image::RgbImage::from_fn(600, 400, |x, y| image::Rgb([(x % 256) as u8, (y % 256) as u8, 128]));
    let mut png = std::io::Cursor::new(Vec::new());
    image::DynamicImage::ImageRgb8(img).write_to(&mut png, image::ImageFormat::Png).unwrap();
    let (st, r) = upload(png.into_inner(), "me.png").await;
    assert_eq!(st, 200, "{r}");
    let url = r["avatarUrl"].as_str().unwrap().to_string();
    assert!(url.starts_with(&format!("/api/users/{owner_id}/avatar?v=")));
    let res = bob.get(format!("{}{url}", app.base)).send().await.unwrap();
    assert_eq!(res.status(), 200);
    assert_eq!(res.headers()["content-type"], "image/png");
    let served = image::load_from_memory(&res.bytes().await.unwrap()).unwrap();
    assert_eq!((served.width(), served.height()), (256, 256));
    let (_, map) = app.get(&bob, "/people/avatars").await;
    assert_eq!(map[&owner_id], url.as_str());

    // not a picture, whatever its name says
    let (st, _) = upload(b"<html><script>alert(1)</script></html>".to_vec(), "evil.png").await;
    assert_eq!(st, 400);

    // removed: initials again
    assert_eq!(app.delete(&owner, "/me/avatar").await, 204);
    let (_, map) = app.get(&bob, "/people/avatars").await;
    assert!(map.get(&owner_id).is_none());
    let res = bob.get(format!("{}/api/users/{owner_id}/avatar", app.base)).send().await.unwrap();
    assert_eq!(res.status(), 404);
}

#[tokio::test(flavor = "multi_thread")]
async fn drawio_setting() {
    let app = common::spawn().await;
    let owner = app.setup().await;
    let (_, f) = app.get(&owner, "/features").await;
    assert_eq!(f["drawioUrl"], "https://embed.diagrams.net", "diagrams.net by default");
    for bad in ["http://draw.example", "https://draw.example/?x=1", "javascript:alert(1)"] {
        let (st, _) = app.put(&owner, "/admin/settings", json!({"drawioUrl": bad})).await;
        assert_eq!(st, 400, "{bad}");
    }
    let (st, s) = app.put(&owner, "/admin/settings", json!({"drawioUrl": "https://draw.acme.test/"})).await;
    assert_eq!(st, 200);
    assert_eq!(s["drawioUrl"], "https://draw.acme.test");
    let (_, f) = app.get(&owner, "/features").await;
    assert_eq!(f["drawioUrl"], "https://draw.acme.test");
    app.put(&owner, "/admin/settings", json!({"drawioUrl": ""})).await;
    let (_, f) = app.get(&owner, "/features").await;
    assert!(f["drawioUrl"].is_null(), "off");
}

#[tokio::test(flavor = "multi_thread")]
async fn task_reminders_on_the_due_date() {
    let app = common::spawn().await;
    let owner = app.setup().await;
    app.post(&owner, "/spaces", json!({"key": "TSK", "name": "Tasks"})).await;
    let (bob, bob_id) = app.user(&owner, "bob@acme.test", "member").await;
    let (st, _) = app.put(&bob, "/me/profile", json!({"timezone": "Mars/Olympus"})).await;
    assert_eq!(st, 400, "time zones the database doesn't know are refused");
    // a time zone where it's past 8 in the morning right now, so today's tasks are due
    let tz: String = sqlx::query_scalar(
        "SELECT name FROM pg_timezone_names WHERE name LIKE '%/%' AND extract(hour FROM now() AT TIME ZONE name) BETWEEN 9 AND 22 ORDER BY name LIMIT 1",
    )
    .fetch_one(&app.state.db)
    .await
    .unwrap();
    let (st, _) = app.put(&bob, "/me/profile", json!({"timezone": tz})).await;
    assert_eq!(st, 200);
    let today: String = sqlx::query_scalar("SELECT ((now() AT TIME ZONE $1)::date)::text").bind(&tz).fetch_one(&app.state.db).await.unwrap();
    let task = |text: &str, due: &str, done: bool| {
        json!({"type": "taskItem", "attrs": {"checked": done}, "content": [{"type": "paragraph", "content": [
            {"type": "text", "text": format!("{text} ")}, {"type": "mention", "attrs": {"id": bob_id, "label": "bob"}}, {"type": "date", "attrs": {"date": due}}
        ]}]})
    };
    let doc = json!({"type": "doc", "content": [{"type": "taskList", "content": [
        task("Send the invoice", &today, false),
        task("Already done", &today, true),
        task("Next month", "2099-01-01", false),
        task("Long overdue", "2020-01-01", false),
    ]}]});
    app.post(&owner, "/pages", json!({"spaceKey": "TSK", "title": "Billing", "content": doc})).await;

    assert_eq!(wikicraft::routes::tasks::remind_due_tasks(&app.state).await.unwrap(), 1, "only the open task due today");
    assert_eq!(wikicraft::routes::tasks::remind_due_tasks(&app.state).await.unwrap(), 0, "once per task and due date");
    let (_, n) = app.get(&bob, "/notifications").await;
    let due: Vec<&serde_json::Value> = n["items"].as_array().unwrap().iter().filter(|i| i["kind"] == "task_due").collect();
    assert_eq!(due.len(), 1);
    assert_eq!(due[0]["data"]["text"], "Send the invoice");
    assert_eq!(due[0]["pageTitle"], "Billing");
    // muted: not listed
    app.put(&bob, "/me/preferences", json!({"muted": ["task_due"]})).await;
    let (_, n) = app.get(&bob, "/notifications").await;
    assert!(n["items"].as_array().unwrap().iter().all(|i| i["kind"] != "task_due"));
}

#[tokio::test(flavor = "multi_thread")]
async fn blog_atom_feed() {
    let app = common::spawn().await;
    let owner = app.setup().await;
    app.post(&owner, "/spaces", json!({"key": "NEWS", "name": "News & Updates"})).await;
    let (_, post) = app
        .post(&owner, "/pages", json!({"spaceKey": "NEWS", "kind": "blog", "title": "Release <2.0>", "markdown": "We shipped. See [the notes](/pages/123)."}))
        .await;
    app.post(&owner, "/pages", json!({"spaceKey": "NEWS", "kind": "blog", "title": "Not yet", "markdown": "secret", "draft": true})).await;
    let feed_url = format!("{}/api/spaces/NEWS/blog.atom", app.base);

    let r = owner.get(&feed_url).send().await.unwrap();
    assert_eq!(r.status(), 200);
    assert!(r.headers()["content-type"].to_str().unwrap().starts_with("application/atom+xml"));
    let xml = r.text().await.unwrap();
    assert!(xml.starts_with("<?xml") && xml.contains("<feed xmlns=\"http://www.w3.org/2005/Atom\">"));
    assert!(xml.contains("<title>News &amp; Updates · Blog</title>"));
    assert!(xml.contains("<title>Release &lt;2.0&gt;</title>"), "titles are escaped");
    assert!(xml.contains(&format!("{}/pages/{}", app.base, post["id"].as_str().unwrap())));
    assert!(xml.contains(&format!("href=&quot;{}/pages/123&quot;", app.base)), "links are absolute: {xml}");
    assert!(!xml.contains("Not yet"), "no drafts");

    // a private space has no anonymous feed; a public one has
    assert!(reqwest::get(&feed_url).await.unwrap().status().is_client_error());
    let (st, _) = app.patch(&owner, "/spaces/NEWS", json!({"anonymousAccess": true})).await;
    assert_eq!(st, 200);
    let r = reqwest::get(&feed_url).await.unwrap();
    assert_eq!(r.status(), 200);
    assert!(r.text().await.unwrap().contains("We shipped."));
}

#[tokio::test(flavor = "multi_thread")]
async fn copying_pages_to_another_space_with_their_attachments() {
    let app = common::spawn().await;
    let owner = app.setup().await;
    app.post(&owner, "/spaces", json!({"key": "SRC", "name": "Source"})).await;
    app.post(&owner, "/spaces", json!({"key": "DST", "name": "Destination", "defaultRole": "viewer"})).await;
    let (bob, _) = app.user(&owner, "bob@acme.test", "member").await;
    let (_, page) = app.post(&owner, "/pages", json!({"spaceKey": "SRC", "title": "Diagram page", "blank": true})).await;
    let page = page["id"].as_str().unwrap().to_string();
    let form = reqwest::multipart::Form::new().part("file", reqwest::multipart::Part::bytes(b"\x89PNG\r\n\x1a\nfake".to_vec()).file_name("arch.png"));
    let up: serde_json::Value = owner.post(app.url(&format!("/pages/{page}/attachments"))).multipart(form).send().await.unwrap().json().await.unwrap();
    let att = up[0]["id"].as_str().unwrap().to_string();
    let content =
        json!({"type": "doc", "content": [{"type": "paragraph", "content": [{"type": "image", "attrs": {"src": format!("/api/attachments/{att}/content")}}]}]});
    app.put(&owner, &format!("/pages/{page}/content"), json!({ "content": content })).await;
    app.post(&owner, "/pages", json!({"spaceKey": "SRC", "title": "Child", "parentId": page, "blank": true})).await;

    // into another space: the copy has its own attachment, sharing the stored file
    let (st, copy) = app.post(&owner, &format!("/pages/{page}/copy"), json!({"spaceKey": "DST", "includeChildren": true})).await;
    assert_eq!(st, 201, "{copy}");
    assert_eq!(copy["copied"], 2);
    let copy_id = copy["id"].as_str().unwrap();
    let (_, c) = app.get(&owner, &format!("/pages/{copy_id}")).await;
    assert_eq!((c["spaceKey"].as_str(), c["parentId"].is_null()), (Some("DST"), true));
    let (_, atts) = app.get(&owner, &format!("/pages/{copy_id}/attachments")).await;
    let new_att = atts[0]["id"].as_str().unwrap().to_string();
    assert_ne!(new_att, att);
    assert!(c["content"].to_string().contains(&format!("/api/attachments/{new_att}/content")), "{}", c["content"]);
    let body = owner.get(app.url(&format!("/attachments/{new_att}/content"))).send().await.unwrap().bytes().await.unwrap();
    assert_eq!(&body[..], b"\x89PNG\r\n\x1a\nfake");
    let (_, kids) = app.get(&owner, &format!("/pages/{copy_id}/children")).await;
    assert_eq!(kids[0]["title"], "Child");

    // without attachments, if asked
    let (_, bare) = app.post(&owner, &format!("/pages/{page}/copy"), json!({"includeAttachments": false})).await;
    let (_, atts) = app.get(&owner, &format!("/pages/{}/attachments", bare["id"].as_str().unwrap())).await;
    assert!(atts.as_array().unwrap().is_empty());

    // only into spaces you may write in
    let (st, _) = app.post(&bob, &format!("/pages/{page}/copy"), json!({"spaceKey": "DST"})).await;
    assert_eq!(st, 403);
}

#[tokio::test(flavor = "multi_thread")]
async fn attachment_size_type_and_storage_rules() {
    let app = common::spawn().await;
    let owner = app.setup().await;
    let (member, _) = app.user(&owner, "mia@acme.test", "member").await;
    app.post(&owner, "/spaces", json!({"key": "AT", "name": "Attachments", "defaultRole": "editor"})).await;
    let (_, page) = app.post(&owner, "/pages", json!({"spaceKey": "AT", "title": "Files"})).await;
    let page = page["id"].as_str().unwrap().to_string();
    let upload = |c: &reqwest::Client, name: &str, size: usize| {
        let form = reqwest::multipart::Form::new().part("file", reqwest::multipart::Part::bytes(vec![b'x'; size]).file_name(name.to_string()));
        c.post(app.url(&format!("/pages/{page}/attachments"))).multipart(form).send()
    };
    const MB: usize = 1024 * 1024;

    // only admins set the rules; values are checked
    assert_eq!(app.put(&member, "/admin/settings", json!({"attachments": {"maxFileMb": 1}})).await.0, 403);
    for bad in [json!({"maxFileMb": 513}), json!({"blockedTypes": ["e x e"]}), json!({"spaceQuotaMb": -1}), json!({"colour": 1})] {
        assert_eq!(app.put(&owner, "/admin/settings", json!({ "attachments": bad })).await.0, 400, "{bad}");
    }
    let (st, s) = app.put(&owner, "/admin/settings", json!({"attachments": {"maxFileMb": 1, "blockedTypes": ".EXE, bat", "spaceQuotaMb": 2}})).await;
    assert_eq!(st, 200, "{s}");
    assert_eq!(s["attachments"], json!({"maxFileMb": 1, "blockedTypes": ["exe", "bat"], "allowedTypes": [], "spaceQuotaMb": 2, "workspaceQuotaMb": 0}));
    assert_eq!(app.get(&member, "/features").await.1["maxFileMb"], 1);

    // size and type
    let r = upload(&member, "big.pdf", MB + 1).await.unwrap();
    assert_eq!(r.status(), 400);
    assert!(r.text().await.unwrap().contains("larger than 1 MB"));
    let r = upload(&member, "setup.EXE", 10).await.unwrap();
    assert_eq!(r.status(), 400);
    assert!(r.text().await.unwrap().contains(".exe files can't be uploaded"));
    // storage per space counts every version
    assert_eq!(upload(&member, "a.pdf", MB).await.unwrap().status(), 201);
    assert_eq!(upload(&member, "a.pdf", MB - 10).await.unwrap().status(), 201, "a new version of a.pdf");
    let r = upload(&member, "b.pdf", 20).await.unwrap();
    assert_eq!(r.status(), 400);
    assert!(r.text().await.unwrap().contains("this space is full"));
    let (_, spaces) = app.get(&owner, "/admin/spaces").await;
    let at = spaces.as_array().unwrap().iter().find(|s| s["key"] == "AT").unwrap();
    assert_eq!(at["storageBytes"], (2 * MB - 10) as u64);
    // a wiki-wide limit too; an allow-list replaces the block-list's job
    app.put(&owner, "/admin/settings", json!({"attachments": {"spaceQuotaMb": 0, "workspaceQuotaMb": 3, "allowedTypes": ["pdf", "png"]}})).await;
    let r = upload(&member, "notes.txt", 5).await.unwrap();
    assert_eq!(r.status(), 400);
    assert!(r.text().await.unwrap().contains("only these file types can be uploaded here: pdf, png"));
    assert_eq!(upload(&member, "c.png", MB).await.unwrap().status(), 201);
    let r = upload(&member, "d.png", MB).await.unwrap();
    assert_eq!(r.status(), 400);
    assert!(r.text().await.unwrap().contains("the wiki is full"));
    let (_, s) = app.get(&owner, "/admin/settings").await;
    assert_eq!(s["attachmentStorageBytes"], (3 * MB - 10) as u64);

    // imports keep the rules: refused files stay out and are reported
    let mut w = zip::ZipWriter::new(std::io::Cursor::new(Vec::new()));
    let o = zip::write::SimpleFileOptions::default();
    use std::io::Write;
    w.start_file("Guide.md", o).unwrap();
    w.write_all(b"# Guide\n\n![shot](shot.gif)\n").unwrap();
    w.start_file("shot.gif", o).unwrap();
    w.write_all(b"GIF89a").unwrap();
    let zip = w.finish().unwrap().into_inner();
    let form = reqwest::multipart::Form::new().text("spaceKey", "AT").part("file", reqwest::multipart::Part::bytes(zip).file_name("guide.zip"));
    let r = member.post(app.url("/imports/markdown")).multipart(form).send().await.unwrap();
    assert_eq!(r.status(), 201);
    let res: Value = r.json().await.unwrap();
    assert_eq!(res["attachments"], 0);
    assert!(res["skippedFiles"][0].as_str().unwrap().contains("shot.gif"), "{res}");
}

#[tokio::test(flavor = "multi_thread")]
async fn system_information_for_admins() {
    let app = common::spawn().await;
    let owner = app.setup().await;
    let (member, _) = app.user(&owner, "mia@acme.test", "member").await;
    assert_eq!(app.get(&member, "/admin/system").await.0, 403);
    app.post(&owner, "/spaces", json!({"key": "SYS", "name": "System"})).await;
    let (st, s) = app.get(&owner, "/admin/system").await;
    assert_eq!(st, 200, "{s}");
    assert_eq!(s["version"], env!("CARGO_PKG_VERSION"));
    assert!(s["database"]["version"].as_str().unwrap().chars().next().unwrap().is_ascii_digit(), "{s}");
    assert!(s["database"]["sizeBytes"].as_i64().unwrap() > 0);
    assert!(s["database"]["schema"].as_i64().unwrap() >= 47, "latest migration: {s}");
    assert_eq!(s["jobs"], json!({"queued": 0, "running": 0, "failedLastDay": 0}));
    assert!(matches!(s["search"]["backend"].as_str(), Some("tantivy" | "postgres")));
    assert!(s["search"]["pages"].as_i64().unwrap() >= 1, "the space's home page");
    assert_eq!(s["mail"]["enabled"], false);
    assert!(s["storage"]["backend"].is_string());
}

/// A stand-in for clamd: answers INSTREAM, finding anything containing "EICAR".
async fn fake_clamd() -> std::net::SocketAddr {
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    let l = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = l.local_addr().unwrap();
    tokio::spawn(async move {
        while let Ok((mut s, _)) = l.accept().await {
            tokio::spawn(async move {
                let mut cmd = [0u8; 10];
                if s.read_exact(&mut cmd).await.is_err() || &cmd != b"zINSTREAM\0" {
                    return;
                }
                let mut data = Vec::new();
                loop {
                    let mut len = [0u8; 4];
                    if s.read_exact(&mut len).await.is_err() {
                        return;
                    }
                    let n = u32::from_be_bytes(len) as usize;
                    if n == 0 {
                        break;
                    }
                    let mut chunk = vec![0u8; n];
                    if s.read_exact(&mut chunk).await.is_err() {
                        return;
                    }
                    data.extend(chunk);
                }
                let reply: &[u8] = if data.windows(5).any(|w| w == b"EICAR") { b"stream: Eicar-Test-Signature FOUND\0" } else { b"stream: OK\0" };
                let _ = s.write_all(reply).await;
            });
        }
    });
    addr
}

#[tokio::test(flavor = "multi_thread")]
async fn uploads_are_scanned_for_viruses() {
    let clamd = fake_clamd().await;
    let app = common::spawn_with(|c| c.clamav = Some(format!("tcp://{clamd}"))).await;
    let owner = app.setup().await;
    app.post(&owner, "/spaces", json!({"key": "AV", "name": "Scanned"})).await;
    let (_, page) = app.post(&owner, "/pages", json!({"spaceKey": "AV", "title": "Files"})).await;
    let page = page["id"].as_str().unwrap().to_string();
    let upload = |app: &common::TestApp, c: &reqwest::Client, page: &str, name: &str, data: &[u8]| {
        let form = reqwest::multipart::Form::new().part("file", reqwest::multipart::Part::bytes(data.to_vec()).file_name(name.to_string()));
        c.post(app.url(&format!("/pages/{page}/attachments"))).multipart(form).send()
    };
    // a large clean file goes through in chunks
    let big = vec![b'a'; 300 * 1024];
    assert_eq!(upload(&app, &owner, &page, "clean.bin", &big).await.unwrap().status(), 201);
    let eicar = b"X5O!P%@AP[4\\PZX54(P^)7CC)7}$EICAR-STANDARD-ANTIVIRUS-TEST-FILE!$H+H*";
    let r = upload(&app, &owner, &page, "invoice.pdf", eicar).await.unwrap();
    assert_eq!(r.status(), 400);
    assert!(r.text().await.unwrap().contains("found Eicar-Test-Signature"));
    let (_, atts) = app.get(&owner, &format!("/pages/{page}/attachments")).await;
    assert_eq!(atts.as_array().unwrap().len(), 1, "the infected file isn't stored");
    let (_, audit) = app.get(&owner, "/admin/audit").await;
    let ev = audit.as_array().unwrap().iter().find(|e| e["action"] == "upload.infected").cloned().unwrap();
    assert_eq!(ev["data"]["file"], "invoice.pdf");

    // imports leave infected files out and say so
    let mut w = zip::ZipWriter::new(std::io::Cursor::new(Vec::new()));
    let o = zip::write::SimpleFileOptions::default();
    use std::io::Write;
    w.start_file("Guide.md", o).unwrap();
    w.write_all(b"# Guide\n\n![a](a.png) ![b](b.png)\n").unwrap();
    w.start_file("a.png", o).unwrap();
    w.write_all(b"\x89PNG\r\n\x1a\nfine").unwrap();
    w.start_file("b.png", o).unwrap();
    w.write_all(eicar).unwrap();
    let zip = w.finish().unwrap().into_inner();
    let form = reqwest::multipart::Form::new().text("spaceKey", "AV").part("file", reqwest::multipart::Part::bytes(zip).file_name("guide.zip"));
    let res: Value = owner.post(app.url("/imports/markdown")).multipart(form).send().await.unwrap().json().await.unwrap();
    assert_eq!(res["attachments"], 1, "{res}");
    assert!(res["skippedFiles"][0].as_str().unwrap().contains("b.png: the virus scanner found"), "{res}");

    // no scanner reachable: refused, unless the wiki fails open
    let gone = {
        let l = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        l.local_addr().unwrap()
    };
    let closed = common::spawn_with(|c| c.clamav = Some(format!("tcp://{gone}"))).await;
    let o2 = closed.setup().await;
    closed.post(&o2, "/spaces", json!({"key": "AV", "name": "Scanned"})).await;
    let (_, p2) = closed.post(&o2, "/pages", json!({"spaceKey": "AV", "title": "Files"})).await;
    let r = upload(&closed, &o2, p2["id"].as_str().unwrap(), "notes.txt", b"hello").await.unwrap();
    assert_eq!(r.status(), 400);
    assert!(r.text().await.unwrap().contains("virus scanner couldn't check"));
    let open = common::spawn_with(|c| {
        c.clamav = Some(format!("tcp://{gone}"));
        c.clamav_fail_open = true;
    })
    .await;
    let o3 = open.setup().await;
    open.post(&o3, "/spaces", json!({"key": "AV", "name": "Scanned"})).await;
    let (_, p3) = open.post(&o3, "/pages", json!({"spaceKey": "AV", "title": "Files"})).await;
    assert_eq!(upload(&open, &o3, p3["id"].as_str().unwrap(), "notes.txt", b"hello").await.unwrap().status(), 201);
}
