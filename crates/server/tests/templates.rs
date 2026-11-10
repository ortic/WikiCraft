//! Every built-in template creates a page.

mod common;

use serde_json::json;

#[tokio::test(flavor = "multi_thread")]
async fn all_builtin_templates_create_pages() {
    let app = common::spawn().await;
    let owner = app.setup().await;
    app.post(&owner, "/spaces", json!({"key": "TPL", "name": "Templates"})).await;
    let (_, list) = app.get(&owner, "/templates").await;
    let builtins: Vec<String> = list.as_array().unwrap().iter().filter(|t| t["builtin"] == true).map(|t| t["id"].as_str().unwrap().to_string()).collect();
    assert!(builtins.len() >= 12, "{builtins:?}");
    for id in builtins {
        let (st, page) = app.post(&owner, "/pages", json!({"spaceKey": "TPL", "templateId": id})).await;
        assert_eq!(st, 201, "{id}: {page}");
        let (_, p) = app.get(&owner, &format!("/pages/{}", page["id"].as_str().unwrap())).await;
        assert!(!p["title"].as_str().unwrap().contains('{'), "{id}: placeholders filled: {}", p["title"]);
        assert!(p["content"]["content"].as_array().unwrap().len() > 1, "{id}: has content");
        let t = list.as_array().unwrap().iter().find(|t| t["id"] == id.as_str()).unwrap();
        assert!(t["icon"].as_str().unwrap().chars().all(|c| c.is_ascii_lowercase() || c == '-'), "{id}: symbol icon, not emoji");
    }
}

#[tokio::test(flavor = "multi_thread")]
async fn page_icons_and_covers_are_validated() {
    let app = common::spawn().await;
    let owner = app.setup().await;
    app.post(&owner, "/spaces", json!({"key": "DEC", "name": "Decor"})).await;
    let (_, page) = app.post(&owner, "/pages", json!({"spaceKey": "DEC", "title": "Pretty"})).await;
    let id = page["id"].as_str().unwrap().to_string();
    for (body, ok) in [
        (json!({"icon": "rocket:violet"}), true),
        (json!({"icon": "\u{1F680}"}), false),
        (json!({"icon": "x\"><b>"}), false),
        (json!({"cover": "gradient:ocean"}), true),
        (json!({"cover": "gradient:nope"}), false),
        (json!({"cover": "javascript:alert(1)"}), false),
        (json!({"cover": "https://img.example/x.png\") ; background:red"}), false),
        (json!({"cover": "https://images.example.com/cover.jpg"}), true),
    ] {
        let (st, _) = app.patch(&owner, &format!("/pages/{id}"), body.clone()).await;
        assert_eq!(st == 204, ok, "{body}");
    }
    let (_, p) = app.get(&owner, &format!("/pages/{id}")).await;
    assert_eq!(p["icon"], "rocket:violet");
    assert_eq!(p["cover"], "https://images.example.com/cover.jpg");
    let (st, _) = app.patch(&owner, &format!("/pages/{id}"), json!({"cover": ""})).await;
    assert_eq!(st, 204);
    assert!(app.get(&owner, &format!("/pages/{id}")).await.1["cover"].is_null());
}

#[tokio::test(flavor = "multi_thread")]
async fn template_variables_are_asked_for_and_filled_in() {
    let app = common::spawn().await;
    let owner = app.setup().await;
    app.post(&owner, "/spaces", json!({"key": "VAR", "name": "Variables"})).await;

    // a built-in asks for what its title needs
    let (_, list) = app.get(&owner, "/templates?spaceKey=VAR").await;
    let postmortem = list.as_array().unwrap().iter().find(|t| t["id"] == "builtin:postmortem").unwrap();
    assert_eq!(postmortem["variables"], json!(["Incident"]));
    let (_, p) = app.post(&owner, "/pages", json!({"spaceKey": "VAR", "templateId": "builtin:postmortem", "variables": {"Incident": "API outage"}})).await;
    let (_, page) = app.get(&owner, &format!("/pages/{}", p["id"].as_str().unwrap())).await;
    assert_eq!(page["title"], "Postmortem: API outage");

    // a template made from a page: variables keep their formatting; code is left alone
    let source = json!({"type": "doc", "content": [
        {"type": "paragraph", "content": [
            {"type": "text", "text": "Customer: "},
            {"type": "text", "text": "{{Customer}}", "marks": [{"type": "bold"}]},
            {"type": "text", "text": ", contact {{Contact}}, region {{Region}}."}
        ]},
        {"type": "codeBlock", "content": [{"type": "text", "text": "Hello {{ user.name }}"}]}
    ]});
    let (_, src) = app.post(&owner, "/pages", json!({"spaceKey": "VAR", "title": "Onboarding source", "content": source})).await;
    let (st, t) = app.post(&owner, "/templates", json!({"name": "Customer onboarding", "pageId": src["id"], "spaceKey": "VAR"})).await;
    assert_eq!(st, 201, "{t}");
    let tid = t["id"].as_str().unwrap();
    let (_, preview) = app.get(&owner, &format!("/templates/{tid}/preview")).await;
    assert_eq!(preview["variables"], json!(["Customer", "Contact", "Region"]));
    assert!(preview["html"].as_str().unwrap().contains("Customer"), "unanswered variables show as hints");

    let (_, p) = app
        .post(&owner, "/pages", json!({"spaceKey": "VAR", "templateId": tid, "title": "ACME onboarding", "variables": {"Customer": "ACME", "Region": ""}}))
        .await;
    let (_, page) = app.get(&owner, &format!("/pages/{}", p["id"].as_str().unwrap())).await;
    let para = &page["content"]["content"][0]["content"];
    assert_eq!(para[1]["text"], "ACME");
    assert_eq!(para[1]["marks"][0]["type"], "bold");
    // no answer: a hint to fill in; an empty answer: removed
    assert_eq!(para[3]["type"], "instructionalText", "{para}");
    assert_eq!(para[3]["attrs"]["text"], "Contact");
    assert_eq!(para[4]["text"], ", region .");
    assert_eq!(page["content"]["content"][1]["content"][0]["text"], "Hello {{ user.name }}");
}
