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

#[tokio::test(flavor = "multi_thread")]
async fn live_editing_across_two_replicas() {
    use common::peer::Peer;
    let cfg = |c: &mut wikicraft::config::Config| {
        c.cluster = "postgres".into();
        c.search = "postgres".into();
    };
    let app1 = common::spawn_with(cfg).await;
    let db_url = app1.state.config.database_url.clone();
    let app2 = common::spawn_with(move |c| {
        cfg(c);
        c.database_url = db_url;
    })
    .await;
    assert_ne!(app1.state.node_id, app2.state.node_id);
    let owner = app1.setup().await;
    let (_, sp) = app1.post(&owner, "/spaces", json!({"key": "HA", "name": "HA"})).await;
    let page = sp["homePageId"].as_str().unwrap().to_string();
    let (_, t) = app1.post(&owner, "/me/tokens", json!({"name": "ws"})).await;
    let token = t["token"].as_str().unwrap().to_string();

    // same page, different replicas (as behind a load balancer)
    let mut a = Peer::connect(&app1.base, &page, &token).await;
    let mut b = Peer::connect(&app2.base, &page, &token).await;
    assert_eq!(a.text(), b.text(), "both replicas load the same seeded document");

    a.append_paragraph("typed on replica one").await;
    b.wait_for_text("typed on replica one").await;
    b.append_paragraph("typed on replica two").await;
    a.wait_for_text("typed on replica two").await;
    // REST edits on one replica reach editors on the other
    let token2 = token.clone();
    let r = reqwest::Client::new()
        .put(app2.url(&format!("/pages/{page}/content")))
        .bearer_auth(&token2)
        .json(&json!({"markdown": "rewritten via replica two API"}))
        .send()
        .await
        .unwrap();
    assert_eq!(r.status(), 200);
    a.wait_for_text("rewritten via replica two API").await;

    // both replicas persist; stored state contains everything (merge-on-persist)
    a.append_paragraph("final words from one").await;
    b.wait_for_text("final words from one").await;
    drop(a);
    drop(b);
    let content = app1
        .eventually(|| async {
            let c: serde_json::Value = sqlx::query_scalar("SELECT content FROM pages WHERE id = $1::uuid").bind(&page).fetch_one(&app1.state.db).await.unwrap();
            let s = c.to_string();
            (s.contains("rewritten via replica two API") && s.contains("final words from one")).then_some(s)
        })
        .await;
    assert!(!content.contains("typed on replica one"), "the REST rewrite replaced earlier content");
    // a third client on replica two sees the final state
    let c = Peer::connect(&app2.base, &page, &token).await;
    assert!(c.text().contains("final words from one"));
}

/// S3-compatible object storage (MinIO on 127.0.0.1:9100, bucket `wikicraft-test`); skipped when absent.
#[tokio::test(flavor = "multi_thread")]
async fn s3_storage_and_job_queue() {
    if reqwest::get("http://127.0.0.1:9100/minio/health/live").await.is_err() {
        eprintln!("skipping: MinIO not running");
        return;
    }
    let app = common::spawn_with(|c| {
        c.s3_bucket = Some("wikicraft-test".into());
        c.s3_endpoint = Some("http://127.0.0.1:9100".into());
        c.s3_region = Some("us-east-1".into());
        c.s3_access_key_id = Some("wikicraft".into());
        c.s3_secret_access_key = Some("wikicraft-secret".into());
    })
    .await;
    assert!(app.state.blobs.backend_name().contains("AmazonS3"), "{}", app.state.blobs.backend_name());
    let c = app.setup().await;
    let (_, sp) = app.post(&c, "/spaces", json!({"key": "S3", "name": "S3"})).await;
    let home = sp["homePageId"].as_str().unwrap().to_string();
    let data = format!("hello from object storage {}", uuid::Uuid::new_v4()).into_bytes();
    let form = reqwest::multipart::Form::new().part("file", reqwest::multipart::Part::bytes(data.clone()).file_name("hello.txt"));
    let r = c.post(app.url(&format!("/pages/{home}/attachments"))).multipart(form).send().await.unwrap();
    assert_eq!(r.status(), 201);
    let id = r.json::<serde_json::Value>().await.unwrap()[0]["id"].as_str().unwrap().to_string();
    let got = c.get(app.url(&format!("/attachments/{id}/content"))).send().await.unwrap().bytes().await.unwrap();
    assert_eq!(got.as_ref(), data.as_slice());
    assert!(!app.state.config.blob_dir().exists(), "nothing stored on local disk");

    // import job: upload goes to object storage, a queue worker runs it
    let zip = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../research/confluence-import/fixtures/modelop-cloud-xml/export.zip");
    if !zip.exists() {
        return;
    }
    let form = reqwest::multipart::Form::new()
        .text("options", json!({"skipHistory": true}).to_string())
        .part("file", reqwest::multipart::Part::bytes(std::fs::read(zip).unwrap()).file_name("export.zip"));
    let r = c.post(app.url("/imports/confluence")).multipart(form).send().await.unwrap();
    let job = r.json::<serde_json::Value>().await.unwrap()["jobId"].as_str().unwrap().to_string();
    let done = app
        .eventually(|| async {
            let (_, j) = app.get(&c, &format!("/jobs/{job}")).await;
            matches!(j["status"].as_str(), Some("succeeded" | "failed")).then_some(j)
        })
        .await;
    assert_eq!(done["status"], "succeeded", "{done}");
    let worker: Option<String> = sqlx::query_scalar("SELECT worker FROM jobs WHERE id = $1::uuid").bind(&job).fetch_one(&app.state.db).await.unwrap();
    assert_eq!(worker, Some(app.state.node_id.to_string()));
    assert_eq!(done["result"]["created"]["attachments"], 202);
}
