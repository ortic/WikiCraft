//! ProseMirror-compatible JSON document model.
//!
//! The canonical WikiCraft document is a ProseMirror/Tiptap JSON tree. Node and mark
//! names follow Tiptap's defaults so the frontend editor can load documents unchanged.

use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};

/// A ProseMirror node (`{"type": ..., "attrs": ..., "content": [...], "text": ..., "marks": [...]}`).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Default)]
pub struct Node {
    #[serde(rename = "type")]
    pub kind: String,
    #[serde(default, skip_serializing_if = "Map::is_empty")]
    pub attrs: Map<String, Value>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub content: Vec<Node>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub text: Option<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub marks: Vec<Mark>,
}

/// A ProseMirror mark (`{"type": ..., "attrs": ...}`).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Default)]
pub struct Mark {
    #[serde(rename = "type")]
    pub kind: String,
    #[serde(default, skip_serializing_if = "Map::is_empty")]
    pub attrs: Map<String, Value>,
}

/// Points mentions of user `from` at user `to` (label: `to`'s name); returns how many changed.
pub fn replace_mentions(doc: &mut Node, from: &str, to: &str, label: &str) -> usize {
    let mut n = 0;
    doc.walk_mut(&mut |node| {
        if node.kind == nodes::MENTION && node.attr_str("id") == Some(from) {
            node.attrs.insert("id".into(), Value::String(to.into()));
            node.attrs.insert("label".into(), Value::String(label.into()));
            n += 1;
        }
    });
    n
}

/// Node type names (Tiptap defaults + WikiCraft custom nodes).
pub mod nodes {
    pub const DOC: &str = "doc";
    pub const PARAGRAPH: &str = "paragraph";
    pub const TEXT: &str = "text";
    pub const HEADING: &str = "heading";
    pub const BULLET_LIST: &str = "bulletList";
    pub const ORDERED_LIST: &str = "orderedList";
    pub const LIST_ITEM: &str = "listItem";
    pub const TASK_LIST: &str = "taskList";
    pub const TASK_ITEM: &str = "taskItem";
    pub const CODE_BLOCK: &str = "codeBlock";
    pub const BLOCKQUOTE: &str = "blockquote";
    pub const HORIZONTAL_RULE: &str = "horizontalRule";
    pub const HARD_BREAK: &str = "hardBreak";
    pub const IMAGE: &str = "image";
    pub const TABLE: &str = "table";
    pub const TABLE_ROW: &str = "tableRow";
    pub const TABLE_HEADER: &str = "tableHeader";
    pub const TABLE_CELL: &str = "tableCell";
    pub const DETAILS: &str = "details";
    pub const DETAILS_SUMMARY: &str = "detailsSummary";
    pub const DETAILS_CONTENT: &str = "detailsContent";
    pub const MENTION: &str = "mention";
    // WikiCraft custom nodes
    pub const CALLOUT: &str = "callout";
    pub const STATUS: &str = "status";
    pub const DATE: &str = "date";
    /// Inline emoticon (`name` = Confluence emoticon name, drawn by our icon set).
    pub const EMOTICON: &str = "emoticon";
    pub const ATTACHMENT: &str = "attachment";
    pub const EMBED: &str = "embed";
    pub const TOC: &str = "toc";
    pub const CHILD_PAGES: &str = "childPages";
    /// Another page's content, live (`pageId`), or only its excerpt (`excerpt`: `true` for the
    /// first one, or an excerpt's `name`); `nopanel` drops the frame around it
    pub const INCLUDE: &str = "include";
    /// A part of the page that other pages can include on its own (Confluence's `excerpt`):
    /// `name` (optional, for several per page), `hidden` (shown only where it's included)
    pub const EXCERPT: &str = "excerpt";
    pub const ANCHOR: &str = "anchor";
    pub const COLUMNS: &str = "columns";
    pub const COLUMN: &str = "column";
    pub const DECISION_LIST: &str = "decisionList";
    pub const DECISION_ITEM: &str = "decisionItem";
    pub const MATH_BLOCK: &str = "blockMath";
    pub const MATH_INLINE: &str = "inlineMath";
    /// A macro we don't (yet) support natively. Preserves name/params/body for later mapping.
    pub const MACRO: &str = "macro";
    /// an Excalidraw drawing: `scene` (JSON) and `preview` (PNG attachment URL)
    pub const DRAWING: &str = "drawing";
    /// instructional text for authors (`text`), e.g. in templates; never shown to readers,
    /// exported or indexed (Confluence's `ac:placeholder`)
    pub const PLACEHOLDER: &str = "instructionalText";
}

/// Mark type names.
pub mod marks {
    pub const BOLD: &str = "bold";
    pub const ITALIC: &str = "italic";
    pub const UNDERLINE: &str = "underline";
    pub const STRIKE: &str = "strike";
    pub const CODE: &str = "code";
    pub const LINK: &str = "link";
    pub const SUBSCRIPT: &str = "subscript";
    pub const SUPERSCRIPT: &str = "superscript";
    pub const TEXT_STYLE: &str = "textStyle";
    pub const HIGHLIGHT: &str = "highlight";
    /// Inline comment anchor (`attrs.commentId`).
    pub const COMMENT: &str = "comment";
}

/// Maximum block nesting depth produced by converters. Deeper input (hostile or broken) is
/// flattened to text so recursive algorithms (conversion, serialization, `Drop`) can't overflow
/// the stack.
/// Each level costs two JSON nesting levels (node object + content array) and serde_json's
/// recursion limit is 128, so stored documents must stay well below 64 levels.
pub const MAX_DEPTH: usize = 40;

/// Deepest indentation of a paragraph or heading.
pub const MAX_INDENT: i64 = 8;

/// A paragraph's or heading's indentation level (`indent` attribute, 0 to [`MAX_INDENT`]).
pub fn indent_level(n: &Node) -> i64 {
    if n.kind != nodes::PARAGRAPH && n.kind != nodes::HEADING {
        return 0;
    }
    n.attr_i64("indent").unwrap_or(0).clamp(0, MAX_INDENT)
}

/// Inline node types (live inside paragraphs/headings next to text).
pub fn is_inline(kind: &str) -> bool {
    matches!(
        kind,
        nodes::TEXT
            | nodes::HARD_BREAK
            | nodes::MENTION
            | nodes::STATUS
            | nodes::DATE
            | nodes::EMOTICON
            | nodes::MATH_INLINE
            | nodes::ANCHOR
            | nodes::PLACEHOLDER
    ) || kind == nodes::IMAGE // images are inline in our schema (Tiptap `inline: true`)
}

/// Turns `{?…}` in text into placeholder nodes (author instructions in Markdown templates).
pub fn expand_placeholders(n: &mut Node) {
    let has = |c: &Node| c.kind == nodes::TEXT && c.text.as_deref().is_some_and(|t| t.contains("{?"));
    if n.content.iter().any(has) {
        // the Markdown parser may split text runs; join neighbours with the same marks first
        let mut joined: Vec<Node> = Vec::with_capacity(n.content.len());
        for c in std::mem::take(&mut n.content) {
            match joined.last_mut() {
                Some(prev) if prev.kind == nodes::TEXT && c.kind == nodes::TEXT && prev.marks == c.marks => {
                    let t = c.text.unwrap_or_default();
                    prev.text.get_or_insert_with(String::new).push_str(&t);
                }
                _ => joined.push(c),
            }
        }
        let mut out = Vec::with_capacity(joined.len());
        for c in joined {
            let text = match (&c.kind[..], &c.text) {
                (nodes::TEXT, Some(t)) if t.contains("{?") => t.clone(),
                _ => {
                    out.push(c);
                    continue;
                }
            };
            let mut rest = text.as_str();
            while let Some(i) = rest.find("{?") {
                let Some(j) = rest[i + 2..].find('}') else { break };
                if i > 0 {
                    out.push(Node::text_marked(&rest[..i], c.marks.clone()));
                }
                let inner = rest[i + 2..i + 2 + j].trim();
                if !inner.is_empty() {
                    out.push(Node::new(nodes::PLACEHOLDER).attr("text", inner));
                }
                rest = &rest[i + 2 + j + 1..];
            }
            if !rest.is_empty() {
                out.push(Node::text_marked(rest, c.marks.clone()));
            }
        }
        n.content = out;
    }
    for c in &mut n.content {
        expand_placeholders(c);
    }
}

impl Node {
    pub fn new(kind: impl Into<String>) -> Self {
        Self { kind: kind.into(), ..Default::default() }
    }

    pub fn doc(content: Vec<Node>) -> Self {
        Self::new(nodes::DOC).with_content(content)
    }

    pub fn paragraph(content: Vec<Node>) -> Self {
        Self::new(nodes::PARAGRAPH).with_content(content)
    }

    pub fn heading(level: u8, content: Vec<Node>) -> Self {
        Self::new(nodes::HEADING).attr("level", level).with_content(content)
    }

    pub fn text(text: impl Into<String>) -> Self {
        Self { kind: nodes::TEXT.into(), text: Some(text.into()), ..Default::default() }
    }

    pub fn text_marked(text: impl Into<String>, marks: Vec<Mark>) -> Self {
        Self { kind: nodes::TEXT.into(), text: Some(text.into()), marks, ..Default::default() }
    }

    pub fn with_content(mut self, content: Vec<Node>) -> Self {
        self.content = content;
        self
    }

    pub fn attr(mut self, key: &str, value: impl Into<Value>) -> Self {
        let v = value.into();
        if !v.is_null() {
            self.attrs.insert(key.to_string(), v);
        }
        self
    }

    pub fn is_text(&self) -> bool {
        self.kind == nodes::TEXT
    }

    pub fn is_inline(&self) -> bool {
        is_inline(&self.kind)
    }

    pub fn attr_str(&self, key: &str) -> Option<&str> {
        self.attrs.get(key).and_then(Value::as_str)
    }

    pub fn attr_i64(&self, key: &str) -> Option<i64> {
        self.attrs.get(key).and_then(|v| v.as_i64().or_else(|| v.as_f64().map(|f| f as i64)).or_else(|| v.as_str()?.parse().ok()))
    }

    pub fn attr_bool(&self, key: &str) -> Option<bool> {
        self.attrs.get(key).and_then(Value::as_bool)
    }

    /// Depth-first visit of all nodes (including self).
    pub fn walk<'a>(&'a self, f: &mut impl FnMut(&'a Node)) {
        f(self);
        for c in &self.content {
            c.walk(f);
        }
    }

    /// Depth-first mutable visit of all nodes (including self).
    pub fn walk_mut(&mut self, f: &mut impl FnMut(&mut Node)) {
        f(self);
        for c in &mut self.content {
            c.walk_mut(f);
        }
    }

    /// An empty document with one empty paragraph (what editors create).
    pub fn empty_doc() -> Self {
        Self::doc(vec![Self::paragraph(vec![])])
    }

    pub fn to_json(&self) -> Value {
        serde_json::to_value(self).expect("node serializes")
    }

    pub fn from_json(v: &Value) -> Result<Self, serde_json::Error> {
        Node::deserialize(v)
    }
}

impl Mark {
    pub fn new(kind: impl Into<String>) -> Self {
        Self { kind: kind.into(), attrs: Map::new() }
    }

    pub fn attr(mut self, key: &str, value: impl Into<Value>) -> Self {
        let v = value.into();
        if !v.is_null() {
            self.attrs.insert(key.to_string(), v);
        }
        self
    }

    pub fn link(href: impl Into<String>) -> Self {
        Self::new(marks::LINK).attr("href", href.into())
    }

    pub fn attr_str(&self, key: &str) -> Option<&str> {
        self.attrs.get(key).and_then(Value::as_str)
    }
}

/// Normalizes a node list so it is valid as *block* content: runs of inline nodes are wrapped
/// in paragraphs; empty input yields a single empty paragraph if `ensure_non_empty`.
pub fn wrap_inline_runs(content: Vec<Node>, ensure_non_empty: bool) -> Vec<Node> {
    let mut out = Vec::with_capacity(content.len());
    let mut run: Vec<Node> = Vec::new();
    for n in content {
        if n.is_inline() {
            run.push(n);
        } else {
            if !run.is_empty() {
                let r = std::mem::take(&mut run);
                if !is_whitespace_run(&r) {
                    out.push(Node::paragraph(trim_run(r)));
                }
            }
            out.push(n);
        }
    }
    if !run.is_empty() && !is_whitespace_run(&run) {
        out.push(Node::paragraph(trim_run(run)));
    }
    if out.is_empty() && ensure_non_empty {
        out.push(Node::paragraph(vec![]));
    }
    out
}

fn is_whitespace_run(run: &[Node]) -> bool {
    run.iter().all(|n| n.is_text() && n.text.as_deref().is_none_or(|t| t.trim().is_empty()))
}

fn trim_run(mut run: Vec<Node>) -> Vec<Node> {
    if let Some(first) = run.first_mut().filter(|n| n.is_text()) {
        let t = first.text.take().unwrap_or_default();
        first.text = Some(t.trim_start().to_string());
    }
    if let Some(last) = run.last_mut().filter(|n| n.is_text()) {
        let t = last.text.take().unwrap_or_default();
        last.text = Some(t.trim_end().to_string());
    }
    run.retain(|n| !n.is_text() || n.text.as_deref().is_some_and(|t| !t.is_empty()));
    run
}

/// Canonical mark order (links outermost, then comment anchors, then formatting), so documents
/// compare equal regardless of where they came from (Yjs attribute maps are unordered).
pub fn mark_rank(kind: &str) -> u8 {
    match kind {
        marks::LINK => 0,
        marks::COMMENT => 1,
        marks::BOLD => 2,
        marks::ITALIC => 3,
        marks::UNDERLINE => 4,
        marks::STRIKE => 5,
        marks::CODE => 6,
        marks::SUBSCRIPT => 7,
        marks::SUPERSCRIPT => 8,
        marks::TEXT_STYLE => 9,
        marks::HIGHLIGHT => 10,
        _ => 11,
    }
}

pub fn sort_marks(marks: &mut [Mark]) {
    marks.sort_by(|a, b| mark_rank(&a.kind).cmp(&mark_rank(&b.kind)).then_with(|| a.kind.cmp(&b.kind)));
}

/// Merges adjacent text nodes with identical marks and drops empty text nodes.
pub fn normalize_text(content: &mut Vec<Node>) {
    let mut out: Vec<Node> = Vec::with_capacity(content.len());
    for n in content.drain(..) {
        if n.is_text() && n.text.as_deref().is_none_or(str::is_empty) {
            continue;
        }
        if let (Some(prev), true) = (out.last_mut(), n.is_text())
            && prev.is_text()
            && prev.marks == n.marks
        {
            prev.text.get_or_insert_with(String::new).push_str(n.text.as_deref().unwrap_or(""));
            continue;
        }
        out.push(n);
    }
    *content = out;
}

/// Ensures structural validity required by the editor schema (recursive):
/// block containers hold blocks, text blocks hold inline content, lists hold list items, etc.
pub fn sanitize_tree(node: &mut Node) {
    limit_depth(node, 0);
    sanitize_rec(node);
}

/// Replaces subtrees nested deeper than [`MAX_DEPTH`] by a paragraph with their text.
fn limit_depth(node: &mut Node, depth: usize) {
    if depth + 1 >= MAX_DEPTH {
        if node.content.iter().any(|c| !c.content.is_empty()) {
            let mut text = String::new();
            for c in &node.content {
                collect_text(c, &mut text);
                text.push(' ');
            }
            let text = text.split_whitespace().collect::<Vec<_>>().join(" ");
            node.content = if text.is_empty() { vec![] } else { vec![Node::paragraph(vec![Node::text(text)])] };
        }
        return;
    }
    for c in &mut node.content {
        limit_depth(c, depth + 1);
    }
}

fn sanitize_rec(node: &mut Node) {
    use nodes::*;
    sort_marks(&mut node.marks);
    sanitize_urls(node);
    for c in &mut node.content {
        sanitize_rec(c);
    }
    match node.kind.as_str() {
        DOC | BLOCKQUOTE | LIST_ITEM | TASK_ITEM | TABLE_CELL | TABLE_HEADER | CALLOUT | DETAILS_CONTENT | COLUMN | DECISION_ITEM => {
            let c = std::mem::take(&mut node.content);
            node.content = wrap_inline_runs(c, true);
            if matches!(node.kind.as_str(), LIST_ITEM | TASK_ITEM) {
                // list items must start with a paragraph
                if node.content.first().is_some_and(|n| n.kind != PARAGRAPH) {
                    node.content.insert(0, Node::paragraph(vec![]));
                }
            }
        }
        PARAGRAPH | HEADING | DETAILS_SUMMARY => {
            // flatten any block children into inline content
            let c = std::mem::take(&mut node.content);
            let mut flat = Vec::new();
            flatten_inline(c, &mut flat);
            normalize_text(&mut flat);
            node.content = flat;
        }
        CODE_BLOCK => {
            let mut s = String::new();
            for c in &node.content {
                collect_text(c, &mut s);
            }
            node.content = if s.is_empty() { vec![] } else { vec![Node::text(s)] };
        }
        BULLET_LIST | ORDERED_LIST => coerce_children(node, LIST_ITEM),
        TASK_LIST => coerce_children(node, TASK_ITEM),
        DECISION_LIST => coerce_children(node, DECISION_ITEM),
        COLUMNS => coerce_children(node, COLUMN),
        TABLE => {
            node.content.retain(|r| r.kind == TABLE_ROW);
            if node.content.is_empty() {
                node.content.push(Node::new(TABLE_ROW).with_content(vec![Node::new(TABLE_CELL).with_content(vec![Node::paragraph(vec![])])]));
            }
        }
        TABLE_ROW => {
            let c = std::mem::take(&mut node.content);
            node.content = c
                .into_iter()
                .map(|n| if n.kind == TABLE_CELL || n.kind == TABLE_HEADER { n } else { Node::new(TABLE_CELL).with_content(wrap_inline_runs(vec![n], true)) })
                .collect();
            if node.content.is_empty() {
                node.content.push(Node::new(TABLE_CELL).with_content(vec![Node::paragraph(vec![])]));
            }
        }
        DETAILS => {
            let has_summary = node.content.first().is_some_and(|n| n.kind == DETAILS_SUMMARY);
            if !has_summary {
                node.content.insert(0, Node::new(DETAILS_SUMMARY));
            }
            let rest: Vec<Node> = node.content.drain(1..).collect();
            let mut body = Vec::new();
            for n in rest {
                if n.kind == DETAILS_CONTENT {
                    body.extend(n.content);
                } else {
                    body.push(n);
                }
            }
            node.content.push(Node::new(DETAILS_CONTENT).with_content(wrap_inline_runs(body, true)));
        }
        _ => {}
    }
}

fn coerce_children(node: &mut Node, item_kind: &str) {
    let c = std::mem::take(&mut node.content);
    let mut out = Vec::new();
    for n in c {
        if n.kind == item_kind {
            out.push(n);
        } else if let Some(last) = out.last_mut().filter(|_| !n.is_inline()) {
            // nested list or stray block: attach to previous item
            let last: &mut Node = last;
            last.content.push(n);
        } else {
            out.push(Node::new(item_kind).with_content(wrap_inline_runs(vec![n], true)));
        }
    }
    if out.is_empty() {
        out.push(Node::new(item_kind).with_content(vec![Node::paragraph(vec![])]));
    }
    node.content = out;
}

fn flatten_inline(content: Vec<Node>, out: &mut Vec<Node>) {
    for n in content {
        if n.is_inline() {
            out.push(n);
        } else {
            if !out.is_empty() && !n.content.is_empty() {
                out.push(Node::new(nodes::HARD_BREAK));
            }
            flatten_inline(n.content, out);
        }
    }
}

fn collect_text(n: &Node, out: &mut String) {
    if let Some(t) = &n.text {
        out.push_str(t);
    } else if n.kind == nodes::HARD_BREAK {
        out.push('\n');
    }
    for c in &n.content {
        collect_text(c, out);
    }
}

/// Drops URLs with dangerous schemes (`javascript:`, `data:`, `vbscript:`, …) from links,
/// images, attachments and embeds of a single node (not recursive).
pub fn sanitize_urls(node: &mut Node) {
    // `wikilink:`/`wcimport:` are inert importer placeholders that get rewritten to real links
    let ok = |u: &str| crate::html::safe_url(u).is_some() || u.starts_with("wikilink:") || u.starts_with("wcimport:");
    node.marks.retain(|m| m.kind != marks::LINK || m.attr_str("href").is_some_and(ok));
    let key = match node.kind.as_str() {
        nodes::IMAGE => Some("src"),
        nodes::ATTACHMENT => Some("href"),
        nodes::EMBED => Some("url"),
        _ => None,
    };
    if let Some(k) = key
        && node.attr_str(k).is_some_and(|u| !ok(u))
    {
        node.attrs.remove(k);
    }
}
