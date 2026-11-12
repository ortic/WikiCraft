//! Markdown ⇄ ProseMirror JSON.
//!
//! * Import: GFM (tables, task lists, strikethrough, autolinks), GitHub alerts (`> [!NOTE]`),
//!   `$math$`, Obsidian `[[wikilinks]]` (as `wikilink:` hrefs for the importer to resolve),
//!   front matter (returned separately).
//! * Export: GFM; nodes without a Markdown equivalent degrade to readable text / HTML comments.

use comrak::nodes::{AlertType, ListType, NodeValue, TableAlignment};
use comrak::{Arena, Options, parse_document};

use crate::node::{MAX_DEPTH, Mark, Node, marks, nodes, normalize_text, sanitize_tree, wrap_inline_runs};

/// Result of importing a Markdown document.
#[derive(Debug, Clone, PartialEq)]
pub struct MarkdownImport {
    pub doc: Node,
    /// Raw front matter (without delimiters), if present.
    pub front_matter: Option<String>,
    /// First level-1 heading text, useful as page title.
    pub title: Option<String>,
}

fn options() -> Options<'static> {
    let mut o = Options::default();
    o.extension.strikethrough = true;
    o.extension.table = true;
    o.extension.autolink = true;
    o.extension.tasklist = true;
    o.extension.superscript = false;
    o.extension.footnotes = false;
    o.extension.alerts = true;
    o.extension.math_dollars = true;
    o.extension.math_code = true;
    o.extension.wikilinks_title_after_pipe = true;
    o.extension.underline = false;
    o.extension.subscript = false;
    o.extension.highlight = true;
    o.extension.front_matter_delimiter = Some("---".to_string());
    o
}

/// Parses Markdown into a ProseMirror document.
pub fn markdown_to_pm(md: &str) -> MarkdownImport {
    let arena = Arena::new();
    let opts = options();
    let root = parse_document(&arena, md, &opts);
    let mut front_matter = None;
    for child in root.children() {
        if let NodeValue::FrontMatter(fm) = &child.data.borrow().value {
            let fm = fm.trim().trim_start_matches("---").trim_end_matches("---").trim().to_string();
            front_matter = Some(fm);
        }
    }
    let content = child_blocks(root, 0);
    let mut doc = Node::doc(wrap_inline_runs(content, true));
    sanitize_tree(&mut doc);
    let title = doc.content.iter().find(|n| n.kind == nodes::HEADING && n.attr_i64("level") == Some(1)).map(crate::text::inline_text);
    MarkdownImport { doc, front_matter, title }
}

type Ast<'a> = comrak::nodes::Node<'a>;

/// All text below `n`, collected iteratively (used when nesting is too deep).
fn flat_text<'a>(n: Ast<'a>) -> String {
    let mut s = String::new();
    for d in n.descendants() {
        match &d.data.borrow().value {
            NodeValue::Text(t) => s.push_str(t),
            NodeValue::Code(c) => s.push_str(&c.literal),
            NodeValue::CodeBlock(c) => s.push_str(&c.literal),
            NodeValue::SoftBreak | NodeValue::LineBreak | NodeValue::Paragraph => s.push(' '),
            _ => {}
        }
    }
    s.split_whitespace().collect::<Vec<_>>().join(" ")
}

fn block<'a>(n: Ast<'a>, depth: usize) -> Vec<Node> {
    if depth > MAX_DEPTH {
        let t = flat_text(n);
        return if t.is_empty() { vec![] } else { vec![Node::paragraph(vec![Node::text(t)])] };
    }
    let value = n.data.borrow().value.clone();
    match value {
        NodeValue::Paragraph => {
            let inl = inlines(n, &[], 0);
            // `$$ … $$` on lines of their own: a math block, not display math inside a paragraph
            if let [m] = inl.as_slice()
                && m.kind == nodes::MATH_INLINE
                && m.attr_bool("display") == Some(true)
            {
                return vec![Node::new(nodes::MATH_BLOCK).attr("latex", m.attr_str("latex").unwrap_or("").trim())];
            }
            vec![Node::paragraph(inl)]
        }
        NodeValue::Heading(h) => vec![Node::heading(h.level.clamp(1, 6), inlines(n, &[], 0))],
        NodeValue::BlockQuote | NodeValue::MultilineBlockQuote(_) => {
            vec![Node::new(nodes::BLOCKQUOTE).with_content(blocks(n, depth))]
        }
        NodeValue::Alert(a) => {
            let kind = match a.alert_type {
                AlertType::Note => "info",
                AlertType::Tip => "success",
                AlertType::Important => "note",
                AlertType::Warning => "warning",
                AlertType::Caution => "error",
            };
            let mut c = Vec::new();
            if let Some(t) = a.title.filter(|t| !t.is_empty()) {
                c.push(Node::paragraph(vec![Node::text_marked(t, vec![Mark::new(marks::BOLD)])]));
            }
            c.extend(blocks(n, depth));
            vec![Node::new(nodes::CALLOUT).attr("type", kind).with_content(c)]
        }
        NodeValue::List(l) => {
            let is_task = l.is_task_list || n.children().any(|c| matches!(c.data.borrow().value, NodeValue::TaskItem(_)));
            let items: Vec<Node> = n
                .children()
                .map(|item| {
                    let checked = match &item.data.borrow().value {
                        NodeValue::TaskItem(t) => Some(t.symbol.is_some()),
                        _ => None,
                    };
                    if is_task {
                        Node::new(nodes::TASK_ITEM).attr("checked", checked.unwrap_or(false)).with_content(blocks(item, depth))
                    } else {
                        Node::new(nodes::LIST_ITEM).with_content(blocks(item, depth))
                    }
                })
                .collect();
            if is_task {
                vec![Node::new(nodes::TASK_LIST).with_content(items)]
            } else if l.list_type == ListType::Ordered {
                vec![Node::new(nodes::ORDERED_LIST).attr("start", l.start as i64).with_content(items)]
            } else {
                vec![Node::new(nodes::BULLET_LIST).with_content(items)]
            }
        }
        NodeValue::CodeBlock(cb) => {
            let lang = cb.info.split_whitespace().next().unwrap_or("").to_string();
            let mut lit = cb.literal.clone();
            if lit.ends_with('\n') {
                lit.pop();
            }
            if lang == "math" {
                return vec![Node::new(nodes::MATH_BLOCK).attr("latex", lit)];
            }
            if lang == "mermaid" {
                return vec![Node::new(nodes::CODE_BLOCK).attr("language", "mermaid").with_content(text_or_empty(lit))];
            }
            vec![
                Node::new(nodes::CODE_BLOCK)
                    .attr("language", if lang.is_empty() { serde_json::Value::Null } else { lang.into() })
                    .with_content(text_or_empty(lit)),
            ]
        }
        NodeValue::ThematicBreak => vec![Node::new(nodes::HORIZONTAL_RULE)],
        NodeValue::Table(t) => {
            let aligns = t.alignments.clone();
            let rows = n
                .children()
                .map(|row| {
                    let header = matches!(row.data.borrow().value, NodeValue::TableRow(true));
                    let cells = row
                        .children()
                        .enumerate()
                        .map(|(i, cell)| {
                            let mut para = Node::paragraph(inlines(cell, &[], 0));
                            match aligns.get(i) {
                                Some(TableAlignment::Center) => para = para.attr("textAlign", "center"),
                                Some(TableAlignment::Right) => para = para.attr("textAlign", "right"),
                                _ => {}
                            }
                            Node::new(if header { nodes::TABLE_HEADER } else { nodes::TABLE_CELL }).with_content(vec![para])
                        })
                        .collect();
                    Node::new(nodes::TABLE_ROW).with_content(cells)
                })
                .collect();
            vec![Node::new(nodes::TABLE).with_content(rows)]
        }
        NodeValue::HtmlBlock(h) => {
            // keep raw HTML visible as code so nothing is silently lost
            let lit = h.literal.trim_end().to_string();
            if lit.starts_with("<!--") {
                return vec![];
            }
            vec![Node::new(nodes::CODE_BLOCK).attr("language", "html").with_content(text_or_empty(lit))]
        }
        NodeValue::Math(m) => vec![Node::new(nodes::MATH_BLOCK).attr("latex", m.literal)],
        NodeValue::Item(_) | NodeValue::TaskItem(_) | NodeValue::Document => blocks(n, depth),
        NodeValue::FrontMatter(_) => vec![],
        // anything else that is block-level: try to keep its inline text
        _ => {
            let inl = inlines(n, &[], 0);
            if inl.is_empty() { blocks(n, depth) } else { vec![Node::paragraph(inl)] }
        }
    }
}

fn text_or_empty(s: String) -> Vec<Node> {
    if s.is_empty() { vec![] } else { vec![Node::text(s)] }
}

fn blocks<'a>(n: Ast<'a>, depth: usize) -> Vec<Node> {
    wrap_inline_runs(child_blocks(n, depth + 1), true)
}

/// The blocks of `n`'s children; `<details><summary>…</summary>` … `</details>` HTML (what our
/// export writes for expand blocks, and GitHub's way to fold content) becomes an expand block.
fn child_blocks<'a>(n: Ast<'a>, depth: usize) -> Vec<Node> {
    let children: Vec<Ast<'a>> = n.children().collect();
    slice_blocks(&children, depth)
}

fn slice_blocks<'a>(children: &[Ast<'a>], depth: usize) -> Vec<Node> {
    let html = |c: &Ast<'a>| match &c.data.borrow().value {
        NodeValue::HtmlBlock(h) => Some(h.literal.trim().to_string()),
        _ => None,
    };
    let mut out = Vec::new();
    let mut i = 0;
    while i < children.len() {
        let c = children[i];
        i += 1;
        if matches!(c.data.borrow().value, NodeValue::FrontMatter(_)) {
            continue;
        }
        let lit = html(&c);
        let Some((summary, open, rest)) = lit.as_deref().and_then(details_open) else {
            out.extend(block(c, depth));
            continue;
        };
        if let Some(inner) = rest.strip_suffix("</details>") {
            // all in one HTML block (no blank lines inside)
            let body = if depth < MAX_DEPTH { markdown_to_pm(inner).doc.content } else { vec![] };
            out.push(details_node(summary, open, body));
            continue;
        }
        // the body is the following siblings, up to the matching `</details>`
        let mut level = 1;
        let end = children.iter().enumerate().skip(i).find_map(|(j, s)| {
            let t = html(s)?;
            if details_open(&t).is_some_and(|(_, _, r)| !r.ends_with("</details>")) {
                level += 1;
            } else if t == "</details>" {
                level -= 1;
            }
            (level == 0).then_some(j)
        });
        match end {
            Some(end) if depth < MAX_DEPTH => {
                let body = wrap_inline_runs(slice_blocks(&children[i..end], depth + 1), true);
                out.push(details_node(summary, open, body));
                i = end + 1;
            }
            _ => out.extend(block(c, depth)),
        }
    }
    out
}

/// `<details>` / `<details open>`, optionally followed by `<summary>…</summary>`: the summary
/// text, whether it's open, and what follows.
fn details_open(html: &str) -> Option<(String, bool, &str)> {
    let rest = html.trim().strip_prefix("<details")?;
    let (open, rest) = match rest.trim_start().strip_prefix("open") {
        Some(r) => (true, r),
        None => (false, rest),
    };
    let rest = rest.trim_start().strip_prefix('>')?.trim_start();
    match rest.strip_prefix("<summary>") {
        Some(r) => {
            let close = r.find("</summary>")?;
            Some((html_unescape(&r[..close]), open, r[close + "</summary>".len()..].trim()))
        }
        None => Some((String::new(), open, rest.trim())),
    }
}

fn details_node(summary: String, open: bool, body: Vec<Node>) -> Node {
    let summary = if summary.trim().is_empty() { "Details".to_string() } else { summary };
    let body = if body.is_empty() { vec![Node::paragraph(vec![])] } else { body };
    let mut d = Node::new(nodes::DETAILS)
        .with_content(vec![Node::new(nodes::DETAILS_SUMMARY).with_content(vec![Node::text(summary)]), Node::new(nodes::DETAILS_CONTENT).with_content(body)]);
    if open {
        d = d.attr("open", true);
    }
    d
}

fn html_unescape(s: &str) -> String {
    s.replace("&lt;", "<").replace("&gt;", ">").replace("&quot;", "\"").replace("&#39;", "'").replace("&amp;", "&")
}

fn inlines<'a>(n: Ast<'a>, active: &[Mark], depth: usize) -> Vec<Node> {
    let mut out = Vec::new();
    if depth > MAX_DEPTH {
        let t = flat_text(n);
        if !t.is_empty() {
            out.push(Node::text_marked(t, active.to_vec()));
        }
        return out;
    }
    for c in n.children() {
        inline(c, active, &mut out, depth + 1);
    }
    normalize_text(&mut out);
    out
}

fn with_mark(active: &[Mark], m: Mark) -> Vec<Mark> {
    let mut v = active.to_vec();
    if !v.iter().any(|x| x.kind == m.kind) {
        v.push(m);
    }
    v
}

fn inline<'a>(n: Ast<'a>, active: &[Mark], out: &mut Vec<Node>, depth: usize) {
    let value = n.data.borrow().value.clone();
    match value {
        NodeValue::Text(t) => out.push(Node::text_marked(t.to_string(), active.to_vec())),
        NodeValue::SoftBreak => out.push(Node::text_marked(" ", active.to_vec())),
        NodeValue::LineBreak => out.push(Node::new(nodes::HARD_BREAK)),
        NodeValue::Code(c) => out.push(Node::text_marked(c.literal, with_mark(active, Mark::new(marks::CODE)))),
        NodeValue::Emph => out.extend(inlines(n, &with_mark(active, Mark::new(marks::ITALIC)), depth)),
        NodeValue::Strong => out.extend(inlines(n, &with_mark(active, Mark::new(marks::BOLD)), depth)),
        NodeValue::Strikethrough => out.extend(inlines(n, &with_mark(active, Mark::new(marks::STRIKE)), depth)),
        NodeValue::Underline => out.extend(inlines(n, &with_mark(active, Mark::new(marks::UNDERLINE)), depth)),
        NodeValue::Highlight => out.extend(inlines(n, &with_mark(active, Mark::new(marks::HIGHLIGHT)), depth)),
        NodeValue::Superscript => out.extend(inlines(n, &with_mark(active, Mark::new(marks::SUPERSCRIPT)), depth)),
        NodeValue::Subscript => out.extend(inlines(n, &with_mark(active, Mark::new(marks::SUBSCRIPT)), depth)),
        NodeValue::Link(l) => {
            let link = Mark::link(l.url.clone()).attr("title", if l.title.is_empty() { serde_json::Value::Null } else { l.title.clone().into() });
            out.extend(inlines(n, &with_mark(active, link), depth));
        }
        NodeValue::WikiLink(w) => {
            let link = Mark::link(format!("wikilink:{}", w.url));
            let inner = inlines(n, &with_mark(active, link.clone()), depth);
            if inner.is_empty() {
                out.push(Node::text_marked(w.url, with_mark(active, link)));
            } else {
                out.extend(inner);
            }
        }
        NodeValue::Image(l) => {
            let alt = crate::text::inline_text(&Node::paragraph(inlines(n, &[], depth)));
            out.push(
                Node::new(nodes::IMAGE)
                    .attr("src", l.url)
                    .attr("alt", if alt.is_empty() { serde_json::Value::Null } else { alt.into() })
                    .attr("title", if l.title.is_empty() { serde_json::Value::Null } else { l.title.into() }),
            );
        }
        NodeValue::Math(m) => {
            if m.display_math {
                out.push(Node::new(nodes::MATH_INLINE).attr("latex", m.literal).attr("display", true));
            } else {
                out.push(Node::new(nodes::MATH_INLINE).attr("latex", m.literal));
            }
        }
        NodeValue::HtmlInline(h) => {
            let lower = h.to_ascii_lowercase();
            if lower.starts_with("<br") {
                out.push(Node::new(nodes::HARD_BREAK));
            } else if !lower.starts_with("<!--") {
                out.push(Node::text_marked(h, with_mark(active, Mark::new(marks::CODE))));
            }
        }
        NodeValue::Escaped => out.extend(inlines(n, active, depth)),
        _ => out.extend(inlines(n, active, depth)),
    }
}

// --------------------------------------------------------------------------------------------
// Export

/// Renders a ProseMirror document as GitHub-flavoured Markdown.
pub fn pm_to_markdown(doc: &Node) -> String {
    let mut w = MdWriter::default();
    w.blocks(&doc.content, "");
    let mut s = w.out.trim_end().to_string();
    s.push('\n');
    s
}

#[derive(Default)]
struct MdWriter {
    out: String,
    /// Use the alternative list marker (`*` / `1)`) for the next list.
    alt_marker: bool,
}

/// Lists that Markdown would merge when they follow each other: bullet-style lists all use `-`,
/// ordered lists all use `.`.
fn list_family(kind: &str) -> Option<u8> {
    match kind {
        nodes::BULLET_LIST | nodes::TASK_LIST | nodes::DECISION_LIST => Some(0),
        nodes::ORDERED_LIST => Some(1),
        _ => None,
    }
}

impl MdWriter {
    fn blocks(&mut self, content: &[Node], prefix: &str) {
        let mut first = true;
        let mut prev: Option<(u8, bool)> = None;
        for n in content {
            if !first {
                self.out.push_str(prefix.trim_end());
                self.out.push('\n');
            }
            first = false;
            // adjacent lists of one family need different markers, or they'd become one list
            let family = list_family(&n.kind);
            self.alt_marker = matches!((family, prev), (Some(f), Some((pf, alt))) if f == pf && !alt);
            prev = family.map(|f| (f, self.alt_marker));
            self.block(n, prefix);
        }
    }

    fn line(&mut self, prefix: &str, s: &str) {
        for (i, l) in s.split('\n').enumerate() {
            let _ = i;
            self.out.push_str(prefix);
            self.out.push_str(l);
            self.out.push('\n');
        }
    }

    fn block(&mut self, n: &Node, prefix: &str) {
        match n.kind.as_str() {
            nodes::PARAGRAPH => {
                let s = inline_md(&n.content);
                // leading blanks are insignificant in Markdown, and four of them would start a code block
                let s: Vec<String> = s.trim_start_matches([' ', '\t']).split('\n').map(escape_block_start).collect();
                self.line(prefix, &s.join("\n"));
            }
            nodes::HEADING => {
                let lvl = n.attr_i64("level").unwrap_or(1).clamp(1, 6) as usize;
                let s = format!("{} {}", "#".repeat(lvl), inline_md(&n.content));
                self.line(prefix, &s);
            }
            nodes::BLOCKQUOTE => self.blocks(&n.content, &format!("{prefix}> ")),
            nodes::CALLOUT => {
                let kind = match n.attr_str("type").unwrap_or("info") {
                    "warning" => "WARNING",
                    "error" => "CAUTION",
                    "success" => "TIP",
                    "note" => "IMPORTANT",
                    _ => "NOTE",
                };
                self.line(prefix, &format!("> [!{kind}]"));
                self.blocks(&n.content, &format!("{prefix}> "));
            }
            nodes::BULLET_LIST | nodes::ORDERED_LIST | nodes::TASK_LIST | nodes::DECISION_LIST => {
                let start = n.attr_i64("start").unwrap_or(1);
                let (bullet, delim) = if std::mem::take(&mut self.alt_marker) { ('*', ')') } else { ('-', '.') };
                for (i, item) in n.content.iter().enumerate() {
                    let marker = match n.kind.as_str() {
                        nodes::ORDERED_LIST => format!("{}{delim} ", start + i as i64),
                        nodes::TASK_LIST => {
                            if item.attr_bool("checked").unwrap_or(false) {
                                format!("{bullet} [x] ")
                            } else {
                                format!("{bullet} [ ] ")
                            }
                        }
                        nodes::DECISION_LIST => format!("{bullet} **Decision:** "),
                        _ => format!("{bullet} "),
                    };
                    // continuation lines align with the item's content column, which for task and
                    // decision items is right after the bullet (`[ ] `/`**Decision:**` are
                    // content); indenting further would turn the following blocks into code
                    let width = if n.kind == nodes::ORDERED_LIST { marker.chars().count() } else { 2 };
                    let indent = " ".repeat(width);
                    let mut sub = MdWriter::default();
                    sub.blocks(&item.content, "");
                    let body = sub.out.trim_end().to_string();
                    for (j, l) in body.split('\n').enumerate() {
                        self.out.push_str(prefix);
                        if j == 0 {
                            self.out.push_str(&marker);
                        } else if !l.is_empty() {
                            self.out.push_str(&indent);
                        }
                        self.out.push_str(l);
                        self.out.push('\n');
                    }
                }
            }
            nodes::CODE_BLOCK => {
                let lang = n.attr_str("language").unwrap_or("");
                let code = crate::text::inline_text(n);
                let fence = if code.contains("```") { "````" } else { "```" };
                self.line(prefix, &format!("{fence}{lang}"));
                if !code.is_empty() {
                    self.line(prefix, &code);
                }
                self.line(prefix, fence);
            }
            nodes::MATH_BLOCK => {
                self.line(prefix, "$$");
                self.line(prefix, n.attr_str("latex").unwrap_or(""));
                self.line(prefix, "$$");
            }
            nodes::HORIZONTAL_RULE => self.line(prefix, "---"),
            nodes::TABLE => self.table(n, prefix),
            nodes::DETAILS => {
                let summary = n.content.first().map(|s| inline_md(&s.content)).unwrap_or_default();
                let open = if n.attr_bool("open").unwrap_or(false) { " open" } else { "" };
                self.line(prefix, &format!("<details{open}><summary>{}</summary>", html_escape(&summary)));
                self.line(prefix, "");
                if let Some(body) = n.content.get(1) {
                    self.blocks(&body.content, prefix);
                }
                self.line(prefix, "");
                self.line(prefix, "</details>");
            }
            nodes::COLUMNS | nodes::COLUMN | nodes::DETAILS_CONTENT | nodes::DECISION_ITEM => self.blocks(&n.content, prefix),
            nodes::IMAGE => self.line(prefix, &inline_md(std::slice::from_ref(n))),
            nodes::DRAWING => {
                if let Some(src) = n.attr_str("preview") {
                    let alt = n.attr_str("title").unwrap_or("Drawing").replace(['[', ']'], "");
                    self.line(prefix, &format!("![{alt}]({src})"));
                }
            }
            nodes::TOC => self.line(prefix, "<!-- table of contents -->"),
            nodes::CHILD_PAGES => self.line(prefix, "<!-- child pages -->"),
            nodes::INCLUDE if !n.content.is_empty() => self.blocks(&n.content, prefix),
            nodes::INCLUDE => {
                let title = n.attr_str("title").unwrap_or("page").replace("--", "–");
                self.line(prefix, &format!("<!-- included page: {title} -->"));
            }
            nodes::EXCERPT if n.attr_bool("hidden") == Some(true) => {}
            nodes::EXCERPT => self.blocks(&n.content, prefix),
            nodes::ATTACHMENT => {
                let name = n.attr_str("name").unwrap_or("attachment");
                let href = n.attr_str("href").unwrap_or("");
                self.line(prefix, &format!("[{}]({})", md_escape(name), href));
            }
            nodes::EMBED => {
                let url = n.attr_str("url").unwrap_or("");
                self.line(prefix, &format!("<{url}>"));
            }
            nodes::MACRO => {
                let name = n.attr_str("name").unwrap_or("macro");
                self.line(prefix, &format!("<!-- macro: {name} -->"));
                if !n.content.is_empty() {
                    self.blocks(&n.content, prefix);
                }
            }
            _ => {
                if n.content.iter().all(|c| c.is_inline()) {
                    self.line(prefix, &inline_md(&n.content));
                } else {
                    self.blocks(&n.content, prefix);
                }
            }
        }
    }

    fn table(&mut self, n: &Node, prefix: &str) {
        let rows: Vec<Vec<String>> = n
            .content
            .iter()
            .map(|r| {
                r.content
                    .iter()
                    .map(|c| {
                        // Markdown cells hold one line: blocks and list items become <br>-separated lines
                        let mut parts = Vec::new();
                        cell_lines(&c.content, "", &mut parts);
                        escape_pipes(&parts.join("<br>")).replace('\n', " ")
                    })
                    .collect()
            })
            .collect();
        let cols = rows.iter().map(Vec::len).max().unwrap_or(0).max(1);
        let header_first = n.content.first().is_some_and(|r| r.content.iter().all(|c| c.kind == nodes::TABLE_HEADER));
        let empty = vec![String::new(); cols];
        let (head, body) = if header_first { (&rows[0], &rows[1..]) } else { (&empty, &rows[..]) };
        let fmt = |r: &Vec<String>| {
            let mut cells = r.clone();
            cells.resize(cols, String::new());
            format!("| {} |", cells.join(" | "))
        };
        self.line(prefix, &fmt(head));
        self.line(prefix, &format!("|{}", " --- |".repeat(cols)));
        for r in body {
            self.line(prefix, &fmt(r));
        }
    }
}

/// The lines of a table cell's blocks; list items get a marker (`• `, `1. `, `[x] `).
fn cell_lines(blocks: &[Node], marker: &str, out: &mut Vec<String>) {
    for (i, b) in blocks.iter().enumerate() {
        // only the item's first block carries its marker
        let m = if i == 0 { marker } else { "" };
        match b.kind.as_str() {
            nodes::BULLET_LIST => b.content.iter().for_each(|item| cell_lines(&item.content, "• ", out)),
            nodes::ORDERED_LIST => {
                let start = b.attr_i64("start").unwrap_or(1);
                for (n, item) in b.content.iter().enumerate() {
                    cell_lines(&item.content, &format!("{}. ", start + n as i64), out);
                }
            }
            nodes::TASK_LIST => {
                for item in &b.content {
                    cell_lines(&item.content, if item.attr_bool("checked").unwrap_or(false) { "[x] " } else { "[ ] " }, out);
                }
            }
            nodes::PARAGRAPH | nodes::HEADING | nodes::CODE_BLOCK => {
                let text = inline_md(&b.content);
                if !text.trim().is_empty() || !m.is_empty() {
                    out.push(format!("{m}{text}"));
                }
            }
            _ => cell_lines(&b.content, m, out),
        }
    }
}

fn md_escape(s: &str) -> String {
    let mut o = String::with_capacity(s.len());
    let mut chars = s.chars().peekable();
    while let Some(ch) = chars.next() {
        let next = chars.peek().copied();
        let escape = match ch {
            // `~` strikes through and `$` starts math (both enabled on import)
            '\\' | '`' | '*' | '_' | '[' | ']' | '<' | '>' | '#' | '|' | '~' | '$' => true,
            // `==highlight==`
            '=' => next == Some('='),
            // entity and character references (`&amp;`, `&#60;`) would be decoded
            '&' => next.is_some_and(|c| c.is_ascii_alphanumeric() || c == '#'),
            _ => false,
        };
        if escape {
            o.push('\\');
        }
        o.push(ch);
    }
    o
}

/// Escapes the pipes of a table cell that aren't escaped yet (text was escaped by
/// [`md_escape`]; code spans and URLs were not).
fn escape_pipes(s: &str) -> String {
    let mut o = String::with_capacity(s.len());
    let mut backslashes = 0;
    for ch in s.chars() {
        if ch == '|' && backslashes % 2 == 0 {
            o.push('\\');
        }
        backslashes = if ch == '\\' { backslashes + 1 } else { 0 };
        o.push(ch);
    }
    o
}

/// Escapes a paragraph line that Markdown would otherwise read as the start of another block:
/// a list item (`- x`, `+ x`, `1. x`, `1) x`) or a setext underline / thematic break (`---`, `=`).
fn escape_block_start(line: &str) -> String {
    let body = line.trim_start_matches([' ', '\t']);
    let lead = &line[..line.len() - body.len()];
    let rest = body.get(1..).unwrap_or("");
    let bullet = (body.starts_with('-') || body.starts_with('+')) && (rest.is_empty() || rest.starts_with([' ', '\t']));
    let trimmed = body.trim_end();
    let underline = !trimmed.is_empty() && (trimmed.chars().all(|c| c == '-') || trimmed.chars().all(|c| c == '='));
    if bullet || underline {
        return format!("{lead}\\{body}");
    }
    let digits = body.bytes().take_while(u8::is_ascii_digit).count();
    let after = &body[digits..];
    if (1..=9).contains(&digits) && after.starts_with(['.', ')']) && (after.len() == 1 || after[1..].starts_with([' ', '\t'])) {
        return format!("{lead}{}\\{after}", &body[..digits]);
    }
    line.to_string()
}

fn html_escape(s: &str) -> String {
    s.replace('&', "&amp;").replace('<', "&lt;").replace('>', "&gt;")
}

/// A link destination: `\`, `(` and `)` escaped, in `<…>` if it contains spaces or brackets.
fn md_url(u: &str) -> String {
    let pointy = u.contains(|c: char| c.is_whitespace() || c == '<' || c == '>');
    let mut o = String::with_capacity(u.len() + 2);
    if pointy {
        o.push('<');
    }
    for ch in u.chars() {
        if matches!(ch, '\\' | '(' | ')' | '<' | '>') {
            o.push('\\');
        }
        o.push(ch);
    }
    if pointy {
        o.push('>');
    }
    o
}

fn inline_md(content: &[Node]) -> String {
    let mut s = String::new();
    // a hard break at the start or end of a paragraph has no Markdown form (`\` there is a
    // literal backslash)
    let blank = |n: &Node| n.kind == nodes::HARD_BREAK || (n.is_text() && n.marks.is_empty() && n.text.as_deref().is_none_or(|t| t.trim().is_empty()));
    let end = content.iter().rposition(|n| !blank(n)).map_or(0, |i| i + 1);
    for n in &content[..end] {
        match n.kind.as_str() {
            nodes::TEXT => {
                let t = n.text.as_deref().unwrap_or("");
                let is_code = n.marks.iter().any(|m| m.kind == marks::CODE);
                let mut body = if is_code {
                    let ticks = if t.contains('`') { "``" } else { "`" };
                    format!("{ticks}{t}{ticks}")
                } else {
                    md_escape(t)
                };
                for m in n.marks.iter().rev() {
                    body = match m.kind.as_str() {
                        marks::BOLD => format!("**{body}**"),
                        marks::ITALIC => format!("_{body}_"),
                        marks::STRIKE => format!("~~{body}~~"),
                        marks::HIGHLIGHT => format!("=={body}=="),
                        marks::LINK => format!("[{}]({})", body, md_url(m.attr_str("href").unwrap_or(""))),
                        _ => body,
                    };
                }
                s.push_str(&body);
            }
            nodes::HARD_BREAK if s.trim().is_empty() => {}
            nodes::HARD_BREAK => s.push_str("\\\n"),
            nodes::IMAGE => {
                // the caption (else the title) as the Markdown image title
                let title = n.attr_str("caption").or(n.attr_str("title")).filter(|t| !t.trim().is_empty());
                let title = title.map(|t| format!(" \"{}\"", t.replace('\\', "\\\\").replace('"', "\\\"").replace('\n', " "))).unwrap_or_default();
                s.push_str(&format!("![{}]({}{title})", md_escape(n.attr_str("alt").unwrap_or("")), n.attr_str("src").unwrap_or("")));
            }
            nodes::MENTION => s.push_str(&format!("@{}", n.attr_str("label").or(n.attr_str("id")).unwrap_or("user"))),
            nodes::STATUS => s.push_str(&format!("`{}`", n.attr_str("text").unwrap_or("").to_uppercase())),
            nodes::DATE => s.push_str(n.attr_str("date").unwrap_or("")),
            nodes::ISSUE => {
                let key = n.attr_str("key").unwrap_or("issue");
                match n.attr_str("url").filter(|u| u.starts_with("https://") || u.starts_with("http://")) {
                    Some(u) => s.push_str(&format!("[{}]({u})", md_escape(key))),
                    None => s.push_str(&format!("`{key}`")),
                }
            }
            nodes::EMOTICON => s.push_str(crate::icons::emoticon_shortcut(n.attr_str("name").unwrap_or(""))),
            nodes::MATH_INLINE if n.attr_bool("display") == Some(true) => s.push_str(&format!("$${}$$", n.attr_str("latex").unwrap_or(""))),
            nodes::MATH_INLINE => s.push_str(&format!("${}$", n.attr_str("latex").unwrap_or(""))),
            nodes::ANCHOR => {}
            _ => s.push_str(&inline_md(&n.content)),
        }
    }
    s
}

#[cfg(test)]
mod tests {
    use super::*;
    use pretty_assertions::assert_eq;

    #[test]
    fn imports_gfm() {
        let md = "---\ntitle: x\n---\n# Hello\n\nSome **bold** and _it_ [link](https://a.b).\n\n- [x] done\n- [ ] todo\n\n| a | b |\n|---|--:|\n| 1 | 2 |\n\n```rust\nfn x() {}\n```\n\n> [!WARNING]\n> careful\n";
        let r = markdown_to_pm(md);
        assert_eq!(r.front_matter.as_deref(), Some("title: x"));
        assert_eq!(r.title.as_deref(), Some("Hello"));
        let kinds: Vec<&str> = r.doc.content.iter().map(|n| n.kind.as_str()).collect();
        assert_eq!(kinds, vec!["heading", "paragraph", "taskList", "table", "codeBlock", "callout"]);
        assert_eq!(r.doc.content[2].content[0].attr_bool("checked"), Some(true));
        assert_eq!(r.doc.content[4].attr_str("language"), Some("rust"));
        assert_eq!(r.doc.content[5].attr_str("type"), Some("warning"));
    }

    #[test]
    fn export_roundtrip_is_stable() {
        let md = "# Title\n\nText with **bold**, _italic_ and `code`.\n\n- a\n- b\n\n1. one\n2. two\n\n- [x] done\n\n```js\nlet x = 1;\n```\n\n| h1 | h2 |\n| --- | --- |\n| c1 | c2 |\n";
        let first = pm_to_markdown(&markdown_to_pm(md).doc);
        let second = pm_to_markdown(&markdown_to_pm(&first).doc);
        assert_eq!(first, second);
        assert!(first.contains("**bold**"));
        assert!(first.contains("- [x] done"));
        assert!(first.contains("| h1 | h2 |"));
    }

    #[test]
    fn adjacent_lists_stay_separate() {
        // found by proptest: `- a\n\n- [ ] b` is one (task) list in Markdown
        let item = |kind: &str, t: &str| Node::new(kind).with_content(vec![Node::paragraph(vec![Node::text(t)])]);
        let list = |kind: &str, item_kind: &str, t: &str| Node::new(kind).with_content(vec![item(item_kind, t)]);
        let doc = Node::doc(vec![
            list(nodes::BULLET_LIST, nodes::LIST_ITEM, "a"),
            list(nodes::TASK_LIST, nodes::TASK_ITEM, "b"),
            list(nodes::BULLET_LIST, nodes::LIST_ITEM, "c"),
            list(nodes::ORDERED_LIST, nodes::LIST_ITEM, "d"),
            list(nodes::ORDERED_LIST, nodes::LIST_ITEM, "e"),
        ]);
        let md = pm_to_markdown(&doc);
        assert_eq!(md, "- a\n\n* [ ] b\n\n- c\n\n1. d\n\n1) e\n");
        let back = markdown_to_pm(&md).doc;
        let kinds: Vec<&str> = back.content.iter().map(|n| n.kind.as_str()).collect();
        assert_eq!(kinds, vec!["bulletList", "taskList", "bulletList", "orderedList", "orderedList"]);
        assert_eq!(pm_to_markdown(&back), md);
    }

    #[test]
    fn task_item_blocks_are_not_code() {
        // found by proptest: a second block in a task item was indented 6 spaces (4 past the
        // item's content column), i.e. it became an indented code block
        let item = Node::new(nodes::TASK_ITEM).attr("checked", false).with_content(vec![
            Node::paragraph(vec![Node::text("a")]),
            Node::paragraph(vec![Node::text("b")]),
            Node::new(nodes::BULLET_LIST).with_content(vec![Node::new(nodes::LIST_ITEM).with_content(vec![Node::paragraph(vec![Node::text("c")])])]),
        ]);
        let doc = Node::doc(vec![Node::new(nodes::TASK_LIST).with_content(vec![item])]);
        let md = pm_to_markdown(&doc);
        assert_eq!(md, "- [ ] a\n\n  b\n\n  - c\n");
        assert_eq!(markdown_to_pm(&md).doc, doc);
    }

    #[test]
    fn text_that_looks_like_markup_stays_text() {
        // found by proptest: entities were decoded, `$x$` became math, `- x` a list, ...
        for t in ["&amp; &#60;", "$x$", "~~no~~", "==no==", "- no list", "+ no", "1. no", "2) no", "---", "a | b", "Tom & Jerry"] {
            let doc = Node::doc(vec![Node::paragraph(vec![Node::text(t)])]);
            let md = pm_to_markdown(&doc);
            assert_eq!(markdown_to_pm(&md).doc, doc, "{t:?} exported as {md:?}");
        }
        // after a hard break, too (`---` would turn the first line into a setext heading)
        let doc = Node::doc(vec![Node::paragraph(vec![Node::text("a"), Node::new(nodes::HARD_BREAK), Node::text("---")])]);
        assert_eq!(markdown_to_pm(&pm_to_markdown(&doc)).doc, doc);
        // a paragraph starting with a tab (or 4 spaces) is not an indented code block
        let doc = Node::doc(vec![Node::paragraph(vec![Node::text("\tx")])]);
        assert_eq!(markdown_to_pm(&pm_to_markdown(&doc)).doc.content[0].kind, nodes::PARAGRAPH);
        // pipes in a table cell are escaped once
        let cell = |t: &str| Node::new(nodes::TABLE_CELL).with_content(vec![Node::paragraph(vec![Node::text(t)])]);
        let table = Node::new(nodes::TABLE).with_content(vec![Node::new(nodes::TABLE_ROW).with_content(vec![cell("a|b"), cell("\\|")])]);
        let md = pm_to_markdown(&Node::doc(vec![table]));
        assert_eq!(pm_to_markdown(&markdown_to_pm(&md).doc), md);
        assert!(md.contains("| a\\|b | \\\\\\| |"), "{md}");
        // link targets with parentheses, backslashes or spaces; trailing hard breaks
        for href in ["https://example.com/a_(b)", "https://example.com/a\\b", "/pages/a b"] {
            let doc = Node::doc(vec![Node::paragraph(vec![Node::text_marked("x", vec![Mark::link(href)])])]);
            assert_eq!(markdown_to_pm(&pm_to_markdown(&doc)).doc, doc, "{href}");
        }
        let doc = Node::doc(vec![Node::paragraph(vec![Node::text("a"), Node::new(nodes::HARD_BREAK)])]);
        assert_eq!(pm_to_markdown(&doc), "a\n");
        let doc = Node::doc(vec![Node::paragraph(vec![Node::new(nodes::HARD_BREAK), Node::text(" a "), Node::new(nodes::HARD_BREAK), Node::text(" ")])]);
        assert_eq!(pm_to_markdown(&doc), "a\n");
    }

    #[test]
    fn wikilinks_become_placeholder_links() {
        let r = markdown_to_pm("See [[Other Page]].");
        let p = &r.doc.content[0];
        let link = p.content.iter().find_map(|n| n.marks.iter().find(|m| m.kind == "link")).unwrap();
        assert_eq!(link.attr_str("href"), Some("wikilink:Other Page"));
    }
}
