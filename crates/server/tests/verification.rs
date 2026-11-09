//! Page verification: badge with expiry, who may verify, review requests, the report.

mod common;

use serde_json::json;

#[tokio::test(flavor = "multi_thread")]
async fn verify_expire_and_request_review() {
    let app = common::spawn().await;
    let owner = app.setup().await;
    let (editor, editor_id) = app.user(&owner, "erin@acme.test", "member").await;
    let (viewer, viewer_id) = app.user(&owner, "vic@acme.test", "member").await;
    let (_, sp) = app.post(&owner, "/spaces", json!({"key": "KB", "name": "Knowledge", "defaultRole": "none"})).await;
    let home = sp["homePageId"].as_str().unwrap().to_string();
    let (st, _) = app
        .put(
            &owner,
            "/spaces/KB/permissions",
            json!({"grants": [{"subjectType": "user", "subjectId": editor_id, "role": "editor"}, {"subjectType": "user", "subjectId": viewer_id, "role": "viewer"}]}),
        )
        .await;
    assert_eq!(st, 204);
    let (_, pg) = app.post(&owner, "/pages", json!({"spaceKey": "KB", "parentId": home, "title": "VPN setup", "markdown": "Use WireGuard."})).await;
    let pg = pg["id"].as_str().unwrap().to_string();

    let (_, v) = app.get(&viewer, &format!("/pages/{pg}/verification")).await;
    assert_eq!(v["status"], "none");
    // viewers can't verify; editors can
    let (st, _) = app.put(&viewer, &format!("/pages/{pg}/verification"), json!({"days": 90})).await;
    assert_eq!(st, 403);
    let (st, v) = app.put(&editor, &format!("/pages/{pg}/verification"), json!({"days": 90, "note": "Checked with IT"})).await;
    assert_eq!(st, 200, "{v}");
    assert_eq!(v["status"], "verified");
    assert_eq!(v["verifiedBy"]["name"], "erin");
    assert_eq!(v["editedSince"], false);
    let (st, _) = app.put(&editor, &format!("/pages/{pg}/verification"), json!({"days": 0})).await;
    assert_eq!(st, 400);

    // viewers ask for a review: only people who may edit can be asked, once while unread
    let (st, _) = app.post(&viewer, &format!("/pages/{pg}/verification/request"), json!({"userId": viewer_id})).await;
    assert_eq!(st, 400);
    for _ in 0..2 {
        let (st, _) = app.post(&viewer, &format!("/pages/{pg}/verification/request"), json!({"userId": editor_id, "note": "Still current?"})).await;
        assert_eq!(st, 204);
    }
    let (_, n) = app.get(&editor, "/notifications").await;
    let reqs: Vec<_> = n["items"].as_array().unwrap().iter().filter(|i| i["kind"] == "verification_request").collect();
    assert_eq!(reqs.len(), 1, "{n}");
    assert_eq!(reqs[0]["data"]["excerpt"], "Still current?");
    assert_eq!(reqs[0]["actorName"], "vic");

    // expired verifications show up as such and in the report; others can't see them
    sqlx::query("UPDATE page_verifications SET expires_at = now() - interval '1 day'").execute(&app.state.db).await.unwrap();
    let (_, v) = app.get(&viewer, &format!("/pages/{pg}/verification")).await;
    assert_eq!(v["status"], "expired");
    let (_, r) = app.get(&viewer, "/reports/verification?status=expired").await;
    assert_eq!(r.as_array().unwrap().len(), 1);
    assert_eq!(r[0]["title"], "VPN setup");
    let (outsider, _) = app.user(&owner, "otto@acme.test", "member").await;
    let (_, r) = app.get(&outsider, "/reports/verification?status=expired").await;
    assert_eq!(r.as_array().unwrap().len(), 0);
    assert_eq!(app.get(&outsider, &format!("/pages/{pg}/verification")).await.0, 404);

    // re-verifying without an expiry; an edit afterwards is flagged
    let (_, v) = app.put(&editor, &format!("/pages/{pg}/verification"), json!({})).await;
    assert_eq!(v["status"], "verified");
    assert!(v["expiresAt"].is_null());
    sqlx::query("UPDATE pages SET updated_at = now() + interval '1 hour'").execute(&app.state.db).await.unwrap();
    let (_, v) = app.get(&viewer, &format!("/pages/{pg}/verification")).await;
    assert_eq!(v["editedSince"], true);
    let (_, r) = app.get(&owner, "/reports/verification?status=edited&space=kb").await;
    assert_eq!(r.as_array().unwrap().len(), 1);

    assert_eq!(app.delete(&viewer, &format!("/pages/{pg}/verification")).await, 403);
    assert_eq!(app.delete(&editor, &format!("/pages/{pg}/verification")).await, 204);
    let (_, v) = app.get(&viewer, &format!("/pages/{pg}/verification")).await;
    assert_eq!(v["status"], "none");
}
