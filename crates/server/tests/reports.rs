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

fn decisions_doc(items: &[(&str, &str)]) -> Value {
    json!({"type": "doc", "content": [
        {"type": "paragraph", "content": [{"type": "text", "text": "Notes"}]},
        {"type": "decisionList", "content": items.iter().map(|(text, state)| json!(
            {"type": "decisionItem", "attrs": {"state": state}, "content": [{"type": "paragraph", "content": [{"type": "text", "text": text}]}]}
        )).collect::<Vec<_>>()}
    ]})
}

#[tokio::test(flavor = "multi_thread")]
async fn decision_log_lists_decisions_people_can_see() {
    let app = common::spawn().await;
    let owner = app.setup().await;
    let (member, _) = app.user(&owner, "dora@acme.test", "member").await;
    app.post(&owner, "/spaces", json!({"key": "ARCH", "name": "Architecture", "defaultRole": "viewer"})).await;
    app.post(&owner, "/spaces", json!({"key": "OTHER", "name": "Other", "defaultRole": "viewer"})).await;
    let mk = |space: &'static str, title: &'static str, doc: Value| {
        let app = &app;
        let owner = &owner;
        async move { app.post(owner, "/pages", json!({"spaceKey": space, "title": title, "content": doc})).await.1["id"].as_str().unwrap().to_string() }
    };
    mk("ARCH", "Search backend", decisions_doc(&[("Use Postgres full-text search for several replicas", "DECIDED"), ("Index attachments?", "UNDECIDED")]))
        .await;
    let secret = mk("ARCH", "Vendor shortlist", decisions_doc(&[("Pick vendor B", "DECIDED")])).await;
    mk("ARCH", "No decisions", json!({"type": "doc", "content": [{"type": "paragraph", "content": [{"type": "text", "text": "decisionItem"}]}]})).await;
    mk("OTHER", "Elsewhere", decisions_doc(&[("Unrelated", "DECIDED")])).await;
    let (_, me) = app.get(&owner, "/me").await;
    let owner_id = me["user"]["id"].as_str().unwrap();
    app.put(&owner, &format!("/pages/{secret}/restrictions"), json!({"restrictions": [{"kind": "view", "subjectType": "user", "subjectId": owner_id}]})).await;

    let texts = |v: &Value| v.as_array().unwrap().iter().map(|d| d["text"].as_str().unwrap().to_string()).collect::<Vec<_>>();
    assert_eq!(app.get(&owner, "/reports/decisions").await.0, 400, "a space is required");
    let (st, all) = app.get(&owner, "/reports/decisions?spaces=ARCH").await;
    assert_eq!(st, 200, "{all}");
    let mut t = texts(&all);
    t.sort();
    assert_eq!(t, vec!["Index attachments?", "Pick vendor B", "Use Postgres full-text search for several replicas"]);
    let (_, decided) = app.get(&member, "/reports/decisions?spaces=ARCH&status=decided").await;
    assert_eq!(texts(&decided), vec!["Use Postgres full-text search for several replicas"], "restricted page and undecided item left out");
    assert_eq!(decided[0]["pageTitle"], "Search backend");
}

#[tokio::test(flavor = "multi_thread")]
async fn typed_page_properties_panel_search_and_reports() {
    let app = common::spawn().await;
    let owner = app.setup().await;
    let (member, member_id) = app.user(&owner, "rita@acme.test", "member").await;
    let (guest, _) = app.user(&owner, "gus@acme.test", "guest").await;
    app.post(&owner, "/spaces", json!({"key": "PRJ", "name": "Projects"})).await;
    let mut ids = std::collections::HashMap::new();
    for title in ["Apollo", "Borealis", "Secret", "Plain"] {
        let (_, page) = app.post(&owner, "/pages", json!({"spaceKey": "PRJ", "title": title})).await;
        ids.insert(title, page["id"].as_str().unwrap().to_string());
    }
    let (_, me) = app.get(&owner, "/me").await;
    let owner_id = me["user"]["id"].as_str().unwrap().to_string();
    app.put(
        &owner,
        &format!("/pages/{}/restrictions", ids["Secret"]),
        json!({"restrictions": [{"kind": "view", "subjectType": "user", "subjectId": owner_id}]}),
    )
    .await;

    // every type, stored normalised
    let path = |t: &str| format!("/pages/{}/properties", ids[t]);
    let (st, r) = app
        .put(
            &owner,
            &path("Apollo"),
            json!({"properties": [
                {"key": "Status", "type": "select", "value": "Done"},
                {"key": "  Effort ", "type": "number", "value": "12"},
                {"key": "Due", "type": "date", "value": "2026-12-01"},
                {"key": "Lead", "type": "user", "value": member_id},
                {"key": "Spec", "type": "url", "value": "https://example.com/spec"},
                {"key": "Parent project", "type": "page", "value": ids["Secret"]},
                {"key": "Notes", "type": "text", "value": "kumquat harvest"},
                {"key": "Budget", "type": "number", "value": null}
            ]}),
        )
        .await;
    assert_eq!(st, 200, "{r}");
    let props = r["properties"].as_array().unwrap();
    assert_eq!(props.len(), 8);
    assert_eq!(props[1], json!({"key": "Effort", "type": "number", "value": 12, "label": null}));
    assert_eq!(props[3]["label"], "rita");
    assert_eq!(props[5]["label"], "Secret", "the owner sees the linked page's title");
    assert_eq!(props[7]["value"], Value::Null, "a property can be empty");

    // page view: the member sees the properties, but not the title of a page they can't open
    let (_, page) = app.get(&member, &format!("/pages/{}", ids["Apollo"])).await;
    let props = page["properties"].as_array().unwrap();
    assert_eq!(props[0]["value"], "Done");
    assert_eq!(props[5]["value"], ids["Secret"].as_str());
    assert_eq!(props[5]["label"], Value::Null);

    // validation
    for (bad, why) in [
        (json!([{"key": "X", "type": "colour", "value": "red"}]), "unknown type"),
        (json!([{"key": "X", "type": "number", "value": "lots"}]), "not a number"),
        (json!([{"key": "X", "type": "date", "value": "1.12.2026"}]), "not ISO"),
        (json!([{"key": "X", "type": "url", "value": "javascript:alert(1)"}]), "not a web address"),
        (json!([{"key": "X", "type": "user", "value": ids["Plain"]}]), "not a person"),
        (json!([{"key": "X", "type": "page", "value": owner_id}]), "not a page"),
        (json!([{"key": "X", "type": "text", "value": "a"}, {"key": "x", "type": "text", "value": "b"}]), "duplicate key"),
        (json!([{"key": " ", "type": "text", "value": "a"}]), "empty key"),
    ] {
        let (st, _) = app.put(&owner, &path("Borealis"), json!({ "properties": bad })).await;
        assert_eq!(st, 400, "{why}");
    }
    // the member can't link a page they can't see; the guest can't edit at all
    let (st, _) = app.put(&member, &path("Borealis"), json!({"properties": [{"key": "P", "type": "page", "value": ids["Secret"]}]})).await;
    assert_eq!(st, 400);
    let (st, _) = app.put(&guest, &path("Borealis"), json!({"properties": [{"key": "Status", "type": "select", "value": "Open"}]})).await;
    assert!(st == 403 || st == 404, "{st}");
    let (st, _) = app
        .put(
            &member,
            &path("Borealis"),
            json!({"properties": [{"key": "Status", "type": "select", "value": "Open"}, {"key": "Effort", "type": "number", "value": 2.5}]}),
        )
        .await;
    assert_eq!(st, 200);

    // searchable by value
    let hits = app
        .eventually(|| async {
            let (_, h) = app.get(&member, "/search?q=kumquat").await;
            (!h.as_array().unwrap().is_empty()).then_some(h)
        })
        .await;
    assert_eq!(hits[0]["title"], "Apollo");

    // suggestions for the space: most used names first, with their values
    let (_, keys) = app.get(&member, "/spaces/PRJ/properties").await;
    let status = keys.as_array().unwrap().iter().find(|k| k["key"] == "Status").unwrap();
    assert_eq!(status["type"], "select");
    assert_eq!(status["pages"], 2);
    assert_eq!(status["values"], json!(["Done", "Open"]));

    // properties report: pages with properties in a space, typed values first, then filters
    let (_, r) = app.get(&member, "/reports/properties?spaces=PRJ").await;
    let rows = r["rows"].as_array().unwrap();
    assert_eq!(rows.len(), 2, "pages without properties are left out: {r}");
    assert_eq!(r["columns"][0], "Status");
    let apollo = rows.iter().find(|x| x["page"]["title"] == "Apollo").unwrap();
    assert_eq!(apollo["properties"]["Lead"], "rita");
    assert_eq!(apollo["properties"]["Parent project"], "", "hidden page title stays hidden");
    assert_eq!(apollo["values"]["Spec"]["type"], "url");
    let titles_where = |w: &'static str| {
        let app = &app;
        let member = &member;
        async move {
            let (_, r) = app.get(member, &format!("/reports/properties?spaces=PRJ&where={}", urlencode(w))).await;
            let mut t: Vec<String> = r["rows"].as_array().unwrap().iter().map(|x| x["page"]["title"].as_str().unwrap().to_string()).collect();
            t.sort();
            t
        }
    };
    assert_eq!(titles_where("Status = done").await, vec!["Apollo"]);
    assert_eq!(titles_where("Effort >= 3").await, vec!["Apollo"], "numbers compare as numbers (12 >= 3, 2.5 < 3)");
    assert_eq!(titles_where("Effort < 10; Status != Done").await, vec!["Borealis"]);
    assert_eq!(titles_where("Due < 2027-01-01").await, vec!["Apollo"]);
    assert_eq!(titles_where("Lead ~ rit").await, vec!["Apollo"]);
    // with a label, the Confluence-style report keeps working alongside typed properties
    app.put(&owner, &format!("/pages/{}/labels", ids["Borealis"]), json!({"labels": ["project"]})).await;
    let (_, r) = app.get(&member, "/reports/properties?labels=project").await;
    assert_eq!(r["rows"].as_array().unwrap().len(), 1);
    assert_eq!(r["rows"][0]["properties"]["Effort"], "2.5");

    // copies keep their properties
    let (st, copy) = app.post(&owner, &format!("/pages/{}/copy", ids["Apollo"]), json!({})).await;
    assert_eq!(st, 201, "{copy}");
    let (_, c) = app.get(&owner, &format!("/pages/{}/properties", copy["id"].as_str().unwrap())).await;
    assert_eq!(c["properties"].as_array().unwrap().len(), 8);
}

fn urlencode(s: &str) -> String {
    s.bytes().map(|b| if b.is_ascii_alphanumeric() || b"-_.~".contains(&b) { (b as char).to_string() } else { format!("%{b:02X}") }).collect()
}

#[tokio::test(flavor = "multi_thread")]
async fn task_reports_filter_by_assignee_and_due_date() {
    let app = common::spawn().await;
    let owner = app.setup().await;
    let (rita, rita_id) = app.user(&owner, "rita@acme.test", "member").await;
    let (_, me) = app.get(&owner, "/me").await;
    let owner_id = me["user"]["id"].as_str().unwrap().to_string();
    app.post(&owner, "/spaces", json!({"key": "PLAN", "name": "Planning"})).await;
    let task = |text: &str, who: &str, due: &str| {
        let mut c = vec![json!({"type": "text", "text": format!("{text} ")})];
        if !who.is_empty() {
            c.push(json!({"type": "mention", "attrs": {"id": who, "label": "x"}}));
        }
        if !due.is_empty() {
            c.push(json!({"type": "date", "attrs": {"date": due}}));
        }
        json!({"type": "taskItem", "attrs": {"checked": false}, "content": [{"type": "paragraph", "content": c}]})
    };
    let doc = json!({"type": "doc", "content": [{"type": "taskList", "content": [
        task("Book venue", &rita_id, "2026-12-01"),
        task("Order food", &rita_id, "2027-01-15"),
        task("Send invites", &owner_id, "2026-12-10"),
        task("Pick a date", "", ""),
    ]}]});
    let (st, _) = app.post(&owner, "/pages", json!({"spaceKey": "PLAN", "title": "Party", "content": doc})).await;
    assert_eq!(st, 201);
    let texts = |v: &Value| -> Vec<String> { v.as_array().unwrap().iter().map(|t| t["text"].as_str().unwrap().to_string()).collect() };

    let (_, r) = app.get(&owner, &format!("/reports/tasks?spaces=PLAN&assignees={rita_id}")).await;
    assert_eq!(texts(&r), vec!["Book venue", "Order food"]);
    let (_, r) = app.get(&rita, "/reports/tasks?spaces=PLAN&assignees=me").await;
    assert_eq!(texts(&r), vec!["Book venue", "Order food"], "me = the reader");
    let (_, r) = app.get(&owner, "/reports/tasks?spaces=PLAN&dueFrom=2026-12-05&dueTo=31-12-2026").await;
    assert_eq!(texts(&r), vec!["Send invites"], "Confluence's dd-MM-yyyy works too");
    let (_, r) = app.get(&owner, "/reports/tasks?spaces=PLAN&assignees=unknown-confluence-key").await;
    assert_eq!(r, json!([]), "unknown people match nothing");
    assert_eq!(app.get(&owner, "/reports/tasks?spaces=PLAN&dueFrom=soon").await.0, 400);
    // imported Confluence user keys map to the imported accounts
    sqlx::query("UPDATE users SET external_ref = 'confluence:ff8080817b' WHERE id = $1::uuid").bind(&rita_id).execute(&app.state.db).await.unwrap();
    let (_, r) = app.get(&owner, "/reports/tasks?spaces=PLAN&assignees=ff8080817b").await;
    assert_eq!(texts(&r), vec!["Book venue", "Order food"]);
}
