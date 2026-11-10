//! Table of contents entries for a `toc` block (shared by the HTML and Word renderers; the
//! editor does the same in `web/src/editor/toc.ts`).
//!
//! Block attributes (Confluence's `toc` macro parameters):
//! * `minLevel` / `maxLevel`: heading levels to list (default 1–6),
//! * `include` / `exclude`: regular expressions on the heading text (Confluence semantics: a
//!   heading is listed when it matches `include`, if set, and doesn't match `exclude`),
//! * `numbered`: outline numbers (1, 1.1, 1.2, 2 …),
//! * `style`: `list` (nested, the default) or `flat` (one line, separated by `·`).

use regex::{Regex, RegexBuilder};

use crate::node::Node;
use crate::text::headings;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TocEntry {
    pub level: u8,
    pub text: String,
    /// the heading's anchor id (see [`headings`])
    pub id: String,
    /// outline number (`1.2`) when the block is `numbered`, else empty
    pub number: String,
    /// nesting depth below the shallowest listed level (0-based)
    pub depth: u8,
}

/// Patterns come from page authors: bounded size, compiled with Rust's linear-time engine.
/// Invalid patterns are ignored (as Confluence does).
fn pattern(toc: &Node, key: &str) -> Option<Regex> {
    let p = toc.attr_str(key)?.trim();
    if p.is_empty() || p.len() > 200 {
        return None;
    }
    RegexBuilder::new(p).size_limit(1 << 18).dfa_size_limit(1 << 18).build().ok()
}

pub fn is_flat(toc: &Node) -> bool {
    toc.attr_str("style") == Some("flat")
}

/// The headings of `doc` that the `toc` block lists, in document order.
pub fn entries(doc: &Node, toc: &Node) -> Vec<TocEntry> {
    let level = |k: &str, d: i64| toc.attr_i64(k).unwrap_or(d).clamp(1, 6) as u8;
    let (min, max) = (level("minLevel", 1), level("maxLevel", 6));
    let include = pattern(toc, "include");
    let exclude = pattern(toc, "exclude");
    let numbered = toc.attr_bool("numbered").unwrap_or(false);
    let listed: Vec<(u8, String, String)> = headings(doc)
        .into_iter()
        .filter(|(lvl, text, _)| {
            *lvl >= min && *lvl <= max && include.as_ref().is_none_or(|r| r.is_match(text)) && !exclude.as_ref().is_some_and(|r| r.is_match(text))
        })
        .collect();
    let base = listed.iter().map(|(l, _, _)| *l).min().unwrap_or(1);
    let mut counters = [0usize; 6];
    listed
        .into_iter()
        .map(|(lvl, text, id)| {
            let depth = lvl - base;
            let d = depth as usize;
            counters[d] += 1;
            for c in counters.iter_mut().skip(d + 1) {
                *c = 0;
            }
            // a deeper heading before any at the parent level counts its parents as 1
            for c in counters.iter_mut().take(d) {
                if *c == 0 {
                    *c = 1;
                }
            }
            let number = if numbered { counters[..=d].iter().map(usize::to_string).collect::<Vec<_>>().join(".") } else { String::new() };
            TocEntry { level: lvl, text, id, number, depth }
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::node::nodes;

    fn doc() -> Node {
        Node::doc(vec![
            Node::new(nodes::TOC),
            Node::heading(1, vec![Node::text("Title")]),
            Node::heading(2, vec![Node::text("Goals")]),
            Node::heading(3, vec![Node::text("Must have")]),
            Node::heading(3, vec![Node::text("Nice to have")]),
            Node::heading(2, vec![Node::text("Appendix A")]),
            Node::heading(2, vec![Node::text("Risks")]),
        ])
    }

    fn texts(e: &[TocEntry]) -> Vec<String> {
        e.iter().map(|e| if e.number.is_empty() { e.text.clone() } else { format!("{} {}", e.number, e.text) }).collect()
    }

    #[test]
    fn all_headings_by_default() {
        let d = doc();
        let e = entries(&d, &d.content[0]);
        assert_eq!(texts(&e), ["Title", "Goals", "Must have", "Nice to have", "Appendix A", "Risks"]);
        assert_eq!(e[2].depth, 2);
        assert_eq!(e[1].id, "goals");
    }

    #[test]
    fn levels_patterns_and_numbers() {
        let d = doc();
        let toc = Node::new(nodes::TOC).attr("minLevel", 2).attr("maxLevel", 3).attr("exclude", "^Appendix").attr("numbered", true);
        let e = entries(&d, &toc);
        assert_eq!(texts(&e), ["1 Goals", "1.1 Must have", "1.2 Nice to have", "2 Risks"]);
        assert_eq!(e[0].depth, 0);
        let only = Node::new(nodes::TOC).attr("include", "have$");
        assert_eq!(texts(&entries(&d, &only)), ["Must have", "Nice to have"]);
        // a deeper heading first: its missing parent counts as 1
        let d2 = Node::doc(vec![Node::heading(3, vec![Node::text("Deep")]), Node::heading(2, vec![Node::text("Top")])]);
        let toc = Node::new(nodes::TOC).attr("minLevel", 2).attr("numbered", true);
        assert_eq!(texts(&entries(&d2, &toc)), ["1.1 Deep", "2 Top"]);
    }

    #[test]
    fn bad_patterns_are_ignored() {
        let d = doc();
        for p in ["(unclosed", &"a".repeat(500)] {
            let toc = Node::new(nodes::TOC).attr("exclude", p);
            assert_eq!(entries(&d, &toc).len(), 6, "{p}");
        }
        // catastrophic for backtracking engines, linear here
        let toc = Node::new(nodes::TOC).attr("exclude", "(a+)+$");
        assert_eq!(entries(&d, &toc).len(), 6);
    }
}
