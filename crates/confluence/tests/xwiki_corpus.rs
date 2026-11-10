//! Edge-case regression tests on the XWiki Confluence importer's test corpus: ~90 small exports,
//! each built around one problem (broken links, buggy history, missing parents, orphans, inline
//! comments, content statuses, Cloud CSV backups, ...). Downloaded by
//! `research/confluence-import/fetch_xwiki_corpus.sh` (LGPL-2.1, not vendored); skipped when absent.

use std::path::{Path, PathBuf};

use wikicraft_confluence::model::ContentStatus;
use wikicraft_confluence::plan::{ImportPlan, PagePlan};
use wikicraft_confluence::{Export, PlanOptions, Planner, Stats};
use wikicraft_doc_model::ydoc;

mod support;

fn cases() -> Option<PathBuf> {
    let p = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../research/confluence-import/xwiki-corpus/cases");
    if p.exists() {
        Some(p)
    } else {
        assert!(std::env::var_os("WIKICRAFT_REQUIRE_XWIKI_CORPUS").is_none(), "corpus missing: run research/confluence-import/fetch_xwiki_corpus.sh");
        eprintln!("skipping: run research/confluence-import/fetch_xwiki_corpus.sh");
        None
    }
}

/// Every export in the corpus: directories holding an `entities.xml`, and zip files.
fn exports(dir: &Path) -> Vec<PathBuf> {
    let mut out: Vec<PathBuf> = std::fs::read_dir(dir)
        .unwrap()
        .flatten()
        .map(|e| e.path())
        .filter(|p| (p.is_dir() && p.join("entities.xml").exists()) || p.extension().is_some_and(|e| e == "zip"))
        .collect();
    out.sort();
    out
}

fn load(name: &str) -> Option<Export> {
    let path = cases()?.join(name);
    Some(wikicraft_confluence::open(&path).unwrap_or_else(|e| panic!("{name}: {e}")))
}

/// What converting every body of a plan (current, history, comments) found.
struct Checked {
    plan: ImportPlan,
    stats: Stats,
    converted: usize,
    errors: Vec<String>,
    yjs_mismatches: usize,
}

fn check(export: &Export, opts: PlanOptions) -> Checked {
    let planner = Planner::new(export, opts);
    let plan = planner.plan();
    let mut stats = Stats::default();
    let (mut converted, mut errors, mut yjs_mismatches) = (0, Vec::new(), 0);
    for space in &plan.spaces {
        for p in space.pages.iter().chain(&space.blog_posts) {
            let ids = std::iter::once(&p.source_id).chain(p.history.iter().map(|h| &h.source_id)).chain(p.comments.iter().map(|c| &c.source_id));
            for id in ids {
                let doc = planner.convert(id, &mut stats);
                converted += 1;
                support::validate(&doc, &format!("{}#{id}", p.title), &mut errors);
                let back = ydoc::update_to_pm(&ydoc::pm_to_update(&doc)).expect("yjs decode");
                if back != doc {
                    yjs_mismatches += 1;
                }
            }
        }
    }
    Checked { plan, stats, converted, errors, yjs_mismatches }
}

fn pages(plan: &ImportPlan) -> impl Iterator<Item = &PagePlan> {
    plan.spaces.iter().flat_map(|s| s.pages.iter().chain(&s.blog_posts))
}

fn page<'p>(plan: &'p ImportPlan, title: &str) -> &'p PagePlan {
    pages(plan).find(|p| p.title == title).unwrap_or_else(|| panic!("no page '{title}' in {:?}", pages(plan).map(|p| &p.title).collect::<Vec<_>>()))
}

const KNOWN_MALFORMED: &[&str] = &["confluence80tags"];

#[test]
fn every_export_in_the_corpus_imports_cleanly() {
    let Some(dir) = cases() else { return };
    let mut failures = Vec::new();
    let mut total = (0, 0);
    for path in exports(&dir) {
        let name = path.file_name().unwrap().to_string_lossy().to_string();
        let outcome = std::panic::catch_unwind(|| {
            let export = wikicraft_confluence::open(&path).map_err(|e| format!("open: {e}"))?;
            let c = check(&export, PlanOptions::default());
            if !c.errors.is_empty() {
                return Err(format!("{} structural errors, first: {}", c.errors.len(), c.errors[0]));
            }
            if c.yjs_mismatches > 0 {
                return Err(format!("{} documents changed in the Yjs round trip", c.yjs_mismatches));
            }
            // known to contain a body with stray closing tags (`</td></tr></tbody></table>`
            // without a table): the lenient fallback keeps its text
            if c.stats.lenient_parses > 0 && !KNOWN_MALFORMED.contains(&name.as_str()) {
                return Err(format!("{} bodies needed lenient parsing", c.stats.lenient_parses));
            }
            Ok((pages(&c.plan).count(), c.converted))
        });
        match outcome {
            Ok(Ok((p, b))) => {
                total.0 += p;
                total.1 += b;
            }
            Ok(Err(e)) => failures.push(format!("{name}: {e}")),
            Err(panic) => {
                failures.push(format!("{name}: panicked: {:?}", panic.downcast_ref::<String>().map(String::as_str).or(panic.downcast_ref::<&str>().copied())))
            }
        }
    }
    eprintln!("xwiki corpus: {} pages, {} bodies converted", total.0, total.1);
    assert!(failures.is_empty(), "{} exports failed:\n{}", failures.len(), failures.join("\n"));
    assert!(total.0 > 150, "the corpus should be mostly importable");
}

/// Prints what the importer would make of one case:
/// `CASE=brokenlink cargo test -p wikicraft-confluence --test xwiki_corpus dump -- --ignored --nocapture`
#[test]
#[ignore]
fn dump() {
    let Ok(name) = std::env::var("CASE") else { return };
    let Some(export) = load(&name) else { return };
    let c = check(&export, PlanOptions::default());
    for s in &c.plan.spaces {
        println!("space {} '{}' home={:?} watchers={:?} favs={:?}", s.key, s.name, s.home_page_source_id, s.watchers, s.favourited_by);
        for p in s.pages.iter().chain(&s.blog_posts) {
            println!(
                "  {:?} {} '{}' parent={:?} pos={:?} v{} by={:?} history={:?} comments={} atts={:?} labels={:?} restr={:?}/{:?} watch={:?}",
                p.status,
                p.source_id,
                p.title,
                p.parent_source_id,
                p.position,
                p.version,
                p.created_by,
                p.history.iter().map(|h| h.version).collect::<Vec<_>>(),
                p.comments.len(),
                p.attachments
                    .iter()
                    .map(|a| format!("{}:{}/{}", a.filename, a.versions.iter().filter(|v| v.path.is_some()).count(), a.versions.len()))
                    .collect::<Vec<_>>(),
                p.labels,
                p.view_restrictions,
                p.edit_restrictions,
                p.watchers,
            );
        }
    }
    println!("counts {:?}", c.plan.report.counts);
    println!("warnings {:?}", c.plan.report.warnings);
    println!(
        "links {}/{} missing {:?}; attachments {}/{} missing {:?}; users {}/{}; lenient {}",
        c.stats.page_links_resolved,
        c.stats.page_links,
        c.stats.missing_link_samples,
        c.stats.attachment_refs_resolved,
        c.stats.attachment_refs,
        c.stats.missing_attachment_samples,
        c.stats.user_refs_resolved,
        c.stats.user_refs,
        c.stats.lenient_parses
    );
}

#[test]
fn buggy_history_is_ordered_and_deduplicated() {
    // the same version number twice with different dates: both kept, oldest first
    let Some(ex) = load("buggyhistory2") else { return };
    let c = check(&ex, PlanOptions::default());
    let ids: Vec<&str> = page(&c.plan, "1.2. Verification of history").history.iter().map(|h| h.source_id.as_str()).collect();
    assert_eq!(ids, ["101", "103", "102"]);
    // ... and with the same date (and body) it's one version stored twice
    let ex = load("buggyhistory3").unwrap();
    let c = check(&ex, PlanOptions::default());
    assert_eq!(page(&c.plan, "1.2. Verification of history").history.len(), 2);
    assert_eq!(c.plan.report.counts["duplicate_versions_dropped"], 1);
}

#[test]
fn content_statuses_and_orphans() {
    let Some(ex) = load("contentStatus") else { return };
    let c = check(&ex, PlanOptions::default());
    assert_eq!(page(&c.plan, "Archived Page").status, ContentStatus::Archived);
    assert_eq!(c.plan.report.counts["skipped_drafts_without_author"], 1);

    let c = check(&load("orphans").unwrap(), PlanOptions::default());
    assert_eq!(page(&c.plan, "Root Orphan").status, ContentStatus::Draft);
    assert_eq!(c.plan.report.counts["skipped_drafts_of_published_pages"], 3);
    assert_eq!(page(&c.plan, "Sub orphan").parent_source_id.as_deref(), Some(page(&c.plan, "Orphan").source_id.as_str()));

    // pages whose parent isn't in the export go to the space root, with a warning
    let c = check(&load("missingparents").unwrap(), PlanOptions::default());
    assert!(page(&c.plan, "Missing parent").parent_source_id.is_none());
    assert_eq!(c.plan.report.counts["orphan_pages"], 1);
}

#[test]
fn wiki_markup_links_without_body_type() {
    // old bodies without a bodyType are wiki markup: [space:], [space:Title], [Title^file]
    let Some(ex) = load("pageswithdots") else { return };
    let c = check(&ex, PlanOptions::default());
    assert_eq!((c.stats.page_links, c.stats.page_links_resolved), (2, 2), "space home and '1. page with dot in title'");
    assert_eq!(c.stats.attachment_refs, 3, "attachment links are attachments, not pages");
    assert_eq!(c.stats.wiki_markup_bodies, 3, "the three pages (the 4th untyped body is the space description)");
}

#[test]
fn brokenlink_is_reported() {
    let Some(ex) = load("brokenlink") else { return };
    let c = check(&ex, PlanOptions::default());
    assert_eq!(c.stats.page_links_missing, 1);
    assert_eq!(c.stats.missing_link_samples, ["MySpace/linktarget"]);
}

#[test]
fn attachment_versions_keep_their_binaries() {
    let Some(ex) = load("attachmenthistory") else { return };
    let c = check(&ex, PlanOptions::default());
    let p = page(&c.plan, "MyPage");
    for (name, versions) in [("API.pdf", 3), ("XWiki_Issues.xlsx", 6)] {
        let a = p.attachments.iter().find(|a| a.filename == name).unwrap();
        assert_eq!(a.versions.len(), versions, "{name}");
        assert!(a.versions.iter().all(|v| v.path.is_some()), "{name}: every version has its file");
    }
}

#[test]
fn inline_comments_are_anchored() {
    let Some(ex) = load("inlinecomments") else { return };
    let c = check(&ex, PlanOptions::default());
    assert_eq!(c.plan.report.counts["inline_comments"], 5);
    let c = check(&load("madinlinecomments").unwrap(), PlanOptions::default());
    assert_eq!(c.plan.report.counts["inline_comments"], 14);
}

#[test]
fn cloud_csv_backups() {
    // a complete Cloud CSV export whose tables are stored uncompressed under .csv.gz names
    let Some(ex) = load("csv-RJTest.zip") else { return };
    assert!(!ex.labellings.is_empty() && !ex.watches.is_empty(), "labels and watches come from their own tables");
    let c = check(&ex, PlanOptions::default());
    assert!(c.plan.report.counts["pages"] > 0);

    // a site backup that stores attachment files by FILESTORE_ID (attachments/<uuid>)
    let ex = load("csv-site-backup.zip").unwrap();
    let c = check(&ex, PlanOptions::default());
    let versions: Vec<_> = pages(&c.plan).flat_map(|p| &p.attachments).flat_map(|a| &a.versions).collect();
    assert!(versions.len() > 30);
    assert!(versions.iter().all(|v| v.path.is_some()), "every attachment version finds its file");
    assert!(c.plan.report.counts["watches"] > 50);
    assert_eq!(c.plan.spaces[0].favourited_by.len(), 2);
}

#[test]
fn space_permission_subjects() {
    // current exports name the subject: all-licensed-users / anonymous-users / all-product-admins
    let Some(ex) = load("permissions") else { return };
    let all_users = ex.space_permissions.iter().filter(|p| p.all_users).count();
    let anonymous = ex.space_permissions.iter().filter(|p| p.anonymous).count();
    assert!(all_users >= 1, "all-licensed-users is everybody signed in");
    assert!(anonymous >= 1, "anonymous-users is public access");
    assert!(
        ex.space_permissions.iter().filter(|p| p.anonymous).all(|p| !p.all_users && p.user_key.is_none() && p.group.is_none()),
        "a grant is to one kind of subject"
    );
}
