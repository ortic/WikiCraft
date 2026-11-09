//! Report queries behind Confluence's dynamic macros (contentbylabel, recently-updated,
//! page properties report, task report) — including permission filtering.

mod common;

use serde_json::{Value, json};

fn props_doc(owner: &str, status: &str) -> Value {
    let row = |k: &str, v: &str| {
        json!({"type": "tableRow", "content": [
            {"type": "tableHeader", "content": [{"type": "paragraph", "content": [{"type": "text", "text": k}]}]},
            {"type": "tableCell", "content": [{"type": "paragraph", "content": [{"type": "text", "text": v}]}]}
        ]})
    };
    json!({"type": "doc", "content": [
        {"type": "table", "content": [row("Owner", owner), row("Status", status)]},
        {"type": "paragraph", "content": [{"type": "text", "text": "Details follow."}]}
    ]})
}

fn titles(v: &Value) -> Vec<String> {
    v.as_array().unwrap().iter().map(|p| p["title"].as_str().unwrap().to_string()).collect()
}

#[tokio::test(flavor = "multi_thread")]
async fn report_macros_respect_labels_spaces_and_permissions() {
    let app = common::spawn().await;
    let owner = app.setup().await;
    let (member, _) = app.user(&owner, "rita@acme.test", "member").await;
    app.post(&owner, "/spaces", json!({"key": "PRJ", "name": "Projects"})).await;
    app.post(&owner, "/spaces", json!({"key": "OPS", "name": "Ops"})).await;

    let mut ids = std::collections::HashMap::new();
    for (space, title, labels, doc) in [
        ("PRJ", "Apollo", vec!["project", "active"], props_doc("Ann", "Green")),
        ("PRJ", "Borealis", vec!["project"], props_doc("Ben", "Amber")),
        ("OPS", "Cassini", vec!["project", "active"], props_doc("Cyd", "Red")),
        ("PRJ", "Unlabelled", vec![], props_doc("X", "Y")),
    ] {
        let (st, page) = app.post(&owner, "/pages", json!({"spaceKey": space, "title": title, "content": doc})).await;
        assert_eq!(st, 201);
        let id = page["id"].as_str().unwrap().to_string();
        app.put(&owner, &format!("/pages/{id}/labels"), json!({"labels": labels})).await;
        ids.insert(title, id);
    }
    // Cassini is restricted to the owner
    let (_, me) = app.get(&owner, "/me").await;
    let owner_id = me["user"]["id"].as_str().unwrap();
    let (st, _) = app
        .put(
            &owner,
            &format!("/pages/{}/restrictions", ids["Cassini"]),
            json!({"restrictions": [{"kind": "view", "subjectType": "user", "subjectId": owner_id}]}),
        )
        .await;
    assert_eq!(st, 204);

    // content by label: any label, all labels, space filter, permission filtering
    let (_, r) = app.get(&owner, "/reports/pages?labels=project&sort=title").await;
    assert_eq!(titles(&r), vec!["Apollo", "Borealis", "Cassini"]);
    let (_, r) = app.get(&member, "/reports/pages?labels=project&sort=title").await;
    assert_eq!(titles(&r), vec!["Apollo", "Borealis"], "restricted pages never show up");
    let (_, r) = app.get(&owner, "/reports/pages?labels=project,active&operator=all&sort=title").await;
    assert_eq!(titles(&r), vec!["Apollo", "Cassini"]);
    let (_, r) = app.get(&owner, "/reports/pages?labels=project&spaces=ops").await;
    assert_eq!(titles(&r), vec!["Cassini"]);
    assert_eq!(r[0]["labels"], json!(["active", "project"]));
    // recently updated: newest first, limited
    let (_, r) = app.get(&owner, "/reports/pages?spaces=PRJ&max=2").await;
    assert_eq!(r.as_array().unwrap().len(), 2);
    assert_eq!(r[0]["title"], "Unlabelled");

    // page properties report
    let (_, r) = app.get(&member, "/reports/properties?labels=project").await;
    assert_eq!(r["columns"], json!(["Owner", "Status"]));
    let rows = r["rows"].as_array().unwrap();
    assert_eq!(rows.len(), 2);
    let apollo = rows.iter().find(|x| x["page"]["title"] == "Apollo").unwrap();
    assert_eq!(apollo["properties"]["Owner"], "Ann");
    assert_eq!(apollo["properties"]["Status"], "Green");
    let (_, r) = app.get(&member, "/reports/properties").await;
    assert_eq!(r["rows"].as_array().unwrap().len(), 0, "a properties report needs a label");

    // task report
    let task_doc = json!({"type": "doc", "content": [{"type": "taskList", "content": [
        {"type": "taskItem", "attrs": {"checked": false}, "content": [{"type": "paragraph", "content": [{"type": "text", "text": "Draft budget"}]}]},
        {"type": "taskItem", "attrs": {"checked": true}, "content": [{"type": "paragraph", "content": [{"type": "text", "text": "Kick-off"}]}]}
    ]}]});
    app.post(&owner, "/pages", json!({"spaceKey": "PRJ", "title": "Plan", "content": task_doc})).await;
    let (_, r) = app.get(&member, "/reports/tasks?spaces=PRJ").await;
    let texts: Vec<&str> = r.as_array().unwrap().iter().map(|t| t["text"].as_str().unwrap()).collect();
    assert_eq!(texts, vec!["Draft budget"]);
    let (_, r) = app.get(&member, "/reports/tasks?spaces=PRJ&status=all").await;
    assert_eq!(r.as_array().unwrap().len(), 2);
}
