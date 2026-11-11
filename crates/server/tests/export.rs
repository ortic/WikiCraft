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
    app.put(&owner, &format!("/attachments/{att}/labels"), json!({"labels": ["architecture"]})).await;
    let (st, _) =
        app.put(&owner, &format!("/pages/{setup}/content"), json!({"markdown": format!("Install it.\n\n![diagram](/api/attachments/{att}/content)")})).await;
    assert_eq!(st, 200);
    let (st, _) = app.put(&owner, &format!("/pages/{guide}/content"), json!({"markdown": format!("Start with [the setup](/pages/{setup}#install).")})).await;
    assert_eq!(st, 200);
    app.put(&owner, &format!("/pages/{guide}/labels"), json!({"labels": ["howto"]})).await;
    let (st, _) = app
        .patch(
            &owner,
            "/spaces/DOC",
            json!({"shortcuts": [{"title": "Guide", "url": format!("/pages/{guide}")}], "look": {"accentColor": "#aa3300", "hideBlog": true}}),
        )
        .await;
    assert_eq!(st, 200);
    let (_, me) = app.get(&owner, "/me").await;
    let (st, _) = app
        .put(
            &owner,
            &format!("/pages/{guide}/properties"),
            json!({"properties": [
                {"key": "Status", "type": "select", "value": "Draft"},
                {"key": "Effort", "type": "number", "value": 3},
                {"key": "Due", "type": "date", "value": "2026-12-01"},
                {"key": "Reviewer", "type": "user", "value": me["user"]["id"]},
                {"key": "Next", "type": "page", "value": setup},
                {"key": "Spec", "type": "url", "value": "https://example.com/spec"}
            ]}),
        )
        .await;
    assert_eq!(st, 200);
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
    assert!(guide_html.contains("<table class=\"properties\"><tr><th>Status</th><td>Draft</td></tr>"), "properties shown: {guide_html}");
    assert!(guide_html.contains("<a href=\"https://example.com/spec\">"), "{guide_html}");
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
    assert!(
        text.contains(
            "properties: {\"Status\": \"Draft\", \"Effort\": 3, \"Due\": \"2026-12-01\", \"Reviewer\": \"Owner\", \"Next\": \"Setup: step 1/2\", \"Spec\": \"https://example.com/spec\"}\n"
        ),
        "{text}"
    );
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
    let types: Vec<(String, String)> =
        g["properties"].as_array().unwrap().iter().map(|p| (p["key"].as_str().unwrap().into(), p["type"].as_str().unwrap().into())).collect();
    let want = [("Status", "text"), ("Effort", "number"), ("Due", "date"), ("Reviewer", "text"), ("Next", "text"), ("Spec", "url")];
    assert_eq!(types, want.map(|(k, t)| (k.to_string(), t.to_string())), "properties come back from front matter (types inferred)");
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
    let (_, doc2) = app.get(&owner, "/spaces/DOC2").await;
    assert_eq!(doc2["shortcuts"], json!([{"title": "Guide", "url": format!("/pages/{r_guide}")}]), "shortcuts restored, pointing at the restored page");
    assert_eq!(doc2["look"], json!({"accentColor": "#aa3300", "hideBlog": true}));
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
    let (_, labelled) = app.get(&owner, "/labels/architecture/attachments").await;
    assert!(labelled.as_array().unwrap().iter().any(|f| f["id"] == new_att.as_str()), "file labels restored: {labelled}");
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
        {"type": "macro", "attrs": {"name": "contentbylabel", "params": {"cql": "label = \"appendix\" and space = currentSpace()"}}},
        {"type": "macro", "attrs": {"name": "tasks-report-macro", "params": {"status": "incomplete"}}},
        {"type": "macro", "attrs": {"name": "gliffy", "params": {}}},
        {"type": "horizontalRule"}
    ]});
    let (st, _) = app.put(&owner, &format!("/pages/{pg}/content"), json!({"content": doc})).await;
    assert_eq!(st, 200);
    app.put(&owner, &format!("/pages/{child}/labels"), json!({"labels": ["appendix"]})).await;

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
    // report macros are evaluated for the reader; unknown macros keep a label
    assert!(xml.contains("Pages labelled appendix") && xml.contains("Tasks (open)") && xml.contains("Macro: gliffy"));
    assert!(xml.contains("Board deck") && xml.matches("Appendix A").count() >= 2, "child page listed by label too");
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

#[tokio::test(flavor = "multi_thread")]
async fn word_export_of_a_page_tree() {
    let app = common::spawn().await;
    let owner = app.setup().await;
    let (member, _) = app.user(&owner, "tree@acme.test", "member").await;
    let (_, sp) = app.post(&owner, "/spaces", json!({"key": "MAN", "name": "Manual"})).await;
    let home = sp["homePageId"].as_str().unwrap().to_string();
    let (_, root) = app.post(&owner, "/pages", json!({"spaceKey": "MAN", "parentId": home, "title": "Handbook"})).await;
    let root = root["id"].as_str().unwrap().to_string();
    let (_, a) = app.post(&owner, "/pages", json!({"spaceKey": "MAN", "parentId": root, "title": "Chapter A"})).await;
    let a = a["id"].as_str().unwrap().to_string();
    let (_, a1) = app.post(&owner, "/pages", json!({"spaceKey": "MAN", "parentId": a, "title": "Section A1", "markdown": "Deep content."})).await;
    let a1 = a1["id"].as_str().unwrap().to_string();
    let (_, b) = app.post(&owner, "/pages", json!({"spaceKey": "MAN", "parentId": root, "title": "Chapter B", "markdown": "B text"})).await;
    let b = b["id"].as_str().unwrap().to_string();
    // the root links to section A1; chapter B is restricted to the owner
    let (st, _) = app.put(&owner, &format!("/pages/{root}/content"), json!({"markdown": format!("Start with [the details](/pages/{a1}).")})).await;
    assert_eq!(st, 200);
    let (_, me) = app.get(&owner, "/me").await;
    let owner_id = me["user"]["id"].as_str().unwrap().to_string();
    app.put(&owner, &format!("/pages/{b}/restrictions"), json!({"restrictions": [{"kind": "view", "subjectType": "user", "subjectId": owner_id}]})).await;

    let docx = |c: reqwest::Client, url: String| async move {
        let r = c.get(url).send().await.unwrap();
        assert_eq!(r.status(), 200);
        let mut z = zip::ZipArchive::new(std::io::Cursor::new(r.bytes().await.unwrap().to_vec())).unwrap();
        entry(&mut z, "word/document.xml")
    };
    let xml = docx(member.clone(), app.url(&format!("/pages/{root}/export?format=docx&tree=true"))).await;
    let order: Vec<usize> =
        ["Handbook", "Chapter A", "Section A1"].iter().map(|t| xml.find(&format!(">{t}<")).unwrap_or_else(|| panic!("{t} missing"))).collect();
    assert!(order[0] < order[1] && order[1] < order[2], "depth-first order");
    assert!(!xml.contains("Chapter B") && !xml.contains("B text"), "restricted descendants are left out");
    assert!(xml.contains("w:pageBreakBefore"), "each page starts on a new page");
    assert!(xml.contains("w:anchor=\"p2\""), "the link to Section A1 points inside the document");
    // the owner gets all four; without tree only the page itself
    let xml = docx(owner.clone(), app.url(&format!("/pages/{root}/export?format=docx&tree=true"))).await;
    assert!(xml.contains("Chapter B") && xml.contains("B text"));
    let xml = docx(owner.clone(), app.url(&format!("/pages/{root}/export?format=docx"))).await;
    assert!(!xml.contains("Chapter A"));
}

#[tokio::test(flavor = "multi_thread")]
async fn drawings_in_exports() {
    let app = common::spawn().await;
    let owner = app.setup().await;
    app.post(&owner, "/spaces", json!({"key": "DRW", "name": "Drawings"})).await;
    let (_, pg) = app.post(&owner, "/pages", json!({"spaceKey": "DRW", "title": "Architecture sketch"})).await;
    let pg = pg["id"].as_str().unwrap().to_string();
    let png = upload(&app, &owner, &pg, "drawing-abc.png", tiny_png(30, 20)).await;
    let scene = r#"{"type":"excalidraw","elements":[{"type":"text","text":"Queue worker"}]}"#;
    let doc = json!({"type": "doc", "content": [{"type": "drawing", "attrs": {"scene": scene, "preview": format!("/api/attachments/{png}/content?v=1")}}]});
    assert_eq!(app.put(&owner, &format!("/pages/{pg}/content"), json!({"content": doc})).await.0, 200);
    let html = owner.get(app.url(&format!("/pages/{pg}/export?format=html"))).send().await.unwrap().text().await.unwrap();
    assert!(html.contains("class=\"drawing\"") && html.contains(&png), "{html}");
    let md = owner.get(app.url(&format!("/pages/{pg}/export?format=md"))).send().await.unwrap().text().await.unwrap();
    assert!(md.contains("![Drawing](/api/attachments/"), "{md}");
    let r = owner.get(app.url(&format!("/pages/{pg}/export?format=docx"))).send().await.unwrap();
    let z = zip::ZipArchive::new(std::io::Cursor::new(r.bytes().await.unwrap().to_vec())).unwrap();
    assert!(names(&z).iter().any(|f| f.starts_with("word/media/") && !f.ends_with('/')), "the drawing is embedded");
    // the drawing's words are searchable
    app.eventually(|| async {
        let (_, r) = app.get(&owner, "/search?q=worker").await;
        (r.as_array().unwrap().len() == 1).then_some(())
    })
    .await;
}

#[tokio::test(flavor = "multi_thread")]
async fn jobs_can_be_cancelled_and_retried() {
    let app = common::spawn().await;
    let owner = app.setup().await;
    let (member, _) = app.user(&owner, "mia@acme.test", "member").await;
    app.post(&owner, "/spaces", json!({"key": "DOC", "name": "Docs"})).await;
    let db = &app.state.db;
    let (_, me) = app.get(&owner, "/me").await;
    let owner_id: uuid::Uuid = me["user"]["id"].as_str().unwrap().parse().unwrap();
    let ws: uuid::Uuid = sqlx::query_scalar("SELECT workspace_id FROM users WHERE id = $1").bind(owner_id).fetch_one(db).await.unwrap();

    // a real export gives us a valid input to reuse
    let (_, body) = app.post(&owner, "/spaces/DOC/export", json!({"format": "markdown"})).await;
    let first = body["jobId"].as_str().unwrap().to_string();
    wait_job(&app, &owner, &first).await;
    let input: Value = sqlx::query_scalar("SELECT input FROM jobs WHERE id = $1::uuid").bind(&first).fetch_one(db).await.unwrap();

    // a queued job is cancelled at once (input-less rows are never picked up by the worker)
    let queued = uuid::Uuid::now_v7();
    sqlx::query("INSERT INTO jobs (id, workspace_id, kind, status, created_by) VALUES ($1, $2, 'export.space', 'queued', $3)")
        .bind(queued)
        .bind(ws)
        .bind(owner_id)
        .execute(db)
        .await
        .unwrap();
    let (st, _) = app.post(&member, &format!("/jobs/{queued}/cancel"), json!({})).await;
    assert_eq!(st, 404, "only its creator or an admin");
    let (st, j) = app.post(&owner, &format!("/jobs/{queued}/cancel"), json!({})).await;
    assert_eq!(st, 200, "{j}");
    assert_eq!(j["status"], "cancelled");
    assert_eq!(j["retryable"], true, "exports need no upload to run again");
    let (st, _) = app.post(&owner, &format!("/jobs/{queued}/cancel"), json!({})).await;
    assert_eq!(st, 409, "already finished");

    // retry runs it again
    sqlx::query("UPDATE jobs SET input = $2 WHERE id = $1").bind(queued).bind(&input).execute(db).await.unwrap();
    let (st, j) = app.post(&owner, &format!("/jobs/{queued}/retry"), json!({})).await;
    assert_eq!(st, 202, "{j}");
    assert_eq!(j["attempts"], 2);
    let done = wait_job(&app, &owner, &queued.to_string()).await;
    assert!(done["result"]["filename"].as_str().unwrap().ends_with(".zip"));
    let (st, _) = app.post(&owner, &format!("/jobs/{queued}/retry"), json!({})).await;
    assert_eq!(st, 409, "only failed or cancelled jobs");

    // a running job stops at its next progress update
    let running = uuid::Uuid::now_v7();
    sqlx::query(
        "INSERT INTO jobs (id, workspace_id, kind, status, created_by, started_at, heartbeat_at) VALUES ($1, $2, 'export.space', 'running', $3, now(), now())",
    )
    .bind(running)
    .bind(ws)
    .bind(owner_id)
    .execute(db)
    .await
    .unwrap();
    let (_, j) = app.post(&owner, &format!("/jobs/{running}/cancel"), json!({})).await;
    assert_eq!(j["status"], "running");
    assert_eq!(j["cancelling"], true);
    let err = wikicraft::jobs::checkpoint(&app.state, running, json!({"phase": "pages"})).await.unwrap_err();
    assert!(err.is::<wikicraft::jobs::Cancelled>());
    wikicraft::jobs::record_failure(&app.state, running, &err).await;
    let (_, j) = app.get(&owner, &format!("/jobs/{running}")).await;
    assert_eq!(j["status"], "cancelled");
    assert_eq!(j["cancelling"], false);

    // a failed import keeps its upload for a retry, which continues it as an update import
    let failed = uuid::Uuid::now_v7();
    sqlx::query(
        "INSERT INTO jobs (id, workspace_id, kind, status, created_by, error, finished_at, input)
         VALUES ($1, $2, 'import.confluence', 'failed', $3, 'disk full', now(), $4)",
    )
    .bind(failed)
    .bind(ws)
    .bind(owner_id)
    .bind(json!({"upload": "uploads/missing-file", "options": {"skipHistory": true}}))
    .execute(db)
    .await
    .unwrap();
    let (_, j) = app.get(&owner, &format!("/jobs/{failed}")).await;
    assert_eq!(j["retryable"], true);
    let (st, j) = app.post(&owner, &format!("/jobs/{failed}/retry"), json!({})).await;
    assert_eq!(st, 202, "{j}");
    let input: Value = sqlx::query_scalar("SELECT input FROM jobs WHERE id = $1").bind(failed).fetch_one(db).await.unwrap();
    assert_eq!(input["options"], json!({"skipHistory": true, "update": true}));
    // (the upload doesn't exist, so this attempt fails again; the upload reference stays)
    let j = app
        .eventually(|| async {
            let (_, j) = app.get(&owner, &format!("/jobs/{failed}")).await;
            (j["status"] == "failed").then_some(j)
        })
        .await;
    assert_eq!(j["attempts"], 2);
    assert_eq!(j["retryable"], true);
    // without its upload it can't run again
    sqlx::query("UPDATE jobs SET input = input - 'upload' WHERE id = $1").bind(failed).execute(db).await.unwrap();
    let (st, body) = app.post(&owner, &format!("/jobs/{failed}/retry"), json!({})).await;
    assert_eq!(st, 409, "{body}");

    // the admin job list shows the state
    let (_, list) = app.get(&owner, "/jobs").await;
    let row = list.as_array().unwrap().iter().find(|x| x["id"] == running.to_string()).unwrap().clone();
    assert_eq!(row["status"], "cancelled");
}

#[tokio::test(flavor = "multi_thread")]
async fn pdf_export_of_a_page_tree() {
    let app = common::spawn().await;
    let owner = app.setup().await;
    let (member, _) = app.user(&owner, "pdf@acme.test", "member").await;
    let (_, sp) = app.post(&owner, "/spaces", json!({"key": "PDF", "name": "Printed"})).await;
    let home = sp["homePageId"].as_str().unwrap().to_string();
    let (_, root) = app.post(&owner, "/pages", json!({"spaceKey": "PDF", "parentId": home, "title": "Handbook"})).await;
    let root = root["id"].as_str().unwrap().to_string();
    let (_, a) =
        app.post(&owner, "/pages", json!({"spaceKey": "PDF", "parentId": root, "title": "Chapter Alpha", "markdown": "## Setup\n\nAlpha body text."})).await;
    let a = a["id"].as_str().unwrap().to_string();
    let (_, b) = app.post(&owner, "/pages", json!({"spaceKey": "PDF", "parentId": root, "title": "Chapter Secret", "markdown": "Secret body"})).await;
    let b = b["id"].as_str().unwrap().to_string();
    // an image, a table, a task, a link into the export; the second chapter is restricted
    let png = {
        let mut buf = std::io::Cursor::new(Vec::new());
        image::RgbImage::from_pixel(40, 20, image::Rgb([30, 120, 200])).write_to(&mut buf, image::ImageFormat::Png).unwrap();
        buf.into_inner()
    };
    let form = reqwest::multipart::Form::new().part("file", reqwest::multipart::Part::bytes(png).file_name("chart.png"));
    let rows: Value = owner.post(app.url(&format!("/pages/{root}/attachments"))).multipart(form).send().await.unwrap().json().await.unwrap();
    let att = rows[0]["id"].as_str().unwrap().to_string();
    let md = format!(
        "Start with [chapter alpha](/pages/{a}).\n\n![chart](/api/attachments/{att}/content)\n\n| Name | Value |\n|---|---|\n| rows | 3 |\n\n- [x] printed task\n\n```rust\nfn main() {{}}\n```"
    );
    assert_eq!(app.put(&owner, &format!("/pages/{root}/content"), json!({ "markdown": md })).await.0, 200);
    let (_, me) = app.get(&owner, "/me").await;
    let owner_id = me["user"]["id"].as_str().unwrap().to_string();
    app.put(&owner, &format!("/pages/{b}/restrictions"), json!({"restrictions": [{"kind": "view", "subjectType": "user", "subjectId": owner_id}]})).await;

    let pdf = |c: reqwest::Client, url: String| async move {
        let r = c.get(url).send().await.unwrap();
        assert_eq!(r.status(), 200);
        assert_eq!(r.headers()["content-type"], "application/pdf");
        assert!(r.headers()["content-disposition"].to_str().unwrap().contains("Handbook.pdf"));
        let bytes = r.bytes().await.unwrap().to_vec();
        assert!(bytes.starts_with(b"%PDF"));
        wikicraft::extract::extract("application/pdf", "x.pdf", &bytes).unwrap_or_default()
    };
    let text = pdf(member.clone(), app.url(&format!("/pages/{root}/export?format=pdf&tree=true&toc=true&cover=true"))).await;
    for want in ["Handbook", "Contents", "Chapter Alpha", "Alpha body text", "printed task", "rows", "fn main", "Printed", "Exported by pdf"] {
        assert!(text.contains(want), "{want} missing in:\n{text}");
    }
    assert!(!text.contains("Chapter Secret") && !text.contains("Secret body"), "restricted pages are left out");
    // without tree: just the page
    let text = pdf(owner.clone(), app.url(&format!("/pages/{root}/export?format=pdf"))).await;
    assert!(text.contains("Handbook") && !text.contains("Alpha body text"));
    // a page tree needs a signed-in reader
    assert_eq!(common::client().get(app.url(&format!("/pages/{root}/export?format=pdf&tree=true"))).send().await.unwrap().status(), 401);
}
