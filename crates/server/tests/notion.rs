//! Notion "Markdown & CSV" exports through the Markdown importer.

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

#[tokio::test(flavor = "multi_thread")]
async fn notion_export_becomes_a_clean_page_tree() {
    let app = common::spawn().await;
    let owner = app.setup().await;
    app.post(&owner, "/spaces", json!({"key": "NOTION", "name": "From Notion"})).await;
    let (a, b, c, d) = ("1".repeat(32), "2".repeat(32), "3".repeat(32), "4".repeat(32));
    let png = b"\x89PNG\r\n\x1a\nnotion".to_vec();
    let root_md = format!(
        "# Team Wiki\n\nWelcome. Start with [Onboarding](Team%20Wiki%20{a}/Onboarding%20{b}.md) and see [Projects](Team%20Wiki%20{a}/Projects%20{c}.csv).\n"
    );
    let onboarding_md = format!("# Onboarding\n\n![Laptop](Onboarding%20{b}/laptop.png)\n\nBack to [Team Wiki](../Team%20Wiki%20{a}.md).\n");
    let apollo_md = "# Apollo\n\nStatus: Active\n\nDetails.\n".to_string();
    let names = [
        format!("Team Wiki {a}.md"),
        format!("Team Wiki {a}/Onboarding {b}.md"),
        format!("Team Wiki {a}/Onboarding {b}/laptop.png"),
        format!("Team Wiki {a}/Projects {c}.csv"),
        format!("Team Wiki {a}/Projects {c}_all.csv"),
        format!("Team Wiki {a}/Projects {c}/Apollo {d}.md"),
    ];
    let inner = zip(&[
        (&names[0], root_md.as_bytes()),
        (&names[1], onboarding_md.as_bytes()),
        (&names[2], &png),
        (&names[3], b"Name,Status\nApollo,Active\n"),
        (&names[4], b"\xef\xbb\xbfName,Status,Owner\r\nApollo,Active,Ann\r\nBorealis,Paused,\"Ben, Bo\"\r\n"),
        (&names[5], apollo_md.as_bytes()),
    ]);
    // Notion's download wraps the export in another zip
    let outer = zip(&[("Export-0f3c2a.zip", &inner)]);
    let form = reqwest::multipart::Form::new().text("spaceKey", "NOTION").part("file", reqwest::multipart::Part::bytes(outer).file_name("notion-export.zip"));
    let r = owner.post(app.url("/imports/markdown")).multipart(form).send().await.unwrap();
    assert_eq!(r.status(), 201, "{}", r.text().await.unwrap());
    let res: Value = r.json().await.unwrap();
    assert_eq!(res["attachments"], 1);

    let (_, tree) = app.get(&owner, "/spaces/NOTION/tree").await;
    let nodes = tree["nodes"].as_array().unwrap();
    let titles: Vec<&str> = nodes.iter().map(|n| n["title"].as_str().unwrap()).collect();
    for t in ["Team Wiki", "Onboarding", "Projects", "Apollo"] {
        assert_eq!(titles.iter().filter(|x| **x == t).count(), 1, "exactly one {t}: {titles:?}");
    }
    assert!(!titles.iter().any(|t| t.contains("1111") || t.ends_with(".csv")), "no Notion ids in titles: {titles:?}");
    let id = |t: &str| nodes.iter().find(|n| n["title"] == t).unwrap()["id"].as_str().unwrap().to_string();
    let parent = |t: &str| nodes.iter().find(|n| n["title"] == t).unwrap()["parentId"].as_str().map(String::from);
    assert_eq!(parent("Onboarding"), Some(id("Team Wiki")));
    assert_eq!(parent("Projects"), Some(id("Team Wiki")));
    assert_eq!(parent("Apollo"), Some(id("Projects")));

    // links between pages and to the database resolve; the image is an attachment
    let (_, root) = app.get(&owner, &format!("/pages/{}", id("Team Wiki"))).await;
    let s = root["content"].to_string();
    assert!(s.contains(&format!("/pages/{}", id("Onboarding"))) && s.contains(&format!("/pages/{}", id("Projects"))), "{s}");
    let (_, onb) = app.get(&owner, &format!("/pages/{}", id("Onboarding"))).await;
    let s = onb["content"].to_string();
    assert!(s.contains("/api/attachments/") && s.contains(&format!("/pages/{}", id("Team Wiki"))), "{s}");

    // the database is a table (all properties), rows link to their pages
    let (_, db) = app.get(&owner, &format!("/pages/{}", id("Projects"))).await;
    let s = db["content"].to_string();
    assert!(s.contains("\"type\":\"table\"") && s.contains("Owner") && s.contains("Ben, Bo"), "{s}");
    assert!(s.contains(&format!("/pages/{}", id("Apollo"))), "row links to its page: {s}");
    assert!(!s.contains("wikilink:Borealis") && !s.contains("missing?space=NOTION&title=Borealis"), "rows without pages aren't links");
}
