mod common;

use serde_json::json;

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
