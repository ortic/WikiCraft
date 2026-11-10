//! Golden tests: hand-written Confluence bodies (`tests/golden/*.xml` storage format,
//! `*.wiki` wiki markup) and what they become: conversion stats, ProseMirror JSON, Markdown and
//! HTML. Any change in the output shows up as a snapshot diff to review
//! (`INSTA_UPDATE=always cargo test -p wikicraft-confluence --test golden` accepts it).
//!
//! The fixtures are our own (real exports can't be committed); they cover what the real-export
//! runs met (PLAN.md §6): macros, links, users, attachments, tables, tasks, layouts, images,
//! emoticons, placeholders, inline comments, ADF decision lists, legacy syntax, wiki markup.

use wikicraft_confluence::Stats;
use wikicraft_confluence::storage::{Resolver, convert, convert_wiki};
use wikicraft_doc_model::{Node, html, markdown, ydoc};

/// The export around the fixtures: space DOC, the page being converted is 100, "Other Page" is
/// 200, diagram.png is attachment 300 on it, user key `abc` is Ada Lovelace.
struct Export;

impl Resolver for Export {
    fn space_key(&self) -> &str {
        "DOC"
    }
    fn content_id(&self) -> &str {
        "100"
    }
    fn page(&self, space: &str, title: &str) -> Option<String> {
        (space == "DOC" && title == "Other Page").then(|| "200".into())
    }
    fn content_exists(&self, id: &str) -> bool {
        id == "200"
    }
    fn attachment(&self, container: &str, filename: &str) -> Option<String> {
        (container == "100" && filename == "diagram.png").then(|| "300".into())
    }
    fn user_name(&self, key: &str) -> Option<String> {
        (key == "abc").then(|| "Ada Lovelace".into())
    }
}

fn report(doc: &Node, stats: &Stats) -> String {
    // every converted document survives the trip through Yjs (what the editor works on) unchanged
    let back = ydoc::update_to_pm(&ydoc::pm_to_update(doc)).expect("valid Yjs");
    assert_eq!(&back, doc, "Yjs round trip changed the document");
    let mut stats = serde_json::to_value(stats).unwrap();
    // zero counters are noise in the snapshot
    if let Some(m) = stats.as_object_mut() {
        m.retain(|_, v| !(v == &serde_json::json!(0) || v == &serde_json::json!([]) || v == &serde_json::json!({})));
    }
    format!(
        "## stats\n{}\n\n## prosemirror\n{}\n\n## markdown\n{}\n\n## html\n{}\n",
        serde_json::to_string_pretty(&stats).unwrap(),
        serde_json::to_string_pretty(doc).unwrap(),
        markdown::pm_to_markdown(doc).trim_end(),
        html::render(doc, &html::HtmlOptions { heading_ids: true }).replace("><", ">\n<"),
    )
}

#[test]
fn storage_format() {
    insta::glob!("golden/*.xml", |path| {
        let body = std::fs::read_to_string(path).unwrap();
        let mut stats = Stats::default();
        let doc = convert(&body, &Export, &mut stats);
        insta::assert_snapshot!(report(&doc, &stats));
    });
}

#[test]
fn wiki_markup() {
    insta::glob!("golden/*.wiki", |path| {
        let body = std::fs::read_to_string(path).unwrap();
        let mut stats = Stats::default();
        let doc = convert_wiki(&body, &mut stats);
        insta::assert_snapshot!(report(&doc, &stats));
    });
}
