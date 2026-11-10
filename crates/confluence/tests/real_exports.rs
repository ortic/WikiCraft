//! Runs the importer over real Confluence exports (downloaded by
//! `research/confluence-import/fetch_fixtures.sh`; skipped when absent).
//!
//! For every current page, blog post, draft, page in the trash, comment and historical version: convert storage → PM,
//! validate the tree, and round-trip it through Yjs exactly like the collaborative editor will.

use std::path::{Path, PathBuf};

use wikicraft_confluence::model::ContentStatus;
use wikicraft_confluence::{PlanOptions, Planner, Stats};
use wikicraft_doc_model::ydoc;

mod support;
use support::validate;

fn fixtures() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../../research/confluence-import/fixtures")
}

struct Outcome {
    converted: usize,
    stats: Stats,
}

fn run(path: &Path, label: &str) -> Option<Outcome> {
    if !path.exists() {
        eprintln!("skipping {label}: {} not found (run fetch_fixtures.sh)", path.display());
        return None;
    }
    let t0 = std::time::Instant::now();
    let export = wikicraft_confluence::open(path).expect("export opens");
    let read_time = t0.elapsed();
    let planner = Planner::new(&export, PlanOptions::default());
    let plan = planner.plan();
    let mut stats = Stats::default();
    // pages in the trash link to whatever existed back then: kept out of the resolution rates
    let mut trash_stats = Stats::default();
    let mut converted = 0;
    let mut errors = Vec::new();
    let mut yjs_failures = 0;
    for space in &plan.spaces {
        for p in space.pages.iter().chain(&space.blog_posts) {
            let ids = std::iter::once(&p.source_id).chain(p.history.iter().map(|h| &h.source_id)).chain(p.comments.iter().map(|c| &c.source_id));
            let st = if p.status == ContentStatus::Deleted { &mut trash_stats } else { &mut stats };
            for id in ids {
                let doc = planner.convert(id, st);
                converted += 1;
                validate(&doc, &format!("{}#{id}", p.title), &mut errors);
                // Yjs round-trip (what the collaborative editor will load)
                let back = ydoc::update_to_pm(&ydoc::pm_to_update(&doc)).expect("yjs decode");
                if back != doc {
                    yjs_failures += 1;
                }
            }
        }
    }
    let r = &plan.report;
    eprintln!(
        "{label}: read {:?}, {} spaces, counts={:?}\n  converted {converted} bodies, lenient parses {}, structural errors {}, yjs mismatches {yjs_failures}\n  page links {}/{} resolved ({} missing), attachments {}/{} resolved, users {}/{} resolved\n  macros: {:?}",
        read_time,
        plan.spaces.len(),
        r.counts,
        stats.lenient_parses,
        errors.len(),
        stats.page_links_resolved,
        stats.page_links,
        stats.page_links_missing,
        stats.attachment_refs_resolved,
        stats.attachment_refs,
        stats.user_refs_resolved,
        stats.user_refs,
        stats.macros.iter().map(|(k, v)| format!("{k}={}", v.0)).collect::<Vec<_>>(),
    );
    for e in errors.iter().take(15) {
        eprintln!("  ERROR {e}");
    }
    assert!(errors.is_empty(), "{label}: {} structural errors", errors.len());
    assert_eq!(yjs_failures, 0, "{label}: documents changed during the Yjs round-trip");
    Some(Outcome { converted, stats })
}

#[test]
fn lf_toip_cloud_csv_export() {
    let Some(o) = run(&fixtures().join("lf-toip-cloud-csv"), "LF ToIP (Cloud CSV, 2026)") else { return };
    // numbers from the Python dry-run analysis (research/confluence-import/report-lf-toip-cloud-csv-2026.txt)
    assert!(o.converted > 2_000);
    assert_eq!(o.stats.lenient_parses, 0, "all bodies must parse strictly");
    assert!(o.stats.tasks >= 5_000);
    // resolution must be at least as good as the Python analyzer's (718/915 links, 1912/1932 attachments)
    assert!(o.stats.page_links_resolved as f64 / o.stats.page_links as f64 >= 0.78);
    assert!(o.stats.attachment_refs_resolved as f64 / o.stats.attachment_refs as f64 >= 0.98);
    assert!(o.stats.user_refs_resolved as f64 / o.stats.user_refs as f64 >= 0.99);
}

#[test]
fn modelop_cloud_xml_export() {
    let Some(o) = run(&fixtures().join("modelop-cloud-xml/export.zip"), "ModelOp (Cloud XML, 2020)") else { return };
    assert_eq!(o.stats.lenient_parses, 0, "all bodies must parse strictly (CDATA un-escaping)");
    // current versions: 12/12 like the Python analyzer; old versions also link via former titles
    assert!(o.stats.page_links_resolved as f64 / o.stats.page_links as f64 >= 0.95, "{}/{}", o.stats.page_links_resolved, o.stats.page_links);
    assert_eq!(o.stats.attachment_refs_resolved, o.stats.attachment_refs);
}
