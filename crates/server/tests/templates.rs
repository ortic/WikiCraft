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
