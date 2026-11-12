//! Server-side HTML rendering of ProseMirror documents.
//!
//! Output is safe by construction: every text and attribute value is escaped, URLs are
//! scheme-checked, colors are validated. Used for public pages, exports, e-mails and
//! no-JS rendering.

use std::fmt::Write as _;

use crate::node::{Mark, Node, marks, nodes};
use crate::text::{headings, slugify};

/// Options influencing link/asset URLs in the rendered HTML.
#[derive(Debug, Clone, Default)]
pub struct HtmlOptions {
    /// Add `id` attributes to headings (for anchors and TOC) and blocks (`block-<blockId>`, for
    /// links to a block).
    pub heading_ids: bool,
}

pub fn escape(s: &str) -> String {
    let mut o = String::with_capacity(s.len() + 8);
    for ch in s.chars() {
        match ch {
            '&' => o.push_str("&amp;"),
            '<' => o.push_str("&lt;"),
            '>' => o.push_str("&gt;"),
            '"' => o.push_str("&quot;"),
            '\'' => o.push_str("&#39;"),
            c => o.push(c),
        }
    }
    o
}

/// Returns the URL if it is safe to put into `href`/`src` (http(s), mailto, tel, relative, anchors).
pub fn safe_url(url: &str) -> Option<&str> {
    let u = url.trim();
    let lower = u.to_ascii_lowercase();
    let lower = lower.replace(['\t', '\n', '\r'], "");
    if lower.starts_with("http://")
        || lower.starts_with("https://")
        || lower.starts_with("mailto:")
        || lower.starts_with("tel:")
        || lower.starts_with('/')
        || lower.starts_with('#')
        || lower.starts_with("./")
        || lower.starts_with("../")
    {
        return Some(u);
    }
    // relative path without scheme
    if !lower.contains(':') {
        return Some(u);
    }
    None
}

/// Accepts `#rgb`, `#rrggbb`, `#rrggbbaa`, `rgb()/rgba()` and plain CSS color names.
pub fn safe_color(c: &str) -> Option<&str> {
    let c = c.trim();
    let ok = if let Some(hex) = c.strip_prefix('#') {
        matches!(hex.len(), 3 | 4 | 6 | 8) && hex.chars().all(|ch| ch.is_ascii_hexdigit())
    } else if c.starts_with("rgb(") || c.starts_with("rgba(") || c.starts_with("hsl(") || c.starts_with("hsla(") {
        c.ends_with(')') && c.chars().all(|ch| ch.is_ascii_alphanumeric() || " ,.%()/".contains(ch))
    } else {
        !c.is_empty() && c.len() < 32 && c.chars().all(|ch| ch.is_ascii_alphabetic())
    };
    ok.then_some(c)
}

pub fn render(doc: &Node, opts: &HtmlOptions) -> String {
    let mut r = Renderer { out: String::new(), opts: opts.clone(), heading_ids: headings(doc).into_iter().map(|h| h.2).collect(), heading_idx: 0, doc };
    r.children(&doc.content);
    r.out
}

struct Renderer<'a> {
    out: String,
    opts: HtmlOptions,
    heading_ids: Vec<String>,
    heading_idx: usize,
    doc: &'a Node,
}

impl Renderer<'_> {
    fn children(&mut self, content: &[Node]) {
        for c in content {
            self.node(c);
        }
    }

    fn wrap(&mut self, open: &str, n: &Node, close: &str) {
        self.out.push_str(open);
        self.children(&n.content);
        self.out.push_str(close);
    }

    /// `style` for a paragraph or heading: alignment and indentation (30px per level).
    fn text_align(n: &Node) -> String {
        let mut style = String::new();
        if let Some(a @ ("left" | "center" | "right" | "justify")) = n.attr_str("textAlign") {
            style.push_str(&format!("text-align:{a};"));
        }
        let indent = crate::node::indent_level(n);
        if indent > 0 {
            style.push_str(&format!("margin-left:{}px;", indent * 30));
        }
        if style.is_empty() { style } else { format!(" style=\"{}\"", style.trim_end_matches(';')) }
    }

    fn node(&mut self, n: &Node) {
        let start = self.out.len();
        self.node_inner(n);
        if self.opts.heading_ids {
            self.block_anchor(n, start);
        }
    }

    /// Puts `id="block-<blockId>"` on the first tag of a block rendered from `start` (headings
    /// keep their slug id and get an empty anchor instead).
    fn block_anchor(&mut self, n: &Node, start: usize) {
        if !crate::blocks::is_block_type(&n.kind) {
            return;
        }
        let Some(id) = n.attr_str(crate::blocks::ATTR).filter(|id| crate::blocks::is_valid_id(id)) else { return };
        let out = &self.out[start..];
        if !out.starts_with('<') || !out[1..].starts_with(|c: char| c.is_ascii_alphabetic()) {
            return;
        }
        let Some(end) = out.find('>') else { return };
        let tag = &out[..end];
        let anchor = format!("block-{}", escape(id));
        if tag.contains(" id=\"") {
            self.out.insert_str(start + end + 1, &format!("<a id=\"{anchor}\" class=\"block-anchor\"></a>"));
        } else {
            let name_end = tag.find([' ', '/']).unwrap_or(end);
            self.out.insert_str(start + name_end, &format!(" id=\"{anchor}\""));
        }
    }

    fn node_inner(&mut self, n: &Node) {
        match n.kind.as_str() {
            nodes::TEXT => self.text(n),
            nodes::PARAGRAPH => {
                let open = format!("<p{}>", Self::text_align(n));
                self.wrap(&open, n, "</p>");
            }
            nodes::HEADING => {
                let lvl = n.attr_i64("level").unwrap_or(1).clamp(1, 6);
                let id = self.heading_ids.get(self.heading_idx).cloned().unwrap_or_default();
                self.heading_idx += 1;
                let id_attr = if self.opts.heading_ids { format!(" id=\"{}\"", escape(&id)) } else { String::new() };
                let open = format!("<h{lvl}{id_attr}{}>", Self::text_align(n));
                self.wrap(&open, n, &format!("</h{lvl}>"));
            }
            nodes::BULLET_LIST => self.wrap("<ul>", n, "</ul>"),
            nodes::ORDERED_LIST => {
                let start = n.attr_i64("start").unwrap_or(1);
                let open = if start != 1 { format!("<ol start=\"{start}\">") } else { "<ol>".into() };
                self.wrap(&open, n, "</ol>");
            }
            nodes::LIST_ITEM => self.wrap("<li>", n, "</li>"),
            nodes::TASK_LIST => self.wrap("<ul class=\"task-list\" data-type=\"taskList\">", n, "</ul>"),
            nodes::TASK_ITEM => {
                let checked = n.attr_bool("checked").unwrap_or(false);
                let open = format!(
                    "<li class=\"task-item\" data-checked=\"{checked}\"><input type=\"checkbox\" disabled{}> <div>",
                    if checked { " checked" } else { "" }
                );
                self.wrap(&open, n, "</div></li>");
            }
            nodes::CODE_BLOCK => {
                let lang = n.attr_str("language").map(|l| format!(" class=\"language-{}\"", escape(l))).unwrap_or_default();
                let _ = write!(self.out, "<pre><code{lang}>{}</code></pre>", escape(&crate::text::inline_text(n)));
            }
            nodes::BLOCKQUOTE => self.wrap("<blockquote>", n, "</blockquote>"),
            nodes::HORIZONTAL_RULE => self.out.push_str("<hr>"),
            nodes::HARD_BREAK => self.out.push_str("<br>"),
            nodes::IMAGE => self.image(n),
            nodes::TABLE => {
                let open = if n.attr_bool("numbered") == Some(true) { "<table data-numbered=\"true\"><tbody>" } else { "<table><tbody>" };
                self.wrap(open, n, "</tbody></table>")
            }
            nodes::TABLE_ROW => self.wrap("<tr>", n, "</tr>"),
            nodes::TABLE_HEADER | nodes::TABLE_CELL => {
                let tag = if n.kind == nodes::TABLE_HEADER { "th" } else { "td" };
                let mut attrs = String::new();
                for k in ["colspan", "rowspan"] {
                    if let Some(v) = n.attr_i64(k).filter(|v| *v > 1) {
                        let _ = write!(attrs, " {k}=\"{v}\"");
                    }
                }
                if let Some(bg) = n.attr_str("background").and_then(safe_color) {
                    let _ = write!(attrs, " style=\"background-color:{bg}\"");
                }
                self.wrap(&format!("<{tag}{attrs}>"), n, &format!("</{tag}>"));
            }
            nodes::DETAILS => {
                let open = if n.attr_bool("open").unwrap_or(false) { "<details open>" } else { "<details>" };
                self.wrap(open, n, "</details>");
            }
            nodes::DETAILS_SUMMARY => self.wrap("<summary>", n, "</summary>"),
            nodes::DETAILS_CONTENT => self.wrap("<div class=\"details-content\">", n, "</div>"),
            nodes::CALLOUT => {
                let kind = n.attr_str("type").filter(|k| matches!(*k, "info" | "note" | "success" | "warning" | "error")).unwrap_or("info");
                // a custom symbol from the icon set (validated), else the type's own glyph
                let icon = match n.attr_str("icon").and_then(crate::icons::parse_symbol) {
                    Some((name, colour)) => crate::icons::line_svg("symbols", name, "callout-icon", "20")
                        .map(|svg| format!("<span style=\"color:{colour}\">{svg}</span>"))
                        .unwrap_or_default(),
                    None => crate::icons::line_svg("callout", kind, "callout-icon", "20").unwrap_or_default(),
                };
                let open = format!("<div class=\"callout callout-{kind}\" data-type=\"callout\">{icon}<div class=\"callout-body\">");
                self.wrap(&open, n, "</div></div>");
            }
            nodes::MENTION => {
                let label = n.attr_str("label").or(n.attr_str("id")).unwrap_or("user");
                let _ = write!(self.out, "<span class=\"mention\" data-id=\"{}\">@{}</span>", escape(n.attr_str("id").unwrap_or("")), escape(label));
            }
            nodes::STATUS => {
                // attribute values never go into markup unchecked
                let color = n.attr_str("color").filter(|c| matches!(*c, "grey" | "blue" | "green" | "yellow" | "red" | "purple")).unwrap_or("grey");
                let _ = write!(self.out, "<span class=\"status status-{color}\">{}</span>", escape(n.attr_str("text").unwrap_or("")));
            }
            nodes::DATE => {
                let d = n.attr_str("date").unwrap_or("");
                let _ = write!(self.out, "<time datetime=\"{0}\">{0}</time>", escape(d));
            }
            nodes::ISSUE => {
                let key = escape(n.attr_str("key").unwrap_or("issue"));
                match n.attr_str("url").and_then(safe_url) {
                    Some(u) => {
                        let _ = write!(self.out, "<a class=\"issue\" href=\"{}\" rel=\"noopener noreferrer\">{key}</a>", escape(u));
                    }
                    None => {
                        let _ = write!(self.out, "<span class=\"issue\">{key}</span>");
                    }
                }
            }
            nodes::EMOTICON => {
                if let Some(svg) = n.attr_str("name").and_then(crate::icons::emoticon_svg) {
                    self.out.push_str(&svg);
                }
            }
            nodes::ATTACHMENT => {
                let name = n.attr_str("name").unwrap_or("attachment");
                let href = n.attr_str("href").and_then(safe_url).unwrap_or("#");
                let clip = crate::icons::line_svg("ui", "paperclip", "attachment-icon", "16").unwrap_or_default();
                let _ = write!(self.out, "<p class=\"attachment\"><a href=\"{}\">{clip} {}</a></p>", escape(href), escape(name));
            }
            nodes::EMBED => {
                let url = n.attr_str("url").and_then(safe_url).unwrap_or("#");
                let _ = write!(self.out, "<p class=\"embed\"><a href=\"{0}\" rel=\"noopener noreferrer\">{0}</a></p>", escape(url));
            }
            nodes::TOC => {
                let entries = crate::toc::entries(self.doc, n);
                let label = |e: &crate::toc::TocEntry| if e.number.is_empty() { escape(&e.text) } else { format!("{} {}", e.number, escape(&e.text)) };
                if crate::toc::is_flat(n) {
                    self.out.push_str("<nav class=\"toc toc-flat\"><p>");
                    for (i, e) in entries.iter().enumerate() {
                        let sep = if i > 0 { " · " } else { "" };
                        let _ = write!(self.out, "{sep}<a href=\"#{}\">{}</a>", escape(&e.id), label(e));
                    }
                    self.out.push_str("</p></nav>");
                } else {
                    self.out.push_str("<nav class=\"toc\"><ul>");
                    for e in &entries {
                        let _ = write!(self.out, "<li class=\"toc-l{}\"><a href=\"#{}\">{}</a></li>", e.depth + 1, escape(&e.id), label(e));
                    }
                    self.out.push_str("</ul></nav>");
                }
            }
            nodes::CHILD_PAGES => self.out.push_str("<div class=\"child-pages\" data-type=\"childPages\"></div>"),
            nodes::INCLUDE => {
                let title = n.attr_str("title").unwrap_or("page");
                let _ = write!(
                    self.out,
                    "<div class=\"include\" data-page-id=\"{}\">Included page: {}</div>",
                    escape(n.attr_str("pageId").unwrap_or("")),
                    escape(title)
                );
            }
            // a hidden excerpt shows only where another page includes it
            nodes::EXCERPT if n.attr_bool("hidden") == Some(true) => {}
            nodes::EXCERPT => self.wrap("<div class=\"excerpt\">", n, "</div>"),
            nodes::ANCHOR => {
                let name = n.attr_str("name").unwrap_or("");
                let _ = write!(self.out, "<a id=\"{}\"></a>", escape(&slugify(name)));
            }
            nodes::COLUMNS => {
                // only the layout names we know (the attribute ends up in the page)
                match n
                    .attr_str("layout")
                    .filter(|l| matches!(*l, "two_equal" | "two_left_sidebar" | "two_right_sidebar" | "three_equal" | "three_with_sidebars"))
                {
                    Some(l) => self.wrap(&format!("<div class=\"columns\" data-layout=\"{l}\">"), n, "</div>"),
                    None => self.wrap("<div class=\"columns\">", n, "</div>"),
                }
            }
            nodes::COLUMN => self.wrap("<div class=\"column\">", n, "</div>"),
            nodes::DECISION_LIST => self.wrap("<ul class=\"decision-list\">", n, "</ul>"),
            nodes::DECISION_ITEM => self.wrap("<li class=\"decision-item\">", n, "</li>"),
            nodes::MATH_BLOCK => {
                let _ = write!(self.out, "<div class=\"math\" data-latex=\"{0}\">{0}</div>", escape(n.attr_str("latex").unwrap_or("")));
            }
            nodes::MATH_INLINE => {
                let _ = write!(self.out, "<span class=\"math\" data-latex=\"{0}\">{0}</span>", escape(n.attr_str("latex").unwrap_or("")));
            }
            nodes::DRAWING => {
                if let Some(src) = n.attr_str("preview").and_then(safe_url) {
                    let alt = n.attr_str("title").unwrap_or("Drawing");
                    let _ = write!(self.out, "<p class=\"drawing\"><img src=\"{}\" alt=\"{}\" loading=\"lazy\"></p>", escape(src), escape(alt));
                }
            }
            nodes::MACRO => {
                let name = n.attr_str("name").unwrap_or("macro");
                let _ = write!(self.out, "<div class=\"macro\" data-macro=\"{0}\"><div class=\"macro-label\">Macro: {0}</div>", escape(name));
                self.children(&n.content);
                self.out.push_str("</div>");
            }
            _ => {
                // unknown node: render children so content is not lost
                if n.content.iter().all(|c| c.is_inline()) && !n.content.is_empty() {
                    self.wrap("<p>", n, "</p>");
                } else {
                    self.children(&n.content);
                }
            }
        }
    }

    fn image(&mut self, n: &Node) {
        let src = n.attr_str("src").and_then(safe_url).unwrap_or("");
        let alt = n.attr_str("alt").unwrap_or("");
        let mut extra = String::new();
        if let Some(w) = n.attr_i64("width").filter(|w| *w > 0) {
            let _ = write!(extra, " width=\"{w}\"");
        }
        if let Some(t) = n.attr_str("title") {
            let _ = write!(extra, " title=\"{}\"", escape(t));
        }
        if n.attr_bool("border").unwrap_or(false) {
            extra.push_str(" class=\"bordered\"");
        }
        let img = format!("<img src=\"{}\" alt=\"{}\"{extra} loading=\"lazy\">", escape(src), escape(alt));
        let align = n.attr_str("align").filter(|a| matches!(*a, "left" | "center" | "right" | "wrap-left" | "wrap-right"));
        let caption = n.attr_str("caption").filter(|c| !c.trim().is_empty());
        if align.is_none() && caption.is_none() {
            self.out.push_str(&img);
            return;
        }
        // images are inline (they live in paragraphs), so a <span> wraps image + caption
        let _ = write!(self.out, "<span class=\"image-block align-{}\">{img}", align.unwrap_or("none"));
        if let Some(c) = caption {
            let _ = write!(self.out, "<span class=\"image-caption\">{}</span>", escape(c));
        }
        self.out.push_str("</span>");
    }

    fn text(&mut self, n: &Node) {
        let mut s = escape(n.text.as_deref().unwrap_or(""));
        for m in n.marks.iter().rev() {
            s = wrap_mark(m, s);
        }
        self.out.push_str(&s);
    }
}

fn wrap_mark(m: &Mark, inner: String) -> String {
    match m.kind.as_str() {
        marks::BOLD => format!("<strong>{inner}</strong>"),
        marks::ITALIC => format!("<em>{inner}</em>"),
        marks::UNDERLINE => format!("<u>{inner}</u>"),
        marks::STRIKE => format!("<s>{inner}</s>"),
        marks::CODE => format!("<code>{inner}</code>"),
        marks::SUBSCRIPT => format!("<sub>{inner}</sub>"),
        marks::SUPERSCRIPT => format!("<sup>{inner}</sup>"),
        marks::LINK => match m.attr_str("href").and_then(safe_url) {
            Some(href) => format!("<a href=\"{}\" rel=\"noopener noreferrer\">{inner}</a>", escape(href)),
            None => inner,
        },
        marks::TEXT_STYLE => match m.attr_str("color").and_then(safe_color) {
            Some(c) => format!("<span style=\"color:{c}\">{inner}</span>"),
            None => inner,
        },
        marks::HIGHLIGHT => match m.attr_str("color").and_then(safe_color) {
            Some(c) => format!("<mark style=\"background-color:{c}\">{inner}</mark>"),
            None => format!("<mark>{inner}</mark>"),
        },
        marks::COMMENT => match m.attr_str("commentId") {
            Some(id) => format!("<span class=\"inline-comment\" data-comment-id=\"{}\">{inner}</span>", escape(id)),
            None => inner,
        },
        _ => inner,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn blocks_get_anchor_ids() {
        let doc = Node::doc(vec![
            Node::heading(2, vec![Node::text("Intro")]).attr("blockId", "hdg12345"),
            Node::paragraph(vec![Node::text("x")]).attr("blockId", "par12345").attr("textAlign", "center"),
            Node::new(nodes::HORIZONTAL_RULE).attr("blockId", "hr123456"),
            Node::paragraph(vec![Node::text("bad")]).attr("blockId", "a\"><script>"),
            Node::new(nodes::BULLET_LIST).attr("blockId", "lst12345").with_content(vec![
                Node::new(nodes::LIST_ITEM).attr("blockId", "itm12345").with_content(vec![Node::paragraph(vec![Node::text("i")]).attr("blockId", "inr12345")]),
            ]),
        ]);
        let html = render(&doc, &HtmlOptions { heading_ids: true });
        assert!(html.starts_with("<h2 id=\"intro\"><a id=\"block-hdg12345\" class=\"block-anchor\"></a>Intro</h2>"), "{html}");
        assert!(html.contains("<p id=\"block-par12345\" style=\"text-align:center\">x</p>"), "{html}");
        assert!(html.contains("<hr id=\"block-hr123456\">"), "{html}");
        assert!(html.contains("<p>bad</p>"), "invalid ids are dropped: {html}");
        let indented = Node::doc(vec![
            Node::paragraph(vec![Node::text("a")]).attr("indent", 2).attr("textAlign", "center"),
            Node::heading(3, vec![Node::text("b")]).attr("indent", 99),
            Node::paragraph(vec![Node::text("c")]).attr("indent", -1),
        ]);
        let table = Node::doc(vec![Node::new(nodes::TABLE).attr("numbered", true).with_content(vec![
            Node::new(nodes::TABLE_ROW).with_content(vec![Node::new(nodes::TABLE_CELL).with_content(vec![Node::paragraph(vec![Node::text("a")])])]),
        ])]);
        assert!(render(&table, &HtmlOptions::default()).starts_with("<table data-numbered=\"true\"><tbody>"));
        let out = render(&indented, &HtmlOptions::default());
        assert!(out.contains("<p style=\"text-align:center;margin-left:60px\">a</p>"), "{out}");
        assert!(out.contains("<h3 style=\"margin-left:240px\">b</h3>"), "capped at 8 levels: {out}");
        assert!(out.contains("<p>c</p>"), "{out}");
        assert!(html.contains("<ul id=\"block-lst12345\"><li id=\"block-itm12345\"><p id=\"block-inr12345\">i</p></li></ul>"), "{html}");
        assert!(!render(&doc, &HtmlOptions::default()).contains("block-"), "only with anchors on");
    }

    #[test]
    fn callout_custom_icon() {
        let mut ok = Node::new(nodes::CALLOUT).attr("type", "note").attr("icon", "rocket:violet");
        ok.content = vec![Node::paragraph(vec![Node::text("x")])];
        let html = render(&Node::doc(vec![ok]), &HtmlOptions::default());
        assert!(html.contains("callout-note") && html.contains("color:#7C3AED"), "{html}");
        let mut bad = Node::new(nodes::CALLOUT).attr("icon", "rocket:x\"><script>alert(1)</script>");
        bad.content = vec![Node::paragraph(vec![Node::text("x")])];
        let html = render(&Node::doc(vec![bad]), &HtmlOptions::default());
        assert!(!html.contains("<script"), "{html}");
        let mut unknown = Node::new(nodes::CALLOUT).attr("icon", "nope:violet");
        unknown.content = vec![Node::paragraph(vec![Node::text("x")])];
        assert!(render(&Node::doc(vec![unknown]), &HtmlOptions::default()).contains("callout-info"));
    }

    #[test]
    fn hostile_attributes_stay_inert() {
        let evil = "x\"><img src=x onerror=alert(1)>";
        let doc = Node::doc(vec![Node::paragraph(vec![
            Node::new(nodes::STATUS).attr("text", "ok").attr("color", evil),
            Node::new(nodes::MENTION).attr("id", evil).attr("label", evil),
            Node::new(nodes::DATE).attr("date", evil),
            Node::new(nodes::EMOTICON).attr("name", evil),
        ])]);
        let mut callout = Node::new(nodes::CALLOUT).attr("type", evil);
        callout.content = vec![Node::paragraph(vec![Node::text("x")])];
        let mut code = Node::new(nodes::CODE_BLOCK).attr("language", evil);
        code.content = vec![Node::text("x")];
        let mut all = doc.clone();
        all.content.push(callout);
        all.content.push(code);
        let html = render(&all, &HtmlOptions { heading_ids: true });
        assert!(!html.contains("<img src=x"), "{html}");
        assert!(html.contains("status status-grey"));
    }

    #[test]
    fn escapes_and_blocks_js_urls() {
        let doc = Node::doc(vec![Node::paragraph(vec![
            Node::text("<script>alert(1)</script>"),
            Node::text_marked("x", vec![Mark::link("javascript:alert(1)")]),
            Node::text_marked("y", vec![Mark::link("https://ok.example/?a=1&b=2")]),
            Node::text_marked("c", vec![Mark::new(marks::TEXT_STYLE).attr("color", "red;background:url(x)")]),
        ])]);
        let html = render(&doc, &HtmlOptions::default());
        assert!(html.contains("&lt;script&gt;"));
        assert!(!html.contains("javascript:"));
        assert!(html.contains("href=\"https://ok.example/?a=1&amp;b=2\""));
        assert!(!html.contains("url(x)"));
    }

    #[test]
    fn renders_toc_and_heading_ids() {
        let doc = Node::doc(vec![Node::new(nodes::TOC), Node::heading(2, vec![Node::text("Intro")])]);
        let html = render(&doc, &HtmlOptions { heading_ids: true });
        assert!(html.contains("<a href=\"#intro\">Intro</a>"));
        assert!(html.contains("<h2 id=\"intro\">"));
    }

    #[test]
    fn toc_options() {
        let doc = Node::doc(vec![
            Node::new(nodes::TOC).attr("minLevel", 2).attr("numbered", true).attr("exclude", "Appendix"),
            Node::new(nodes::TOC).attr("style", "flat").attr("maxLevel", 2),
            Node::heading(1, vec![Node::text("Title")]),
            Node::heading(2, vec![Node::text("Goals & scope")]),
            Node::heading(3, vec![Node::text("Must have")]),
            Node::heading(2, vec![Node::text("Appendix")]),
        ]);
        let html = render(&doc, &HtmlOptions { heading_ids: true });
        assert!(
            html.starts_with("<nav class=\"toc\"><ul><li class=\"toc-l1\"><a href=\"#goals-scope\">1 Goals &amp; scope</a></li><li class=\"toc-l2\"><a href=\"#must-have\">1.1 Must have</a></li></ul></nav>"),
            "{html}"
        );
        assert!(
            html.contains("<nav class=\"toc toc-flat\"><p><a href=\"#title\">Title</a> · <a href=\"#goals-scope\">Goals &amp; scope</a> · <a href=\"#appendix\">Appendix</a></p></nav>"),
            "{html}"
        );
    }
}
