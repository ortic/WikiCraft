//! Options for scaled deployments: shared Postgres search, multi-replica collaboration relay.

mod common;

use serde_json::json;

#[tokio::test(flavor = "multi_thread")]
async fn postgres_search_backend() {
    let app = common::spawn_with(|c| c.search = "postgres".into()).await;
    assert_eq!(app.state.search.backend_name(), "postgres");
    let owner = app.setup().await;
    let (member, _) = app.user(&owner, "kim@acme.test", "member").await;
    app.post(&owner, "/spaces", json!({"key": "PG", "name": "PG", "defaultRole": "viewer"})).await;
    let (_, a) = app
        .post(&owner, "/pages", json!({"spaceKey": "PG", "title": "Roadmap 2027", "markdown": "We will <script>alert(1)</script> ship the kangaroo feature."}))
        .await;
    let (_, b) = app.post(&owner, "/pages", json!({"spaceKey": "PG", "title": "Secret plans", "markdown": "kangaroo acquisition"})).await;
    let (_me, me) = app.get(&owner, "/me").await;
    app.put(
        &owner,
        &format!("/pages/{}/restrictions", b["id"].as_str().unwrap()),
        json!({"restrictions": [{"kind": "view", "subjectType": "user", "subjectId": me["user"]["id"]}]}),
    )
    .await;
    app.put(&owner, &format!("/pages/{}/labels", a["id"].as_str().unwrap()), json!({"labels": ["strategy"]})).await;

    let hits = app
        .eventually(|| async {
            let (_, h) = app.get(&owner, "/search?q=kangaroo").await;
            (h.as_array().unwrap().len() == 2).then_some(h)
        })
        .await;
    let snippet = hits.as_array().unwrap().iter().find(|h| h["title"] == "Roadmap 2027").unwrap()["snippet"].as_str().unwrap().to_string();
    assert!(snippet.contains("<b>kangaroo</b>"), "{snippet}");
    assert!(!snippet.contains("<script>"), "page text must be escaped: {snippet}");
    // prefix (search-as-you-type), title and label matches
    let (_, h) = app.get(&owner, "/search?q=roadm").await;
    assert_eq!(h[0]["title"], "Roadmap 2027");
    let (_, h) = app
        .eventually(|| async {
            let r = app.get(&owner, "/search?q=strategy").await;
            (r.1.as_array().unwrap().len() == 1).then_some(r)
        })
        .await;
    assert_eq!(h[0]["title"], "Roadmap 2027");
    // restrictions apply
    let (_, h) = app.get(&member, "/search?q=kangaroo").await;
    assert_eq!(h.as_array().unwrap().len(), 1);
    // trash removes from results
    assert_eq!(app.delete(&owner, &format!("/pages/{}", a["id"].as_str().unwrap())).await, 204);
    let (_, h) = app.get(&owner, "/search?q=roadmap").await;
    assert!(h.as_array().unwrap().is_empty());
    // attachment text is searchable
    let (_, sp) = app.get(&owner, "/spaces/PG").await;
    let home = sp["homePageId"].as_str().unwrap().to_string();
    let form = reqwest::multipart::Form::new().part("file", reqwest::multipart::Part::bytes(b"quokka inventory list".to_vec()).file_name("notes.txt"));
    owner.post(app.url(&format!("/pages/{home}/attachments"))).multipart(form).send().await.unwrap();
    app.eventually(|| async {
        let (_, h) = app.get(&owner, "/search?q=quokka").await;
        (h.as_array().unwrap().len() == 1).then_some(())
    })
    .await;
}
