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

#[tokio::test(flavor = "multi_thread")]
async fn blueprints_file_pages_under_an_index_page() {
    let app = common::spawn().await;
    let owner = app.setup().await;
    app.post(&owner, "/spaces", json!({"key": "BP", "name": "Blueprints"})).await;
    let (_, list) = app.get(&owner, "/templates?spaceKey=BP").await;
    let notes = list.as_array().unwrap().iter().find(|t| t["id"] == "builtin:meeting-notes").unwrap();
    assert_eq!(notes["filedUnder"], "Meeting notes");

    let create = |body: serde_json::Value| {
        let (app, owner) = (&app, &owner);
        async move { app.post(owner, "/pages", body).await.1 }
    };
    let first = create(json!({"spaceKey": "BP", "templateId": "builtin:meeting-notes"})).await;
    let second = create(json!({"spaceKey": "BP", "templateId": "builtin:meeting-notes"})).await;
    let index = first["parentId"].as_str().expect("filed under the index page").to_string();
    assert_eq!(second["parentId"], index.as_str(), "one index page per space");
    let (_, idx) = app.get(&owner, &format!("/pages/{index}")).await;
    assert_eq!(idx["title"], "Meeting notes");
    assert!(idx["parentId"].is_null());
    assert!(idx["content"].to_string().contains("childPages"), "it lists its pages");
    let (_, page) = app.get(&owner, &format!("/pages/{}", first["id"].as_str().unwrap())).await;
    assert!(page["labels"].as_array().unwrap().iter().any(|l| l == "meeting-notes"), "{}", page["labels"]);

    // a parent picked by the person wins; templates that aren't blueprints stay where they're made
    let (_, home) = app.get(&owner, "/spaces/BP").await;
    let home_id = home["homePageId"].as_str().unwrap();
    let below = create(json!({"spaceKey": "BP", "templateId": "builtin:meeting-notes", "parentId": home_id})).await;
    assert_eq!(below["parentId"], home_id);
    let okrs = create(json!({"spaceKey": "BP", "templateId": "builtin:okrs"})).await;
    assert!(okrs["parentId"].is_null());
}

#[tokio::test(flavor = "multi_thread")]
async fn managing_templates() {
    let app = common::spawn().await;
    let owner = app.setup().await;
    let (lead, lead_id) = app.user(&owner, "lead@acme.test", "member").await;
    let (ed, ed_id) = app.user(&owner, "ed@acme.test", "member").await;
    app.post(&owner, "/spaces", json!({"key": "MT", "name": "Managed"})).await;
    let grants = |extra: Option<&str>| {
        let mut g =
            vec![json!({"subjectType": "user", "subjectId": lead_id, "role": "admin"}), json!({"subjectType": "user", "subjectId": ed_id, "role": "editor"})];
        if let Some(o) = extra {
            g.push(json!({"subjectType": "user", "subjectId": o, "role": "editor"}));
        }
        json!({"defaultRole": "viewer", "grants": g})
    };
    let (st, b) = app.put(&owner, "/spaces/MT/permissions", grants(None)).await;
    assert!(st < 300, "{st} {b}");
    let (_, src) = app.post(&ed, "/pages", json!({"spaceKey": "MT", "title": "Runbook", "markdown": "Step one"})).await;
    let page = src["id"].as_str().unwrap().to_string();
    // workspace-wide templates are for workspace admins
    assert_eq!(app.post(&ed, "/templates", json!({"name": "Everyone's", "pageId": page})).await.0, 400);
    let (st, wt) = app.post(&owner, "/templates", json!({"name": "Company runbook", "pageId": page})).await;
    assert_eq!(st, 201);
    let wt = wt["id"].as_str().unwrap().to_string();
    let (_, st_) = app.post(&ed, "/templates", json!({"name": "Team runbook", "pageId": page, "spaceKey": "MT"})).await;
    let tt = st_["id"].as_str().unwrap().to_string();
    app.patch(&lead, "/spaces/MT", json!({"defaultTemplate": tt})).await;

    let can = |list: &serde_json::Value, id: &str| list.as_array().unwrap().iter().find(|t| t["id"] == id).map(|t| t["canManage"].as_bool().unwrap());
    let (_, l) = app.get(&ed, "/templates?spaceKey=MT").await;
    assert_eq!((can(&l, &wt), can(&l, &tt), can(&l, "builtin:how-to")), (Some(false), Some(true), Some(false)), "creators manage their own");
    let (_, l) = app.get(&lead, "/templates?spaceKey=MT").await;
    assert_eq!((can(&l, &wt), can(&l, &tt)), (Some(false), Some(true)), "space admins manage the space's templates");
    let (_, l) = app.get(&owner, "/templates?spaceKey=MT").await;
    assert_eq!((can(&l, &wt), can(&l, &tt)), (Some(true), Some(true)));

    // rename, describe, new content
    assert_eq!(app.patch(&ed, &format!("/templates/{wt}"), json!({"name": "Mine now"})).await.0, 403);
    assert_eq!(app.patch(&lead, &format!("/templates/{tt}"), json!({"name": "  "})).await.0, 400);
    assert_eq!(app.patch(&lead, &format!("/templates/{tt}"), json!({"name": " Team  runbook v2 ", "description": "For on-call"})).await.0, 204);
    app.put(&ed, &format!("/pages/{page}/content"), json!({"markdown": "Step one\n\nStep two"})).await;
    assert_eq!(app.patch(&ed, &format!("/templates/{tt}"), json!({"pageId": page})).await.0, 204);
    let (_, l) = app.get(&ed, "/templates?spaceKey=MT").await;
    let t = l.as_array().unwrap().iter().find(|t| t["id"] == tt.as_str()).unwrap().clone();
    assert_eq!((t["name"].as_str(), t["description"].as_str()), (Some("Team runbook v2"), Some("For on-call")));
    let (_, pv) = app.get(&ed, &format!("/templates/{tt}/preview")).await;
    assert!(pv["html"].as_str().unwrap().contains("Step two"), "{pv}");

    // deleting: not by other editors; the space stops starting pages from it
    let (other, other_id) = app.user(&owner, "other@acme.test", "member").await;
    app.put(&owner, "/spaces/MT/permissions", grants(Some(&other_id))).await;
    assert_eq!(app.delete(&other, &format!("/templates/{tt}")).await, 403);
    assert_eq!(app.delete(&lead, &format!("/templates/{tt}")).await, 204);
    let (_, s) = app.get(&ed, "/spaces/MT").await;
    assert!(s["defaultTemplate"].is_null(), "{s}");
    assert_eq!(app.delete(&lead, &format!("/templates/{wt}")).await, 403, "workspace templates: workspace admins");
    assert_eq!(app.delete(&owner, &format!("/templates/{wt}")).await, 204);
}
