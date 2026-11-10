//! Conversion between Yjs documents (as produced by `y-prosemirror` / Tiptap Collaboration)
//! and ProseMirror JSON.
//!
//! Encoding (y-prosemirror compatible):
//! * the document is the `XmlFragment` named [`FIELD`] (Tiptap default: `"default"`),
//! * every non-text node is an `XmlElement` whose tag is the node type and whose attributes are
//!   the node attrs (non-null values, stored as Yjs `Any`),
//! * runs of text are an `XmlText`; marks are formatting attributes `{markName: markAttrs}`.
//!   Overlapping marks of the same type use keys `markName--<hash>`.

use std::collections::HashMap;
use std::sync::Arc;

use serde_json::{Map, Value};
use yrs::types::Attrs;
use yrs::types::text::Diff;
use yrs::types::xml::{Xml, XmlElementPrelim, XmlFragment as _, XmlOut, XmlTextPrelim};
use yrs::updates::decoder::Decode;
use yrs::{Any, Doc, GetString, Out, ReadTxn, Text, Transact, Update, XmlElementRef, XmlTextRef};

use crate::node::{Mark, Node, nodes};

/// Name of the root XML fragment used by Tiptap's Collaboration extension.
pub const FIELD: &str = "default";

#[derive(Debug, thiserror::Error)]
pub enum YdocError {
    #[error("invalid yjs update: {0}")]
    Decode(String),
    #[error("failed to apply yjs update: {0}")]
    Apply(String),
}

fn any_to_json(a: &Any) -> Value {
    serde_json::to_value(a).unwrap_or(Value::Null)
}

fn json_to_any(v: &Value) -> Any {
    serde_json::from_value::<Any>(v.clone()).unwrap_or(Any::Null)
}

fn out_to_json<T: ReadTxn>(o: &Out, _txn: &T) -> Value {
    match o {
        Out::Any(a) => any_to_json(a),
        other => Value::String(other.to_string()),
    }
}

/// Creates a Yjs document with JS-compatible (UTF-16) string offsets.
///
/// yrs defaults to UTF-8 byte offsets while Yjs in the browser uses UTF-16 code units; all
/// WikiCraft documents must use UTF-16 so indices mean the same thing on both sides.
pub fn new_doc() -> Doc {
    Doc::with_options(yrs::Options { offset_kind: yrs::OffsetKind::Utf16, ..yrs::Options::default() })
}

/// Reads the ProseMirror document stored in `doc` (root fragment [`FIELD`]).
pub fn doc_to_pm(doc: &Doc) -> Node {
    let frag = doc.get_or_insert_xml_fragment(FIELD);
    let txn = doc.transact();
    let mut root = Node::new(nodes::DOC);
    root.content = children_to_pm(&frag, &txn, 0);
    if root.content.is_empty() {
        root.content.push(Node::paragraph(vec![]));
    }
    root
}

/// Decodes a Yjs v1 update (full state) and reads the ProseMirror document from it.
pub fn update_to_pm(update: &[u8]) -> Result<Node, YdocError> {
    let doc = new_doc();
    apply_update(&doc, update)?;
    Ok(doc_to_pm(&doc))
}

/// Applies a Yjs v1 update to `doc`.
pub fn apply_update(doc: &Doc, update: &[u8]) -> Result<(), YdocError> {
    if update.is_empty() {
        return Ok(());
    }
    let u = Update::decode_v1(update).map_err(|e| YdocError::Decode(e.to_string()))?;
    doc.transact_mut().apply_update(u).map_err(|e| YdocError::Apply(e.to_string()))
}

/// Encodes the full state of `doc` as a Yjs v1 update.
pub fn encode_state(doc: &Doc) -> Vec<u8> {
    doc.transact().encode_state_as_update_v1(&yrs::StateVector::default())
}

/// Creates a fresh Yjs document containing `pm` and returns its full state as a v1 update.
pub fn pm_to_update(pm: &Node) -> Vec<u8> {
    let doc = new_doc();
    write_pm(&doc, pm);
    encode_state(&doc)
}

/// Replaces the content of `doc`'s root fragment with `pm` (one transaction).
pub fn write_pm(doc: &Doc, pm: &Node) {
    let frag = doc.get_or_insert_xml_fragment(FIELD);
    let mut txn = doc.transact_mut();
    let len = frag.len(&txn);
    if len > 0 {
        frag.remove_range(&mut txn, 0, len);
    }
    let content: &[Node] = if pm.kind == nodes::DOC { &pm.content } else { std::slice::from_ref(pm) };
    write_children(&frag, &mut txn, content);
}

/// Yjs documents come from clients: cap nesting (see [`crate::node::MAX_DEPTH`]).
const MAX_YDOC_DEPTH: usize = crate::node::MAX_DEPTH;

fn children_to_pm<C: yrs::types::xml::XmlFragment, T: ReadTxn>(parent: &C, txn: &T, depth: usize) -> Vec<Node> {
    let mut out = Vec::new();
    if depth > MAX_YDOC_DEPTH {
        // flatten the remaining subtree to text with the iterative tree walker
        let mut s = String::new();
        for d in parent.successors(txn) {
            if let XmlOut::Text(t) = d {
                s.push_str(&t.get_string(txn));
                s.push(' ');
            }
        }
        let s = s.trim().to_string();
        if !s.is_empty() {
            out.push(Node::text(s));
        }
        return out;
    }
    for child in parent.children(txn) {
        match child {
            XmlOut::Element(el) => out.push(element_to_pm(&el, txn, depth + 1)),
            XmlOut::Text(t) => text_to_pm(&t, txn, &mut out),
            XmlOut::Fragment(f) => out.extend(children_to_pm(&f, txn, depth + 1)),
        }
    }
    out
}

fn element_to_pm<T: ReadTxn>(el: &XmlElementRef, txn: &T, depth: usize) -> Node {
    let mut node = Node::new(el.tag().to_string());
    let mut attrs: Vec<(String, Value)> = el.attributes(txn).map(|(k, v)| (k.to_string(), out_to_json(&v, txn))).collect();
    attrs.sort_by(|a, b| a.0.cmp(&b.0));
    for (k, v) in attrs {
        if k == "ychange" || v.is_null() {
            continue;
        }
        node.attrs.insert(k, v);
    }
    node.content = children_to_pm(el, txn, depth);
    node
}

fn text_to_pm<T: ReadTxn>(t: &XmlTextRef, txn: &T, out: &mut Vec<Node>) {
    let diff: Vec<Diff<()>> = t.diff(txn, |_| ());
    for d in diff {
        let text = match &d.insert {
            Out::Any(Any::String(s)) => s.to_string(),
            Out::Any(other) => match other {
                Any::Null | Any::Undefined => continue,
                a => a.to_string(),
            },
            // embeds of shared types are not produced by y-prosemirror; render their string form
            other => other.to_string(),
        };
        if text.is_empty() {
            continue;
        }
        let marks = d.attributes.as_deref().map(attrs_to_marks).unwrap_or_default();
        out.push(Node::text_marked(text, marks));
    }
}

fn attrs_to_marks(attrs: &Attrs) -> Vec<Mark> {
    let mut marks: Vec<Mark> = attrs
        .iter()
        .filter(|(k, v)| k.as_ref() != "ychange" && !matches!(v, Any::Null | Any::Undefined))
        .map(|(k, v)| {
            let name = k.split("--").next().unwrap_or(k).to_string();
            let attrs = match any_to_json(v) {
                Value::Object(m) => m,
                _ => Map::new(),
            };
            Mark { kind: name, attrs }
        })
        .collect();
    // deterministic order (Yjs attribute maps are unordered)
    crate::node::sort_marks(&mut marks);
    marks
}

fn write_children<C: yrs::types::xml::XmlFragment>(parent: &C, txn: &mut yrs::TransactionMut, content: &[Node]) {
    let mut i = 0;
    while i < content.len() {
        if content[i].is_text() {
            // one XmlText per run of consecutive text nodes
            let t: XmlTextRef = parent.push_back(txn, XmlTextPrelim::new(""));
            let mut idx = 0u32;
            while i < content.len() && content[i].is_text() {
                let n = &content[i];
                let s = n.text.as_deref().unwrap_or("");
                if !s.is_empty() {
                    t.insert_with_attributes(txn, idx, s, marks_to_attrs(&n.marks));
                    idx += s.encode_utf16().count() as u32;
                }
                i += 1;
            }
            continue;
        }
        let n = &content[i];
        let el: XmlElementRef = parent.push_back(txn, XmlElementPrelim::empty(n.kind.as_str()));
        for (k, v) in &n.attrs {
            if v.is_null() {
                continue;
            }
            el.insert_attribute(txn, k.as_str(), json_to_any(v));
        }
        write_children(&el, txn, &n.content);
        i += 1;
    }
}

fn marks_to_attrs(marks: &[Mark]) -> Attrs {
    let mut attrs: Attrs = HashMap::new();
    let mut seen: HashMap<&str, usize> = HashMap::new();
    for m in marks {
        let count = seen.entry(m.kind.as_str()).or_default();
        let key: Arc<str> = if *count == 0 { Arc::from(m.kind.as_str()) } else { Arc::from(format!("{}--{}", m.kind, count)) };
        *count += 1;
        attrs.insert(key, json_to_any(&Value::Object(m.attrs.clone())));
    }
    attrs
}

/// Points mentions of user `from` at user `to` (with `to`'s name as the label), as live Yjs
/// attribute changes that merge with concurrent edits. Returns how many mentions changed.
pub fn replace_mentions(doc: &Doc, from: &str, to: &str, label: &str) -> usize {
    let frag = doc.get_or_insert_xml_fragment(FIELD);
    let mut hits: Vec<XmlElementRef> = Vec::new();
    {
        let txn = doc.transact();
        for d in frag.successors(&txn) {
            if let XmlOut::Element(el) = d
                && el.tag().as_ref() == nodes::MENTION
                && matches!(el.get_attribute(&txn, "id"), Some(Out::Any(Any::String(s))) if &*s == from)
            {
                hits.push(el);
            }
        }
    }
    if !hits.is_empty() {
        let mut txn = doc.transact_mut();
        for el in &hits {
            el.insert_attribute(&mut txn, "id", Any::String(to.into()));
            el.insert_attribute(&mut txn, "label", Any::String(label.into()));
        }
    }
    hits.len()
}

/// Plain-text helper on a Yjs doc (useful for quick checks).
pub fn doc_plain_text(doc: &Doc) -> String {
    let frag = doc.get_or_insert_xml_fragment(FIELD);
    let txn = doc.transact();
    frag.get_string(&txn)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::node::marks;
    use pretty_assertions::assert_eq;

    fn sample() -> Node {
        Node::doc(vec![
            Node::heading(1, vec![Node::text("Title")]),
            Node::paragraph(vec![
                Node::text("Hello "),
                Node::text_marked("bold", vec![Mark::new(marks::BOLD)]),
                Node::text(" and "),
                Node::text_marked("link", vec![Mark::link("https://example.com"), Mark::new(marks::ITALIC)]),
                Node::new(nodes::HARD_BREAK),
                Node::text("äöü 😀 next line"),
            ]),
            Node::new(nodes::BULLET_LIST).with_content(vec![
                Node::new(nodes::LIST_ITEM).with_content(vec![Node::paragraph(vec![Node::text("one")])]),
                Node::new(nodes::LIST_ITEM).with_content(vec![Node::paragraph(vec![Node::text("two")])]),
            ]),
            Node::new(nodes::CODE_BLOCK).attr("language", "rust").with_content(vec![Node::text("fn main() {}\n")]),
            Node::new(nodes::TASK_LIST)
                .with_content(vec![Node::new(nodes::TASK_ITEM).attr("checked", true).with_content(vec![Node::paragraph(vec![Node::text("done")])])]),
            Node::new(nodes::CALLOUT).attr("type", "warning").with_content(vec![Node::paragraph(vec![Node::text("careful")])]),
            Node::paragraph(vec![]),
        ])
    }

    #[test]
    fn roundtrip_pm_yjs_pm() {
        let pm = sample();
        let update = pm_to_update(&pm);
        let back = update_to_pm(&update).unwrap();
        assert_eq!(back, pm);
    }

    #[test]
    fn rewrite_replaces_content() {
        let doc = new_doc();
        write_pm(&doc, &sample());
        let small = Node::doc(vec![Node::paragraph(vec![Node::text("x")])]);
        write_pm(&doc, &small);
        assert_eq!(doc_to_pm(&doc), small);
    }

    #[test]
    fn mentions_are_repointed() {
        let mention = |id: &str, label: &str| Node::new(nodes::MENTION).attr("id", id).attr("label", label);
        let pm = Node::doc(vec![Node::paragraph(vec![Node::text("hi "), mention("old", "Confluence user 1"), Node::text(" and "), mention("other", "Bo")])]);
        let doc = new_doc();
        write_pm(&doc, &pm);
        assert_eq!(replace_mentions(&doc, "old", "new", "Ada"), 1);
        assert_eq!(replace_mentions(&doc, "old", "new", "Ada"), 0);
        let back = doc_to_pm(&doc);
        assert_eq!(back.content[0].content[1], mention("new", "Ada"));
        assert_eq!(back.content[0].content[3], mention("other", "Bo"));
        let mut json = pm.clone();
        assert_eq!(crate::node::replace_mentions(&mut json, "old", "new", "Ada"), 1);
        assert_eq!(json, back);
    }

    #[test]
    fn empty_doc_has_paragraph() {
        let doc = new_doc();
        assert_eq!(doc_to_pm(&doc), Node::empty_doc());
    }

    #[test]
    fn incremental_updates_merge() {
        // two replicas edit concurrently; both converge
        let a = Doc::with_options(yrs::Options { client_id: yrs::block::ClientID::new(1), offset_kind: yrs::OffsetKind::Utf16, ..Default::default() });
        write_pm(&a, &Node::doc(vec![Node::paragraph(vec![Node::text("base")])]));
        let b = Doc::with_options(yrs::Options { client_id: yrs::block::ClientID::new(2), offset_kind: yrs::OffsetKind::Utf16, ..Default::default() });
        apply_update(&b, &encode_state(&a)).unwrap();
        {
            let frag = a.get_or_insert_xml_fragment(FIELD);
            let mut txn = a.transact_mut();
            frag.push_back(&mut txn, XmlElementPrelim::empty("horizontalRule"));
        }
        {
            let frag = b.get_or_insert_xml_fragment(FIELD);
            let mut txn = b.transact_mut();
            frag.push_back(&mut txn, XmlElementPrelim::empty("horizontalRule"));
        }
        let ua = encode_state(&a);
        let ub = encode_state(&b);
        apply_update(&a, &ub).unwrap();
        apply_update(&b, &ua).unwrap();
        assert_eq!(doc_to_pm(&a), doc_to_pm(&b));
        assert_eq!(doc_to_pm(&a).content.len(), 3);
    }
}

#[cfg(test)]
mod depth {
    use super::*;

    #[test]
    fn deeply_nested_ydoc_is_flattened_not_overflowing() {
        let doc = new_doc();
        {
            let frag = doc.get_or_insert_xml_fragment(FIELD);
            let mut txn = doc.transact_mut();
            let mut cur: XmlElementRef = frag.push_back(&mut txn, XmlElementPrelim::empty("blockquote"));
            for _ in 0..20_000 {
                cur = yrs::types::xml::XmlFragment::push_back(&cur, &mut txn, XmlElementPrelim::empty("blockquote"));
            }
            let t: XmlTextRef = yrs::types::xml::XmlFragment::push_back(&cur, &mut txn, XmlTextPrelim::new(""));
            t.insert(&mut txn, 0, "deep");
        }
        let handle = std::thread::Builder::new().stack_size(2 << 20).spawn(move || {
            let pm = doc_to_pm(&doc);
            crate::text::plain_text(&pm)
        });
        assert_eq!(handle.unwrap().join().unwrap(), "deep");
    }

    #[test]
    fn deeply_nested_markdown_is_flattened() {
        let md = format!("{} x", ">".repeat(20_000));
        let r = std::thread::Builder::new().stack_size(2 << 20).spawn(move || crate::text::plain_text(&crate::markdown::markdown_to_pm(&md).doc));
        assert_eq!(r.unwrap().join().unwrap(), "x");
    }

    #[test]
    fn dangerous_urls_are_removed() {
        let mut d = Node::doc(vec![Node::paragraph(vec![
            Node::text_marked("x", vec![Mark::link("javascript:alert(1)")]),
            Node::new(nodes::IMAGE).attr("src", "data:text/html,boom"),
        ])]);
        crate::node::sanitize_tree(&mut d);
        assert!(d.content[0].content[0].marks.is_empty());
        assert!(d.content[0].content[1].attr_str("src").is_none());
    }
}

#[cfg(test)]
mod repro {
    use super::*;
    #[test]
    fn multi_mark_text_then_more_blocks() {
        let j = serde_json::json!({"type":"doc","content":[
            {"type":"bulletList","content":[{"type":"listItem","content":[{"type":"paragraph","content":[
                {"type":"text","text":"Grüße — go\u{00a0}"},
                {"type":"text","text":"here","marks":[{"type":"link","attrs":{"href":"wcimport:page/22992544"}},{"type":"bold"}]},
                {"type":"text","text":" to request "},
                {"type":"text","text":"video","marks":[{"type":"bold"}]},
                {"type":"text","text":" recording."}]}]}]},
            {"type":"paragraph","content":[{"type":"text","text":"after"}]}
        ]});
        let pm = Node::from_json(&j).unwrap();
        let back = update_to_pm(&pm_to_update(&pm)).unwrap();
        assert_eq!(serde_json::to_string_pretty(&back).unwrap(), serde_json::to_string_pretty(&pm).unwrap());
    }
}
