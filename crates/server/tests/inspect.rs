//! Inspect permissions: the explanation matches what the API enforces.

mod common;

use serde_json::{Value, json};

fn texts(v: &Value) -> String {
    v["steps"].as_array().unwrap().iter().map(|s| s["text"].as_str().unwrap()).collect::<Vec<_>>().join("\n")
}

#[tokio::test(flavor = "multi_thread")]
async fn explains_space_roles_groups_and_inherited_restrictions() {
    let app = common::spawn().await;
    let owner = app.setup().await;
    let (fiona, fiona_id) = app.user(&owner, "fiona@acme.test", "member").await;
    let (_, mark_id) = app.user(&owner, "mark@acme.test", "member").await;
    let (_, g) = app.post(&owner, "/groups", json!({"name": "Finance"})).await;
    let gid = g["id"].as_str().unwrap().to_string();
    app.post(&owner, &format!("/groups/{gid}/members"), json!({"userId": fiona_id})).await;

    // private space; Finance may view; the "Budget" subtree is restricted to Finance
    let (_, sp) = app.post(&owner, "/spaces", json!({"key": "FIN", "name": "Finance", "defaultRole": "none"})).await;
    let home = sp["homePageId"].as_str().unwrap().to_string();
    let (st, _) = app
        .put(
            &owner,
            "/spaces/FIN/permissions",
            json!({"grants": [{"subjectType": "group", "subjectId": gid, "role": "editor"}, {"subjectType": "user", "subjectId": mark_id, "role": "viewer"}]}),
        )
        .await;
    assert_eq!(st, 204);
    let (_, budget) = app.post(&owner, "/pages", json!({"spaceKey": "FIN", "parentId": home, "title": "Budget"})).await;
    let budget = budget["id"].as_str().unwrap().to_string();
    let (_, q1) = app.post(&owner, "/pages", json!({"spaceKey": "FIN", "parentId": budget, "title": "Q1"})).await;
    let q1 = q1["id"].as_str().unwrap().to_string();
    let (st, _) =
        app.put(&owner, &format!("/pages/{budget}/restrictions"), json!({"restrictions": [{"kind": "view", "subjectType": "group", "subjectId": gid}]})).await;
    assert_eq!(st, 204);

    // Fiona: editor through the group, on the inherited restriction list
    let (st, r) = app.get(&owner, &format!("/pages/{q1}/access/{fiona_id}")).await;
    assert_eq!(st, 200, "{r}");
    assert_eq!(
        r["access"],
        json!({"view": true, "comment": true, "edit": true, "admin": true, "perms": [
            "page.add", "page.delete", "blog.add", "blog.delete", "comment.add", "comment.delete",
            "attachment.add", "attachment.delete", "restrict", "export", "archive", "delete.own"
        ]})
    );
    assert_eq!(r["spaceRole"], "editor");
    let t = texts(&r);
    assert!(t.contains("Space role editor — member of group “Finance”"), "{t}");
    assert!(t.contains("View restriction on ancestor “Budget” (inherited): only group “Finance” — on the list"), "{t}");

    // Mark: viewer by direct grant, but not on the restriction list
    let (_, r) = app.get(&owner, &format!("/pages/{q1}/access/{mark_id}")).await;
    assert_eq!(r["access"]["view"], false);
    let t = texts(&r);
    assert!(t.contains("Space role viewer — granted directly") && t.contains("— not on the list"), "{t}");
    // ... which is what the API enforces
    let (mark, _) = (common::client(), ());
    let (st, _) = app.post(&mark, "/auth/login", json!({"email": "mark@acme.test", "password": "password123"})).await;
    assert_eq!(st, 200);
    assert_eq!(app.get(&mark, &format!("/pages/{q1}")).await.0, 404);
    assert_eq!(app.get(&mark, &format!("/pages/{home}")).await.0, 200);

    // the owner bypasses restrictions as a workspace admin; anonymous has no access
    let (_, me) = app.get(&owner, "/me").await;
    let (_, r) = app.get(&owner, &format!("/pages/{q1}/access/{}", me["user"]["id"].as_str().unwrap())).await;
    assert_eq!(r["access"]["admin"], true);
    assert!(texts(&r).contains("Workspace admins have full access"));
    let (_, r) = app.get(&owner, &format!("/pages/{q1}/access/anonymous")).await;
    assert_eq!(r["access"]["view"], false);
    assert!(texts(&r).contains("doesn't allow anonymous access"));

    // only people who administer the page may inspect it
    let (st, _) = app.get(&mark, &format!("/pages/{home}/access/{fiona_id}")).await;
    assert_eq!(st, 403);
    let (st, _) = app.get(&fiona, &format!("/pages/{q1}/access/{mark_id}")).await;
    assert_eq!(st, 200, "editors on the page administer it");
}

#[tokio::test(flavor = "multi_thread")]
async fn request_access_notifies_page_admins_only() {
    let app = common::spawn().await;
    let owner = app.setup().await;
    let (alice, alice_id) = app.user(&owner, "alice@acme.test", "member").await;
    let (bob, _) = app.user(&owner, "bob@acme.test", "member").await;
    let (carol, _) = app.user(&owner, "carol@acme.test", "member").await;
    // HR space: alice administers it, carol may view, bob can't see it
    let (_, sp) = app.post(&owner, "/spaces", json!({"key": "HR", "name": "HR", "defaultRole": "none"})).await;
    let home = sp["homePageId"].as_str().unwrap().to_string();
    let (_, me) = app.get(&carol, "/me").await;
    let carol_id = me["user"]["id"].as_str().unwrap().to_string();
    app.put(
        &owner,
        "/spaces/HR/permissions",
        json!({"grants": [{"subjectType": "user", "subjectId": alice_id, "role": "admin"}, {"subjectType": "user", "subjectId": carol_id, "role": "viewer"}]}),
    )
    .await;
    let (_, pg) = app.post(&owner, "/pages", json!({"spaceKey": "HR", "parentId": home, "title": "Salary bands"})).await;
    let pg = pg["id"].as_str().unwrap().to_string();
    assert_eq!(app.get(&bob, &format!("/pages/{pg}")).await.0, 404);

    for _ in 0..2 {
        let (st, _) = app.post(&bob, &format!("/pages/{pg}/access-request"), json!({"message": "Need it for planning"})).await;
        assert_eq!(st, 204);
    }
    // unknown pages answer the same
    let (st, _) = app.post(&bob, &format!("/pages/{}/access-request", uuid::Uuid::new_v4()), json!({})).await;
    assert_eq!(st, 204);

    let (_, n) = app.get(&alice, "/notifications").await;
    let reqs: Vec<_> = n["items"].as_array().unwrap().iter().filter(|i| i["kind"] == "access_request").collect();
    assert_eq!(reqs.len(), 1, "one request per day: {n}");
    assert_eq!(reqs[0]["actorName"], "bob");
    assert_eq!(reqs[0]["pageTitle"], "Salary bands");
    assert_eq!(reqs[0]["data"]["excerpt"], "Need it for planning");
    // viewers aren't asked; workspace admins aren't either when the space has admins
    for c in [&carol, &owner] {
        let (_, n) = app.get(c, "/notifications").await;
        assert!(!n["items"].as_array().unwrap().iter().any(|i| i["kind"] == "access_request"), "{n}");
    }
}

#[tokio::test(flavor = "multi_thread")]
async fn granting_a_request_opens_the_page_and_the_restricted_pages_above() {
    let app = common::spawn().await;
    let owner = app.setup().await;
    let (alice, alice_id) = app.user(&owner, "alice@acme.test", "member").await;
    let (bob, bob_id) = app.user(&owner, "bob@acme.test", "member").await;
    let (carol, carol_id) = app.user(&owner, "carol@acme.test", "member").await;
    let (_, sp) = app.post(&owner, "/spaces", json!({"key": "HR", "name": "HR", "defaultRole": "none"})).await;
    let home = sp["homePageId"].as_str().unwrap().to_string();
    // alice edits the space (so she administers its pages) but isn't a space admin
    app.put(&owner, "/spaces/HR/permissions", json!({"grants": [{"subjectType": "user", "subjectId": alice_id, "role": "editor"}]})).await;
    let (_, me) = app.get(&owner, "/me").await;
    let owner_id = me["user"]["id"].as_str().unwrap().to_string();
    // Pay (restricted to owner and alice) > Bands (restricted too) > Q1 (open below)
    let (_, pay) = app.post(&owner, "/pages", json!({"spaceKey": "HR", "parentId": home, "title": "Pay"})).await;
    let pay = pay["id"].as_str().unwrap().to_string();
    let (_, bands) = app.post(&owner, "/pages", json!({"spaceKey": "HR", "parentId": pay, "title": "Bands"})).await;
    let bands = bands["id"].as_str().unwrap().to_string();
    let only = |ids: &[&str]| json!({"restrictions": ids.iter().map(|i| json!({"kind": "view", "subjectType": "user", "subjectId": i})).collect::<Vec<_>>()});
    app.put(&owner, &format!("/pages/{pay}/restrictions"), only(&[&owner_id, &alice_id])).await;
    app.put(&owner, &format!("/pages/{bands}/restrictions"), only(&[&owner_id, &alice_id])).await;
    let (_, q1) = app.post(&owner, "/pages", json!({"spaceKey": "HR", "parentId": bands, "title": "Q1 review"})).await;
    let q1 = q1["id"].as_str().unwrap().to_string();

    // bob has no access to the space: alice (no space admin) can't add him; the owner can
    app.post(&bob, &format!("/pages/{q1}/access-request"), json!({"message": "please"})).await;
    let (_, n) = app.get(&owner, "/notifications").await;
    assert!(
        n["items"].as_array().unwrap().iter().any(|i| i["kind"] == "access_request" && i["data"]["requesterId"] == bob_id.as_str()),
        "space admins are asked: {n}"
    );
    let (st, body) = app.post(&alice, &format!("/pages/{q1}/access-grant"), json!({"userId": bob_id})).await;
    assert_eq!(st, 403, "{body}");
    assert!(body.to_string().contains("space admin"), "{body}");
    assert_eq!(app.post(&carol, &format!("/pages/{q1}/access-grant"), json!({"userId": bob_id})).await.0, 404, "carol can't see it at all");
    let (st, r) = app.post(&owner, &format!("/pages/{q1}/access-grant"), json!({"userId": bob_id})).await;
    assert_eq!(st, 200, "{r}");
    assert_eq!(r, json!({"granted": true, "spaceAccess": true, "pagesAbove": 2}));
    let (st, page) = app.get(&bob, &format!("/pages/{q1}")).await;
    assert_eq!(st, 200);
    assert_eq!(page["access"]["edit"], false, "read access only");
    let (_, n) = app.get(&bob, "/notifications").await;
    assert!(n["items"].as_array().unwrap().iter().any(|i| i["kind"] == "access_granted" && i["pageTitle"] == "Q1 review"), "{n}");
    // the request is closed for everyone who got it
    let (_, n) = app.get(&owner, "/notifications").await;
    assert!(n["items"].as_array().unwrap().iter().filter(|i| i["kind"] == "access_request").all(|i| !i["readAt"].is_null()), "{n}");

    // carol is in the space: alice can open the restricted pages for her
    app.put(
        &owner,
        "/spaces/HR/permissions",
        json!({"grants": [{"subjectType": "user", "subjectId": alice_id, "role": "editor"}, {"subjectType": "user", "subjectId": carol_id, "role": "viewer"}, {"subjectType": "user", "subjectId": bob_id, "role": "viewer"}]}),
    )
    .await;
    assert_eq!(app.get(&carol, &format!("/pages/{bands}")).await.0, 404);
    let (st, r) = app.post(&alice, &format!("/pages/{bands}/access-grant"), json!({"userId": carol_id})).await;
    assert_eq!(st, 200, "{r}");
    assert_eq!(r["spaceAccess"], false);
    assert_eq!(app.get(&carol, &format!("/pages/{bands}")).await.0, 200);
    // granting again changes nothing; dismissing closes a request without granting
    assert_eq!(app.post(&alice, &format!("/pages/{bands}/access-grant"), json!({"userId": carol_id})).await.1["hadAccess"], true);
    let (_, secret) = app.post(&owner, "/pages", json!({"spaceKey": "HR", "parentId": home, "title": "Secret"})).await;
    let secret = secret["id"].as_str().unwrap().to_string();
    app.put(&owner, &format!("/pages/{secret}/restrictions"), only(&[&owner_id])).await;
    app.post(&carol, &format!("/pages/{secret}/access-request"), json!({})).await;
    assert_eq!(app.post(&owner, &format!("/pages/{secret}/access-dismiss"), json!({"userId": carol_id})).await.0, 204);
    let (_, n) = app.get(&owner, "/notifications").await;
    assert!(n["items"].as_array().unwrap().iter().filter(|i| i["kind"] == "access_request").all(|i| !i["readAt"].is_null()));
    assert_eq!(app.get(&carol, &format!("/pages/{secret}")).await.0, 404);
}
