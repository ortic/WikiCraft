//! Minimal converter for legacy Confluence wiki markup (Confluence ≤ 3.x bodies, space
//! descriptions and the `unmigrated-wiki-markup` macro).
//!
//! Supported: `h1.`–`h6.`, `*`/`#` lists (nested), `||header||`/`|cell|` tables, `{code}`,
//! `{noformat}`, `{quote}`, `bq.`, `----`, `{info}`/`{note}`/`{tip}`/`{warning}`/`{panel}`,
//! inline `*bold*`, `_italic_`, `-strike-`, `+underline+`, `{{mono}}`, `^sup^`, `~sub~`,
//! `[text|url]`, `[url]`, `[~user]`, `!image.png!`, `\\` line breaks.

use wikicraft_doc_model::node::{sanitize_tree, wrap_inline_runs};
use wikicraft_doc_model::{Mark, Node, marks, nodes};

pub fn wiki_to_pm(src: &str) -> Node {
    let split = split_block_macros(src);
    let lines: Vec<&str> = split.iter().map(String::as_str).collect();
    let mut i = 0;
    let content = parse_blocks(&lines, &mut i, None);
    let mut doc = Node::doc(wrap_inline_runs(content, true));
    sanitize_tree(&mut doc);
    doc
}

const BLOCK_MACROS: &[&str] = &["code", "noformat", "quote", "info", "note", "tip", "warning", "panel", "expand"];

/// The next block macro tag (`{info}`, `{code:java}`, …) in `s`: (start, end, name). Inside a
/// `{code}` / `{noformat}` body only its closing tag counts; `{{mono}}` is never a tag.
fn next_tag(s: &str, inside: Option<&str>) -> Option<(usize, usize, String)> {
    if let Some(name) = inside {
        let close = format!("{{{name}}}");
        let start = s.to_ascii_lowercase().find(&close)?;
        return Some((start, start + close.len(), name.to_string()));
    }
    let b = s.as_bytes();
    let mut from = 0;
    while let Some(off) = s[from..].find('{') {
        let start = from + off;
        from = start + 1;
        if b.get(start + 1) == Some(&b'{') || (start > 0 && b[start - 1] == b'{') {
            from = start + 2;
            continue;
        }
        let Some(close) = s[start..].find('}') else { break };
        let inner = &s[start + 1..start + close];
        let name = inner.split(':').next().unwrap_or("").to_ascii_lowercase();
        if BLOCK_MACROS.contains(&name.as_str()) {
            return Some((start, start + close + 1, name));
        }
    }
    None
}

/// Puts every block macro tag on a line of its own, so `{info}Short note.{info}` and
/// `{code}x = 1{code}` parse like their multi-line forms. Blank lines (paragraph breaks) and
/// code bodies are kept as they are.
fn split_block_macros(src: &str) -> Vec<String> {
    let mut out = Vec::new();
    let mut inside: Option<String> = None;
    for line in src.lines() {
        let mut rest = line;
        let mut split = false;
        while let Some((start, end, name)) = next_tag(rest, inside.as_deref()) {
            let before = &rest[..start];
            if !before.trim().is_empty() {
                out.push(before.to_string());
            }
            out.push(rest[start..end].to_string());
            inside = match inside {
                Some(_) => None,
                None if name == "code" || name == "noformat" => Some(name),
                None => None,
            };
            rest = &rest[end..];
            split = true;
        }
        if !split || !rest.trim().is_empty() {
            out.push(rest.to_string());
        }
    }
    out
}

fn block_macro_start(line: &str) -> Option<(String, Option<String>)> {
    let l = line.trim();
    let rest = l.strip_prefix('{')?;
    let end = rest.find('}')?;
    let inner = &rest[..end];
    let (name, params) = match inner.split_once(':') {
        Some((n, p)) => (n, Some(p.to_string())),
        None => (inner, None),
    };
    let name = name.to_ascii_lowercase();
    BLOCK_MACROS.contains(&name.as_str()).then_some((name, params))
}

fn parse_blocks(lines: &[&str], i: &mut usize, until: Option<&str>) -> Vec<Node> {
    let mut out = Vec::new();
    let mut para: Vec<String> = Vec::new();
    let flush = |para: &mut Vec<String>, out: &mut Vec<Node>| {
        if !para.is_empty() {
            let text = para.join("\n");
            out.push(Node::paragraph(inline(&text)));
            para.clear();
        }
    };
    while *i < lines.len() {
        let line = lines[*i];
        let t = line.trim();
        if let Some(end) = until
            && t.eq_ignore_ascii_case(&format!("{{{end}}}"))
        {
            *i += 1;
            flush(&mut para, &mut out);
            return out;
        }
        if t.is_empty() {
            flush(&mut para, &mut out);
            *i += 1;
            continue;
        }
        if let Some((name, params)) = block_macro_start(t) {
            flush(&mut para, &mut out);
            *i += 1;
            match name.as_str() {
                "code" | "noformat" => {
                    let mut body = Vec::new();
                    while *i < lines.len() && !lines[*i].trim().eq_ignore_ascii_case(&format!("{{{name}}}")) {
                        body.push(lines[*i]);
                        *i += 1;
                    }
                    *i += 1;
                    let lang = params.as_deref().and_then(|p| {
                        p.split('|').find(|kv| !kv.contains('=') || kv.starts_with("language=")).map(|s| s.trim_start_matches("language=").to_string())
                    });
                    let text = body.join("\n");
                    out.push(Node::new(nodes::CODE_BLOCK).attr("language", lang).with_content(if text.is_empty() { vec![] } else { vec![Node::text(text)] }));
                }
                "quote" => out.push(Node::new(nodes::BLOCKQUOTE).with_content(parse_blocks(lines, i, Some("quote")))),
                "expand" => {
                    let inner = parse_blocks(lines, i, Some("expand"));
                    out.push(Node::new(nodes::DETAILS).with_content(vec![
                        Node::new(nodes::DETAILS_SUMMARY).with_content(vec![Node::text(params.unwrap_or_else(|| "Expand".into()))]),
                        Node::new(nodes::DETAILS_CONTENT).with_content(inner),
                    ]));
                }
                other => {
                    let kind = match other {
                        "tip" => "success",
                        "note" => "warning",
                        "warning" => "error",
                        "panel" => "note",
                        _ => "info",
                    };
                    let inner = parse_blocks(lines, i, Some(other));
                    out.push(Node::new(nodes::CALLOUT).attr("type", kind).with_content(inner));
                }
            }
            continue;
        }
        if let Some((lvl, rest)) = heading(t) {
            flush(&mut para, &mut out);
            out.push(Node::heading(lvl, inline(rest)));
            *i += 1;
            continue;
        }
        if let Some(rest) = t.strip_prefix("bq. ") {
            flush(&mut para, &mut out);
            out.push(Node::new(nodes::BLOCKQUOTE).with_content(vec![Node::paragraph(inline(rest))]));
            *i += 1;
            continue;
        }
        if t == "----" {
            flush(&mut para, &mut out);
            out.push(Node::new(nodes::HORIZONTAL_RULE));
            *i += 1;
            continue;
        }
        if t.starts_with('|') {
            flush(&mut para, &mut out);
            let mut rows = Vec::new();
            while *i < lines.len() && lines[*i].trim().starts_with('|') {
                rows.push(table_row(lines[*i].trim()));
                *i += 1;
            }
            out.push(Node::new(nodes::TABLE).with_content(rows));
            continue;
        }
        if list_marker(t).is_some() {
            flush(&mut para, &mut out);
            let mut items: Vec<(String, String)> = Vec::new();
            while *i < lines.len() {
                let lt = lines[*i].trim();
                match list_marker(lt) {
                    // `*` and `#` items at the top level are separate lists
                    Some((m, _)) if items.first().is_some_and(|(f, _)| f.as_bytes()[0] != m.as_bytes()[0]) => break,
                    Some((m, rest)) => items.push((m.to_string(), rest.to_string())),
                    None => break,
                }
                *i += 1;
            }
            out.push(build_list(&items, 0));
            continue;
        }
        para.push(line.to_string());
        *i += 1;
    }
    flush(&mut para, &mut out);
    out
}

fn heading(t: &str) -> Option<(u8, &str)> {
    let b = t.as_bytes();
    if b.len() >= 4 && b[0] == b'h' && (b'1'..=b'6').contains(&b[1]) && b[2] == b'.' && b[3] == b' ' {
        return Some((b[1] - b'0', &t[4..]));
    }
    None
}

fn list_marker(t: &str) -> Option<(&str, &str)> {
    let end = t.find(|c: char| c != '*' && c != '#' && c != '-')?;
    if end == 0 || !t[end..].starts_with(' ') {
        return None;
    }
    let marker = &t[..end];
    if marker.starts_with('-') && marker.len() > 1 {
        return None;
    }
    if marker == "----" {
        return None;
    }
    Some((marker, t[end..].trim_start()))
}

fn build_list(items: &[(String, String)], depth: usize) -> Node {
    let ordered = items.first().is_some_and(|(m, _)| m.as_bytes().get(depth) == Some(&b'#'));
    let mut list = Node::new(if ordered { nodes::ORDERED_LIST } else { nodes::BULLET_LIST });
    let mut idx = 0;
    while idx < items.len() {
        let (m, text) = &items[idx];
        if m.len() == depth + 1 {
            let mut li = Node::new(nodes::LIST_ITEM).with_content(vec![Node::paragraph(inline(text))]);
            idx += 1;
            let start = idx;
            while idx < items.len() && items[idx].0.len() > depth + 1 {
                idx += 1;
            }
            if idx > start {
                li.content.push(build_list(&items[start..idx], depth + 1));
            }
            list.content.push(li);
        } else {
            // deeper item without parent: attach as nested list to a synthetic item
            let start = idx;
            while idx < items.len() && items[idx].0.len() > depth + 1 {
                idx += 1;
            }
            list.content.push(Node::new(nodes::LIST_ITEM).with_content(vec![Node::paragraph(vec![]), build_list(&items[start..idx], depth + 1)]));
        }
    }
    list
}

fn table_row(line: &str) -> Node {
    let mut cells = Vec::new();
    let mut rest = line;
    while !rest.is_empty() {
        let header = rest.starts_with("||");
        rest = rest.trim_start_matches('|');
        if rest.is_empty() {
            break;
        }
        let end = rest.find('|').unwrap_or(rest.len());
        let text = rest[..end].trim();
        rest = &rest[end..];
        cells.push(Node::new(if header { nodes::TABLE_HEADER } else { nodes::TABLE_CELL }).with_content(vec![Node::paragraph(inline(text))]));
    }
    Node::new(nodes::TABLE_ROW).with_content(cells)
}

/// Inline wiki markup → inline nodes.
pub fn inline(s: &str) -> Vec<Node> {
    let mut out = Vec::new();
    parse_inline(s, &[], &mut out);
    wikicraft_doc_model::node::normalize_text(&mut out);
    out
}

fn delim_mark(c: char) -> Option<&'static str> {
    match c {
        '*' => Some(marks::BOLD),
        '_' => Some(marks::ITALIC),
        '-' => Some(marks::STRIKE),
        '+' => Some(marks::UNDERLINE),
        '^' => Some(marks::SUPERSCRIPT),
        '~' => Some(marks::SUBSCRIPT),
        _ => None,
    }
}

fn parse_inline(s: &str, active: &[Mark], out: &mut Vec<Node>) {
    let chars: Vec<char> = s.chars().collect();
    let mut buf = String::new();
    let mut i = 0;
    let flush = |buf: &mut String, out: &mut Vec<Node>| {
        if !buf.is_empty() {
            out.push(Node::text_marked(std::mem::take(buf), active.to_vec()));
        }
    };
    while i < chars.len() {
        let c = chars[i];
        // line break
        if c == '\\' && chars.get(i + 1) == Some(&'\\') {
            flush(&mut buf, out);
            out.push(Node::new(nodes::HARD_BREAK));
            i += 2;
            continue;
        }
        if c == '\n' {
            buf.push(' ');
            i += 1;
            continue;
        }
        // {{monospace}}
        if c == '{'
            && chars.get(i + 1) == Some(&'{')
            && let Some(end) = find_seq(&chars, i + 2, &['}', '}'])
        {
            flush(&mut buf, out);
            let inner: String = chars[i + 2..end].iter().collect();
            let mut m = active.to_vec();
            m.push(Mark::new(marks::CODE));
            out.push(Node::text_marked(inner, m));
            i = end + 2;
            continue;
        }
        // [link]
        if c == '['
            && let Some(end) = chars[i + 1..].iter().position(|&x| x == ']').map(|p| p + i + 1)
        {
            let inner: String = chars[i + 1..end].iter().collect();
            flush(&mut buf, out);
            let (text, target) = match inner.split_once('|') {
                Some((t, u)) => (t.to_string(), u.trim().to_string()),
                None => (inner.clone(), inner.trim().to_string()),
            };
            if let Some(user) = target.strip_prefix('~') {
                out.push(Node::new(nodes::MENTION).attr("id", format!("wcimport:user/{user}")).attr("label", user.to_string()));
            } else {
                let href = if target.contains("://") || target.starts_with("mailto:") || target.starts_with('#') {
                    target.clone()
                } else {
                    // page link: "SPACE:Title" or "Title"
                    let (space, title) = target.split_once(':').unwrap_or(("", target.as_str()));
                    format!("wcimport:wikilink/{}/{}", space, title)
                };
                let mut m = active.to_vec();
                m.push(Mark::link(href));
                out.push(Node::text_marked(if text.is_empty() { target } else { text }, m));
            }
            i = end + 1;
            continue;
        }
        // !image!
        if c == '!'
            && let Some(end) = chars[i + 1..].iter().position(|&x| x == '!').map(|p| p + i + 1)
        {
            let inner: String = chars[i + 1..end].iter().collect();
            if !inner.contains(' ') && !inner.is_empty() && inner.contains('.') {
                flush(&mut buf, out);
                let src = inner.split('|').next().unwrap_or("").to_string();
                let src = if src.contains("://") { src } else { format!("wcimport:missing-attachment/{src}") };
                out.push(Node::new(nodes::IMAGE).attr("src", src));
                i = end + 1;
                continue;
            }
        }
        // *bold* etc. (must start at word boundary)
        if let Some(mark) = delim_mark(c) {
            let prev_ok = i == 0 || !chars[i - 1].is_alphanumeric();
            if prev_ok
                && chars.get(i + 1).is_some_and(|n| !n.is_whitespace())
                && let Some(end) =
                    (i + 1..chars.len()).find(|&j| chars[j] == c && !chars[j - 1].is_whitespace() && chars.get(j + 1).is_none_or(|n| !n.is_alphanumeric()))
            {
                flush(&mut buf, out);
                let inner: String = chars[i + 1..end].iter().collect();
                let mut m = active.to_vec();
                m.push(Mark::new(mark));
                parse_inline(&inner, &m, out);
                i = end + 1;
                continue;
            }
        }
        buf.push(c);
        i += 1;
    }
    flush(&mut buf, out);
}

fn find_seq(chars: &[char], from: usize, seq: &[char]) -> Option<usize> {
    (from..chars.len().saturating_sub(seq.len() - 1)).find(|&j| chars[j..].starts_with(seq))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn converts_common_markup() {
        let doc = wiki_to_pm(
            "h1. Title\n\nSome *bold* and _it_ {{code}} [Google|https://google.com]\n\n* a\n** nested\n* b\n\n||h1||h2||\n|c1|c2|\n\n{code:java}\nint x;\n{code}\n\n{info}\nnote\n{info}",
        );
        let kinds: Vec<&str> = doc.content.iter().map(|n| n.kind.as_str()).collect();
        assert_eq!(kinds, vec!["heading", "paragraph", "bulletList", "table", "codeBlock", "callout"]);
        let list = &doc.content[2];
        assert_eq!(list.content[0].content[1].kind, "bulletList");
        assert_eq!(doc.content[4].attr_str("language"), Some("java"));
        let p = serde_json::to_string(&doc.content[1]).unwrap();
        assert!(p.contains("\"bold\"") && p.contains("https://google.com") && p.contains("\"code\""));
    }

    #[test]
    fn one_line_block_macros_and_mixed_lists() {
        let doc = wiki_to_pm(
            "{info}Ask in #help.{info}

after

{code}x = {y}{code}
* bullet
# one
# two

See {{code}} here.

{note}start
more{note}",
        );
        let kinds: Vec<&str> = doc.content.iter().map(|n| n.kind.as_str()).collect();
        assert_eq!(kinds, ["callout", "paragraph", "codeBlock", "bulletList", "orderedList", "paragraph", "callout"]);
        let text = |n: &Node| wikicraft_doc_model::text::plain_text(n);
        assert_eq!(text(&doc.content[0]), "Ask in #help.");
        assert_eq!(text(&doc.content[1]), "after", "the callout ends where its closing tag is");
        assert_eq!(text(&doc.content[2]), "x = {y}");
        assert_eq!(doc.content[4].content.len(), 2);
        assert!(serde_json::to_string(&doc.content[5]).unwrap().contains("\"code\""), "{{mono}} stays inline");
        assert_eq!(text(&doc.content[6]), "start more");
    }
}
