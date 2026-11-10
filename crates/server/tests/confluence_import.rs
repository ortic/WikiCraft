//! End-to-end Confluence import through the HTTP API using the real exports downloaded by
//! `research/confluence-import/fetch_fixtures.sh` (skipped when absent).

mod common;

use std::io::Write;
use std::path::{Path, PathBuf};

use serde_json::{Value, json};

fn fixtures() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../../research/confluence-import/fixtures")
}

async fn import(app: &common::TestApp, c: &reqwest::Client, zip: Vec<u8>, options: Value) -> Value {
    let form = reqwest::multipart::Form::new().text("options", options.to_string()).part("file", reqwest::multipart::Part::bytes(zip).file_name("export.zip"));
    let r = c.post(app.url("/imports/confluence")).multipart(form).send().await.unwrap();
    assert_eq!(r.status(), 202, "{}", r.text().await.unwrap());
    let job = r.json::<Value>().await.unwrap()["jobId"].as_str().unwrap().to_string();
    for _ in 0..1200 {
        let (_, j) = app.get(c, &format!("/jobs/{job}")).await;
        match j["status"].as_str() {
            Some("succeeded") => return j["result"].clone(),
            Some("failed") => panic!("import failed: {}", j["error"]),
            _ => tokio::time::sleep(std::time::Duration::from_millis(250)).await,
        }
    }
    panic!("import timed out");
}

async fn assert_no_placeholders(app: &common::TestApp) {
    let leftovers: i64 = sqlx::query_scalar("SELECT count(*) FROM pages WHERE content::text LIKE '%wcimport:%'").fetch_one(&app.state.db).await.unwrap();
    assert_eq!(leftovers, 0, "all import placeholders must be rewritten");
    let leftovers: i64 =
        sqlx::query_scalar("SELECT count(*) FROM page_versions WHERE content::text LIKE '%wcimport:page/%'").fetch_one(&app.state.db).await.unwrap();
    assert_eq!(leftovers, 0);
}

#[tokio::test(flavor = "multi_thread")]
async fn imports_classic_xml_export() {
    let zip = fixtures().join("modelop-cloud-xml/export.zip");
    if !zip.exists() {
        eprintln!("skipping: fixture missing");
        return;
    }
    let app = common::spawn().await;
    let c = app.setup().await;
    let result = import(&app, &c, std::fs::read(&zip).unwrap(), json!({})).await;
    eprintln!("{}", serde_json::to_string_pretty(&result["created"]).unwrap());
    eprintln!("{}", serde_json::to_string_pretty(&result["links"]).unwrap());
    assert_eq!(result["spaces"][0]["key"], "DV21");
    assert_eq!(result["created"]["pages"], 45);
    assert_eq!(result["created"]["attachments"], 202);
    assert_eq!(result["created"]["attachmentBinariesMissing"].as_i64().unwrap_or(0), 0);
    assert_no_placeholders(&app).await;

    // tree + history + attachments are reachable through the API
    let (_, tree) = app.get(&c, "/spaces/DV21/tree").await;
    // archived Confluence pages go to the archive: they and their children leave the tree
    let archived: i64 = sqlx::query_scalar(
        "WITH RECURSIVE gone AS (SELECT p.id FROM pages p JOIN spaces s ON s.id = p.space_id WHERE s.key = 'DV21' AND p.archived_at IS NOT NULL
                                 UNION SELECT p.id FROM pages p JOIN gone g ON p.parent_id = g.id)
         SELECT count(*) FROM gone",
    )
    .fetch_one(&app.state.db)
    .await
    .unwrap();
    eprintln!("archived pages (with children): {archived}");
    assert_eq!(tree["nodes"].as_array().unwrap().len() as i64, 45 - archived);
    let home = tree["homePageId"].as_str().unwrap();
    let (_, home_page) = app.get(&c, &format!("/pages/{home}")).await;
    assert_eq!(home_page["title"], "v2.1 ModelOp Center Documentation Hub");
    let versions: i64 = sqlx::query_scalar("SELECT count(*) FROM page_versions").fetch_one(&app.state.db).await.unwrap();
    assert!(versions >= 45 + 286, "history imported: {versions}");
    // an image in a page points at a downloadable attachment
    let src: String = sqlx::query_scalar(
        "SELECT substring(content::text from '/api/attachments/[0-9a-f-]+/content') FROM pages WHERE content::text LIKE '%/api/attachments/%' LIMIT 1",
    )
    .fetch_one(&app.state.db)
    .await
    .unwrap();
    let r = c.get(format!("{}{}", app.base, src)).send().await.unwrap();
    assert_eq!(r.status(), 200);
    assert!(r.bytes().await.unwrap().len() > 100);
    // old Confluence ids resolve (redirects for bookmarks)
    let source_id: String =
        sqlx::query_scalar("SELECT source_id FROM import_mappings WHERE source_type = 'page' LIMIT 1").fetch_one(&app.state.db).await.unwrap();
    let (_, res) = app.get(&c, &format!("/resolve?source=confluence:{source_id}")).await;
    assert!(res["pageId"].is_string());
    // old Confluence URLs redirect to the imported pages
    let no_redirect = {
        let mut h = reqwest::header::HeaderMap::new();
        h.insert("x-wikicraft", "1".parse().unwrap());
        reqwest::Client::builder().redirect(reqwest::redirect::Policy::none()).default_headers(h).build().unwrap()
    };
    let target: String = sqlx::query_scalar("SELECT target_id::text FROM import_mappings WHERE source_type = 'page' AND source_id = $1")
        .bind(&source_id)
        .fetch_one(&app.state.db)
        .await
        .unwrap();
    for path in [format!("/pages/viewpage.action?pageId={source_id}"), format!("/wiki/spaces/DV21/pages/{source_id}/Some+Title")] {
        let r = no_redirect.get(format!("{}{}", app.base, path)).send().await.unwrap();
        assert!(r.status().is_redirection(), "{path}");
        assert_eq!(r.headers()["location"], format!("/pages/{target}").as_str(), "{path}");
    }
    let tiny = {
        use base64::Engine;
        let bytes = source_id.parse::<u64>().unwrap().to_le_bytes();
        let enc = base64::engine::general_purpose::STANDARD_NO_PAD.encode(bytes).replace('/', "-").replace('+', "_");
        enc.trim_end_matches('A').to_string()
    };
    let r = no_redirect.get(format!("{}/x/{tiny}", app.base)).send().await.unwrap();
    assert_eq!(r.headers()["location"], format!("/pages/{target}").as_str());
    let r = no_redirect.get(format!("{}/display/DV21/v2.1+ModelOp+Center+Documentation+Hub", app.base)).send().await.unwrap();
    assert_eq!(r.headers()["location"], format!("/pages/{home}").as_str());

    // search finds imported content
    let hits = app
        .eventually(|| async {
            let (_, h) = app.get(&c, "/search?q=champion%20challenger").await;
            (!h.as_array().unwrap().is_empty()).then_some(h)
        })
        .await;
    assert!(hits[0]["title"].as_str().unwrap().to_lowercase().contains("champion"));

    // importing again yields a second space with a suffixed key, links stay inside each import
    let result2 = import(&app, &c, std::fs::read(&zip).unwrap(), json!({"skipHistory": true, "skipAttachments": true})).await;
    assert_eq!(result2["spaces"][0]["key"], "DV212");
}

#[tokio::test(flavor = "multi_thread")]
async fn imports_cloud_csv_export() {
    let dir = fixtures().join("lf-toip-cloud-csv");
    if !dir.exists() {
        eprintln!("skipping: fixture missing");
        return;
    }
    // zip the CSV export directory
    let mut buf = std::io::Cursor::new(Vec::new());
    {
        let mut z = zip::ZipWriter::new(&mut buf);
        for f in ["exportDescriptor.properties", "spaces.csv.gz", "content.csv.gz", "bodycontent.csv.gz", "export-users.csv"] {
            z.start_file(f, zip::write::SimpleFileOptions::default()).unwrap();
            z.write_all(&std::fs::read(dir.join(f)).unwrap()).unwrap();
        }
        z.finish().unwrap();
    }
    let app = common::spawn().await;
    let c = app.setup().await;
    let result = import(&app, &c, buf.into_inner(), json!({"spaceKey": "TOIP"})).await;
    eprintln!("{}", serde_json::to_string_pretty(&result["created"]).unwrap());
    eprintln!("{}", serde_json::to_string_pretty(&result["links"]).unwrap());
    assert_eq!(result["spaces"][0]["key"], "TOIP");
    assert!(result["created"]["pages"].as_i64().unwrap() >= 2_100);
    assert!(result["links"]["linksToPages"].as_i64().unwrap() > 800);
    assert!(result["links"]["mentions"].as_i64().unwrap() > 29_000);
    assert!(result["links"]["inlineAnchors"].as_i64().unwrap() >= 20, "inline comments anchored by highlighted text");
    assert_no_placeholders(&app).await;
    // Confluence authors became placeholder users that can be claimed later
    let placeholders: i64 = sqlx::query_scalar("SELECT count(*) FROM users WHERE status = 'placeholder'").fetch_one(&app.state.db).await.unwrap();
    assert!(placeholders > 600);
    let (_, tree) = app.get(&c, "/spaces/TOIP/tree").await;
    assert!(tree["nodes"].as_array().unwrap().len() >= 2_100);
}
