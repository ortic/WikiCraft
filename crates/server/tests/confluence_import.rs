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
    // 202 on live pages + 15 that went to the trash with their 3 deleted pages
    assert_eq!(result["created"]["attachments"], 217);
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

    // the export's 3 deleted pages are in the trash with their attachments, and can be restored
    assert_eq!(result["created"]["trashedPages"], 3);
    let (_, trash) = app.get(&c, "/spaces/DV21/trash").await;
    let trash = trash.as_array().unwrap();
    assert_eq!(trash.len(), 3, "{trash:?}");
    let in_trash_with_files: i64 =
        sqlx::query_scalar("SELECT count(DISTINCT p.id) FROM pages p JOIN attachments a ON a.page_id = p.id WHERE p.deleted_at IS NOT NULL")
            .fetch_one(&app.state.db)
            .await
            .unwrap();
    assert!(in_trash_with_files > 0, "trashed pages keep their attachments");
    let restored = trash[0]["id"].as_str().unwrap();
    let (st, _) = app.post(&c, &format!("/pages/{restored}/restore"), json!({})).await;
    assert_eq!(st, 204);
    let (st, _) = app.get(&c, &format!("/pages/{restored}")).await;
    assert_eq!(st, 200);
    // Cloud's shared drafts of published pages are not old versions
    assert_eq!(result["report"]["counts"]["skipped_drafts_of_published_pages"], 48);
    // 149 watch rows = 124 distinct (person, page) pairs (34 rows point at an old version of the page)
    assert_eq!(result["created"]["watches"], 124);
    // the export's only label is someone's my:favourite on the space description
    assert_eq!(result["created"]["spaceFavourites"], 1);

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
    let source_id: String = sqlx::query_scalar(
        "SELECT m.source_id FROM import_mappings m JOIN pages p ON p.id = m.target_id WHERE m.source_type = 'page' AND p.deleted_at IS NULL ORDER BY m.source_id LIMIT 1",
    )
    .fetch_one(&app.state.db)
    .await
    .unwrap();
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

    // an update import of the unchanged export continues the latest import and changes no page
    let result3 = import(&app, &c, std::fs::read(&zip).unwrap(), json!({"update": true, "skipAttachments": true})).await;
    eprintln!("{}", serde_json::to_string_pretty(&result3["created"]).unwrap());
    assert_eq!(result3["spaces"][0]["key"], "DV212");
    assert!(result3["created"]["pages"].is_null() && result3["created"]["pagesUpdated"].is_null(), "{}", result3["created"]);
    assert!(result3["created"]["conflicts"].is_null() && result3["created"]["pagesMoved"].is_null(), "{}", result3["created"]);
    assert_eq!(result3["created"]["pagesUnchanged"], 48, "45 pages and 3 in the trash");
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

/// A minimal classic XML export (`entities.xml`) built from `(class, id, properties)`; a
/// property value starting with `@Class:` is a reference to another object.
fn entities_zip(objects: &[(&str, &str, &[(&str, &str)])]) -> Vec<u8> {
    entities_zip_with(objects, &[])
}

/// [`entities_zip`] plus files (attachment binaries: `attachments/<page>/<attachment>/<version>`).
fn entities_zip_with(objects: &[(&str, &str, &[(&str, &str)])], files: &[(&str, &[u8])]) -> Vec<u8> {
    let mut xml = String::from("<?xml version=\"1.0\" encoding=\"UTF-8\"?>\n<hibernate-generic>\n");
    for (class, id, props) in objects {
        let id_name = if *class == "ConfluenceUserImpl" { "key" } else { "id" };
        xml.push_str(&format!("<object class=\"{class}\" package=\"com.atlassian.confluence\"><id name=\"{id_name}\">{id}</id>"));
        for (name, value) in *props {
            if let Some(r) = value.strip_prefix('@') {
                let (class, target) = r.split_once(':').unwrap();
                let id_name = if class == "ConfluenceUserImpl" { "key" } else { "id" };
                xml.push_str(&format!(
                    "<property name=\"{name}\" class=\"{class}\" package=\"com.atlassian.confluence\"><id name=\"{id_name}\">{target}</id></property>"
                ));
            } else {
                xml.push_str(&format!("<property name=\"{name}\"><![CDATA[{value}]]></property>"));
            }
        }
        xml.push_str("</object>\n");
    }
    xml.push_str("</hibernate-generic>\n");
    let mut buf = std::io::Cursor::new(Vec::new());
    {
        let mut z = zip::ZipWriter::new(&mut buf);
        z.start_file("entities.xml", zip::write::SimpleFileOptions::default()).unwrap();
        z.write_all(xml.as_bytes()).unwrap();
        for (name, bytes) in files {
            z.start_file(*name, zip::write::SimpleFileOptions::default()).unwrap();
            z.write_all(bytes).unwrap();
        }
        z.finish().unwrap();
    }
    buf.into_inner()
}

#[tokio::test(flavor = "multi_thread")]
async fn drafts_and_trash_from_confluence() {
    let app = common::spawn().await;
    let c = app.setup().await;
    let (bob, _) = app.user(&c, "bob@acme.test", "member").await;
    let page = |title: &'static str, status: &'static str, creator: &'static str| -> Vec<(&'static str, &'static str)> {
        vec![("title", title), ("contentStatus", status), ("space", "@Space:1"), ("creator", creator), ("version", "1")]
    };
    let with = |mut v: Vec<(&'static str, &'static str)>, extra: &[(&'static str, &'static str)]| {
        v.extend_from_slice(extra);
        v
    };
    let home = page("Home", "current", "@ConfluenceUserImpl:u-owner");
    let idea = with(page("Owner's idea", "draft", "@ConfluenceUserImpl:u-owner"), &[("parent", "@Page:10")]);
    let shared = with(page("Home", "draft", "@ConfluenceUserImpl:u-owner"), &[("originalVersion", "@Page:10")]);
    let unowned = with(page("Nobody's draft", "draft", ""), &[("parent", "@Page:10")]);
    let old = with(page("Old stuff", "deleted", "@ConfluenceUserImpl:u-bob"), &[("parent", "@Page:10")]);
    let survivor = with(page("Survivor", "current", "@ConfluenceUserImpl:u-bob"), &[("parent", "@Page:13")]);
    let comment: Vec<(&str, &str)> = vec![("containerContent", "@Page:13"), ("contentStatus", "deleted"), ("creator", "@ConfluenceUserImpl:u-bob")];
    let body =
        |content: &'static str, html: &'static str| -> Vec<(&'static str, &'static str)> { vec![("content", content), ("body", html), ("bodyType", "2")] };
    let zip = entities_zip(&[
        ("Space", "1", &[("key", "SYN"), ("name", "Synthetic"), ("homePage", "@Page:10")]),
        ("ConfluenceUserImpl", "u-owner", &[("name", "owner"), ("email", "owner@acme.test")]),
        ("ConfluenceUserImpl", "u-bob", &[("name", "bob"), ("email", "bob@acme.test")]),
        ("Page", "10", &home),
        ("Page", "11", &idea),
        ("Page", "12", &shared),
        ("Page", "15", &unowned),
        ("Page", "13", &old),
        ("Page", "14", &survivor),
        ("Comment", "20", &comment),
        ("BodyContent", "100", &body("@Page:10", "<p>Welcome</p>")),
        ("BodyContent", "101", &body("@Page:11", "<p>secret plan</p>")),
        ("BodyContent", "102", &body("@Page:12", "<p>Welcome, edited but not published</p>")),
        ("BodyContent", "105", &body("@Page:15", "<p>orphaned draft</p>")),
        ("BodyContent", "103", &body("@Page:13", "<p>obsolete</p>")),
        ("BodyContent", "104", &body("@Page:14", "<p>still here</p>")),
        ("BodyContent", "120", &body("@Comment:20", "<p>why was this deleted?</p>")),
    ]);
    let result = import(&app, &c, zip, json!({})).await;
    eprintln!("{}", serde_json::to_string_pretty(&result["created"]).unwrap());
    assert_eq!(result["created"]["pages"], 2, "Home and Survivor");
    assert_eq!(result["created"]["drafts"], 1);
    assert_eq!(result["created"]["trashedPages"], 1);
    assert_eq!(result["report"]["counts"]["skipped_drafts_of_published_pages"], 1);
    assert_eq!(result["report"]["counts"]["skipped_drafts_without_author"], 1);

    let id_of = |title: &'static str| {
        let db = app.state.db.clone();
        async move { sqlx::query_scalar::<_, uuid::Uuid>("SELECT id FROM pages WHERE title = $1").bind(title).fetch_one(&db).await.unwrap().to_string() }
    };
    let (home_id, idea_id, old_id, survivor_id) = (id_of("Home").await, id_of("Owner's idea").await, id_of("Old stuff").await, id_of("Survivor").await);

    // the never-published page is the owner's private draft, under Home
    let (_, drafts) = app.get(&c, "/me/drafts").await;
    assert_eq!(drafts[0]["id"], idea_id.as_str());
    let (_, p) = app.get(&c, &format!("/pages/{idea_id}")).await;
    assert_eq!(p["draft"], true);
    assert_eq!(p["parentId"], home_id.as_str());
    let (st, _) = app.get(&bob, &format!("/pages/{idea_id}")).await;
    assert_eq!(st, 404, "nobody else sees a draft");
    let (_, tree) = app.get(&bob, "/spaces/SYN/tree").await;
    let titles: Vec<&str> = tree["nodes"].as_array().unwrap().iter().map(|n| n["title"].as_str().unwrap()).collect();
    assert!(!titles.contains(&"Owner's idea"), "{titles:?}");

    // the shared draft of Home is not an old version of it
    let (_, versions) = app.get(&c, &format!("/pages/{home_id}/versions")).await;
    assert_eq!(versions.as_array().unwrap().len(), 1, "{versions}");

    // the trashed page is in the trash with its comment; its live child moved up to Home
    let (_, s) = app.get(&c, &format!("/pages/{survivor_id}")).await;
    assert_eq!(s["parentId"], home_id.as_str());
    let (_, trash) = app.get(&c, "/spaces/SYN/trash").await;
    assert_eq!(trash.as_array().unwrap().len(), 1);
    assert_eq!(trash[0]["id"], old_id.as_str());
    let (st, _) = app.post(&c, &format!("/pages/{old_id}/restore"), json!({})).await;
    assert_eq!(st, 204);
    let (_, comments) = app.get(&c, &format!("/pages/{old_id}/comments")).await;
    assert_eq!(comments.as_array().unwrap().len(), 1, "{comments}");

    // both can be left out
    let zip2 = entities_zip(&[
        ("Space", "1", &[("key", "SYN"), ("name", "Synthetic")]),
        ("ConfluenceUserImpl", "u-owner", &[("name", "owner"), ("email", "owner@acme.test")]),
        ("Page", "10", &home),
        ("Page", "11", &idea),
        ("Page", "13", &old),
        ("BodyContent", "101", &body("@Page:11", "<p>secret plan</p>")),
    ]);
    let result = import(&app, &c, zip2, json!({"skipDrafts": true, "skipTrash": true})).await;
    assert_eq!(result["created"]["pages"], 1);
    assert!(result["created"]["drafts"].is_null() && result["created"]["trashedPages"].is_null(), "{}", result["created"]);
}

#[tokio::test(flavor = "multi_thread")]
async fn watches_and_favourites_from_confluence() {
    let app = common::spawn().await;
    let c = app.setup().await;
    let (bob, _) = app.user(&c, "bob@acme.test", "member").await;
    let page = |title: &'static str| -> Vec<(&'static str, &'static str)> {
        vec![("title", title), ("contentStatus", "current"), ("space", "@Space:1"), ("creator", "@ConfluenceUserImpl:u-owner"), ("version", "1")]
    };
    let zip = entities_zip(&[
        ("Space", "1", &[("key", "WAT"), ("name", "Watched"), ("description", "@SpaceDescription:5")]),
        ("SpaceDescription", "5", &[("space", "@Space:1"), ("contentStatus", "current")]),
        ("ConfluenceUserImpl", "u-owner", &[("name", "owner"), ("email", "owner@acme.test")]),
        ("ConfluenceUserImpl", "u-bob", &[("name", "bob"), ("email", "bob@acme.test")]),
        ("Page", "10", &page("Runbook")),
        ("Page", "11", &page("Changelog")),
        // bob watches the runbook and the space; a daily digest subscription is not a watch
        (
            "Notification",
            "30",
            &[("content", "@Page:10"), ("receiver", "@ConfluenceUserImpl:u-bob"), ("type", "page"), ("digest", "false"), ("network", "false")],
        ),
        ("Notification", "31", &[("space", "@Space:1"), ("receiver", "@ConfluenceUserImpl:u-bob"), ("digest", "false"), ("network", "false")]),
        ("Notification", "32", &[("receiver", "@ConfluenceUserImpl:u-owner"), ("digest", "true"), ("network", "false")]),
        // favourites: the newer relation and the older personal label (on the space: its description)
        (
            "User2ContentRelationEntity",
            "40",
            &[("sourceContent", "u-bob"), ("targetContent", "@Page:11"), ("relationName", "favourite"), ("targetType", "PAGE")],
        ),
        (
            "User2ContentRelationEntity",
            "41",
            &[("sourceContent", "u-owner"), ("targetContent", "@Page:10"), ("relationName", "touched"), ("targetType", "PAGE")],
        ),
        ("Label", "50", &[("name", "favourite"), ("namespace", "my")]),
        ("Labelling", "51", &[("label", "@Label:50"), ("content", "@Page:10"), ("owningUser", "@ConfluenceUserImpl:u-owner")]),
        ("Labelling", "52", &[("label", "@Label:50"), ("content", "@SpaceDescription:5"), ("owningUser", "@ConfluenceUserImpl:u-bob")]),
    ]);
    let result = import(&app, &c, zip, json!({})).await;
    eprintln!("{}", serde_json::to_string_pretty(&result["created"]).unwrap());
    assert_eq!(result["created"]["watches"], 1);
    assert_eq!(result["created"]["spaceWatches"], 1);
    assert_eq!(result["created"]["favourites"], 2);
    assert_eq!(result["created"]["spaceFavourites"], 1);

    // bob's favourites list has the changelog; the owner's has the runbook ("touched" isn't a favourite)
    let (_, favs) = app.get(&bob, "/favorites").await;
    let titles: Vec<&str> = favs.as_array().unwrap().iter().map(|p| p["title"].as_str().unwrap()).collect();
    assert_eq!(titles, ["Changelog"]);
    let (_, favs) = app.get(&c, "/favorites").await;
    let titles: Vec<&str> = favs.as_array().unwrap().iter().map(|p| p["title"].as_str().unwrap()).collect();
    assert_eq!(titles, ["Runbook"]);
    let watching: Vec<String> =
        sqlx::query_scalar("SELECT p.title FROM watches w JOIN pages p ON p.id = w.page_id JOIN users u ON u.id = w.user_id WHERE u.email = 'bob@acme.test'")
            .fetch_all(&app.state.db)
            .await
            .unwrap();
    assert_eq!(watching, ["Runbook"]);
    let (_, space) = app.get(&bob, "/spaces/WAT").await;
    assert_eq!(space["watching"], true, "{space}");
    let starred: i64 = sqlx::query_scalar("SELECT count(*) FROM space_favorites f JOIN users u ON u.id = f.user_id WHERE u.email = 'bob@acme.test'")
        .fetch_one(&app.state.db)
        .await
        .unwrap();
    assert_eq!(starred, 1);
}

#[tokio::test(flavor = "multi_thread")]
async fn update_import_brings_changes_across() {
    let app = common::spawn().await;
    let c = app.setup().await;
    type Props = Vec<(&'static str, &'static str)>;
    let page = |title: &'static str, parent: Option<&'static str>, version: &'static str| -> Props {
        let mut v: Props =
            vec![("title", title), ("contentStatus", "current"), ("space", "@Space:1"), ("creator", "@ConfluenceUserImpl:u-owner"), ("version", version)];
        if let Some(p) = parent {
            v.push(("parent", p));
        }
        v
    };
    let with = |mut v: Props, extra: &[(&'static str, &'static str)]| {
        v.extend_from_slice(extra);
        v
    };
    let body = |content: &'static str, html: &'static str| -> Props { vec![("content", content), ("body", html), ("bodyType", "2")] };
    let space: Props = vec![("key", "SYNC"), ("name", "Synced"), ("homePage", "@Page:10")];
    let user: Props = vec![("name", "owner"), ("email", "owner@acme.test")];
    let comment = |page: &'static str| -> Props { vec![("containerContent", page), ("contentStatus", "current"), ("creator", "@ConfluenceUserImpl:u-owner")] };
    let attachment = |version: &'static str| -> Props {
        vec![("title", "notes.txt"), ("containerContent", "@Page:11"), ("contentStatus", "current"), ("version", version), ("space", "@Space:1")]
    };

    // the first import
    let first = entities_zip_with(
        &[
            ("Space", "1", &space),
            ("ConfluenceUserImpl", "u-owner", &user),
            ("Page", "10", &page("Home", None, "1")),
            ("Page", "11", &page("Guide", Some("@Page:10"), "1")),
            ("Page", "12", &page("Notes", Some("@Page:10"), "1")),
            ("Page", "13", &page("Mover", Some("@Page:11"), "1")),
            ("Page", "15", &page("Old", Some("@Page:10"), "1")),
            ("Comment", "20", &comment("@Page:11")),
            ("Attachment", "30", &attachment("1")),
            ("BodyContent", "100", &body("@Page:10", "<p>Welcome</p>")),
            ("BodyContent", "101", &body("@Page:11", "<p>Guide one</p>")),
            ("BodyContent", "102", &body("@Page:12", "<p>Notes one</p>")),
            ("BodyContent", "103", &body("@Page:13", "<p>Mover</p>")),
            ("BodyContent", "105", &body("@Page:15", "<p>Old</p>")),
            ("BodyContent", "120", &body("@Comment:20", "<p>first comment</p>")),
        ],
        &[("attachments/11/30/1", b"version one")],
    );
    let r1 = import(&app, &c, first, json!({})).await;
    assert_eq!(r1["created"]["pages"], 5);
    let id_of = |title: &'static str| {
        let db = app.state.db.clone();
        async move { sqlx::query_scalar::<_, uuid::Uuid>("SELECT id FROM pages WHERE title = $1").bind(title).fetch_one(&db).await.unwrap().to_string() }
    };
    let (home, guide, notes, mover, old) = (id_of("Home").await, id_of("Guide").await, id_of("Notes").await, id_of("Mover").await, id_of("Old").await);
    // meanwhile, someone edits Notes here
    app.put(&c, &format!("/pages/{notes}/content"), json!({"markdown": "Notes edited here"})).await;

    // Confluence moved on: Guide and Notes edited, Mover moved, Fresh added, Old deleted, a new
    // comment, a new version of the attachment
    let second = entities_zip_with(
        &[
            ("Space", "1", &space),
            ("ConfluenceUserImpl", "u-owner", &user),
            ("Page", "10", &page("Home", None, "1")),
            ("Page", "11", &page("Guide", Some("@Page:10"), "2")),
            ("Page", "111", &with(page("Guide", Some("@Page:10"), "1"), &[("originalVersion", "@Page:11")])),
            ("Page", "12", &page("Notes", Some("@Page:10"), "2")),
            ("Page", "112", &with(page("Notes", Some("@Page:10"), "1"), &[("originalVersion", "@Page:12")])),
            ("Page", "13", &page("Mover", Some("@Page:10"), "1")),
            ("Page", "14", &page("Fresh", Some("@Page:11"), "1")),
            ("Page", "15", &with(page("Old", Some("@Page:10"), "1"), &[("contentStatus", "deleted")])),
            ("Comment", "20", &comment("@Page:11")),
            ("Comment", "21", &comment("@Page:11")),
            ("Attachment", "30", &attachment("2")),
            ("Attachment", "31", &with(attachment("1"), &[("originalVersion", "@Attachment:30")])),
            ("BodyContent", "100", &body("@Page:10", "<p>Welcome</p>")),
            ("BodyContent", "101", &body("@Page:11", "<p>Guide two, see <ac:link><ri:page ri:content-title=\"Fresh\" /></ac:link></p>")),
            ("BodyContent", "111", &body("@Page:111", "<p>Guide one</p>")),
            ("BodyContent", "102", &body("@Page:12", "<p>Notes two</p>")),
            ("BodyContent", "112", &body("@Page:112", "<p>Notes one</p>")),
            ("BodyContent", "103", &body("@Page:13", "<p>Mover</p>")),
            ("BodyContent", "104", &body("@Page:14", "<p>Fresh page</p>")),
            ("BodyContent", "105", &body("@Page:15", "<p>Old</p>")),
            ("BodyContent", "120", &body("@Comment:20", "<p>first comment</p>")),
            ("BodyContent", "121", &body("@Comment:21", "<p>second comment</p>")),
        ],
        &[("attachments/11/30/1", b"version one"), ("attachments/11/30/2", b"version two")],
    );
    let r2 = import(&app, &c, second.clone(), json!({"update": true})).await;
    eprintln!("{}", serde_json::to_string_pretty(&r2["created"]).unwrap());
    let created = &r2["created"];
    assert_eq!(r2["spaces"][0]["key"], "SYNC", "the same space, not SYNC2");
    assert_eq!(r2["spaces"][0]["updated"], true);
    assert_eq!(created["pagesUpdated"], 1, "Guide");
    assert_eq!(created["conflicts"], 1, "Notes");
    assert_eq!(created["pages"], 1, "Fresh");
    assert_eq!(created["pagesTrashedInConfluence"], 1);
    assert_eq!(created["pagesMoved"], 1);
    assert_eq!(created["commentsAdded"], 1);
    assert_eq!(created["attachmentVersionsAdded"], 1);
    assert_eq!(r2["conflicts"][0]["title"], "Notes");

    let fresh = id_of("Fresh").await;
    let (_, g) = app.get(&c, &format!("/pages/{guide}")).await;
    assert!(g["content"].to_string().contains("Guide two"));
    assert!(g["content"].to_string().contains(&format!("/pages/{fresh}")), "links to the new page resolve");
    let (_, versions) = app.get(&c, &format!("/pages/{guide}/versions")).await;
    assert_eq!(versions.as_array().unwrap().len(), 2, "{versions}");
    let (_, n) = app.get(&c, &format!("/pages/{notes}")).await;
    assert!(n["content"].to_string().contains("Notes edited here"), "a conflict keeps the local edit");
    let (_, m) = app.get(&c, &format!("/pages/{mover}")).await;
    assert_eq!(m["parentId"], home.as_str());
    let (_, f) = app.get(&c, &format!("/pages/{fresh}")).await;
    assert_eq!(f["parentId"], guide.as_str());
    let (_, trash) = app.get(&c, "/spaces/SYNC/trash").await;
    assert_eq!(trash[0]["id"], old.as_str());
    let (_, comments) = app.get(&c, &format!("/pages/{guide}/comments")).await;
    assert_eq!(comments.as_array().unwrap().len(), 2);
    let (_, atts) = app.get(&c, &format!("/pages/{guide}/attachments")).await;
    let att = atts.as_array().unwrap().iter().find(|a| a["filename"] == "notes.txt").unwrap();
    assert_eq!(att["version"], 2);
    let (_, att_versions) = app.get(&c, &format!("/attachments/{}/versions", att["id"].as_str().unwrap())).await;
    assert_eq!(att_versions.as_array().unwrap().len(), 2);

    // the same export again: nothing left to do
    let r3 = import(&app, &c, second.clone(), json!({"update": true})).await;
    assert!(r3["created"]["pagesUpdated"].is_null() && r3["created"]["pages"].is_null(), "{}", r3["created"]);
    assert_eq!(r3["created"]["conflicts"], 1, "Notes still differs on both sides");

    // asked to, Confluence's content wins; the local edit stays in the history
    let r4 = import(&app, &c, second, json!({"update": true, "overwriteLocalChanges": true})).await;
    assert_eq!(r4["created"]["pagesUpdated"], 1);
    let (_, n) = app.get(&c, &format!("/pages/{notes}")).await;
    assert!(n["content"].to_string().contains("Notes two"));
    let (_, versions) = app.get(&c, &format!("/pages/{notes}/versions")).await;
    let messages: Vec<&str> = versions.as_array().unwrap().iter().filter_map(|v| v["message"].as_str()).collect();
    assert!(messages.contains(&"Updated from Confluence"), "{versions}");
    assert!(versions.as_array().unwrap().len() >= 3, "imported, local edit, from Confluence: {versions}");
}
