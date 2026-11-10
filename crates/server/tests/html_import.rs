//! HTML files / zip and Confluence HTML space exports through the folder importer.

mod common;

use std::io::Write;

use serde_json::{Value, json};

fn zip(files: &[(&str, &[u8])]) -> Vec<u8> {
    let mut w = zip::ZipWriter::new(std::io::Cursor::new(Vec::new()));
    for (name, data) in files {
        w.start_file(*name, zip::write::SimpleFileOptions::default()).unwrap();
        w.write_all(data).unwrap();
    }
    w.finish().unwrap().into_inner()
}

async fn upload(app: &common::TestApp, c: &reqwest::Client, space: &str, name: &str, data: Vec<u8>) -> Value {
    let form =
        reqwest::multipart::Form::new().text("spaceKey", space.to_string()).part("file", reqwest::multipart::Part::bytes(data).file_name(name.to_string()));
    let r = c.post(app.url("/imports/markdown")).multipart(form).send().await.unwrap();
    assert_eq!(r.status(), 201, "{}", r.text().await.unwrap());
    r.json().await.unwrap()
}

#[tokio::test(flavor = "multi_thread")]
async fn html_zip_becomes_a_page_tree_with_attachments_and_links() {
    let app = common::spawn().await;
    let owner = app.setup().await;
    app.post(&owner, "/spaces", json!({"key": "HTML", "name": "From HTML"})).await;
    let png = b"\x89PNG\r\n\x1a\nhtml-image".to_vec();
    let intro = "<!DOCTYPE html><html><head><title>Introduction</title><script>var a = 1 < 2; document.write('<p>evil</p>')</script>\
        <style>p{color:red}</style></head><body><nav><a href=\"../index.html\">Home</a></nav><h1>Introduction</h1>\
        <p>Read the <a href=\"setup.html#install\">setup guide</a> and the <a href=\"files/spec.pdf\">spec</a>.</p>\
        <p><img src=\"img/logo.png\" alt=\"Logo\"></p><ul><li>one</li><li>two</li></ul></body></html>";
    let setup = "<html><head><title>Setup</title></head><body><h2 id=\"install\">Install</h2><pre><code>cargo run</code></pre>\
        <table><tr><th>Key</th><th>Value</th></tr><tr><td>port</td><td>8080</td></tr></table>\
        <p>Back to <a href=\"intro.html\">the introduction</a>, see <a href=\"https://example.com\">example</a>.</p></body></html>";
    let data = zip(&[
        ("guide/intro.html", intro.as_bytes()),
        ("guide/setup.html", setup.as_bytes()),
        ("guide/img/logo.png", &png),
        ("guide/files/spec.pdf", b"%PDF-1.4 spec"),
        ("guide/styles/site.css", b"body{}"),
    ]);
    let res = upload(&app, &owner, "HTML", "site.zip", data).await;
    assert_eq!(res["pages"], 3, "{res}");
    assert_eq!(res["attachments"], 2, "{res}");

    let (_, tree) = app.get(&owner, "/spaces/HTML/tree").await;
    let nodes = tree["nodes"].as_array().unwrap();
    let titles: Vec<&str> = nodes.iter().map(|n| n["title"].as_str().unwrap()).collect();
    for t in ["guide", "Introduction", "Setup"] {
        assert_eq!(titles.iter().filter(|x| **x == t).count(), 1, "exactly one {t}: {titles:?}");
    }
    let id = |t: &str| nodes.iter().find(|n| n["title"] == t).unwrap()["id"].as_str().unwrap().to_string();
    let parent = |t: &str| nodes.iter().find(|n| n["title"] == t).unwrap()["parentId"].as_str().map(String::from);
    assert_eq!(parent("Introduction"), Some(id("guide")));
    assert_eq!(parent("Setup"), Some(id("guide")));

    let (_, intro) = app.get(&owner, &format!("/pages/{}", id("Introduction"))).await;
    let s = intro["content"].to_string();
    assert!(s.contains(&format!("/pages/{}#install", id("Setup"))), "cross link: {s}");
    assert!(s.contains("\"bulletList\"") && s.contains("setup guide"), "{s}");
    assert!(!s.contains("evil") && !s.contains("color:red") && !s.contains("Home"), "scripts, styles and navigation are dropped: {s}");
    assert!(!s.contains("\"level\":1"), "the title heading is not repeated: {s}");
    assert!(!s.contains("img/logo.png") && !s.contains("files/spec.pdf"), "local files are rewritten: {s}");

    let (_, atts) = app.get(&owner, &format!("/pages/{}/attachments", id("Introduction"))).await;
    let atts = atts.as_array().unwrap_or_else(|| atts["attachments"].as_array().unwrap()).clone();
    let names: Vec<&str> = atts.iter().map(|a| a["filename"].as_str().or(a["name"].as_str()).unwrap()).collect();
    assert!(names.contains(&"logo.png") && names.contains(&"spec.pdf"), "{names:?}");
    for a in &atts {
        let aid = a["id"].as_str().unwrap();
        assert!(s.contains(&format!("/api/attachments/{aid}/content")), "{aid} referenced: {s}");
    }
    let logo = atts.iter().find(|a| a["filename"] == "logo.png" || a["name"] == "logo.png").unwrap()["id"].as_str().unwrap().to_string();
    let bytes = owner.get(app.url(&format!("/attachments/{logo}/content"))).send().await.unwrap().bytes().await.unwrap();
    assert_eq!(bytes.as_ref(), png.as_slice());

    let (_, setup) = app.get(&owner, &format!("/pages/{}", id("Setup"))).await;
    let s = setup["content"].to_string();
    assert!(s.contains(&format!("/pages/{}", id("Introduction"))) && s.contains("https://example.com"), "{s}");
    assert!(s.contains("\"codeBlock\"") && s.contains("cargo run") && s.contains("\"table\"") && s.contains("8080"), "{s}");

    // a single HTML file works too
    let res = upload(&app, &owner, "HTML", "note.html", b"<html><body><h1>Lone note</h1><p>Text</p></body></html>".to_vec()).await;
    assert_eq!(res["pages"], 1);
    let (_, tree) = app.get(&owner, "/spaces/HTML/tree").await;
    assert!(tree["nodes"].as_array().unwrap().iter().any(|n| n["title"] == "Lone note"), "{tree}");
}

#[tokio::test(flavor = "multi_thread")]
async fn confluence_html_export_keeps_hierarchy_and_attachment_names() {
    let app = common::spawn().await;
    let owner = app.setup().await;
    app.post(&owner, "/spaces", json!({"key": "CHTML", "name": "Confluence HTML"})).await;
    let page = |title: &str, crumbs: &[(&str, &str)], body: &str, atts: &[(&str, &str)]| {
        let crumbs: String = crumbs.iter().map(|(h, t)| format!("<li><span><a href=\"{h}\">{t}</a></span></li>")).collect();
        let atts: String = atts.iter().map(|(h, n)| format!("<img src=\"images/icons/bullet_blue.gif\"/> <a href=\"{h}\">{n}</a> (image/png)<br/>")).collect();
        let section = if atts.is_empty() {
            String::new()
        } else {
            format!(
                "<div class=\"pageSection group\"><div class=\"pageSectionHeader\"><h2 id=\"attachments\" class=\"pageSectionTitle\">Attachments:</h2></div><div class=\"greybox\" align=\"left\">{atts}</div></div>"
            )
        };
        format!(
            "<!DOCTYPE html><html><head><title>Team : {title}</title><link rel=\"stylesheet\" href=\"styles/site.css\" type=\"text/css\" /></head><body class=\"theme-default aui-theme-default\">\
             <div id=\"page\"><div id=\"main\" class=\"aui-page-panel\"><div id=\"main-header\"><div id=\"breadcrumb-section\"><ol id=\"breadcrumbs\">{crumbs}</ol></div>\
             <h1 id=\"title-heading\" class=\"pagetitle\"><span id=\"title-text\"> Team : {title} </span></h1></div>\
             <div id=\"content\" class=\"view\"><div class=\"page-metadata\">Created by <span class='author'> Ann</span></div>\
             <div id=\"main-content\" class=\"wiki-content group\">{body}</div>{section}</div></div>\
             <div id=\"footer\" role=\"contentinfo\"><section class=\"footer-body\"><p>Document generated by Confluence</p></section></div></div></body></html>"
        )
    };
    let home = page("Home", &[("index.html", "Team")], "<p>Welcome, see <a href=\"Child_200.html\">Child</a>.</p>", &[]);
    let child = page(
        "Child",
        &[("index.html", "Team"), ("Home_100.html", "Home")],
        "<p>Diagram: <span class=\"confluence-embedded-file-wrapper\"><img class=\"confluence-embedded-image\" src=\"attachments/200/300.png?width=250\" data-image-src=\"attachments/200/300.png\"></span></p>",
        &[("attachments/200/300.png", "diagram.png"), ("attachments/200/301.txt", "notes.txt")],
    );
    let index = "<html><head><title>Team</title></head><body><div id=\"main-content\"><h2>Available Pages:</h2><ul><li><a href=\"Home_100.html\">Home</a><ul><li><a href=\"Child_200.html\">Child</a></li></ul></li></ul></div></body></html>";
    let data = zip(&[
        ("TEAM/index.html", index.as_bytes()),
        ("TEAM/Home_100.html", home.as_bytes()),
        ("TEAM/Child_200.html", child.as_bytes()),
        ("TEAM/attachments/200/300.png", b"\x89PNG\r\n\x1a\ndiagram"),
        ("TEAM/attachments/200/301.txt", b"notes"),
        ("TEAM/images/icons/bullet_blue.gif", b"GIF89a"),
        ("TEAM/styles/site.css", b"body{}"),
    ]);
    let res = upload(&app, &owner, "CHTML", "Confluence-space-export-TEAM.html.zip", data).await;
    assert_eq!(res["pages"], 3, "{res}");
    assert_eq!(res["attachments"], 2, "{res}");

    let (_, tree) = app.get(&owner, "/spaces/CHTML/tree").await;
    let nodes = tree["nodes"].as_array().unwrap();
    let titles: Vec<&str> = nodes.iter().map(|n| n["title"].as_str().unwrap()).collect();
    for t in ["Team", "Home", "Child"] {
        assert_eq!(titles.iter().filter(|x| **x == t).count(), 1, "exactly one {t}: {titles:?}");
    }
    let id = |t: &str| nodes.iter().find(|n| n["title"] == t).unwrap()["id"].as_str().unwrap().to_string();
    let parent = |t: &str| nodes.iter().find(|n| n["title"] == t).unwrap()["parentId"].as_str().map(String::from);
    assert_eq!(parent("Home"), Some(id("Team")));
    assert_eq!(parent("Child"), Some(id("Home")), "breadcrumbs give the hierarchy");

    let (_, c) = app.get(&owner, &format!("/pages/{}", id("Child"))).await;
    let s = c["content"].to_string();
    assert!(s.contains("Diagram") && s.contains("/api/attachments/"), "{s}");
    assert!(!s.contains("Created by") && !s.contains("generated by Confluence") && !s.contains("Attachments:"), "{s}");
    let (_, atts) = app.get(&owner, &format!("/pages/{}/attachments", id("Child"))).await;
    let atts = atts.as_array().unwrap_or_else(|| atts["attachments"].as_array().unwrap()).clone();
    let mut names: Vec<&str> = atts.iter().map(|a| a["filename"].as_str().or(a["name"].as_str()).unwrap()).collect();
    names.sort();
    assert_eq!(names, vec!["diagram.png", "notes.txt"]);
    let (_, h) = app.get(&owner, &format!("/pages/{}", id("Home"))).await;
    assert!(h["content"].to_string().contains(&format!("/pages/{}", id("Child"))), "{}", h["content"]);
}
