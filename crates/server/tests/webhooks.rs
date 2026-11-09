//! Outgoing webhooks against a local receiver.

mod common;

use std::sync::{Arc, Mutex};

use axum::extract::State;
use axum::http::{HeaderMap, StatusCode};
use serde_json::{Value, json};

#[derive(Clone, Default)]
struct Receiver {
    got: Arc<Mutex<Vec<(HeaderMap, Vec<u8>)>>>,
    /// answer this many requests with 500 first
    fail_first: Arc<Mutex<usize>>,
}

async fn receive(State(r): State<Receiver>, headers: HeaderMap, body: axum::body::Bytes) -> StatusCode {
    let mut fail = r.fail_first.lock().unwrap();
    if *fail > 0 {
        *fail -= 1;
        return StatusCode::INTERNAL_SERVER_ERROR;
    }
    r.got.lock().unwrap().push((headers, body.to_vec()));
    StatusCode::NO_CONTENT
}

async fn spawn_receiver() -> (String, Receiver) {
    let r = Receiver::default();
    let app = axum::Router::new().route("/hook", axum::routing::post(receive)).with_state(r.clone());
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
    (format!("http://{addr}/hook"), r)
}

fn events(r: &Receiver) -> Vec<(String, Value, String)> {
    r.got
        .lock()
        .unwrap()
        .iter()
        .map(|(h, b)| {
            (h["x-wikicraft-event"].to_str().unwrap().to_string(), serde_json::from_slice(b).unwrap(), h["x-wikicraft-signature"].to_str().unwrap().to_string())
        })
        .collect()
}

#[tokio::test(flavor = "multi_thread")]
async fn webhooks_are_signed_filtered_and_retried() {
    let app = common::spawn().await;
    let owner = app.setup().await;
    let (url, recv) = spawn_receiver().await;

    // validation and access
    let (member, _) = app.user(&owner, "max@acme.test", "member").await;
    let (st, _) = app.post(&member, "/admin/webhooks", json!({"url": url})).await;
    assert_eq!(st, 403);
    let (st, _) = app.post(&owner, "/admin/webhooks", json!({"url": "ftp://example.com"})).await;
    assert_eq!(st, 400);
    let (st, _) = app.post(&owner, "/admin/webhooks", json!({"url": url, "events": ["page.exploded"]})).await;
    assert_eq!(st, 400);

    let (st, hook) = app.post(&owner, "/admin/webhooks", json!({"name": "CI", "url": url, "events": ["page.created", "comment.created"]})).await;
    assert_eq!(st, 201, "{hook}");
    let secret = hook["secret"].as_str().unwrap().to_string();
    let hook_id = hook["id"].as_str().unwrap().to_string();
    assert!(secret.starts_with("whsec_"));

    let (_, sp) = app.post(&owner, "/spaces", json!({"key": "HOOK", "name": "Hooks"})).await;
    let (_, page) = app.post(&owner, "/pages", json!({"spaceKey": "HOOK", "title": "Release notes", "parentId": sp["homePageId"]})).await;
    let page_id = page["id"].as_str().unwrap().to_string();
    app.post(&owner, &format!("/pages/{page_id}/comments"), json!({"text": "Ship it"})).await;

    app.eventually(|| async { (events(&recv).len() >= 2).then_some(()) }).await;
    let got = events(&recv);
    let kinds: Vec<&str> = got.iter().map(|(e, _, _)| e.as_str()).collect();
    assert_eq!(kinds, vec!["page.created", "comment.created"], "space.created isn't subscribed");
    let (_, created, signature) = &got[0];
    assert_eq!(created["data"]["page"]["title"], "Release notes");
    assert_eq!(created["data"]["space"]["key"], "HOOK");
    assert_eq!(created["data"]["actor"]["email"], "owner@acme.test");
    assert!(created["deliveryId"].is_string() && created["webhookId"] == hook_id.as_str());
    // the signature covers the exact body
    let raw = recv.got.lock().unwrap()[0].1.clone();
    assert_eq!(signature, &wikicraft::webhooks::sign(&secret, &raw));
    assert_eq!(got[1].1["data"]["comment"]["text"], "Ship it");

    // a hook scoped to another space doesn't see these events
    let (url2, recv2) = spawn_receiver().await;
    app.post(&owner, "/spaces", json!({"key": "OTHER", "name": "Other"})).await;
    app.post(&owner, "/admin/webhooks", json!({"url": url2, "spaceKey": "OTHER"})).await;
    app.post(&owner, "/pages", json!({"spaceKey": "HOOK", "title": "Not for hook 2"})).await;
    app.post(&owner, "/pages", json!({"spaceKey": "OTHER", "title": "For hook 2"})).await;
    app.eventually(|| async { (events(&recv2).iter().any(|(e, _, _)| e == "page.created")).then_some(()) }).await;
    let titles: Vec<String> =
        events(&recv2).iter().filter(|(e, _, _)| e == "page.created").map(|(_, v, _)| v["data"]["page"]["title"].as_str().unwrap().to_string()).collect();
    assert_eq!(titles, vec!["For hook 2"]);

    // failures are retried with backoff
    *recv.fail_first.lock().unwrap() = 1;
    let before = events(&recv).len();
    app.post(&owner, "/pages", json!({"spaceKey": "HOOK", "title": "Flaky"})).await;
    app.eventually(|| async {
        let (_, d) = app.get(&owner, &format!("/admin/webhooks/{hook_id}/deliveries")).await;
        (d[0]["attempts"] == 1 && d[0]["responseStatus"] == 500).then_some(())
    })
    .await;
    assert_eq!(events(&recv).len(), before);
    sqlx::query("UPDATE webhook_deliveries SET next_attempt_at = now()").execute(&app.state.db).await.unwrap();
    app.eventually(|| async { (events(&recv).len() > before).then_some(()) }).await;
    // the delivery row is updated once the worker commits
    let d = app
        .eventually(|| async {
            let (_, d) = app.get(&owner, &format!("/admin/webhooks/{hook_id}/deliveries")).await;
            d[0]["deliveredAt"].is_string().then_some(d)
        })
        .await;
    assert_eq!(d[0]["attempts"], 2);
    assert_eq!(d[0]["responseStatus"], 204);

    // test ping, listing, deactivation
    let (st, _) = app.post(&owner, &format!("/admin/webhooks/{hook_id}/test"), json!({})).await;
    assert_eq!(st, 202);
    app.eventually(|| async { events(&recv).iter().any(|(e, _, _)| e == "ping").then_some(()) }).await;
    let (_, list) = app.get(&owner, "/admin/webhooks").await;
    assert_eq!(list["hooks"].as_array().unwrap().len(), 2);
    assert!(list["hooks"][0].get("secret").is_none(), "secrets aren't listed");
    assert_eq!(app.patch(&owner, &format!("/admin/webhooks/{hook_id}"), json!({"active": false})).await.0, 204);
    let n = events(&recv).len();
    app.post(&owner, "/pages", json!({"spaceKey": "HOOK", "title": "Silent"})).await;
    tokio::time::sleep(std::time::Duration::from_secs(3)).await;
    assert_eq!(events(&recv).len(), n, "inactive hooks get nothing");
}

#[tokio::test(flavor = "multi_thread")]
async fn private_targets_are_refused_by_default() {
    let app = common::spawn_with(|c| c.webhook_allow_private = false).await;
    let owner = app.setup().await;
    let (url, recv) = spawn_receiver().await;
    let (_, hook) = app.post(&owner, "/admin/webhooks", json!({"url": url})).await;
    let id = hook["id"].as_str().unwrap().to_string();
    app.post(&owner, &format!("/admin/webhooks/{id}/test"), json!({})).await;
    app.eventually(|| async {
        let (_, d) = app.get(&owner, &format!("/admin/webhooks/{id}/deliveries")).await;
        d[0]["lastError"].as_str().filter(|e| e.contains("private address")).map(|_| ())
    })
    .await;
    assert!(events(&recv).is_empty());
}
