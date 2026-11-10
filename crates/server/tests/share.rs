//! Public share links.

mod common;

use serde_json::{Value, json};

#[tokio::test(flavor = "multi_thread")]
async fn public_links_share_pages_without_leaking_restricted_content() {
    let app = common::spawn().await;
    let owner = app.setup().await;
    let (viewer, _) = app.user(&owner, "vic@acme.test", "member").await;
    let anon = reqwest::Client::new();
    // a private space: only the public link opens it to outsiders
    let (_, sp) = app.post(&owner, "/spaces", json!({"key": "PUB", "name": "Handbook", "defaultRole": "viewer"})).await;
    let home = sp["homePageId"].as_str().unwrap().to_string();
    let mk = |title: &str, parent: &str, md: &str| json!({"spaceKey": "PUB", "parentId": parent, "title": title, "markdown": md});
    let (_, root) = app.post(&owner, "/pages", mk("Guide", &home, "Welcome")).await;
    let root = root["id"].as_str().unwrap().to_string();
    let (_, child) = app.post(&owner, "/pages", mk("Chapter 1", &root, "First chapter")).await;
    let child = child["id"].as_str().unwrap().to_string();
    let (_, grand) = app.post(&owner, "/pages", mk("Section 1.1", &child, "Details")).await;
    let grand = grand["id"].as_str().unwrap().to_string();
    let (_, secret) = app.post(&owner, "/pages", mk("Salaries", &root, "confidential numbers")).await;
    let secret = secret["id"].as_str().unwrap().to_string();
    let (_, below_secret) = app.post(&owner, "/pages", mk("Salary bands", &secret, "more numbers")).await;
    let below_secret = below_secret["id"].as_str().unwrap().to_string();
    let (_, me) = app.get(&owner, "/me").await;
    let owner_id = me["user"]["id"].as_str().unwrap().to_string();
    let (st, _) = app
        .put(&owner, &format!("/pages/{secret}/restrictions"), json!({"restrictions": [{"kind": "view", "subjectType": "user", "subjectId": owner_id}]}))
        .await;
    assert_eq!(st, 204);

    // an image on chapter 1, linked from the guide; plus a link to the restricted page
    let png = b"\x89PNG\r\n\x1a\npublic".to_vec();
    let form = reqwest::multipart::Form::new().part("file", reqwest::multipart::Part::bytes(png.clone()).file_name("chart.png"));
    let rows: Value = owner.post(app.url(&format!("/pages/{child}/attachments"))).multipart(form).send().await.unwrap().json().await.unwrap();
    let att = rows[0]["id"].as_str().unwrap().to_string();
    let form = reqwest::multipart::Form::new().part("file", reqwest::multipart::Part::bytes(b"secret".to_vec()).file_name("bands.png"));
    let rows: Value = owner.post(app.url(&format!("/pages/{secret}/attachments"))).multipart(form).send().await.unwrap().json().await.unwrap();
    let secret_att = rows[0]["id"].as_str().unwrap().to_string();
    app.put(&owner, &format!("/pages/{root}/content"), json!({"markdown": format!("Start with [chapter 1](/pages/{child}#intro).\n\n![chart](/api/attachments/{att}/content)\n\nSee [salaries](/pages/{secret}).")})).await;

    // viewers can't share; editors (owner) can; restricted pages can't be shared
    let (st, _) = app.post(&viewer, &format!("/pages/{root}/share-links"), json!({"includeChildren": true})).await;
    assert_eq!(st, 403);
    let (st, _) = app.post(&owner, &format!("/pages/{secret}/share-links"), json!({})).await;
    assert_eq!(st, 400);
    let (st, link) = app.post(&owner, &format!("/pages/{root}/share-links"), json!({"includeChildren": true})).await;
    assert_eq!(st, 201, "{link}");
    let token = link["token"].as_str().unwrap().to_string();
    assert!(link["url"].as_str().unwrap().ends_with(&format!("/s/{token}")));

    // anonymous reader
    let get = |path: String| {
        let anon = anon.clone();
        let url = app.url(&path);
        async move {
            let r = anon.get(url).send().await.unwrap();
            let st = r.status().as_u16();
            (st, r.bytes().await.unwrap().to_vec())
        }
    };
    let (st, body) = get(format!("/public/{token}")).await;
    assert_eq!(st, 200);
    let page: Value = serde_json::from_slice(&body).unwrap();
    assert_eq!(page["title"], "Guide");
    let html = page["html"].as_str().unwrap();
    assert!(html.contains(&format!("/s/{token}/{child}#intro")), "links inside the share stay inside: {html}");
    assert!(html.contains(&format!("/api/public/{token}/attachments/{att}")), "{html}");
    assert!(html.contains(&format!("/pages/{secret}")) && !html.contains(&format!("/s/{token}/{secret}")), "links out of the share aren't opened up");
    let tree: Vec<String> = page["tree"].as_array().unwrap().iter().map(|n| n["title"].as_str().unwrap().to_string()).collect();
    assert!(tree.contains(&"Chapter 1".to_string()) && tree.contains(&"Section 1.1".to_string()));
    assert!(!tree.iter().any(|t| t.starts_with("Salar")), "restricted sub-pages never appear: {tree:?}");
    assert_eq!(get(format!("/public/{token}/pages/{grand}")).await.0, 200);
    assert_eq!(get(format!("/public/{token}/pages/{secret}")).await.0, 404);
    assert_eq!(get(format!("/public/{token}/pages/{below_secret}")).await.0, 404, "nothing below a restricted page");
    assert_eq!(get(format!("/public/{token}/pages/{home}")).await.0, 404, "the parent isn't shared");
    let (st, bytes) = get(format!("/public/{token}/attachments/{att}")).await;
    assert_eq!((st, bytes), (200, png.clone()));
    assert_eq!(get(format!("/public/{token}/attachments/{secret_att}")).await.0, 404);
    assert_eq!(get("/public/not-a-token".into()).await.0, 404);
    // the regular API stays closed to anonymous users
    assert!(matches!(get(format!("/pages/{root}")).await.0, 401 | 404));

    // a link without children opens only its page
    let (_, single) = app.post(&owner, &format!("/pages/{child}/share-links"), json!({"expiresInDays": 7})).await;
    let single = single["token"].as_str().unwrap().to_string();
    assert_eq!(get(format!("/public/{single}")).await.0, 200);
    assert_eq!(get(format!("/public/{single}/pages/{grand}")).await.0, 404);
    // expiry
    sqlx::query("UPDATE share_links SET expires_at = now() - interval '1 minute' WHERE token = $1").bind(&single).execute(&app.state.db).await.unwrap();
    assert_eq!(get(format!("/public/{single}")).await.0, 404);

    // views are counted; links are listed for editors
    let (_, links) = app.get(&owner, &format!("/pages/{root}/share-links")).await;
    assert_eq!(links["links"].as_array().unwrap().len(), 1);
    assert!(links["links"][0]["views"].as_i64().unwrap() >= 2);

    // workspace switch
    let (_, s) = app.put(&owner, "/admin/settings", json!({"publicLinks": false})).await;
    assert_eq!(s["publicLinks"], false);
    assert_eq!(get(format!("/public/{token}")).await.0, 404);
    let (st, _) = app.post(&owner, &format!("/pages/{root}/share-links"), json!({})).await;
    assert_eq!(st, 400);
    app.put(&owner, "/admin/settings", json!({"publicLinks": true})).await;
    assert_eq!(get(format!("/public/{token}")).await.0, 200);

    // restricting the shared page later closes the link
    let (st, _) = app
        .put(&owner, &format!("/pages/{root}/restrictions"), json!({"restrictions": [{"kind": "view", "subjectType": "user", "subjectId": owner_id}]}))
        .await;
    assert_eq!(st, 204);
    assert_eq!(get(format!("/public/{token}")).await.0, 404);
    app.put(&owner, &format!("/pages/{root}/restrictions"), json!({"restrictions": []})).await;
    assert_eq!(get(format!("/public/{token}")).await.0, 200);

    // revoke
    let id = links["links"][0]["id"].as_str().unwrap();
    assert_eq!(app.delete(&owner, &format!("/share-links/{id}")).await, 204);
    assert_eq!(get(format!("/public/{token}")).await.0, 404);
}

#[tokio::test(flavor = "multi_thread")]
async fn password_protected_links() {
    let app = common::spawn().await;
    let owner = app.setup().await;
    app.post(&owner, "/spaces", json!({"key": "PW", "name": "Partners"})).await;
    let (_, page) = app.post(&owner, "/pages", json!({"spaceKey": "PW", "title": "Price list", "markdown": "Partner prices"})).await;
    let page = page["id"].as_str().unwrap().to_string();
    let (st, _) = app.post(&owner, &format!("/pages/{page}/share-links"), json!({"password": "short"})).await;
    assert_eq!(st, 400, "at least 6 characters");
    let (st, link) = app.post(&owner, &format!("/pages/{page}/share-links"), json!({"password": "open sesame"})).await;
    assert_eq!(st, 201, "{link}");
    let token = link["token"].as_str().unwrap().to_string();
    let (_, links) = app.get(&owner, &format!("/pages/{page}/share-links")).await;
    assert_eq!(links["links"][0]["hasPassword"], true);

    // a browser of its own (cookies), not signed in
    let visitor = reqwest::Client::builder().cookie_store(true).build().unwrap();
    let url = |p: &str| format!("{}/api/public/{token}{p}", app.base);
    assert_eq!(visitor.get(url("")).send().await.unwrap().status(), 401, "asks for the password");
    let unlock = |pw: &'static str| {
        let (visitor, url) = (visitor.clone(), url("/unlock"));
        async move { visitor.post(url).json(&json!({ "password": pw })).send().await.unwrap() }
    };
    assert_eq!(unlock("guess").await.status(), 403);
    let ok = unlock("open sesame").await;
    assert_eq!(ok.status(), 204);
    let cookie = ok.headers()["set-cookie"].to_str().unwrap().to_string();
    assert!(cookie.contains("HttpOnly") && cookie.contains(&format!("Path=/api/public/{token}")), "{cookie}");
    let r = visitor.get(url("")).send().await.unwrap();
    assert_eq!(r.status(), 200);
    assert!(r.json::<Value>().await.unwrap()["html"].as_str().unwrap().contains("Partner prices"));
    // another browser still has to unlock it
    assert_eq!(reqwest::get(url("")).await.unwrap().status(), 401);

    // wrong guesses are throttled per link
    let other = reqwest::Client::new();
    for _ in 0..10 {
        other.post(url("/unlock")).json(&json!({"password": "nope nope"})).send().await.unwrap();
    }
    let r = other.post(url("/unlock")).json(&json!({"password": "open sesame"})).send().await.unwrap();
    assert_eq!(r.status(), 400, "too many wrong passwords");

    // a revoked link opens for nobody, unlocked or not
    let id = links["links"][0]["id"].as_str().unwrap();
    app.delete(&owner, &format!("/share-links/{id}")).await;
    assert_eq!(visitor.get(url("")).send().await.unwrap().status(), 404);
}
