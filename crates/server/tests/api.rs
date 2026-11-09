mod common;

use serde_json::{Value, json};

use common::{bare_client, client, spawn};

#[tokio::test(flavor = "multi_thread")]
async fn setup_login_and_csrf() {
    let app = spawn().await;
    let (_, s) = app.get(&client(), "/setup").await;
    assert_eq!(s["needsSetup"], true);
    let owner = app.setup().await;
    let (_, me) = app.get(&owner, "/me").await;
    assert_eq!(me["user"]["role"], "owner");
    assert_eq!(me["workspace"]["name"], "Acme");
    // second setup is rejected
    let (st, _) = app.post(&client(), "/setup", json!({"workspaceName": "x", "name": "x", "email": "x@x.test", "password": "password123"})).await;
    assert_eq!(st, 409);
    // wrong password
    let (st, _) = app.post(&client(), "/auth/login", json!({"email": "owner@acme.test", "password": "nope-nope"})).await;
    assert_eq!(st, 400);
    // cookie-authenticated mutation without the CSRF header is rejected
    let bare = bare_client();
    let r = bare.post(app.url("/auth/login")).json(&json!({"email": "owner@acme.test", "password": "correct horse"})).send().await.unwrap();
    assert_eq!(r.status(), 200);
    let r = bare.post(app.url("/spaces")).json(&json!({"key": "X", "name": "X"})).send().await.unwrap();
    assert_eq!(r.status(), 400);
    // API tokens work without CSRF header
    let (_, tok) = app.post(&owner, "/me/tokens", json!({"name": "ci"})).await;
    let token = tok["token"].as_str().unwrap();
    let r = reqwest::Client::new().post(app.url("/spaces")).bearer_auth(token).json(&json!({"key": "API", "name": "Via API"})).send().await.unwrap();
    assert_eq!(r.status(), 201);
    // logout invalidates the session
    let r = owner.post(app.url("/auth/logout")).send().await.unwrap();
    assert_eq!(r.status(), 204);
    let (st, _) = app.get(&owner, "/recent").await;
    assert_eq!(st, 401);
}

#[tokio::test(flavor = "multi_thread")]
async fn spaces_pages_tree_versions_and_trash() {
    let app = spawn().await;
    let c = app.setup().await;
    let (st, space) = app.post(&c, "/spaces", json!({"key": "eng", "name": "Engineering"})).await;
    assert_eq!(st, 201);
    assert_eq!(space["key"], "ENG");
    let home = space["homePageId"].as_str().unwrap().to_string();
    let (st, _) = app.post(&c, "/spaces", json!({"key": "ENG", "name": "dup"})).await;
    assert_eq!(st, 409);

    let (_, a) = app.post(&c, "/pages", json!({"spaceKey": "ENG", "parentId": home, "title": "A", "markdown": "# A\n\nHello **world**"})).await;
    let a = a["id"].as_str().unwrap().to_string();
    let (_, b) = app.post(&c, "/pages", json!({"spaceKey": "ENG", "parentId": home, "title": "B"})).await;
    let b = b["id"].as_str().unwrap().to_string();
    let (_, c1) = app.post(&c, "/pages", json!({"spaceKey": "ENG", "parentId": a, "title": "C"})).await;
    let c1 = c1["id"].as_str().unwrap().to_string();

    let (_, tree) = app.get(&c, "/spaces/ENG/tree").await;
    assert_eq!(tree["nodes"].as_array().unwrap().len(), 4);

    let (_, page) = app.get(&c, &format!("/pages/{c1}")).await;
    assert_eq!(page["breadcrumbs"].as_array().unwrap().iter().map(|b| b["title"].as_str().unwrap()).collect::<Vec<_>>(), vec!["Engineering", "A"]);
    assert_eq!(page["access"]["edit"], true);

    // move B before A, then C under B
    let (st, _) = app.post(&c, &format!("/pages/{b}/move"), json!({"parentId": home, "beforeId": a})).await;
    assert_eq!(st, 204);
    let (_, kids) = app.get(&c, &format!("/pages/{home}/children")).await;
    assert_eq!(kids.as_array().unwrap().iter().map(|k| k["title"].as_str().unwrap()).collect::<Vec<_>>(), vec!["B", "A"]);
    // cycle protection
    let (st, _) = app.post(&c, &format!("/pages/{a}/move"), json!({"parentId": c1})).await;
    assert_eq!(st, 400);

    // content update creates versions; restore brings old content back
    let (st, v) = app.put(&c, &format!("/pages/{a}/content"), json!({"markdown": "Second version", "message": "rewrite"})).await;
    assert_eq!(st, 200, "{v}");
    let (_, versions) = app.get(&c, &format!("/pages/{a}/versions")).await;
    let versions = versions.as_array().unwrap();
    assert_eq!(versions.len(), 2);
    assert_eq!(versions[0]["message"], "rewrite");
    let (st, _) = app.post(&c, &format!("/pages/{a}/versions/1/restore"), json!({})).await;
    assert_eq!(st, 200);
    let (_, page) = app.get(&c, &format!("/pages/{a}")).await;
    assert!(page["content"].to_string().contains("world"), "{}", page["content"]);

    // markdown export
    let md = c.get(app.url(&format!("/pages/{a}/export?format=md"))).send().await.unwrap().text().await.unwrap();
    assert!(md.contains("**world**"), "{md}");

    // labels
    let (_, labels) = app.put(&c, &format!("/pages/{a}/labels"), json!({"labels": ["Runbook", "how to", "runbook"]})).await;
    assert_eq!(labels, json!(["how-to", "runbook"]));
    let (_, by_label) = app.get(&c, "/labels/runbook").await;
    assert_eq!(by_label.as_array().unwrap().len(), 1);

    // trash removes subtree; restore brings it back
    assert_eq!(app.delete(&c, &format!("/pages/{a}")).await, 204);
    let (st, _) = app.get(&c, &format!("/pages/{c1}")).await;
    assert_eq!(st, 404);
    let (_, trash) = app.get(&c, "/spaces/ENG/trash").await;
    assert_eq!(trash.as_array().unwrap().len(), 1);
    let (st, _) = app.post(&c, &format!("/pages/{a}/restore"), json!({})).await;
    assert_eq!(st, 204);
    let (st, _) = app.get(&c, &format!("/pages/{c1}")).await;
    assert_eq!(st, 200);

    // copy with children
    let (st, copied) = app.post(&c, &format!("/pages/{a}/copy"), json!({"includeChildren": true})).await;
    assert_eq!(st, 201);
    assert_eq!(copied["copied"], 2);
}

#[tokio::test(flavor = "multi_thread")]
async fn permissions_and_restrictions() {
    let app = spawn().await;
    let owner = app.setup().await;
    let (member, member_id) = app.user(&owner, "mia@acme.test", "member").await;
    let (guest, _) = app.user(&owner, "gus@acme.test", "guest").await;

    app.post(&owner, "/spaces", json!({"key": "HR", "name": "HR", "defaultRole": "viewer"})).await;
    let (_, tree) = app.get(&owner, "/spaces/HR/tree").await;
    let home = tree["homePageId"].as_str().unwrap().to_string();
    let (_, secret) = app.post(&owner, "/pages", json!({"spaceKey": "HR", "parentId": home, "title": "Salaries", "markdown": "secret numbers zebra"})).await;
    let secret = secret["id"].as_str().unwrap().to_string();
    let (_, child) = app.post(&owner, "/pages", json!({"spaceKey": "HR", "parentId": secret, "title": "Salary details"})).await;
    let child = child["id"].as_str().unwrap().to_string();

    // member: viewer by default → can read, can't edit or create
    let (st, p) = app.get(&member, &format!("/pages/{secret}")).await;
    assert_eq!(st, 200);
    assert_eq!(p["access"]["edit"], false);
    let (st, _) = app.put(&member, &format!("/pages/{secret}/content"), json!({"markdown": "hacked"})).await;
    assert_eq!(st, 403);
    let (st, _) = app.post(&member, "/pages", json!({"spaceKey": "HR", "title": "x"})).await;
    assert_eq!(st, 403);
    // guest: no access at all
    let (st, _) = app.get(&guest, "/spaces/HR").await;
    assert_eq!(st, 404);
    let (_, spaces) = app.get(&guest, "/spaces").await;
    assert!(spaces.as_array().unwrap().is_empty());
    // anonymous: none
    let (st, _) = app.get(&client(), &format!("/pages/{secret}")).await;
    assert_eq!(st, 401);

    // view restriction to owner only → hidden from member (page + inherited child + tree + search)
    let (_, me) = app.get(&owner, "/me").await;
    let owner_id = me["user"]["id"].as_str().unwrap().to_string();
    let (st, _) = app
        .put(&owner, &format!("/pages/{secret}/restrictions"), json!({"restrictions": [{"kind": "view", "subjectType": "user", "subjectId": owner_id}]}))
        .await;
    assert_eq!(st, 204);
    let (st, _) = app.get(&member, &format!("/pages/{secret}")).await;
    assert_eq!(st, 404);
    let (st, _) = app.get(&member, &format!("/pages/{child}")).await;
    assert_eq!(st, 404, "view restrictions are inherited");
    let (_, tree) = app.get(&member, "/spaces/HR/tree").await;
    assert_eq!(tree["nodes"].as_array().unwrap().len(), 1);
    // search: owner finds it, member doesn't
    let hits = app
        .eventually(|| async {
            let (_, h) = app.get(&owner, "/search?q=zebra").await;
            (!h.as_array().unwrap().is_empty()).then_some(h)
        })
        .await;
    assert_eq!(hits[0]["title"], "Salaries");
    let (_, h) = app.get(&member, "/search?q=zebra").await;
    assert!(h.as_array().unwrap().is_empty());

    // group-based grant: editors group gets editor role
    let (_, g) = app.post(&owner, "/groups", json!({"name": "hr-editors"})).await;
    let gid = g["id"].as_str().unwrap().to_string();
    app.post(&owner, &format!("/groups/{gid}/members"), json!({"userId": member_id})).await;
    let (st, _) = app.put(&owner, "/spaces/HR/permissions", json!({"defaultRole": "viewer", "grants": [{"subjectType": "user", "subjectId": owner_id, "role": "admin"}, {"subjectType": "group", "subjectId": gid, "role": "editor"}]})).await;
    assert_eq!(st, 204);
    let (st, _) = app.put(&member, &format!("/pages/{home}/content"), json!({"markdown": "edited by member"})).await;
    assert_eq!(st, 200);
    // but still can't see the restricted page
    let (st, _) = app.get(&member, &format!("/pages/{secret}")).await;
    assert_eq!(st, 404);
    // restricting yourself out is prevented for non-admins
    let (st, _) = app
        .put(&member, &format!("/pages/{home}/restrictions"), json!({"restrictions": [{"kind": "edit", "subjectType": "user", "subjectId": owner_id}]}))
        .await;
    assert_eq!(st, 400);

    // anonymous access on
    app.patch(&owner, "/spaces/HR", json!({"anonymousAccess": true})).await;
    let (st, _) = app.get(&client(), &format!("/pages/{home}")).await;
    assert_eq!(st, 200);
    let (st, _) = app.get(&client(), &format!("/pages/{secret}")).await;
    assert_eq!(st, 401, "restricted pages stay hidden from anonymous users");

    // audit log recorded permission changes
    let (_, audit) = app.get(&owner, "/admin/audit").await;
    assert!(audit.as_array().unwrap().iter().any(|e| e["action"] == "page.restrictions"));
    let (st, _) = app.get(&member, "/admin/audit").await;
    assert_eq!(st, 403);
}

#[tokio::test(flavor = "multi_thread")]
async fn comments_and_attachments() {
    let app = spawn().await;
    let c = app.setup().await;
    let (_, space) = app.post(&c, "/spaces", json!({"key": "DOC", "name": "Docs"})).await;
    let home = space["homePageId"].as_str().unwrap().to_string();

    let (st, cm) = app.post(&c, &format!("/pages/{home}/comments"), json!({"text": "Looks **good**"})).await;
    assert_eq!(st, 201);
    let cid = cm["id"].as_str().unwrap().to_string();
    let (st, _) = app.post(&c, &format!("/pages/{home}/comments"), json!({"text": "thanks", "parentId": cid})).await;
    assert_eq!(st, 201);
    let (_, inline) = app.post(&c, &format!("/pages/{home}/comments"), json!({"text": "typo?", "inline": {"text": "Welcome"}})).await;
    let (st, resolved) = app.patch(&c, &format!("/comments/{}", inline["id"].as_str().unwrap()), json!({"resolved": true})).await;
    assert_eq!(st, 200);
    assert!(resolved["resolvedAt"].is_string());
    let (_, list) = app.get(&c, &format!("/pages/{home}/comments")).await;
    assert_eq!(list.as_array().unwrap().len(), 3);
    assert_eq!(app.delete(&c, &format!("/comments/{cid}")).await, 204);
    let (_, list) = app.get(&c, &format!("/pages/{home}/comments")).await;
    assert_eq!(list.as_array().unwrap().len(), 1, "deleting a comment deletes its replies");

    // attachments: upload, new version, download
    let png: Vec<u8> = b"\x89PNG\r\n\x1a\nfakepng".to_vec();
    let form = reqwest::multipart::Form::new().part("file", reqwest::multipart::Part::bytes(png.clone()).file_name("../../etc/diagram.png"));
    let r = c.post(app.url(&format!("/pages/{home}/attachments"))).multipart(form).send().await.unwrap();
    assert_eq!(r.status(), 201);
    let rows: Value = r.json().await.unwrap();
    assert_eq!(rows[0]["filename"], "diagram.png", "path components are stripped");
    assert_eq!(rows[0]["mediaType"], "image/png");
    let att = rows[0]["id"].as_str().unwrap().to_string();
    let form = reqwest::multipart::Form::new().part("file", reqwest::multipart::Part::bytes(b"\x89PNG v2".to_vec()).file_name("diagram.png"));
    let r = c.post(app.url(&format!("/pages/{home}/attachments"))).multipart(form).send().await.unwrap();
    let rows: Value = r.json().await.unwrap();
    assert_eq!(rows[0]["version"], 2);
    let r = c.get(app.url(&format!("/attachments/{att}/content"))).send().await.unwrap();
    assert_eq!(r.headers()["content-type"], "image/png");
    assert_eq!(r.bytes().await.unwrap().as_ref(), b"\x89PNG v2");
    // html is never served inline
    let form = reqwest::multipart::Form::new().part("file", reqwest::multipart::Part::bytes(b"<script>alert(1)</script>".to_vec()).file_name("x.html"));
    let r = c.post(app.url(&format!("/pages/{home}/attachments"))).multipart(form).send().await.unwrap();
    let rows: Value = r.json().await.unwrap();
    let r = c.get(app.url(&format!("/attachments/{}/content", rows[0]["id"].as_str().unwrap()))).send().await.unwrap();
    assert_eq!(r.headers()["content-type"], "application/octet-stream");
    assert!(r.headers()["content-disposition"].to_str().unwrap().starts_with("attachment"));
}

#[tokio::test(flavor = "multi_thread")]
async fn markdown_zip_import() {
    let app = spawn().await;
    let c = app.setup().await;
    app.post(&c, "/spaces", json!({"key": "KB", "name": "KB"})).await;
    let mut buf = std::io::Cursor::new(Vec::new());
    {
        let mut z = zip::ZipWriter::new(&mut buf);
        let o: zip::write::SimpleFileOptions = Default::default();
        use std::io::Write;
        z.start_file("guide/index.md", o).unwrap();
        z.write_all(b"# Guide\n\nSee [[Setup]] and ![logo](img/logo.png)").unwrap();
        z.start_file("guide/Setup.md", o).unwrap();
        z.write_all(b"# Setup\n\nInstall it.").unwrap();
        z.start_file("guide/img/logo.png", o).unwrap();
        z.write_all(b"\x89PNG logo").unwrap();
        z.finish().unwrap();
    }
    let form = reqwest::multipart::Form::new().text("spaceKey", "KB").part("file", reqwest::multipart::Part::bytes(buf.into_inner()).file_name("export.zip"));
    let r = c.post(app.url("/imports/markdown")).multipart(form).send().await.unwrap();
    assert_eq!(r.status(), 201, "{}", r.text().await.unwrap());
    let (_, tree) = app.get(&c, "/spaces/KB/tree").await;
    let nodes = tree["nodes"].as_array().unwrap();
    let guide = nodes.iter().find(|n| n["title"] == "Guide").expect("folder page titled from index.md");
    let (_, page) = app.get(&c, &format!("/pages/{}", guide["id"].as_str().unwrap())).await;
    let content = page["content"].to_string();
    assert!(content.contains("/api/attachments/"), "image uploaded: {content}");
    assert!(content.contains("/pages/"), "wikilink resolved: {content}");
}
