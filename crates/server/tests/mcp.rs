//! MCP server over streamable HTTP (raw JSON-RPC, like any MCP client would speak it).

mod common;

use serde_json::{Value, json};

/// Sends a JSON-RPC message; parses JSON or SSE (`data:` lines) responses.
async fn rpc(c: &reqwest::Client, url: &str, token: &str, session: Option<&str>, body: Value) -> (u16, Option<String>, Option<Value>) {
    let mut req =
        c.post(url).bearer_auth(token).header("accept", "application/json, text/event-stream").header("mcp-protocol-version", "2025-06-18").json(&body);
    if let Some(s) = session {
        req = req.header("mcp-session-id", s);
    }
    let r = req.send().await.unwrap();
    let status = r.status().as_u16();
    let sid = r.headers().get("mcp-session-id").map(|v| v.to_str().unwrap().to_string());
    let text = r.text().await.unwrap();
    let value = serde_json::from_str::<Value>(&text).ok().or_else(|| {
        text.lines()
            .filter_map(|l| l.strip_prefix("data:"))
            .map(str::trim)
            .filter(|d| !d.is_empty())
            .filter_map(|d| serde_json::from_str::<Value>(d).ok())
            .find(|v| v.get("id").is_some())
    });
    (status, sid, value)
}

fn tool_text(v: &Value) -> String {
    v["result"]["content"][0]["text"].as_str().unwrap_or_default().to_string()
}

#[tokio::test(flavor = "multi_thread")]
async fn mcp_tools_respect_permissions() {
    let app = common::spawn().await;
    let owner = app.setup().await;
    app.post(&owner, "/spaces", json!({"key": "AI", "name": "AI space"})).await;
    let (_, t) = app.post(&owner, "/me/tokens", json!({"name": "mcp"})).await;
    let token = t["token"].as_str().unwrap().to_string();
    let url = app.url("/mcp");
    let c = reqwest::Client::new();

    let (st, sid, init) = rpc(&c, &url, &token, None, json!({"jsonrpc": "2.0", "id": 1, "method": "initialize", "params": {"protocolVersion": "2025-06-18", "capabilities": {}, "clientInfo": {"name": "test", "version": "1"}}})).await;
    assert_eq!(st, 200, "{init:?}");
    assert_eq!(init.as_ref().unwrap()["result"]["serverInfo"]["name"], "wikicraft");
    let sid = sid.expect("session id");
    let (st, _, _) = rpc(&c, &url, &token, Some(&sid), json!({"jsonrpc": "2.0", "method": "notifications/initialized"})).await;
    assert!(st == 202 || st == 200);

    let (_, _, tools) = rpc(&c, &url, &token, Some(&sid), json!({"jsonrpc": "2.0", "id": 2, "method": "tools/list"})).await;
    let names: Vec<String> = tools.unwrap()["result"]["tools"].as_array().unwrap().iter().map(|t| t["name"].as_str().unwrap().to_string()).collect();
    for n in ["search", "get_page", "create_page", "update_page", "append_to_page", "add_comment", "list_spaces", "list_children", "recent_changes"] {
        assert!(names.contains(&n.to_string()), "missing tool {n}: {names:?}");
    }

    let (_, _, created) = rpc(&c, &url, &token, Some(&sid), json!({"jsonrpc": "2.0", "id": 3, "method": "tools/call", "params": {"name": "create_page", "arguments": {"space_key": "AI", "title": "Written by an agent", "markdown": "# Hi\n\nThe **quarterly** plan.\n\n- [ ] follow up"}}})).await;
    let created: Value = serde_json::from_str(&tool_text(created.as_ref().unwrap())).unwrap();
    let page_id = created["pageId"].as_str().unwrap().to_string();

    let (_, _, page) = rpc(
        &c,
        &url,
        &token,
        Some(&sid),
        json!({"jsonrpc": "2.0", "id": 4, "method": "tools/call", "params": {"name": "get_page", "arguments": {"page_id": page_id}}}),
    )
    .await;
    let md = tool_text(page.as_ref().unwrap());
    assert!(md.contains("# Written by an agent") && md.contains("**quarterly**") && md.contains("- [ ] follow up"), "{md}");

    let (_, _, appended) = rpc(&c, &url, &token, Some(&sid), json!({"jsonrpc": "2.0", "id": 5, "method": "tools/call", "params": {"name": "append_to_page", "arguments": {"page_id": page_id, "markdown": "Appended line"}}})).await;
    assert!(tool_text(appended.as_ref().unwrap()).contains("version"));

    let hits = app
        .eventually(|| async {
            let (_, _, r) = rpc(
                &c,
                &url,
                &token,
                Some(&sid),
                json!({"jsonrpc": "2.0", "id": 6, "method": "tools/call", "params": {"name": "search", "arguments": {"query": "quarterly"}}}),
            )
            .await;
            let t = tool_text(r.as_ref().unwrap());
            t.contains("Written by an agent").then_some(t)
        })
        .await;
    assert!(hits.contains("url"));

    // a user without access to the space gets an error result, not the content
    let (other, _) = app.user(&owner, "eve@acme.test", "guest").await;
    let (_, t2) = app.post(&other, "/me/tokens", json!({"name": "mcp"})).await;
    let token2 = t2["token"].as_str().unwrap().to_string();
    let (_, sid2, _) = rpc(&c, &url, &token2, None, json!({"jsonrpc": "2.0", "id": 1, "method": "initialize", "params": {"protocolVersion": "2025-06-18", "capabilities": {}, "clientInfo": {"name": "test", "version": "1"}}})).await;
    let sid2 = sid2.unwrap();
    rpc(&c, &url, &token2, Some(&sid2), json!({"jsonrpc": "2.0", "method": "notifications/initialized"})).await;
    let (_, _, denied) = rpc(
        &c,
        &url,
        &token2,
        Some(&sid2),
        json!({"jsonrpc": "2.0", "id": 7, "method": "tools/call", "params": {"name": "get_page", "arguments": {"page_id": page_id}}}),
    )
    .await;
    let denied = denied.unwrap();
    assert_eq!(denied["result"]["isError"], true, "{denied}");
    assert!(!tool_text(&denied).contains("quarterly"));

    // no token → tools refuse
    let (_, _, anon) = rpc(
        &c,
        &url,
        "not-a-token",
        Some(&sid),
        json!({"jsonrpc": "2.0", "id": 8, "method": "tools/call", "params": {"name": "list_spaces", "arguments": {}}}),
    )
    .await;
    if let Some(a) = anon {
        assert!(a["result"]["isError"] == true || a.get("error").is_some(), "{a}");
    }
}

/// Read-only tokens: reading works (REST and MCP), changing anything doesn't.
#[tokio::test(flavor = "multi_thread")]
async fn read_only_tokens() {
    let app = common::spawn().await;
    let owner = app.setup().await;
    app.post(&owner, "/spaces", json!({"key": "RO", "name": "Read only"})).await;
    let (_, page) = app.post(&owner, "/pages", json!({"spaceKey": "RO", "title": "Handbook", "markdown": "quokkaline rules"})).await;
    let page_id = page["id"].as_str().unwrap().to_string();
    let (st, t) = app.post(&owner, "/me/tokens", json!({"name": "assistant", "scope": "read"})).await;
    assert_eq!(st, 200, "{t}");
    assert_eq!(t["scope"], "read");
    assert_eq!(app.post(&owner, "/me/tokens", json!({"name": "x", "scope": "admin"})).await.0, 400);
    let token = t["token"].as_str().unwrap().to_string();
    let c = reqwest::Client::new();

    // REST: reads and read-like POSTs pass, changes are refused
    let r = c.get(app.url(&format!("/pages/{page_id}"))).bearer_auth(&token).send().await.unwrap();
    assert_eq!(r.status(), 200);
    let r = c.post(app.url("/markdown")).bearer_auth(&token).json(&json!({"markdown": "**x**"})).send().await.unwrap();
    assert_eq!(r.status(), 200, "converting Markdown changes nothing");
    let r = c.post(app.url("/pages")).bearer_auth(&token).json(&json!({"spaceKey": "RO", "title": "Nope"})).send().await.unwrap();
    assert_eq!(r.status(), 403);
    assert!(r.text().await.unwrap().contains("read-only"));
    let r = c.delete(app.url(&format!("/pages/{page_id}"))).bearer_auth(&token).send().await.unwrap();
    assert_eq!(r.status(), 403);
    let (_, list) = app.get(&owner, "/me/tokens").await;
    assert_eq!(list[0]["scope"], "read");

    // MCP: one POST endpoint, read tools work, write tools refuse
    let url = app.url("/mcp");
    let (st, sid, _) = rpc(&c, &url, &token, None, json!({"jsonrpc": "2.0", "id": 1, "method": "initialize", "params": {"protocolVersion": "2025-06-18", "capabilities": {}, "clientInfo": {"name": "test", "version": "1"}}})).await;
    assert_eq!(st, 200);
    let sid = sid.unwrap();
    rpc(&c, &url, &token, Some(&sid), json!({"jsonrpc": "2.0", "method": "notifications/initialized"})).await;
    let (_, _, got) = rpc(
        &c,
        &url,
        &token,
        Some(&sid),
        json!({"jsonrpc": "2.0", "id": 2, "method": "tools/call", "params": {"name": "get_page", "arguments": {"page_id": page_id}}}),
    )
    .await;
    assert!(tool_text(got.as_ref().unwrap()).contains("quokkaline"), "{got:?}");
    let (_, _, denied) = rpc(&c, &url, &token, Some(&sid), json!({"jsonrpc": "2.0", "id": 3, "method": "tools/call", "params": {"name": "create_page", "arguments": {"space_key": "RO", "title": "Sneaky", "markdown": "nope"}}})).await;
    let denied = denied.unwrap();
    assert!(denied["result"]["isError"] == true || denied.get("error").is_some(), "{denied}");
    assert!(denied.to_string().contains("read-only"), "{denied}");
    let (_, tree) = app.get(&owner, "/spaces/RO/tree").await;
    assert!(!tree.to_string().contains("Sneaky"));
}
