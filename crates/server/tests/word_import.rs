//! Word (.docx) import: documents written by pandoc and LibreOffice, and a round trip of
//! WikiCraft's own Word export.

mod common;

use serde_json::{Value, json};

use common::TestApp;

async fn import(app: &TestApp, c: &reqwest::Client, space: &str, name: &str, bytes: Vec<u8>) -> Value {
    let form =
        reqwest::multipart::Form::new().text("spaceKey", space.to_string()).part("file", reqwest::multipart::Part::bytes(bytes).file_name(name.to_string()));
    let r = c.post(app.url("/imports/markdown")).multipart(form).send().await.unwrap();
    assert_eq!(r.status(), 201, "{}", r.text().await.unwrap());
    r.json().await.unwrap()
}

/// The imported page (by title) and its document.
async fn page(app: &TestApp, c: &reqwest::Client, space: &str, title: &str) -> (String, Value) {
    let (_, tree) = app.get(c, &format!("/spaces/{space}/tree")).await;
    let nodes = tree["nodes"].as_array().unwrap();
    let id = nodes.iter().find(|n| n["title"] == title).unwrap_or_else(|| panic!("no page {title}: {tree}"))["id"].as_str().unwrap().to_string();
    let (_, p) = app.get(c, &format!("/pages/{id}")).await;
    (id, p["content"].clone())
}

fn find<'a>(doc: &'a Value, kind: &str) -> Vec<&'a Value> {
    let mut out = Vec::new();
    fn go<'a>(n: &'a Value, kind: &str, out: &mut Vec<&'a Value>) {
        if n["type"] == kind {
            out.push(n);
        }
        if let Some(c) = n["content"].as_array() {
            for x in c {
                go(x, kind, out);
            }
        }
    }
    go(doc, kind, &mut out);
    out
}

fn text(n: &Value) -> String {
    let mut s = String::new();
    fn go(n: &Value, s: &mut String) {
        if let Some(t) = n["text"].as_str() {
            s.push_str(t);
        }
        if let Some(c) = n["content"].as_array() {
            for x in c {
                go(x, s);
            }
        }
    }
    go(n, &mut s);
    s
}

/// What pandoc and LibreOffice both get right from the same handbook.
fn check_handbook(doc: &Value, writer: &str) {
    let h1 = find(doc, "heading");
    assert!(h1.iter().any(|h| h["attrs"]["level"] == 1 && text(h) == "Getting started"), "{writer}: {doc}");
    assert!(h1.iter().any(|h| h["attrs"]["level"] == 2 && text(h) == "Tools"), "{writer}");
    let all = text(doc);
    assert!(all.contains("Welcome to the team.") && all.contains("Ask questions early."), "{writer}: {all}");
    // marks and links
    let texts = find(doc, "text");
    assert!(texts.iter().any(|t| t["text"] == "team" && t["marks"].as_array().is_some_and(|m| m.iter().any(|m| m["type"] == "bold"))), "{writer}: bold");
    assert!(texts.iter().any(|t| t["text"] == "whole" && t["marks"].as_array().is_some_and(|m| m.iter().any(|m| m["type"] == "italic"))), "{writer}: italic");
    assert!(
        texts.iter().any(|t| t["marks"].as_array().is_some_and(|m| m.iter().any(|m| m["type"] == "link" && m["attrs"]["href"] == "https://example.com/team"))),
        "{writer}: link"
    );
    // a nested bullet list and a numbered list
    let bullets = find(doc, "bulletList");
    assert!(
        bullets.iter().any(|b| text(b).contains("Laptop") && !find(b, "bulletList").is_empty() && text(b).contains("Charger")),
        "{writer}: nested bullets {doc}"
    );
    let ordered = find(doc, "orderedList");
    assert!(ordered.iter().any(|o| o["content"].as_array().unwrap().len() == 3 && text(o).contains("Set up 2FA")), "{writer}: ordered");
    // the table with a header row
    let table = find(doc, "table");
    assert_eq!(table.len(), 1, "{writer}");
    assert_eq!(table[0]["content"].as_array().unwrap().len(), 3);
    assert!(text(table[0]).contains("Owner") && text(table[0]).contains("120"));
    // code, quote, image, footnote
    let code = find(doc, "codeBlock");
    assert!(code.iter().any(|c| text(c) == "cargo build --release\n./run.sh"), "{writer}: code {code:?}");
    assert!(find(doc, "blockquote").iter().any(|q| text(q).contains("Ask questions early.")), "{writer}: quote");
    let images = find(doc, "image");
    assert_eq!(images.len(), 1, "{writer}: {images:?}");
    assert!(images[0]["attrs"]["src"].as_str().unwrap().starts_with("/api/attachments/"), "{writer}: image stored as attachment");
    assert_eq!(images[0]["attrs"]["caption"], "Architecture diagram", "{writer}: caption attached to the image");
    assert!(all.contains("Like this one."), "{writer}: footnote text kept");
}

#[tokio::test(flavor = "multi_thread")]
async fn word_documents_from_pandoc_and_libreoffice() {
    let app = common::spawn().await;
    let owner = app.setup().await;
    app.post(&owner, "/spaces", json!({"key": "DOCS", "name": "Docs"})).await;
    for (writer, file) in [("pandoc", "pandoc-handbook.docx"), ("libreoffice", "libreoffice-handbook.docx")] {
        let bytes = std::fs::read(format!("{}/tests/fixtures/{file}", env!("CARGO_MANIFEST_DIR"))).unwrap();
        let res = import(&app, &owner, "DOCS", file, bytes).await;
        assert_eq!(res["pages"], 1, "{writer}: {res}");
        assert_eq!(res["attachments"], 1, "{writer}: {res}");
        // the document title becomes the page title
        let (_, doc) = page(&app, &owner, "DOCS", "Team Handbook").await;
        check_handbook(&doc, writer);
        // next writer: rename this one out of the way
        let (_, tree) = app.get(&owner, "/spaces/DOCS/tree").await;
        let id = tree["nodes"].as_array().unwrap().iter().find(|n| n["title"] == "Team Handbook").unwrap()["id"].as_str().unwrap().to_string();
        let (st, _) = app.patch(&owner, &format!("/pages/{id}"), json!({"title": format!("Handbook ({writer})")})).await;
        assert!(st == 200 || st == 204, "{st}");
    }
    // garbage is refused cleanly
    let form =
        reqwest::multipart::Form::new().text("spaceKey", "DOCS").part("file", reqwest::multipart::Part::bytes(b"not a zip".to_vec()).file_name("broken.docx"));
    let r = owner.post(app.url("/imports/markdown")).multipart(form).send().await.unwrap();
    assert_eq!(r.status(), 400);
}

#[tokio::test(flavor = "multi_thread")]
async fn word_export_round_trips() {
    let app = common::spawn().await;
    let owner = app.setup().await;
    app.post(&owner, "/spaces", json!({"key": "SRC", "name": "Source"})).await;
    app.post(&owner, "/spaces", json!({"key": "DST", "name": "Destination"})).await;
    let (_, pg) = app.post(&owner, "/pages", json!({"spaceKey": "SRC", "title": "Runbook"})).await;
    let pg = pg["id"].as_str().unwrap().to_string();
    let t = |s: &str| json!({"type": "text", "text": s});
    let p = |c: Value| json!({"type": "paragraph", "content": c});
    let doc = json!({"type": "doc", "content": [
        {"type": "heading", "attrs": {"level": 2}, "content": [t("Steps")]},
        {"type": "orderedList", "attrs": {"start": 4}, "content": [
            {"type": "listItem", "content": [p(json!([t("Drain the node")]))]},
            {"type": "listItem", "content": [p(json!([t("Upgrade")]))]}]},
        {"type": "callout", "attrs": {"type": "warning"}, "content": [p(json!([t("Never on Fridays.")]))]},
        {"type": "callout", "attrs": {"type": "success"}, "content": [p(json!([t("All green.")]))]},
        {"type": "codeBlock", "content": [t("kubectl drain node-1\nkubectl uncordon node-1")]},
        {"type": "table", "content": [
            {"type": "tableRow", "content": [
                {"type": "tableHeader", "content": [p(json!([t("Env")]))]},
                {"type": "tableHeader", "attrs": {"colspan": 2}, "content": [p(json!([t("Windows")]))]}]},
            {"type": "tableRow", "content": [
                {"type": "tableCell", "attrs": {"rowspan": 2}, "content": [p(json!([t("Prod")]))]},
                {"type": "tableCell", "content": [p(json!([t("Tue")]))]},
                {"type": "tableCell", "content": [p(json!([t("Thu")]))]}]},
            {"type": "tableRow", "content": [
                {"type": "tableCell", "content": [p(json!([t("Sat")]))]},
                {"type": "tableCell", "content": [p(json!([t("Sun")]))]}]}]},
        p(json!([t("See "), {"type": "text", "text": "the docs", "marks": [{"type": "link", "attrs": {"href": "https://example.com/docs"}}, {"type": "italic"}]}])),
        {"type": "heading", "attrs": {"level": 3, "indent": 1}, "content": [t("Indented heading")]},
        {"type": "paragraph", "attrs": {"indent": 2, "textAlign": "right"}, "content": [t("Indented twice")]}
    ]});
    let (st, _) = app.put(&owner, &format!("/pages/{pg}/content"), json!({"content": doc})).await;
    assert_eq!(st, 200);
    let bytes = owner.get(app.url(&format!("/pages/{pg}/export?format=docx"))).send().await.unwrap().bytes().await.unwrap().to_vec();
    import(&app, &owner, "DST", "Runbook.docx", bytes).await;
    let (_, back) = page(&app, &owner, "DST", "Runbook").await;

    assert!(find(&back, "heading").iter().any(|h| h["attrs"]["level"] == 2 && text(h) == "Steps"), "{back}");
    let ol = find(&back, "orderedList");
    assert_eq!(ol.len(), 1, "{back}");
    assert_eq!(ol[0]["attrs"]["start"], 4);
    let callouts = find(&back, "callout");
    let kinds: Vec<&str> = callouts.iter().map(|c| c["attrs"]["type"].as_str().unwrap()).collect();
    assert_eq!(kinds, vec!["warning", "success"], "{back}");
    assert_eq!(text(callouts[0]), "Never on Fridays.", "the exported icon is dropped again");
    assert!(find(&back, "codeBlock").iter().any(|c| text(c) == "kubectl drain node-1\nkubectl uncordon node-1"), "{back}");
    let table = find(&back, "table")[0];
    let rows = table["content"].as_array().unwrap();
    assert_eq!(rows.len(), 3);
    assert_eq!(rows[0]["content"][0]["type"], "tableHeader");
    assert_eq!(rows[0]["content"][1]["attrs"]["colspan"], 2);
    assert_eq!(rows[1]["content"][0]["attrs"]["rowspan"], 2);
    assert_eq!(rows[2]["content"].as_array().unwrap().len(), 2, "the merged cell is not repeated");
    let link = find(&back, "text").into_iter().find(|t| t["text"] == "the docs").cloned().unwrap();
    let marks: Vec<&str> = link["marks"].as_array().unwrap().iter().map(|m| m["type"].as_str().unwrap()).collect();
    assert!(marks.contains(&"link") && marks.contains(&"italic"), "{link}");
    let indented = find(&back, "paragraph").into_iter().find(|p| text(p) == "Indented twice").cloned().unwrap();
    assert_eq!(indented["attrs"]["indent"], 2, "{indented}");
    assert_eq!(indented["attrs"]["textAlign"], "right");
    assert!(find(&back, "heading").iter().any(|h| text(h) == "Indented heading" && h["attrs"]["indent"] == 1), "{back}");
}
