//! Retrieval for AI applications (RAG): passages cut along headings, permission filtering,
//! filters, attachment text, read-only tokens, the MCP tool and semantic retrieval against a
//! mock embeddings API.

mod common;

use std::sync::{Arc, Mutex};

use axum::extract::State;
use axum::http::StatusCode;
use serde_json::{Value, json};

async fn create(app: &common::TestApp, c: &reqwest::Client, space: &str, title: &str, md: &str) -> String {
    let (st, p) = app.post(c, "/pages", json!({"spaceKey": space, "title": title, "markdown": md})).await;
    assert_eq!(st, 201, "{p}");
    p["id"].as_str().unwrap().to_string()
}

fn q(s: &str) -> String {
    s.replace(' ', "%20")
}

#[tokio::test(flavor = "multi_thread")]
async fn passages_with_permissions_and_filters() {
    let app = common::spawn().await;
    let owner = app.setup().await;
    let (member, _) = app.user(&owner, "max@acme.test", "member").await;
    app.post(&owner, "/spaces", json!({"key": "IT", "name": "IT"})).await;
    app.post(&owner, "/spaces", json!({"key": "HR", "name": "People"})).await;
    let filler = "Our office keeps the coffee machine on the second floor next to the plants. ".repeat(12);
    let guide = create(
        &app,
        &owner,
        "IT",
        "VPN guide",
        &format!(
            "Everything about remote access.\n\n# Setup\n\nInstall WireGuard and import the company profile.\n\n{filler}\n\n# Troubleshooting\n\n## Handshake\n\nIf the handshake fails, open UDP port 51820 on your home router.\n\n{filler}"
        ),
    )
    .await;
    let (st, _) = app
        .post(&owner, "/pages", json!({"spaceKey": "IT", "parentId": guide, "title": "VPN for contractors", "markdown": "Contractors get a separate VPN profile from the service desk."}))
        .await;
    assert_eq!(st, 201);
    let secret = create(&app, &owner, "IT", "VPN admin keys", "The VPN root key is kumquat-secret-42.").await;
    let (_, me) = app.get(&owner, "/me").await;
    let owner_id = me["user"]["id"].as_str().unwrap().to_string();
    app.put(&owner, &format!("/pages/{secret}/restrictions"), json!({"restrictions": [{"kind": "view", "subjectType": "user", "subjectId": owner_id}]})).await;
    let leave = create(&app, &owner, "HR", "Leave policy", "# Vacation\n\nEveryone gets 25 vacation days per year.").await;
    app.put(&owner, &format!("/pages/{leave}/labels"), json!({"labels": ["policy"]})).await;

    // every page indexed and cut into passages
    app.eventually(|| async {
        let (_, s) = app.get(&owner, "/admin/system").await;
        (s["retrieval"]["pagesPending"] == 0 && s["retrieval"]["pages"].as_i64() >= Some(4)).then_some(())
    })
    .await;

    // the passage under its heading, linked to its first block (the search index catches up
    // a moment after the passages are cut)
    let r = app
        .eventually(|| async {
            let (_, r) = app.get(&member, &format!("/retrieve?q={}", q("What do I do when the VPN handshake fails?"))).await;
            (r["passages"][0]["title"] == "VPN guide").then_some(r)
        })
        .await;
    assert_eq!(r["mode"], "keyword");
    let top = &r["passages"][0];
    assert_eq!(top["title"], "VPN guide", "{r}");
    assert_eq!(top["headings"], json!(["Troubleshooting", "Handshake"]));
    assert!(top["text"].as_str().unwrap().contains("51820"), "{top}");
    assert!(top["url"].as_str().unwrap().contains(&format!("/pages/{guide}#block-")), "{top}");
    assert_eq!(top["spaceKey"], "IT");
    assert_eq!(top["matchedBy"], json!(["keyword"]));
    assert!(top["blockId"].is_string());
    assert!(r.get("note").is_none());

    // titles of the pages above
    let r = app
        .eventually(|| async {
            let (_, r) = app.get(&member, &format!("/retrieve?q={}", q("contractors service desk"))).await;
            (r["passages"][0]["title"] == "VPN for contractors").then_some(r)
        })
        .await;
    assert_eq!(r["passages"][0]["path"], json!(["VPN guide"]));

    // restricted pages stay out for those who can't open them
    let (_, r) = app.get(&member, "/retrieve?q=vpn%20root%20key").await;
    assert!(!r.to_string().contains("kumquat"), "{r}");
    app.eventually(|| async {
        let (_, r) = app.get(&owner, "/retrieve?q=vpn%20root%20key").await;
        r["passages"][0]["text"].as_str().is_some_and(|t| t.contains("kumquat")).then_some(())
    })
    .await;

    // filters: spaces, labels, passages per page, limit, character budget
    let (_, r) = app.get(&member, "/retrieve?q=vacation%20days&space=IT").await;
    assert_eq!(r["passages"], json!([]), "{r}");
    let r = app
        .eventually(|| async {
            let (_, r) = app.get(&member, "/retrieve?q=vacation%20days&space=it,hr&label=policy").await;
            (r["passages"][0]["title"] == "Leave policy").then_some(r)
        })
        .await;
    assert_eq!(r["passages"][0]["labels"], json!(["policy"]));
    let (_, r) = app.get(&member, "/retrieve?q=vpn%20coffee%20office&perPage=1").await;
    let pages: Vec<&str> = r["passages"].as_array().unwrap().iter().map(|p| p["pageId"].as_str().unwrap()).collect();
    let unique: std::collections::HashSet<&&str> = pages.iter().collect();
    assert!(pages.len() >= 2 && unique.len() == pages.len() && pages.contains(&guide.as_str()), "{r}");
    let (_, r) = app.get(&member, "/retrieve?q=vpn%20coffee%20office&limit=1").await;
    assert_eq!(r["passages"].as_array().unwrap().len(), 1);
    let (_, r) = app.get(&member, "/retrieve?q=vpn%20coffee%20office&maxChars=10").await;
    assert_eq!(r["passages"].as_array().unwrap().len(), 1, "always at least one passage");

    // semantic retrieval asked for but not configured: keyword results and a note
    let (_, r) = app.get(&member, "/retrieve?q=handshake&mode=semantic").await;
    assert_eq!(r["mode"], "keyword");
    assert!(r["note"].as_str().unwrap().contains("off"), "{r}");
    let (st, _) = app.get(&member, "/retrieve?q=handshake&mode=psychic").await;
    assert_eq!(st, 400);
    let (st, _) = app.get(&member, "/retrieve?q=%20").await;
    assert_eq!(st, 400);

    // attached files add passages of their own
    let form = reqwest::multipart::Form::new().part(
        "file",
        reqwest::multipart::Part::bytes(b"Backup runbook\n\nThe restore drill runs every quarter on the staging cluster.".to_vec()).file_name("runbook.txt"),
    );
    let r = owner.post(app.url(&format!("/pages/{guide}/attachments"))).multipart(form).send().await.unwrap();
    assert_eq!(r.status(), 201, "{}", r.text().await.unwrap());
    let r = app
        .eventually(|| async {
            let (_, r) = app.get(&member, "/retrieve?q=restore%20drill%20quarter").await;
            (r["passages"][0]["attachment"]["filename"] == "runbook.txt").then_some(r)
        })
        .await;
    let p = &r["passages"][0];
    assert_eq!(p["headings"], json!(["runbook.txt"]));
    assert!(p["text"].as_str().unwrap().contains("staging cluster"));
    assert!(p["attachment"]["url"].as_str().unwrap().ends_with("/content"));

    // editing a page re-cuts it; trashing it removes its passages
    app.put(&owner, &format!("/pages/{leave}/content"), json!({"markdown": "# Vacation\n\nEveryone gets 28 vacation days per year."})).await;
    app.eventually(|| async {
        let (_, r) = app.get(&member, "/retrieve?q=vacation%20days").await;
        r["passages"][0]["text"].as_str().is_some_and(|t| t.contains("28")).then_some(())
    })
    .await;
    assert_eq!(app.delete(&owner, &format!("/pages/{leave}")).await, 204);
    app.eventually(|| async {
        let (_, r) = app.get(&owner, "/retrieve?q=vacation%20days").await;
        (r["passages"] == json!([])).then_some(())
    })
    .await;

    // read-only tokens may retrieve, GET and POST alike
    let (_, t) = app.post(&member, "/me/tokens", json!({"name": "rag", "scope": "read"})).await;
    let token = t["token"].as_str().unwrap().to_string();
    let c = reqwest::Client::new();
    let r = c.post(app.url("/retrieve")).bearer_auth(&token).json(&json!({"query": "handshake fails", "spaces": ["IT"], "limit": 2})).send().await.unwrap();
    assert_eq!(r.status(), 200);
    let v: Value = r.json().await.unwrap();
    assert_eq!(v["passages"][0]["headings"], json!(["Troubleshooting", "Handshake"]));
    let r = c.get(app.url("/retrieve?q=handshake")).bearer_auth(&token).send().await.unwrap();
    assert_eq!(r.status(), 200);

    // the admin overview counts the passages
    let (_, sys) = app.get(&owner, "/admin/system").await;
    assert!(sys["retrieval"]["passages"].as_i64().unwrap() >= 5, "{sys}");
    assert_eq!(sys["retrieval"]["semantic"]["configured"], false);

    // the MCP tool: numbered passages with their place and URL
    let url = app.url("/mcp");
    let init = json!({"jsonrpc": "2.0", "id": 1, "method": "initialize", "params": {"protocolVersion": "2025-06-18", "capabilities": {}, "clientInfo": {"name": "test", "version": "1"}}});
    let r = c.post(&url).bearer_auth(&token).header("accept", "application/json, text/event-stream").json(&init).send().await.unwrap();
    let sid = r.headers()["mcp-session-id"].to_str().unwrap().to_string();
    let call = |body: Value| {
        c.post(&url)
            .bearer_auth(&token)
            .header("accept", "application/json, text/event-stream")
            .header("mcp-protocol-version", "2025-06-18")
            .header("mcp-session-id", &sid)
            .json(&body)
            .send()
    };
    call(json!({"jsonrpc": "2.0", "method": "notifications/initialized"})).await.unwrap();
    let text = call(json!({"jsonrpc": "2.0", "id": 2, "method": "tools/call", "params": {"name": "retrieve", "arguments": {"query": "VPN handshake fails"}}}))
        .await
        .unwrap()
        .text()
        .await
        .unwrap();
    assert!(text.contains("[1] VPN guide › Troubleshooting › Handshake"), "{text}");
    assert!(text.contains("51820") && text.contains("#block-"), "{text}");
    assert!(!text.contains("kumquat"));
}

/// A stand-in embeddings API: bag-of-words vectors where some words share a dimension
/// (synonyms), so "holidays" finds "vacation" without sharing a word.
#[derive(Clone, Default)]
struct Mock {
    inputs: Arc<Mutex<Vec<String>>>,
    auth: Arc<Mutex<Vec<String>>>,
}

fn embed_text(t: &str) -> Vec<f32> {
    let mut v = vec![0f32; 48];
    for w in t.to_lowercase().split(|c: char| !c.is_alphanumeric()).filter(|w| w.len() > 2) {
        let w = match w {
            "holidays" | "holiday" | "vacation" | "leave" | "days" => "vacation",
            "automobile" | "vehicle" | "car" | "parking" => "car",
            other => other,
        };
        let h = w.bytes().fold(7u32, |a, b| a.wrapping_mul(31).wrapping_add(b as u32));
        v[(h % 48) as usize] += 1.0;
    }
    v
}

async fn embeddings(State(m): State<Mock>, headers: axum::http::HeaderMap, axum::Json(body): axum::Json<Value>) -> (StatusCode, axum::Json<Value>) {
    m.auth.lock().unwrap().push(headers.get("authorization").map(|v| v.to_str().unwrap().to_string()).unwrap_or_default());
    let inputs: Vec<String> = body["input"].as_array().unwrap().iter().map(|v| v.as_str().unwrap().to_string()).collect();
    if inputs.iter().any(|i| i.contains("explode")) {
        return (StatusCode::BAD_REQUEST, axum::Json(json!({"error": {"message": "input rejected"}})));
    }
    assert_eq!(body["model"], "mock-embed");
    m.inputs.lock().unwrap().extend(inputs.iter().cloned());
    let data: Vec<Value> = inputs.iter().enumerate().map(|(i, t)| json!({"object": "embedding", "index": i, "embedding": embed_text(t)})).collect();
    (StatusCode::OK, axum::Json(json!({"object": "list", "data": data, "model": "mock-embed"})))
}

#[tokio::test(flavor = "multi_thread")]
async fn semantic_retrieval_with_embeddings() {
    let mock = Mock::default();
    let router = axum::Router::new().route("/v1/embeddings", axum::routing::post(embeddings)).with_state(mock.clone());
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let mock_url = format!("http://{}/v1", listener.local_addr().unwrap());
    tokio::spawn(async move { axum::serve(listener, router).await.unwrap() });
    let app = common::spawn_with(|c| {
        c.embeddings_url = Some(mock_url.clone());
        c.embeddings_model = Some("mock-embed".into());
        c.embeddings_api_key = Some("emb-key".into());
        c.embeddings_document_prefix = "search_document: ".into();
        c.embeddings_query_prefix = "search_query: ".into();
    })
    .await;
    let owner = app.setup().await;
    let (member, _) = app.user(&owner, "max@acme.test", "member").await;
    app.post(&owner, "/spaces", json!({"key": "HR", "name": "People"})).await;
    let leave =
        create(&app, &owner, "HR", "Leave policy", "# Annual leave\n\nEveryone gets 25 vacation days per year.\n\n# Sick leave\n\nCall your manager before 9.")
            .await;
    create(&app, &owner, "HR", "Office", "# Parking\n\nThe garage has 40 spaces for cars.").await;
    let secret = create(&app, &owner, "HR", "Salaries", "Holiday bonus for executives: 3 months.").await;
    let (_, me) = app.get(&owner, "/me").await;
    let owner_id = me["user"]["id"].as_str().unwrap().to_string();
    app.put(&owner, &format!("/pages/{secret}/restrictions"), json!({"restrictions": [{"kind": "view", "subjectType": "user", "subjectId": owner_id}]})).await;

    // every passage gets embedded (with the document prefix and the API key)
    let sys = app
        .eventually(|| async {
            let (_, s) = app.get(&owner, "/admin/system").await;
            let sem = &s["retrieval"]["semantic"];
            (sem["dims"] == 48 && sem["pending"] == 0 && sem["embedded"].as_i64() >= Some(4)).then_some(s)
        })
        .await;
    assert_eq!(sys["retrieval"]["semantic"]["model"], "mock-embed");
    assert!(mock.inputs.lock().unwrap().iter().all(|i| i.starts_with("search_document: ")));
    assert!(mock.inputs.lock().unwrap().iter().any(|i| i.contains("Leave policy\nAnnual leave\n\n# Annual leave")), "title and headings are embedded too");
    assert!(mock.auth.lock().unwrap().iter().all(|a| a == "Bearer emb-key"));

    // "holidays" shares no word with the passage: only the vectors find it
    let (_, r) = app.get(&member, "/retrieve?q=how%20many%20holidays%20per%20year").await;
    assert_eq!(r["mode"], "hybrid", "{r}");
    let top = &r["passages"][0];
    assert_eq!(top["title"], "Leave policy", "{r}");
    assert_eq!(top["headings"], json!(["Annual leave"]));
    assert!(top["matchedBy"].as_array().unwrap().contains(&json!("semantic")));
    assert!(!r.to_string().contains("executives"), "restricted pages stay out of semantic results too: {r}");
    let (_, r) = app.get(&member, "/retrieve?q=holidays&mode=semantic").await;
    assert_eq!(r["mode"], "semantic");
    assert_eq!(r["passages"][0]["matchedBy"], json!(["semantic"]));
    let (_, r) = app.get(&member, "/retrieve?q=holidays&mode=keyword").await;
    assert_eq!(r["mode"], "keyword");
    assert!(mock.inputs.lock().unwrap().iter().any(|i| i == "search_query: how many holidays per year"));

    // editing one section re-embeds only that section
    let before = mock.inputs.lock().unwrap().len();
    app.put(
        &owner,
        &format!("/pages/{leave}/content"),
        json!({"markdown": "# Annual leave\n\nEveryone gets 25 vacation days per year.\n\n# Sick leave\n\nCall your manager before 8."}),
    )
    .await;
    app.eventually(|| async {
        let arrived = mock.inputs.lock().unwrap()[before..].iter().any(|i| i.contains("before 8"));
        let (_, s) = app.get(&owner, "/admin/system").await;
        (arrived && s["retrieval"]["semantic"]["pending"] == 0).then_some(())
    })
    .await;
    let new: Vec<String> = mock.inputs.lock().unwrap()[before..].to_vec();
    assert!(!new.iter().any(|i| i.contains("25 vacation days")), "the unchanged section keeps its vector: {new:?}");

    // the embeddings API failing for a question: keyword results and a note
    let (st, r) = app.get(&member, "/retrieve?q=explode%20vacation").await;
    assert_eq!(st, 200);
    assert_eq!(r["mode"], "keyword");
    assert!(r["note"].as_str().unwrap().contains("input rejected"), "{r}");
    assert_eq!(r["passages"][0]["title"], "Leave policy");
}
