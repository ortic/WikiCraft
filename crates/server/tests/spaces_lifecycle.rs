//! Archiving, unarchiving and deleting spaces.

mod common;

use serde_json::{Value, json};

#[tokio::test(flavor = "multi_thread")]
async fn archive_is_read_only_and_delete_needs_archive_and_confirmation() {
    let app = common::spawn().await;
    let owner = app.setup().await;
    let (ed, _) = app.user(&owner, "ed@acme.test", "member").await;
    app.post(&owner, "/spaces", json!({"key": "OLD", "name": "Old project", "defaultRole": "editor"})).await;
    let (_, page) = app.post(&owner, "/pages", json!({"spaceKey": "OLD", "title": "Retro notes", "markdown": "zanzibar lessons"})).await;
    let id = page["id"].as_str().unwrap().to_string();
    app.eventually(|| async { (app.get(&ed, "/search?q=zanzibar").await.1.as_array().unwrap().len() == 1).then_some(()) }).await;
    assert_eq!(app.put(&ed, &format!("/pages/{id}/content"), json!({"markdown": "edit ok"})).await.0, 200);

    // delete needs an archived space
    let r = owner.delete(app.url("/admin/spaces/OLD")).json(&json!({"confirm": "OLD"})).send().await.unwrap();
    assert_eq!(r.status(), 400);

    assert_eq!(app.delete(&owner, "/spaces/OLD").await, 204, "archive");
    // read-only for everyone, out of search, still readable
    assert_eq!(app.get(&ed, &format!("/pages/{id}")).await.0, 200);
    assert_eq!(app.put(&ed, &format!("/pages/{id}/content"), json!({"markdown": "nope"})).await.0, 403);
    assert_eq!(app.post(&ed, "/pages", json!({"spaceKey": "OLD", "title": "New"})).await.0, 403);
    assert!(app.get(&ed, "/search?q=zanzibar").await.1.as_array().unwrap().is_empty());
    let (_, spaces) = app.get(&owner, "/admin/spaces").await;
    let old = spaces.as_array().unwrap().iter().find(|s| s["key"] == "OLD").unwrap().clone();
    assert!(old["archivedAt"].is_string());
    assert_eq!(old["pages"], 2, "home + page");
    assert_eq!(app.get(&ed, "/admin/spaces").await.0, 403);

    // unarchive restores editing
    assert_eq!(app.post(&owner, "/admin/spaces/OLD/unarchive", json!({})).await.0, 204);
    assert_eq!(app.put(&ed, &format!("/pages/{id}/content"), json!({"markdown": "edit again"})).await.0, 200);

    // permanent delete: archived + typed key
    app.delete(&owner, "/spaces/OLD").await;
    let r = owner.delete(app.url("/admin/spaces/OLD")).header("x-wikicraft", "1").json(&json!({"confirm": "WRONG"})).send().await.unwrap();
    assert_eq!(r.status(), 400);
    let r = owner.delete(app.url("/admin/spaces/OLD")).header("x-wikicraft", "1").json(&json!({"confirm": "old"})).send().await.unwrap();
    assert_eq!(r.status(), 204);
    assert_eq!(app.get(&owner, "/spaces/OLD").await.0, 404);
    assert_eq!(app.get(&owner, &format!("/pages/{id}")).await.0, 404);
}

#[tokio::test(flavor = "multi_thread")]
async fn renaming_a_space_key_keeps_old_links() {
    let app = common::spawn().await;
    let owner = app.setup().await;
    let (member, _) = app.user(&owner, "mo@acme.test", "member").await;
    let (_, sp) = app.post(&owner, "/spaces", json!({"key": "OPS", "name": "Operations"})).await;
    let home = sp["homePageId"].as_str().unwrap().to_string();
    app.post(&owner, "/spaces", json!({"key": "HR", "name": "People"})).await;

    // only space admins rename; keys are validated and unique (also against former keys)
    assert_eq!(app.patch(&member, "/spaces/OPS", json!({"key": "PLATFORM"})).await.0, 403);
    assert_eq!(app.patch(&owner, "/spaces/OPS", json!({"key": "bad key!"})).await.0, 400);
    assert_eq!(app.patch(&owner, "/spaces/OPS", json!({"key": "hr"})).await.0, 409);
    let (st, s) = app.patch(&owner, "/spaces/OPS", json!({"key": "platform"})).await;
    assert_eq!(st, 200, "{s}");
    assert_eq!(s["key"], "PLATFORM");

    // the old key still resolves (API and Confluence-style links), to the new space
    let (st, s) = app.get(&member, "/spaces/ops").await;
    assert_eq!(st, 200);
    assert_eq!(s["key"], "PLATFORM");
    let (_, p) = app.get(&member, &format!("/pages/{home}")).await;
    assert_eq!(p["spaceKey"], "PLATFORM");
    let (st, _) = app.post(&owner, "/pages", json!({"spaceKey": "OPS", "title": "Via the old key"})).await;
    assert_eq!(st, 201);
    // a new space can't take the old key; renaming back is fine
    let (st, _) = app.post(&owner, "/spaces", json!({"key": "OPS", "name": "Impostor"})).await;
    assert_eq!(st, 409);
    let (st, s) = app.patch(&owner, "/spaces/PLATFORM", json!({"key": "OPS"})).await;
    assert_eq!(st, 200);
    assert_eq!(s["key"], "OPS");
    let (st, s) = app.get(&member, "/spaces/platform").await;
    assert_eq!((st, s["key"].as_str()), (200, Some("OPS")));
}

#[tokio::test(flavor = "multi_thread")]
async fn archived_pages_are_read_only_and_hidden() {
    let app = common::spawn().await;
    let owner = app.setup().await;
    let (member, _) = app.user(&owner, "ed@acme.test", "member").await;
    let (_, sp) = app.post(&owner, "/spaces", json!({"key": "ARC", "name": "Archive test"})).await;
    let home = sp["homePageId"].as_str().unwrap().to_string();
    let (_, a) = app.post(&owner, "/pages", json!({"spaceKey": "ARC", "parentId": home, "title": "Old project", "markdown": "legacy pumpernickel"})).await;
    let a = a["id"].as_str().unwrap().to_string();
    let (_, a1) = app.post(&owner, "/pages", json!({"spaceKey": "ARC", "parentId": a, "title": "Old notes", "markdown": "more pumpernickel"})).await;
    let a1 = a1["id"].as_str().unwrap().to_string();
    app.eventually(|| async {
        let (_, r) = app.get(&member, "/search?q=pumpernickel").await;
        (r.as_array().unwrap().len() == 2).then_some(())
    })
    .await;

    assert_eq!(app.post(&member, &format!("/pages/{home}/archive"), json!({})).await.0, 400, "home page");
    let (st, _) = app.post(&member, &format!("/pages/{a}/archive"), json!({})).await;
    assert_eq!(st, 204);
    assert_eq!(app.post(&member, &format!("/pages/{a}/archive"), json!({})).await.0, 409);

    // gone from the tree and search, still readable by link, but read-only
    let (_, tree) = app.get(&member, "/spaces/ARC/tree").await;
    let titles: Vec<&str> = tree["nodes"].as_array().unwrap().iter().map(|n| n["title"].as_str().unwrap()).collect();
    assert!(!titles.contains(&"Old project") && !titles.contains(&"Old notes"), "{titles:?}");
    let (_, r) = app.get(&member, "/search?q=pumpernickel").await;
    assert_eq!(r.as_array().unwrap().len(), 0);
    let (st, p) = app.get(&member, &format!("/pages/{a1}")).await;
    assert_eq!(st, 200);
    assert_eq!(p["access"]["edit"], false);
    assert_eq!(p["access"]["archivedPage"], a.as_str());
    assert_eq!(app.put(&member, &format!("/pages/{a1}/content"), json!({"markdown": "edit"})).await.0, 403);
    assert_eq!(app.post(&member, &format!("/pages/{a1}/comments"), json!({"text": "hi"})).await.0, 403);
    assert_eq!(app.post(&member, &format!("/pages/{a1}/unarchive"), json!({})).await.0, 400, "restore the archived ancestor instead");

    // the space lists it; restoring brings everything back
    let (_, list) = app.get(&member, "/spaces/ARC/archived").await;
    assert_eq!(list.as_array().unwrap().len(), 1);
    assert_eq!(list[0]["title"], "Old project");
    assert_eq!(list[0]["descendants"], 1);
    assert_eq!(list[0]["canRestore"], true);
    assert_eq!(app.post(&member, &format!("/pages/{a}/unarchive"), json!({})).await.0, 204);
    let (_, p) = app.get(&member, &format!("/pages/{a1}")).await;
    assert_eq!(p["access"]["edit"], true);
    assert!(p["access"]["archivedPage"].is_null());
    let (_, tree) = app.get(&member, "/spaces/ARC/tree").await;
    assert_eq!(tree["nodes"].as_array().unwrap().len(), 3);
}

#[tokio::test(flavor = "multi_thread")]
async fn spaces_from_templates() {
    let app = common::spawn().await;
    let owner = app.setup().await;
    let (_, list) = app.get(&owner, "/space-templates").await;
    let ids: Vec<&str> = list.as_array().unwrap().iter().map(|t| t["id"].as_str().unwrap()).collect();
    assert_eq!(ids, vec!["team", "project", "knowledge", "engineering"]);
    assert_eq!(app.post(&owner, "/spaces", json!({"key": "X", "name": "X", "template": "nope"})).await.0, 400);

    let (st, sp) = app.post(&owner, "/spaces", json!({"key": "APOLLO", "name": "Apollo", "template": "project"})).await;
    assert_eq!(st, 201, "{sp}");
    let (_, tree) = app.get(&owner, "/spaces/APOLLO/tree").await;
    let nodes = tree["nodes"].as_array().unwrap();
    let title_of = |id: &Value| nodes.iter().find(|n| n["id"] == *id).map(|n| n["title"].as_str().unwrap().to_string());
    let titles: Vec<&str> = nodes.iter().map(|n| n["title"].as_str().unwrap()).collect();
    for t in ["Apollo", "Project plan", "Requirements", "Decisions", "Decision: scope of the first release", "Status reports", "Status report", "Retrospective"]
    {
        assert!(titles.contains(&t), "{t} missing: {titles:?}");
    }
    let decision = nodes.iter().find(|n| n["title"] == "Decision: scope of the first release").unwrap();
    assert_eq!(title_of(&decision["parentId"]).as_deref(), Some("Decisions"));
    // the home page lists its children and keeps the template's content
    let (_, home) = app.get(&owner, &format!("/pages/{}", sp["homePageId"].as_str().unwrap())).await;
    let content = home["content"].to_string();
    assert!(content.contains("childPages") && content.contains("Owner"), "{content}");
    let plan = nodes.iter().find(|n| n["title"] == "Project plan").unwrap();
    let (_, plan) = app.get(&owner, &format!("/pages/{}", plan["id"].as_str().unwrap())).await;
    assert!(plan["content"].to_string().contains("Milestones") || plan["content"]["content"].as_array().unwrap().len() > 2, "{plan}");
}

#[tokio::test(flavor = "multi_thread")]
async fn personal_spaces_are_private() {
    let app = common::spawn().await;
    let owner = app.setup().await;
    let (ann, _) = app.user(&owner, "ann.lee@acme.test", "member").await;
    let (bob, _) = app.user(&owner, "bob@acme.test", "member").await;
    let (_, none) = app.get(&ann, "/me/space").await;
    assert!(none["key"].is_null());
    let (st, sp) = app.post(&ann, "/me/space", json!({})).await;
    assert_eq!(st, 201, "{sp}");
    assert_eq!(sp["key"], "~ANNLEE");
    let (st, again) = app.post(&ann, "/me/space", json!({})).await;
    assert_eq!((st, again["id"].clone()), (200, sp["id"].clone()), "one per person");
    let (_, mine) = app.get(&ann, "/me/space").await;
    assert_eq!(mine["key"], "~ANNLEE");
    // Ann administers it; Bob neither sees nor finds it
    let (st, s) = app.get(&ann, "/spaces/~annlee").await;
    assert_eq!((st, s["role"].as_str()), (200, Some("admin")));
    let (st, _) = app.post(&ann, "/pages", json!({"spaceKey": "~ANNLEE", "title": "Draft idea", "markdown": "quokka plan"})).await;
    assert_eq!(st, 201);
    assert_eq!(app.get(&bob, "/spaces/~ANNLEE").await.0, 404);
    let (_, list) = app.get(&bob, "/spaces").await;
    assert!(!list.to_string().contains("~ANNLEE"));
    app.eventually(|| async {
        let (_, r) = app.get(&ann, "/search?q=quokka").await;
        (r.as_array().unwrap().len() == 1).then_some(())
    })
    .await;
    let (_, r) = app.get(&bob, "/search?q=quokka").await;
    assert_eq!(r.as_array().unwrap().len(), 0);
}
