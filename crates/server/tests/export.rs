//! Space export (HTML / Markdown / backup) and backup restore.

mod common;

use std::io::Read;

use serde_json::{Value, json};

use common::TestApp;

async fn wait_job(app: &TestApp, c: &reqwest::Client, job: &str) -> Value {
    app.eventually(|| async {
        let (_, j) = app.get(c, &format!("/jobs/{job}")).await;
        match j["status"].as_str() {
            Some("succeeded") => Some(j),
            Some("failed") => panic!("job failed: {j}"),
            _ => None,
        }
    })
    .await
}

/// Runs an export job and downloads the archive.
async fn export_bytes(app: &TestApp, c: &reqwest::Client, space: &str, format: &str) -> Vec<u8> {
    let (st, body) = app.post(c, &format!("/spaces/{space}/export"), json!({"format": format})).await;
    assert_eq!(st, 202, "{body}");
    let job = body["jobId"].as_str().unwrap().to_string();
    let done = wait_job(app, c, &job).await;
    assert!(done["result"]["filename"].as_str().unwrap().ends_with(".zip"));
    let r = c.get(app.url(&format!("/jobs/{job}/download"))).send().await.unwrap();
    assert_eq!(r.status(), 200);
    assert_eq!(r.headers()["content-type"], "application/zip");
    r.bytes().await.unwrap().to_vec()
}

async fn export(app: &TestApp, c: &reqwest::Client, space: &str, format: &str) -> zip::ZipArchive<std::io::Cursor<Vec<u8>>> {
    zip::ZipArchive::new(std::io::Cursor::new(export_bytes(app, c, space, format).await)).unwrap()
}

fn entry(z: &mut zip::ZipArchive<std::io::Cursor<Vec<u8>>>, name: &str) -> String {
    let mut s = String::new();
    z.by_name(name).unwrap_or_else(|_| panic!("{name} missing")).read_to_string(&mut s).unwrap();
    s
}

fn names(z: &zip::ZipArchive<std::io::Cursor<Vec<u8>>>) -> Vec<String> {
    z.file_names().filter_map(|n| n.ok()).map(|n| n.into_owned()).collect()
}

#[tokio::test(flavor = "multi_thread")]
async fn export_and_restore_a_space() {
    let app = common::spawn().await;
    let owner = app.setup().await;
    let (member, _) = app.user(&owner, "mia@acme.test", "member").await;

    // Home → Guide → Setup (with an image), Home → Secret (restricted), plus a label and a comment
    let (_, sp) = app.post(&owner, "/spaces", json!({"key": "DOC", "name": "Docs", "description": "Team docs"})).await;
    let home = sp["homePageId"].as_str().unwrap().to_string();
    let (_, guide) = app.post(&owner, "/pages", json!({"spaceKey": "DOC", "parentId": home, "title": "Guide"})).await;
    let guide = guide["id"].as_str().unwrap().to_string();
    let (_, setup) = app.post(&owner, "/pages", json!({"spaceKey": "DOC", "parentId": guide, "title": "Setup: step 1/2"})).await;
    let setup = setup["id"].as_str().unwrap().to_string();
    let png: Vec<u8> = b"\x89PNG\r\n\x1a\nexport-test".to_vec();
    let form = reqwest::multipart::Form::new().part("file", reqwest::multipart::Part::bytes(png.clone()).file_name("diagram.png"));
    let rows: Value = owner.post(app.url(&format!("/pages/{setup}/attachments"))).multipart(form).send().await.unwrap().json().await.unwrap();
    let att = rows[0]["id"].as_str().unwrap().to_string();
    let (st, _) =
        app.put(&owner, &format!("/pages/{setup}/content"), json!({"markdown": format!("Install it.\n\n![diagram](/api/attachments/{att}/content)")})).await;
    assert_eq!(st, 200);
    let (st, _) = app.put(&owner, &format!("/pages/{guide}/content"), json!({"markdown": format!("Start with [the setup](/pages/{setup}#install).")})).await;
    assert_eq!(st, 200);
    app.put(&owner, &format!("/pages/{guide}/labels"), json!({"labels": ["howto"]})).await;
    let (st, _) = app.post(&owner, &format!("/pages/{guide}/comments"), json!({"text": "Looks good"})).await;
    assert_eq!(st, 201);
    let (_, secret) = app.post(&owner, "/pages", json!({"spaceKey": "DOC", "parentId": home, "title": "Secret plans", "markdown": "kumquat"})).await;
    let secret = secret["id"].as_str().unwrap().to_string();
    // an include (excerpt) of another page references it through a node attribute
    let doc = json!({"type": "doc", "content": [
        {"type": "paragraph", "content": [{"type": "text", "text": "kumquat"}]},
        {"type": "include", "attrs": {"href": format!("/pages/{setup}"), "title": "Setup: step 1/2"}}
    ]});
    let (st, _) = app.put(&owner, &format!("/pages/{secret}/content"), json!({"content": doc})).await;
    assert_eq!(st, 200);
    let (_, me) = app.get(&owner, "/me").await;
    let owner_id = me["user"]["id"].as_str().unwrap().to_string();
    let (st, _) = app
        .put(&owner, &format!("/pages/{secret}/restrictions"), json!({"restrictions": [{"kind": "view", "subjectType": "user", "subjectId": owner_id}]}))
        .await;
    assert_eq!(st, 204);

    // ---- HTML: a browsable site with relative links, only what the member may see
    let mut z = export(&app, &member, "DOC", "html").await;
    let files = names(&z);
    assert!(files.contains(&"index.html".to_string()) && files.contains(&"style.css".to_string()), "{files:?}");
    let guide_file = files.iter().find(|f| f.ends_with("Guide/index.html")).cloned().unwrap_or_else(|| panic!("{files:?}"));
    let setup_file = files.iter().find(|f| f.ends_with("Setup- step 1-2.html")).cloned().unwrap_or_else(|| panic!("{files:?}"));
    assert!(files.iter().any(|f| f == &format!("_attachments/{att}/diagram.png")), "{files:?}");
    assert!(!files.iter().any(|f| f.contains("Secret")), "restricted page must not be exported for the member: {files:?}");
    let guide_html = entry(&mut z, &guide_file);
    assert!(guide_html.contains("href=\"Setup-%20step%201-2.html#install\""), "{guide_html}");
    assert!(guide_html.contains("howto"), "labels shown");
    let depth = setup_file.matches('/').count();
    let setup_html = entry(&mut z, &setup_file);
    assert!(setup_html.contains(&format!("src=\"{}_attachments/{att}/diagram.png\"", "../".repeat(depth))), "{setup_html}");
    let index = entry(&mut z, "index.html");
    assert!(index.contains("Guide") && index.contains("Team docs") && !index.contains("Secret"));
    let mut img = Vec::new();
    z.by_name(&format!("_attachments/{att}/diagram.png")).unwrap().read_to_end(&mut img).unwrap();
    assert_eq!(img, png);

    // only the requester may download an export
    let (_, body) = app.post(&member, "/spaces/DOC/export", json!({"format": "markdown"})).await;
    let job = body["jobId"].as_str().unwrap().to_string();
    wait_job(&app, &member, &job).await;
    let r = owner.get(app.url(&format!("/jobs/{job}/download"))).send().await.unwrap();
    assert_eq!(r.status(), 404);

    // ---- Markdown: round-trips through the Markdown importer
    let mut z = export(&app, &owner, "DOC", "markdown").await;
    let files = names(&z);
    let guide_md = files.iter().find(|f| f.ends_with("Guide/index.md")).cloned().unwrap_or_else(|| panic!("{files:?}"));
    let text = entry(&mut z, &guide_md);
    assert!(text.starts_with("---\ntitle: \"Guide\"\nlabels: [\"howto\"]\n") && text.contains("\n# Guide\n"), "{text}");
    assert!(text.contains("Setup-%20step%201-2.md#install"), "{text}");
    let raw = z.into_inner().into_inner();
    app.post(&owner, "/spaces", json!({"key": "MD", "name": "Markdown copy"})).await;
    let form = reqwest::multipart::Form::new().text("spaceKey", "MD").part("file", reqwest::multipart::Part::bytes(raw).file_name("docs.zip"));
    let r = owner.post(app.url("/imports/markdown")).multipart(form).send().await.unwrap();
    assert_eq!(r.status(), 201, "{}", r.text().await.unwrap());
    let (_, tree) = app.get(&owner, "/spaces/MD/tree").await;
    let find = |title: &str| tree["nodes"].as_array().unwrap().iter().find(|n| n["title"] == title).map(|n| n["id"].as_str().unwrap().to_string());
    let (md_guide, md_setup) = (find("Guide").expect("Guide imported"), find("Setup: step 1/2").expect("Setup imported with its real title"));
    let (_, g) = app.get(&owner, &format!("/pages/{md_guide}")).await;
    assert!(g["content"].to_string().contains(&format!("/pages/{md_setup}#install")), "link re-resolved: {}", g["content"]);
    assert!(!g["content"].to_string().contains("howto"), "front matter isn't body text");
    let (_, labels) = app.get(&owner, &format!("/pages/{md_guide}/labels")).await;
    assert!(labels.to_string().contains("howto"), "labels come back from front matter: {labels}");
    let (_, s) = app.get(&owner, &format!("/pages/{md_setup}")).await;
    assert!(s["content"].to_string().contains("/api/attachments/"), "image re-imported as attachment");

    // ---- Backup: space admins only; restores everything into a new space
    let (st, _) = app.post(&member, "/spaces/DOC/export", json!({"format": "backup"})).await;
    assert_eq!(st, 403);
    let mut z = export(&app, &owner, "DOC", "backup").await;
    let manifest: Value = serde_json::from_str(&entry(&mut z, "manifest.json")).unwrap();
    assert_eq!(manifest["format"], "wikicraft-space-backup");
    assert_eq!(manifest["pages"].as_array().unwrap().len(), 4);
    let raw = z.into_inner().into_inner();
    let restore = |key: &str, data: Vec<u8>| {
        let form =
            reqwest::multipart::Form::new().text("spaceKey", key.to_string()).part("file", reqwest::multipart::Part::bytes(data).file_name("backup.zip"));
        owner.post(app.url("/imports/backup")).multipart(form).send()
    };
    let r = restore("DOC", raw.clone()).await.unwrap();
    assert_eq!(r.status(), 409, "existing key");
    let r = restore("NOPE", b"PK\x05\x06\0\0\0\0\0\0\0\0\0\0\0\0\0\0\0\0\0\0".to_vec()).await.unwrap();
    assert_eq!(r.status(), 400, "not a backup");
    let r = restore("DOC2", raw).await.unwrap();
    assert_eq!(r.status(), 202);
    let body: Value = r.json().await.unwrap();
    let done = wait_job(&app, &owner, body["jobId"].as_str().unwrap()).await;
    assert_eq!(done["result"]["pages"], 4);
    assert_eq!(done["result"]["attachments"], 1);
    assert_eq!(done["result"]["comments"], 1);

    let (_, tree) = app.get(&owner, "/spaces/DOC2/tree").await;
    let nodes = tree["nodes"].as_array().unwrap();
    let id_of =
        |title: &str| nodes.iter().find(|n| n["title"] == title).map(|n| n["id"].as_str().unwrap().to_string()).unwrap_or_else(|| panic!("{title} missing"));
    let (r_guide, r_setup, r_secret) = (id_of("Guide"), id_of("Setup: step 1/2"), id_of("Secret plans"));
    assert!(nodes.iter().find(|n| n["title"] == "Setup: step 1/2").unwrap()["parentId"] == r_guide.as_str(), "tree preserved");
    assert_eq!(tree["homePageId"].as_str().unwrap(), id_of("Docs"), "home page restored");
    let (_, g) = app.get(&owner, &format!("/pages/{r_guide}")).await;
    assert!(g["content"].to_string().contains(&format!("/pages/{r_setup}#install")), "links point at the restored page");
    let (_, labels) = app.get(&owner, &format!("/pages/{r_guide}/labels")).await;
    assert!(labels.to_string().contains("howto"), "{labels}");
    let (_, comments) = app.get(&owner, &format!("/pages/{r_guide}/comments")).await;
    assert_eq!(comments.as_array().unwrap().len(), 1);
    assert_eq!(comments[0]["authorId"].as_str(), Some(owner_id.as_str()), "author matched by e-mail");
    let (_, s) = app.get(&owner, &format!("/pages/{r_setup}")).await;
    let content = s["content"].to_string();
    assert!(!content.contains(&att), "image points at the restored attachment");
    let new_att = content.split("/api/attachments/").nth(1).unwrap().split('/').next().unwrap().to_string();
    let bytes = owner.get(app.url(&format!("/attachments/{new_att}/content"))).send().await.unwrap().bytes().await.unwrap();
    assert_eq!(bytes.as_ref(), png.as_slice());
    let (_, sec) = app.get(&owner, &format!("/pages/{r_secret}")).await;
    assert!(sec["content"].to_string().contains(&format!("/pages/{r_setup}")), "include points at the restored page: {}", sec["content"]);
    // restrictions survive the round trip
    let (st, _) = app.get(&member, &format!("/pages/{r_secret}")).await;
    assert_eq!(st, 404, "restored restriction hides the page");
    let (st, _) = app.get(&owner, &format!("/pages/{r_secret}")).await;
    assert_eq!(st, 200);
    // the restored space opens with the original default role only after the restore
    let (_, doc2) = app.get(&owner, "/spaces/DOC2").await;
    assert_eq!(doc2["defaultRole"], "editor");

    // a broken backup (attachment file missing) fails without leaving a half-restored space
    let mut broken = zip::ZipWriter::new(std::io::Cursor::new(Vec::new()));
    broken.start_file("manifest.json", zip::write::SimpleFileOptions::default()).unwrap();
    std::io::Write::write_all(&mut broken, serde_json::to_vec(&manifest).unwrap().as_slice()).unwrap();
    let broken = broken.finish().unwrap().into_inner();
    let r = restore("DOC3", broken).await.unwrap();
    assert_eq!(r.status(), 202);
    let body: Value = r.json().await.unwrap();
    let job = body["jobId"].as_str().unwrap().to_string();
    app.eventually(|| async {
        let (_, j) = app.get(&owner, &format!("/jobs/{job}")).await;
        (j["status"] == "failed").then_some(())
    })
    .await;
    assert_eq!(app.get(&owner, "/spaces/DOC3").await.0, 404, "cleaned up");
}

fn tiny_png(w: u32, h: u32) -> Vec<u8> {
    let img = image::RgbImage::from_fn(w, h, |x, y| image::Rgb([(x * 40) as u8, (y * 60) as u8, 200]));
    let mut out = std::io::Cursor::new(Vec::new());
    img.write_to(&mut out, image::ImageFormat::Png).unwrap();
    out.into_inner()
}

async fn upload(app: &TestApp, c: &reqwest::Client, page: &str, name: &str, bytes: Vec<u8>) -> String {
    let form = reqwest::multipart::Form::new().part("file", reqwest::multipart::Part::bytes(bytes).file_name(name.to_string()));
    let rows: Value = c.post(app.url(&format!("/pages/{page}/attachments"))).multipart(form).send().await.unwrap().json().await.unwrap();
    rows[0]["id"].as_str().unwrap().to_string()
}

#[tokio::test(flavor = "multi_thread")]
async fn word_export_of_a_page() {
    let app = common::spawn().await;
    let owner = app.setup().await;
    let (member, _) = app.user(&owner, "mo@acme.test", "member").await;
    let (_, sp) = app.post(&owner, "/spaces", json!({"key": "WRD", "name": "Word"})).await;
    let home = sp["homePageId"].as_str().unwrap().to_string();
    let (_, pg) = app.post(&owner, "/pages", json!({"spaceKey": "WRD", "parentId": home, "title": "Quarterly report"})).await;
    let pg = pg["id"].as_str().unwrap().to_string();
    let (_, child) = app.post(&owner, "/pages", json!({"spaceKey": "WRD", "parentId": pg, "title": "Appendix A"})).await;
    let child = child["id"].as_str().unwrap().to_string();
    // an image on a page the member can't see
    let (_, secret) = app.post(&owner, "/pages", json!({"spaceKey": "WRD", "parentId": home, "title": "Secret"})).await;
    let secret = secret["id"].as_str().unwrap().to_string();
    let hidden = upload(&app, &owner, &secret, "hidden.png", tiny_png(5, 5)).await;
    let (_, me) = app.get(&owner, "/me").await;
    let owner_id = me["user"]["id"].as_str().unwrap().to_string();
    app.put(&owner, &format!("/pages/{secret}/restrictions"), json!({"restrictions": [{"kind": "view", "subjectType": "user", "subjectId": owner_id}]})).await;
    let chart = upload(&app, &owner, &pg, "chart.png", tiny_png(40, 30)).await;
    let svg = upload(
        &app,
        &owner,
        &pg,
        "logo.svg",
        b"<svg xmlns=\"http://www.w3.org/2000/svg\" width=\"20\" height=\"10\"><rect width=\"20\" height=\"10\" fill=\"#f00\"/></svg>".to_vec(),
    )
    .await;

    let t = |s: &str| json!({"type": "text", "text": s});
    let p = |c: Value| json!({"type": "paragraph", "content": c});
    let doc = json!({"type": "doc", "content": [
        {"type": "toc"},
        {"type": "heading", "attrs": {"level": 1}, "content": [t("Summary")]},
        p(json!([t("Revenue grew "), {"type": "text", "text": "12%", "marks": [{"type": "bold"}]}, t(" — see "),
            {"type": "text", "text": "the dashboard", "marks": [{"type": "link", "attrs": {"href": "https://example.com/dash"}}]}, t(" and "),
            {"type": "text", "text": "details", "marks": [{"type": "link", "attrs": {"href": "#details"}}]},
            t(". "), {"type": "status", "attrs": {"text": "On track", "color": "green"}}, t(" "), {"type": "emoticon", "attrs": {"name": "tick"}},
            {"type": "mention", "attrs": {"id": owner_id, "label": "Olivia"}}])),
        {"type": "orderedList", "attrs": {"start": 3}, "content": [
            {"type": "listItem", "content": [p(json!([t("Third")])), {"type": "bulletList", "content": [{"type": "listItem", "content": [p(json!([t("Nested bullet")]))]}]}]},
            {"type": "listItem", "content": [p(json!([t("Fourth")]))]}]},
        {"type": "taskList", "content": [
            {"type": "taskItem", "attrs": {"checked": true}, "content": [p(json!([t("Close the books")]))]},
            {"type": "taskItem", "attrs": {"checked": false}, "content": [p(json!([t("Board deck")]))]}]},
        {"type": "callout", "attrs": {"type": "warning"}, "content": [p(json!([t("Numbers are preliminary.")]))]},
        {"type": "codeBlock", "attrs": {"language": "sql"}, "content": [t("SELECT 1;\nSELECT 2;")]},
        {"type": "table", "content": [
            {"type": "tableRow", "content": [
                {"type": "tableHeader", "content": [p(json!([t("Region")]))]},
                {"type": "tableHeader", "attrs": {"colspan": 2}, "content": [p(json!([t("Q1 / Q2")]))]}]},
            {"type": "tableRow", "content": [
                {"type": "tableCell", "attrs": {"rowspan": 2}, "content": [p(json!([t("EMEA")]))]},
                {"type": "tableCell", "content": [p(json!([t("10")]))]},
                {"type": "tableCell", "attrs": {"background": "#fef3c7"}, "content": [p(json!([t("11")]))]}]},
            {"type": "tableRow", "content": [
                {"type": "tableCell", "content": [p(json!([t("20")]))]},
                {"type": "tableCell", "content": [p(json!([t("21")]))]}]}]},
        {"type": "heading", "attrs": {"level": 2}, "content": [t("Details")]},
        p(json!([{"type": "image", "attrs": {"src": format!("/api/attachments/{chart}/content"), "alt": "chart", "caption": "Figure 1: revenue", "align": "center"}}])),
        p(json!([{"type": "image", "attrs": {"src": format!("/api/attachments/{svg}/content"), "alt": "logo"}}])),
        p(json!([{"type": "image", "attrs": {"src": format!("/api/attachments/{hidden}/content"), "alt": "secret diagram"}}])),
        p(json!([{"type": "image", "attrs": {"src": "https://example.com/remote.png", "alt": "remote"}}])),
        {"type": "blockquote", "content": [p(json!([t("Quoted wisdom")]))]},
        {"type": "childPages"},
        {"type": "horizontalRule"}
    ]});
    let (st, _) = app.put(&owner, &format!("/pages/{pg}/content"), json!({"content": doc})).await;
    assert_eq!(st, 200);

    let r = member.get(app.url(&format!("/pages/{pg}/export?format=docx"))).send().await.unwrap();
    assert_eq!(r.status(), 200);
    assert_eq!(r.headers()["content-type"], "application/vnd.openxmlformats-officedocument.wordprocessingml.document");
    assert!(r.headers()["content-disposition"].to_str().unwrap().contains("Quarterly_report.docx"));
    let mut z = zip::ZipArchive::new(std::io::Cursor::new(r.bytes().await.unwrap().to_vec())).unwrap();
    let files = names(&z);
    let xml = entry(&mut z, "word/document.xml");
    for needle in [
        "Quarterly report",
        "Last updated",
        "Revenue grew",
        "Nested bullet",
        "Close the books",
        "Numbers are preliminary.",
        "SELECT 2;",
        "EMEA",
        "Figure 1: revenue",
        "Quoted wisdom",
        "@Olivia",
        "ON TRACK",
        "Appendix A",
    ] {
        assert!(xml.contains(needle), "{needle} missing");
    }
    assert!(xml.contains("w:vMerge") && xml.contains("w:gridSpan"), "merged cells");
    assert!(xml.contains("w:anchor=\"h1\"") && xml.contains("w:name=\"h1\""), "internal link to the bookmarked heading");
    assert!(xml.contains("w:numId"), "lists are numbered");
    assert!(xml.contains("Heading1") && xml.contains("Heading2"));
    assert!(xml.contains("[Image: secret diagram]") && xml.contains("[Image: remote]"), "unviewable / remote images become links");
    let rels = entry(&mut z, "word/_rels/document.xml.rels");
    assert!(rels.contains("https://example.com/dash") && rels.contains(&format!("/pages/{child}")), "{rels}");
    let numbering = entry(&mut z, "word/numbering.xml");
    assert!(numbering.contains("w:startOverride w:val=\"3\""), "ordered list starts at 3");
    // bullets use our bullet list, not docx-rs's built-in decimal list (num 1)
    assert!(xml.contains("<w:numId w:val=\"2\"") && !xml.contains("<w:numId w:val=\"1\""), "bullet list id");
    assert!(numbering.contains("<w:num w:numId=\"2\"><w:abstractNumId w:val=\"10\""), "{numbering}");
    // chart + logo + icons (checkboxes, emoticon, callout) are embedded; the hidden image is not
    let media: Vec<&String> = files.iter().filter(|f| f.starts_with("word/media/") && !f.ends_with('/')).collect();
    assert!(media.len() >= 6, "{files:?}");
    let hidden_png = tiny_png(5, 5);
    for m in &media {
        let mut b = Vec::new();
        z.by_name(m).unwrap().read_to_end(&mut b).unwrap();
        assert!(b.starts_with(b"\x89PNG"), "{m} is not a PNG");
        assert_ne!(b, hidden_png, "restricted attachment leaked into the export");
    }

    // owners see the hidden image embedded; anonymous visitors get nothing
    let r = owner.get(app.url(&format!("/pages/{pg}/export?format=docx"))).send().await.unwrap();
    let mut z = zip::ZipArchive::new(std::io::Cursor::new(r.bytes().await.unwrap().to_vec())).unwrap();
    assert!(!entry(&mut z, "word/document.xml").contains("[Image: secret diagram]"));
    let anon = common::client();
    assert_ne!(anon.get(app.url(&format!("/pages/{pg}/export?format=docx"))).send().await.unwrap().status(), 200);
}
