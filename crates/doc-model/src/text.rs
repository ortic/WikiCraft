//! Plain-text extraction (search indexing, excerpts, AI chunking).

use crate::node::{Node, nodes};

/// Concatenated text of a node's inline content (no block separators).
pub fn inline_text(n: &Node) -> String {
    let mut s = String::new();
    fn go(n: &Node, s: &mut String) {
        if let Some(t) = &n.text {
            s.push_str(t);
        }
        match n.kind.as_str() {
            nodes::HARD_BREAK => s.push('\n'),
            nodes::MENTION => {
                s.push('@');
                s.push_str(n.attr_str("label").unwrap_or(""));
            }
            nodes::STATUS => s.push_str(n.attr_str("text").unwrap_or("")),
            nodes::DATE => s.push_str(n.attr_str("date").unwrap_or("")),
            _ => {}
        }
        for c in &n.content {
            go(c, s);
        }
    }
    go(n, &mut s);
    s
}

/// Full plain text of a document; blocks separated by newlines.
pub fn plain_text(doc: &Node) -> String {
    let mut out = String::new();
    block_text(doc, &mut out);
    // collapse 3+ newlines
    let mut res = String::with_capacity(out.len());
    let mut nl = 0;
    for ch in out.chars() {
        if ch == '\n' {
            nl += 1;
            if nl > 2 {
                continue;
            }
        } else {
            nl = 0;
        }
        res.push(ch);
    }
    res.trim().to_string()
}

fn block_text(n: &Node, out: &mut String) {
    if n.content.iter().all(|c| c.is_inline()) && n.kind != nodes::DOC {
        let t = inline_text(n);
        let extra = match n.kind.as_str() {
            nodes::IMAGE => n.attr_str("alt").unwrap_or("").to_string(),
            nodes::ATTACHMENT => n.attr_str("name").unwrap_or("").to_string(),
            nodes::MATH_BLOCK => n.attr_str("latex").unwrap_or("").to_string(),
            nodes::EMBED => n.attr_str("url").unwrap_or("").to_string(),
            nodes::MACRO => n.attr_str("bodyText").unwrap_or("").to_string(),
            nodes::DRAWING => drawing_text(n),
            _ => String::new(),
        };
        if !t.is_empty() {
            out.push_str(&t);
            out.push('\n');
        }
        if !extra.is_empty() {
            out.push_str(&extra);
            out.push('\n');
        }
        return;
    }
    if n.kind == nodes::TABLE_ROW {
        let cells: Vec<String> = n.content.iter().map(|c| plain_text(c).replace('\n', " ")).collect();
        out.push_str(&cells.join(" | "));
        out.push('\n');
        return;
    }
    for c in &n.content {
        block_text(c, out);
    }
    if !matches!(n.kind.as_str(), nodes::DOC | nodes::LIST_ITEM | nodes::TASK_ITEM) {
        out.push('\n');
    }
}

/// A short excerpt (first `max_chars` characters of the plain text, on a word boundary).
pub fn excerpt(doc: &Node, max_chars: usize) -> String {
    let t = plain_text(doc).replace('\n', " ");
    if t.chars().count() <= max_chars {
        return t;
    }
    let cut: String = t.chars().take(max_chars).collect();
    let cut = match cut.rfind(' ') {
        Some(i) if i > max_chars / 2 => cut[..i].to_string(),
        _ => cut,
    };
    format!("{cut}…")
}

/// Headings (level, text, anchor id) in document order, for tables of contents.
pub fn headings(doc: &Node) -> Vec<(u8, String, String)> {
    let mut out = Vec::new();
    let mut used = std::collections::HashMap::<String, usize>::new();
    doc.walk(&mut |n| {
        if n.kind == nodes::HEADING {
            let text = inline_text(n);
            let base = slugify(&text);
            let count = used.entry(base.clone()).or_default();
            let id = if *count == 0 { base.clone() } else { format!("{base}-{count}") };
            *count += 1;
            out.push((n.attr_i64("level").unwrap_or(1).clamp(1, 6) as u8, text, id));
        }
    });
    out
}

/// URL-friendly slug (lowercase, ASCII alphanumerics and dashes; unicode letters kept).
pub fn slugify(s: &str) -> String {
    let mut out = String::new();
    let mut dash = false;
    for ch in s.chars().flat_map(char::to_lowercase) {
        if ch.is_alphanumeric() {
            out.push(ch);
            dash = false;
        } else if !dash && !out.is_empty() {
            out.push('-');
            dash = true;
        }
    }
    while out.ends_with('-') {
        out.pop();
    }
    if out.is_empty() { "section".into() } else { out }
}

/// Number of words in a document (whitespace separated).
pub fn word_count(doc: &Node) -> usize {
    plain_text(doc).split_whitespace().count()
}

/// The text written in a drawing (its text elements), for search.
pub fn drawing_text(n: &Node) -> String {
    if let Some(xml) = n.attr_str("scene").filter(|s| s.trim_start().starts_with('<')) {
        return drawio_text(xml);
    }
    let Some(scene) = n.attr_str("scene").and_then(|s| serde_json::from_str::<serde_json::Value>(s).ok()) else { return String::new() };
    scene["elements"]
        .as_array()
        .map(|els| {
            els.iter().filter(|e| e["type"] == "text" && e["isDeleted"] != true).filter_map(|e| e["text"].as_str()).take(2_000).collect::<Vec<_>>().join(" ")
        })
        .unwrap_or_default()
}

/// The labels of a draw.io diagram (`value` attributes of its cells, which may hold HTML).
/// Compressed diagrams have none to read.
fn drawio_text(xml: &str) -> String {
    let mut out: Vec<String> = Vec::new();
    let mut rest = xml;
    while let Some(i) = rest.find(" value=\"") {
        rest = &rest[i + 8..];
        let Some(end) = rest.find('"') else { break };
        let raw = &rest[..end];
        rest = &rest[end..];
        // the attribute is XML-escaped; HTML labels are HTML inside it (escaped once more)
        let unescape = |s: &str| {
            s.replace("&lt;", "<")
                .replace("&gt;", ">")
                .replace("&quot;", "\"")
                .replace("&#xa;", " ")
                .replace("&#10;", " ")
                .replace("&nbsp;", " ")
                .replace("&amp;", "&")
        };
        let unescaped = unescape(raw);
        let mut text = String::with_capacity(unescaped.len());
        let mut in_tag = false;
        for c in unescaped.chars() {
            match c {
                '<' => {
                    in_tag = true;
                    text.push(' ');
                }
                '>' => in_tag = false,
                c if !in_tag => text.push(c),
                _ => {}
            }
        }
        let text = unescape(&text).split_whitespace().collect::<Vec<_>>().join(" ");
        if !text.is_empty() && out.len() < 2_000 {
            out.push(text);
        }
    }
    out.join(" ")
}

#[cfg(test)]
mod tests {
    #[test]
    fn drawio_labels_are_searchable() {
        let xml = r#"<mxfile><diagram><mxGraphModel><root><mxCell id="0"/><mxCell id="2" value="Load &lt;b&gt;balancer&lt;/b&gt;" vertex="1"/><mxCell id="3" value="" edge="1"/><mxCell id="4" value="DB &amp;amp; cache"/></root></mxGraphModel></diagram></mxfile>"#;
        let n = crate::node::Node::new(crate::node::nodes::DRAWING).attr("kind", "drawio").attr("scene", xml);
        assert_eq!(super::drawing_text(&n), "Load balancer DB & cache");
    }

    #[test]
    fn placeholders_are_for_authors_only() {
        use crate::node::{Node, expand_placeholders, nodes};
        let mut doc = crate::markdown::markdown_to_pm("## Context\n\n{?What problem are we solving? Who's affected?} Then text.\n").doc;
        expand_placeholders(&mut doc);
        let p = &doc.content[1];
        assert_eq!(p.content[0].kind, nodes::PLACEHOLDER);
        assert_eq!(p.content[0].attr_str("text"), Some("What problem are we solving? Who's affected?"));
        assert_eq!(p.content[1].text.as_deref(), Some(" Then text."));
        let page = Node::doc(doc.content.clone());
        assert!(!super::plain_text(&page).contains("problem"), "not indexed");
        assert!(!crate::html::render(&page, &crate::html::HtmlOptions { heading_ids: false }).contains("problem"), "not rendered for readers");
        assert!(!crate::markdown::pm_to_markdown(&page).contains("problem"), "not exported");
    }

    use super::*;

    #[test]
    fn plain_text_and_headings() {
        let doc = Node::doc(vec![
            Node::heading(1, vec![Node::text("Über uns")]),
            Node::paragraph(vec![Node::text("Hello"), Node::new(nodes::HARD_BREAK), Node::text("World")]),
            Node::heading(2, vec![Node::text("Über uns")]),
        ]);
        assert_eq!(plain_text(&doc), "Über uns\nHello\nWorld\nÜber uns");
        let h = headings(&doc);
        assert_eq!(h[0].2, "über-uns");
        assert_eq!(h[1].2, "über-uns-1");
        assert_eq!(word_count(&doc), 6);
    }

    #[test]
    fn excerpt_cuts_on_word() {
        let doc = Node::doc(vec![Node::paragraph(vec![Node::text("alpha beta gamma delta")])]);
        assert_eq!(excerpt(&doc, 12), "alpha beta…");
    }

    #[test]
    fn drawing_text_is_searchable() {
        let scene =
            r#"{"type":"excalidraw","elements":[{"type":"rectangle"},{"type":"text","text":"Load balancer"},{"type":"text","text":"gone","isDeleted":true}]}"#;
        let n = Node::new(nodes::DRAWING).attr("scene", scene).attr("preview", "/api/attachments/x/content");
        assert_eq!(super::drawing_text(&n), "Load balancer");
        assert!(super::plain_text(&Node::doc(vec![n])).contains("Load balancer"));
    }
}
