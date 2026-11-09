//! Archiving, unarchiving and deleting spaces.

mod common;

use serde_json::json;

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
