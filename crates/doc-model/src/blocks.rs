//! Stable block ids.
//!
//! Every block node of the types listed in `assets/block_types.json` (shared with the web editor)
//! carries a short random id in the `blockId` attribute. Ids are unique within a document and
//! stay with the block through edits and moves, so links (`/pages/<id>#block-<blockId>`), inline
//! comments and AI citations can point at a block.
//!
//! The editor assigns ids to blocks it creates; the server assigns them to everything else:
//! imported and API-written content ([`ensure_ids`]) and live Yjs documents that predate block
//! ids ([`ensure_ydoc_ids`], run when a page is opened, so the first reader isn't an editor).

use std::collections::HashSet;
use std::sync::LazyLock;

use serde_json::Value;
use yrs::types::xml::{XmlFragment as _, XmlOut};
use yrs::{Doc, Transact, Xml, XmlElementRef};

use crate::node::Node;
use crate::ydoc::FIELD;

/// The attribute holding a block's id.
pub const ATTR: &str = "blockId";

const SHARED_JSON: &str = include_str!("../assets/block_types.json");

static TYPES: LazyLock<HashSet<String>> = LazyLock::new(|| {
    let v: Value = serde_json::from_str(SHARED_JSON).expect("valid block_types.json");
    v["types"].as_array().expect("types").iter().filter_map(Value::as_str).map(str::to_string).collect()
});

/// Whether nodes of this type carry a block id.
pub fn is_block_type(kind: &str) -> bool {
    TYPES.contains(kind)
}

const ALPHABET: &[u8] = b"abcdefghijklmnopqrstuvwxyz0123456789";

/// A new id: 8 characters from `[a-z0-9]`, starting with a letter (same as the editor).
pub fn new_id() -> String {
    let mut s = String::with_capacity(8);
    s.push(ALPHABET[fastrand::usize(..26)] as char);
    for _ in 1..8 {
        s.push(ALPHABET[fastrand::usize(..ALPHABET.len())] as char);
    }
    s
}

/// Ids come from clients and imports: short and URL-safe, or they are replaced.
pub fn is_valid_id(id: &str) -> bool {
    !id.is_empty() && id.len() <= 64 && id.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'-' || b == b'_')
}

fn fresh(seen: &mut HashSet<String>) -> String {
    loop {
        let id = new_id();
        if seen.insert(id.clone()) {
            return id;
        }
    }
}

/// Gives every block without a (valid, unique) id a new one; returns how many were assigned.
pub fn ensure_ids(doc: &mut Node) -> usize {
    let mut seen = HashSet::new();
    let mut assigned = 0;
    doc.walk_mut(&mut |n| {
        if !is_block_type(&n.kind) {
            return;
        }
        let keep = n.attr_str(ATTR).is_some_and(|id| is_valid_id(id) && seen.insert(id.to_string()));
        if !keep {
            n.attrs.insert(ATTR.into(), Value::String(fresh(&mut seen)));
            assigned += 1;
        }
    });
    assigned
}

/// Before [`ensure_ids`] on content that replaces `old` as a whole (REST and MCP updates, version
/// restore): blocks of `new` without an id take the id of an unchanged block of `old` (same type
/// and text), in document order, so links to blocks that didn't change keep working. Returns how
/// many ids were assigned in total.
pub fn adopt_ids(old: &Node, new: &mut Node) -> usize {
    use std::collections::{HashMap, VecDeque};
    let mut by_key: HashMap<(String, String), VecDeque<String>> = HashMap::new();
    old.walk(&mut |n| {
        if is_block_type(&n.kind)
            && let Some(id) = n.attr_str(ATTR).filter(|id| is_valid_id(id))
        {
            by_key.entry((n.kind.clone(), crate::text::plain_text(n))).or_default().push_back(id.to_string());
        }
    });
    let mut taken = HashSet::new();
    new.walk(&mut |n| {
        if let Some(id) = n.attr_str(ATTR) {
            taken.insert(id.to_string());
        }
    });
    new.walk_mut(&mut |n| {
        if !is_block_type(&n.kind) || n.attr_str(ATTR).is_some() {
            return;
        }
        if let Some(q) = by_key.get_mut(&(n.kind.clone(), crate::text::plain_text(n))) {
            while let Some(id) = q.pop_front() {
                if taken.insert(id.clone()) {
                    n.attrs.insert(ATTR.into(), Value::String(id));
                    break;
                }
            }
        }
    });
    ensure_ids(new)
}

/// Removes all block ids (content copied into another document that should get its own).
pub fn strip_ids(doc: &mut Node) {
    doc.walk_mut(&mut |n| {
        n.attrs.remove(ATTR);
    });
}

/// The block with `id`, if the document has one.
pub fn find<'a>(doc: &'a Node, id: &str) -> Option<&'a Node> {
    let mut found = None;
    doc.walk(&mut |n| {
        if found.is_none() && n.attr_str(ATTR) == Some(id) {
            found = Some(n);
        }
    });
    found
}

/// Blocks of `new` that are new or changed compared with `old` (matched by block id): for each
/// change the innermost block it is in, at most `limit`. Removed blocks aren't listed.
pub fn changed_blocks(old: &Node, new: &Node, limit: usize) -> Vec<String> {
    let mut before: std::collections::HashMap<&str, &Node> = std::collections::HashMap::new();
    old.walk(&mut |n| {
        if let Some(id) = n.attr_str(ATTR) {
            before.insert(id, n);
        }
    });
    fn diff(n: &Node, before: &std::collections::HashMap<&str, &Node>, out: &mut Vec<String>, limit: usize) -> bool {
        if out.len() >= limit {
            return false;
        }
        let inner = |out: &mut Vec<String>| n.content.iter().fold(false, |any, c| diff(c, before, out, limit) | any);
        match n.attr_str(ATTR) {
            Some(id) => match before.get(id) {
                None => {
                    out.push(id.to_string());
                    true
                }
                Some(old) if *old == n => false,
                Some(_) => {
                    // the change is inside a child block, or in this block's own text and attributes
                    if !inner(out) {
                        out.push(id.to_string());
                    }
                    true
                }
            },
            None => inner(out),
        }
    }
    let mut out = Vec::new();
    diff(new, &before, &mut out, limit);
    out
}

/// Whether [`ensure_ydoc_ids`] would change anything (read only).
pub fn ydoc_needs_ids(doc: &Doc) -> bool {
    let frag = doc.get_or_insert_xml_fragment(FIELD);
    let txn = doc.transact();
    let mut seen = HashSet::new();
    for d in frag.successors(&txn) {
        let XmlOut::Element(el) = d else { continue };
        if !is_block_type(el.tag()) {
            continue;
        }
        let ok = match el.get_attribute(&txn, ATTR) {
            Some(yrs::Out::Any(yrs::Any::String(s))) => is_valid_id(&s) && seen.insert(s.to_string()),
            _ => false,
        };
        if !ok {
            return true;
        }
    }
    false
}

/// Like [`ensure_ids`] on a live Yjs document: sets the attribute on elements that lack one, as
/// ordinary Yjs changes (they merge with concurrent edits). Returns whether anything changed.
pub fn ensure_ydoc_ids(doc: &Doc) -> bool {
    let frag = doc.get_or_insert_xml_fragment(FIELD);
    // collect first (reading), then write in one transaction
    let mut todo: Vec<XmlElementRef> = Vec::new();
    {
        let txn = doc.transact();
        let mut seen = HashSet::new();
        for d in frag.successors(&txn) {
            let XmlOut::Element(el) = d else { continue };
            if !is_block_type(el.tag()) {
                continue;
            }
            let id = match el.get_attribute(&txn, ATTR) {
                Some(yrs::Out::Any(yrs::Any::String(s))) => Some(s.to_string()),
                _ => None,
            };
            let keep = id.is_some_and(|id| is_valid_id(&id) && seen.insert(id));
            if !keep {
                todo.push(el);
            }
        }
        drop(txn);
        if todo.is_empty() {
            return false;
        }
        // `seen` holds the ids already present: they aren't handed out again
        let mut txn = doc.transact_mut();
        for el in &todo {
            let id = fresh(&mut seen);
            el.insert_attribute(&mut txn, ATTR, yrs::Any::String(id.into()));
        }
    }
    true
}

#[cfg(test)]
mod tests {

    use super::*;

    #[test]
    fn changed_blocks_are_the_innermost_changes() {
        let p = |id: &str, t: &str| Node::paragraph(vec![Node::text(t)]).attr(ATTR, id);
        let item = |id: &str, pid: &str, t: &str| Node::new(nodes::LIST_ITEM).attr(ATTR, id).with_content(vec![p(pid, t)]);
        let old = Node::doc(vec![
            p("aaaaaaa1", "Intro"),
            Node::new(nodes::BULLET_LIST).attr(ATTR, "lst00001").with_content(vec![item("itm00001", "par00001", "one"), item("itm00002", "par00002", "two")]),
            p("aaaaaaa2", "Gone"),
            p("aaaaaaa3", "Same"),
        ]);
        let new = Node::doc(vec![
            p("aaaaaaa1", "Intro, longer"),
            Node::new(nodes::BULLET_LIST).attr(ATTR, "lst00001").with_content(vec![
                item("itm00001", "par00001", "one"),
                item("itm00002", "par00002", "TWO"),
                item("itm00003", "par00003", "three"),
            ]),
            p("aaaaaaa3", "Same"),
            p("bbbbbbb1", "New"),
        ]);
        assert_eq!(changed_blocks(&old, &new, 100), vec!["aaaaaaa1", "par00002", "itm00003", "bbbbbbb1"]);
        assert_eq!(changed_blocks(&old, &new, 2).len(), 2);
        assert!(changed_blocks(&new, &new, 100).is_empty());
    }
    use crate::node::nodes;
    use crate::ydoc;

    fn sample() -> Node {
        Node::doc(vec![
            Node::heading(1, vec![Node::text("Title")]),
            Node::paragraph(vec![Node::text("one")]),
            Node::new(nodes::BULLET_LIST).with_content(vec![Node::new(nodes::LIST_ITEM).with_content(vec![Node::paragraph(vec![Node::text("item")])])]),
            Node::new(nodes::TABLE).with_content(vec![
                Node::new(nodes::TABLE_ROW).with_content(vec![Node::new(nodes::TABLE_CELL).with_content(vec![Node::paragraph(vec![Node::text("cell")])])]),
            ]),
        ])
    }

    fn ids(doc: &Node) -> Vec<(String, String)> {
        let mut out = Vec::new();
        doc.walk(&mut |n| {
            if let Some(id) = n.attr_str(ATTR) {
                out.push((n.kind.clone(), id.to_string()));
            }
        });
        out
    }

    #[test]
    fn shared_list_matches_the_constant() {
        let v: Value = serde_json::from_str(SHARED_JSON).unwrap();
        assert_eq!(v["attribute"], ATTR, "the editor uses the same attribute");
        for t in ["paragraph", "heading", "listItem", "table", "callout", "codeBlock"] {
            assert!(is_block_type(t), "{t}");
        }
        for t in ["doc", "text", "tableRow", "tableCell", "mention", "image", "hardBreak"] {
            assert!(!is_block_type(t), "{t}");
        }
    }

    #[test]
    fn new_ids_have_the_editor_format() {
        for _ in 0..200 {
            let id = new_id();
            assert_eq!(id.len(), 8);
            assert!(id.as_bytes()[0].is_ascii_lowercase());
            assert!(is_valid_id(&id));
        }
    }

    #[test]
    fn assigns_missing_ids_once() {
        let mut d = sample();
        assert_eq!(ensure_ids(&mut d), 7); // heading, 3 paragraphs, list, item, table
        let first = ids(&d);
        let kinds: Vec<&str> = first.iter().map(|(k, _)| k.as_str()).collect();
        assert_eq!(kinds, ["heading", "paragraph", "bulletList", "listItem", "paragraph", "table", "paragraph"]);
        assert_eq!(ensure_ids(&mut d), 0, "stable on the second run");
        assert_eq!(ids(&d), first);
        let unique: HashSet<_> = first.iter().map(|(_, id)| id).collect();
        assert_eq!(unique.len(), first.len());
    }

    #[test]
    fn replaces_duplicate_and_invalid_ids() {
        let mut d = Node::doc(vec![
            Node::paragraph(vec![Node::text("a")]).attr(ATTR, "same"),
            Node::paragraph(vec![Node::text("b")]).attr(ATTR, "same"),
            Node::paragraph(vec![Node::text("c")]).attr(ATTR, "<script>"),
            Node::paragraph(vec![Node::text("d")]).attr(ATTR, 7),
        ]);
        assert_eq!(ensure_ids(&mut d), 3);
        assert_eq!(d.content[0].attr_str(ATTR), Some("same"), "the first keeps its id");
        assert_ne!(d.content[1].attr_str(ATTR), Some("same"));
        assert!(d.content.iter().all(|p| p.attr_str(ATTR).is_some_and(is_valid_id)));
        assert!(find(&d, "same").is_some_and(|n| crate::text::plain_text(n) == "a"));
    }

    #[test]
    fn replaced_content_keeps_the_ids_of_unchanged_blocks() {
        let mut old = sample();
        ensure_ids(&mut old);
        let id_of = |d: &Node, text: &str, kind: &str| {
            let mut out = None;
            d.walk(&mut |n| {
                if out.is_none() && n.kind == kind && crate::text::plain_text(n) == text {
                    out = n.attr_str(ATTR).map(str::to_string);
                }
            });
            out
        };
        // same content without ids, one paragraph edited, a new one in front
        let mut new = sample();
        new.content[1] = Node::paragraph(vec![Node::text("one, edited")]);
        new.content.insert(0, Node::paragraph(vec![Node::text("cell")]));
        adopt_ids(&old, &mut new);
        assert_eq!(id_of(&new, "Title", "heading"), id_of(&old, "Title", "heading"));
        assert_eq!(id_of(&new, "item", "listItem"), id_of(&old, "item", "listItem"));
        assert_ne!(id_of(&new, "one, edited", "paragraph"), id_of(&old, "one", "paragraph"));
        // the new first paragraph has the text of the table cell's paragraph: one of them gets
        // the old id, never both
        let mut all = Vec::new();
        new.walk(&mut |n| all.extend(n.attr_str(ATTR).map(str::to_string)));
        let unique: HashSet<_> = all.iter().collect();
        assert_eq!(unique.len(), all.len());
        assert_eq!(ensure_ids(&mut new), 0);
    }

    #[test]
    fn strip_removes_all() {
        let mut d = sample();
        ensure_ids(&mut d);
        strip_ids(&mut d);
        assert!(ids(&d).is_empty());
    }

    #[test]
    fn ydoc_ids_are_assigned_and_survive_the_round_trip() {
        let doc = ydoc::new_doc();
        let mut pm = sample();
        pm.content[1].attrs.insert(ATTR.into(), Value::String("keepme12".into()));
        ydoc::write_pm(&doc, &pm);
        assert!(ensure_ydoc_ids(&doc));
        assert!(!ensure_ydoc_ids(&doc), "nothing left to do");
        let back = ydoc::doc_to_pm(&doc);
        assert_eq!(back.content[1].attr_str(ATTR), Some("keepme12"));
        let mut again = back.clone();
        assert_eq!(ensure_ids(&mut again), 0, "every block has a unique id");
        // plain round trip through Yjs keeps them
        assert_eq!(ydoc::update_to_pm(&ydoc::pm_to_update(&back)).unwrap(), back);
    }

    #[test]
    fn ydoc_ids_merge_with_concurrent_edits() {
        let a = ydoc::new_doc();
        ydoc::write_pm(&a, &Node::doc(vec![Node::paragraph(vec![Node::text("base")])]));
        let b = ydoc::new_doc();
        ydoc::apply_update(&b, &ydoc::encode_state(&a)).unwrap();
        // the server assigns ids while a client types into the same paragraph
        assert!(ensure_ydoc_ids(&a));
        {
            let frag = b.get_or_insert_xml_fragment(FIELD);
            let mut txn = b.transact_mut();
            let Some(XmlOut::Element(p)) = frag.get(&txn, 0) else { panic!("paragraph") };
            let Some(XmlOut::Text(t)) = p.get(&txn, 0) else { panic!("text") };
            yrs::Text::insert(&t, &mut txn, 4, " typed");
        }
        let (ua, ub) = (ydoc::encode_state(&a), ydoc::encode_state(&b));
        ydoc::apply_update(&a, &ub).unwrap();
        ydoc::apply_update(&b, &ua).unwrap();
        let (pa, pb) = (ydoc::doc_to_pm(&a), ydoc::doc_to_pm(&b));
        assert_eq!(pa, pb);
        assert_eq!(crate::text::plain_text(&pa), "base typed");
        assert!(pa.content[0].attr_str(ATTR).is_some());
    }
}
