//! Confluence storage format (XHTML + `ac:`/`ri:`) → ProseMirror JSON.
//!
//! References to other content are emitted as `wcimport:` placeholder URIs which the WikiCraft
//! importer rewrites once target ids are known:
//!
//! | Placeholder | Meaning |
//! |---|---|
//! | `wcimport:page/<sourceId>[#anchor]` | page or blog post contained in an import |
//! | `wcimport:missing/<spaceKey>/<title>` | link to a page not in the export (red link) |
//! | `wcimport:attachment/<sourceId>` | attachment binary |
//! | `wcimport:missing-attachment/<filename>` | attachment not in the export |
//! | `wcimport:space/<spaceKey>` | space home |
//! | `wcimport:user/<userKey>` | user (mention `id`) |
//! | `wcimport:inline/<markerRef>` | inline comment anchor (comment mark `commentId`) |

use std::collections::BTreeMap;

use serde::Serialize;
use serde_json::{Map, Value, json};
use wikicraft_doc_model::node::{normalize_text, sanitize_tree, wrap_inline_runs};
use wikicraft_doc_model::{Mark, Node, marks, nodes};

use crate::dom::{self, Dom, Element, ParseMode};

/// Looks up targets of references while converting one body.
pub trait Resolver {
    /// Space key of the content being converted.
    fn space_key(&self) -> &str;
    /// Source id of the content being converted (container for relative attachment refs).
    fn content_id(&self) -> &str;
    /// Page or blog post by (space key, title).
    fn page(&self, space_key: &str, title: &str) -> Option<String>;
    /// Whether a content id exists in the export.
    fn content_exists(&self, id: &str) -> bool;
    /// Attachment by filename on a container (`None` = current content).
    fn attachment(&self, container_id: &str, filename: &str) -> Option<String>;
    /// Display name for a user key / account id / username.
    fn user_name(&self, key: &str) -> Option<String>;
    /// Whether an absolute URL's host is (likely) the Confluence site the export comes from:
    /// title-based links (`/display/KEY/Title`) are only rewritten for those.
    fn site_host(&self, _host: &str) -> bool {
        true
    }
}

/// How well a macro is supported.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Support {
    /// Mapped to an equivalent native node.
    Native,
    /// Mapped, but some parameters or behaviour are lost.
    Partial,
    /// Kept as an inert `macro` placeholder node (name/params/body preserved).
    Placeholder,
}

/// Counters collected while converting bodies.
#[derive(Debug, Default, Clone, Serialize)]
pub struct Stats {
    pub bodies: u64,
    pub lenient_parses: u64,
    pub macros: BTreeMap<String, (u64, Option<Support>)>,
    pub page_links: u64,
    pub page_links_resolved: u64,
    pub page_links_missing: u64,
    pub attachment_refs: u64,
    pub attachment_refs_resolved: u64,
    pub user_refs: u64,
    pub user_refs_resolved: u64,
    pub images: u64,
    pub tasks: u64,
    pub inline_comment_markers: u64,
    pub layouts: u64,
    pub emoticons: u64,
    pub wiki_markup_bodies: u64,
    /// Absolute links into the Confluence site turned into internal links.
    pub site_links_rewritten: u64,
    /// Up to 50 samples of `title → target` for missing links.
    pub missing_link_samples: Vec<String>,
    pub missing_attachment_samples: Vec<String>,
}

impl Stats {
    fn macro_seen(&mut self, name: &str, support: Support) {
        let e = self.macros.entry(name.to_string()).or_insert((0, Some(support)));
        e.0 += 1;
        e.1 = Some(support);
    }

    pub fn merge(&mut self, o: &Stats) {
        self.bodies += o.bodies;
        self.lenient_parses += o.lenient_parses;
        for (k, (n, s)) in &o.macros {
            let e = self.macros.entry(k.clone()).or_insert((0, *s));
            e.0 += n;
        }
        self.page_links += o.page_links;
        self.page_links_resolved += o.page_links_resolved;
        self.page_links_missing += o.page_links_missing;
        self.attachment_refs += o.attachment_refs;
        self.attachment_refs_resolved += o.attachment_refs_resolved;
        self.user_refs += o.user_refs;
        self.user_refs_resolved += o.user_refs_resolved;
        self.images += o.images;
        self.tasks += o.tasks;
        self.inline_comment_markers += o.inline_comment_markers;
        self.layouts += o.layouts;
        self.emoticons += o.emoticons;
        self.wiki_markup_bodies += o.wiki_markup_bodies;
        self.site_links_rewritten += o.site_links_rewritten;
        for s in &o.missing_link_samples {
            if self.missing_link_samples.len() < 50 {
                self.missing_link_samples.push(s.clone());
            }
        }
        for s in &o.missing_attachment_samples {
            if self.missing_attachment_samples.len() < 50 {
                self.missing_attachment_samples.push(s.clone());
            }
        }
    }
}

/// Converts a storage-format body into a ProseMirror document.
pub fn convert(body: &str, r: &dyn Resolver, stats: &mut Stats) -> Node {
    stats.bodies += 1;
    let (root, mode) = dom::parse(body);
    if mode == ParseMode::Lenient {
        stats.lenient_parses += 1;
    }
    convert_children(&root, r, stats)
}

/// Converts the children of an already parsed element (plain HTML pages: the caller picks the
/// content element and drops navigation first).
pub fn convert_element(root: &Element, r: &dyn Resolver, stats: &mut Stats) -> Node {
    stats.bodies += 1;
    convert_children(root, r, stats)
}

fn convert_children(root: &Element, r: &dyn Resolver, stats: &mut Stats) -> Node {
    let mut c = Conv { r, stats };
    let content = c.children(&root.children, &[], false);
    let mut doc = Node::doc(wrap_inline_runs(content, true));
    sanitize_tree(&mut doc);
    doc
}

/// Converts a legacy wiki-markup body.
pub fn convert_wiki(body: &str, r: &dyn Resolver, stats: &mut Stats) -> Node {
    stats.bodies += 1;
    stats.wiki_markup_bodies += 1;
    let mut doc = crate::wiki::wiki_to_pm(body);
    resolve_wiki_refs(&mut doc, r, stats);
    doc
}

/// The wiki-markup parser has no export at hand: it leaves its link targets
/// (`wcimport:wikilink/<space>/<rest>`) and image sources (`wcimport:missing-attachment/<raw>`)
/// as written. This resolves them like storage-format references, with the same placeholders
/// and statistics.
fn resolve_wiki_refs(doc: &mut Node, r: &dyn Resolver, stats: &mut Stats) {
    doc.walk_mut(&mut |n| {
        let mut label = None;
        for m in n.marks.iter_mut().filter(|m| m.kind == marks::LINK) {
            let Some(rest) = m.attr_str("href").and_then(|h| h.strip_prefix("wcimport:wikilink/")).map(String::from) else { continue };
            let (space, title) = rest.split_once('/').unwrap_or(("", rest.as_str()));
            let target = if space.is_empty() { title.to_string() } else { format!("{space}:{title}") };
            let href = wiki_target(&target, r, stats);
            m.attrs.insert("href".into(), serde_json::Value::String(href));
            // a link without alias shows its target: like Confluence, show the title / file name
            if n.text.as_deref().is_some_and(|t| t.trim() == target.trim()) {
                label = Some(wiki_label(&target, r));
            }
        }
        if let Some(l) = label.filter(|l| !l.is_empty()) {
            n.text = Some(l);
        }
        if n.kind == nodes::IMAGE
            && let Some(raw) = n.attr_str("src").and_then(|s| s.strip_prefix("wcimport:missing-attachment/")).map(String::from)
        {
            let (src, filename) = wiki_attachment(&raw, r, stats);
            // the importer replaces a missing image with a marker that names it
            if src.starts_with("wcimport:missing-attachment/") && !n.attrs.contains_key("alt") {
                n.attrs.insert("title".into(), serde_json::Value::String(filename));
            }
            n.attrs.insert("src".into(), serde_json::Value::String(src));
        }
    });
}

/// `[SPACE:Title#anchor]`, `[Title]`, `[SPACE:]` (space home), `[/2024/01/31/Blog title]`,
/// `[Title^file.pdf]`, `[^file.pdf]` → a placeholder href.
fn wiki_target(target: &str, r: &dyn Resolver, stats: &mut Stats) -> String {
    if target.contains('^') {
        return wiki_attachment(target, r, stats).0;
    }
    let (target, anchor) = match target.split_once('#') {
        Some((t, a)) => (t, Some(a)),
        None => (target, None),
    };
    let (space, title) = split_space(target, r);
    stats.page_links += 1;
    if title.is_empty() {
        stats.page_links_resolved += 1;
        return format!("wcimport:space/{}", enc(&space));
    }
    // blog posts: /YYYY/MM/DD/Title
    let title = blog_title(&title).unwrap_or(title);
    let found = r.page(&space, &title).or_else(|| (target.contains(':')).then(|| r.page(r.space_key(), target.trim())).flatten());
    match found {
        Some(id) => {
            stats.page_links_resolved += 1;
            placeholder_page(&id, anchor)
        }
        None => {
            stats.page_links_missing += 1;
            if stats.missing_link_samples.len() < 50 {
                stats.missing_link_samples.push(format!("{space}/{title}"));
            }
            format!("wcimport:missing/{}/{}", enc(&space), enc(&title))
        }
    }
}

/// `file.png`, `Title^file.png`, `SPACE:Title^file.png` → (placeholder, filename).
fn wiki_attachment(raw: &str, r: &dyn Resolver, stats: &mut Stats) -> (String, String) {
    stats.attachment_refs += 1;
    let (page, filename) = raw.rsplit_once('^').unwrap_or(("", raw));
    let filename = filename.trim().to_string();
    let container = if page.trim().is_empty() {
        Some(r.content_id().to_string())
    } else {
        let (space, title) = split_space(page, r);
        r.page(&space, &blog_title(&title).unwrap_or(title))
    };
    match container.and_then(|c| r.attachment(&c, &filename)) {
        Some(id) => {
            stats.attachment_refs_resolved += 1;
            (format!("wcimport:attachment/{id}"), filename)
        }
        None => {
            if stats.missing_attachment_samples.len() < 50 {
                stats.missing_attachment_samples.push(filename.clone());
            }
            (format!("wcimport:missing-attachment/{}", enc(&filename)), filename)
        }
    }
}

/// What Confluence shows for a link without alias: the file name, the page or post title, or
/// the space key.
fn wiki_label(target: &str, r: &dyn Resolver) -> String {
    if let Some((_, file)) = target.rsplit_once('^') {
        return file.trim().to_string();
    }
    let target = target.split_once('#').map_or(target, |(t, _)| t);
    let (space, title) = split_space(target, r);
    if title.is_empty() {
        return space;
    }
    blog_title(&title).unwrap_or(title)
}

/// `SPACE:Title` → (SPACE, Title); anything else is a title in the current space. A colon in a
/// title ("Meeting: 2024") is not a space key: keys have no spaces.
fn split_space(target: &str, r: &dyn Resolver) -> (String, String) {
    match target.split_once(':') {
        Some((s, t)) if !s.is_empty() && !s.contains(char::is_whitespace) => (s.to_string(), t.trim().to_string()),
        _ => (r.space_key().to_string(), target.trim().to_string()),
    }
}

/// The title of a blog post link `/2024/01/31/Title`.
fn blog_title(path: &str) -> Option<String> {
    let mut parts = path.strip_prefix('/')?.splitn(4, '/');
    let (y, m, d, title) = (parts.next()?, parts.next()?, parts.next()?, parts.next()?);
    [y, m, d].iter().all(|p| p.chars().all(|c| c.is_ascii_digit())).then(|| title.to_string())
}

/// Confluence's heading anchors are "PageTitle-HeadingText" (spaces dropped); ours are the
/// heading's slug, so the title part goes.
fn heading_anchor(anchor: Option<String>, title: Option<&str>) -> Option<String> {
    let anchor = anchor?;
    let Some(title) = title else { return Some(anchor) };
    let squashed: String = title.chars().filter(|c| !c.is_whitespace()).collect();
    Some(anchor.strip_prefix(&format!("{squashed}-")).map(String::from).unwrap_or(anchor))
}

pub fn placeholder_page(id: &str, anchor: Option<&str>) -> String {
    match anchor.filter(|a| !a.is_empty()) {
        Some(a) => format!("wcimport:page/{id}#{}", wikicraft_doc_model::text::slugify(a)),
        None => format!("wcimport:page/{id}"),
    }
}

fn enc(s: &str) -> String {
    let mut o = String::new();
    for b in s.bytes() {
        if b.is_ascii_alphanumeric() || b"-_.~ ".contains(&b) {
            o.push(if b == b' ' { '+' } else { b as char });
        } else {
            o.push_str(&format!("%{b:02X}"));
        }
    }
    o
}

struct Conv<'a> {
    r: &'a dyn Resolver,
    stats: &'a mut Stats,
}

fn style_prop<'s>(el: &'s Element, prop: &str) -> Option<&'s str> {
    let style = el.attr("style")?;
    style.split(';').find_map(|decl| {
        let (k, v) = decl.split_once(':')?;
        (k.trim().eq_ignore_ascii_case(prop)).then(|| v.trim())
    })
}

fn add_mark(active: &[Mark], m: Mark) -> Vec<Mark> {
    let mut v: Vec<Mark> = active.iter().filter(|x| x.kind != m.kind).cloned().collect();
    v.push(m);
    v
}

fn collapse_ws(s: &str) -> String {
    let mut o = String::with_capacity(s.len());
    let mut ws = false;
    for ch in s.chars() {
        if matches!(ch, ' ' | '\t' | '\n' | '\r') {
            if !ws {
                o.push(' ');
            }
            ws = true;
        } else {
            o.push(ch);
            ws = false;
        }
    }
    o
}

/// Splits a text block (paragraph/heading) whose converted children contain block nodes:
/// inline runs stay in copies of `template`, blocks are lifted out in order.
fn split_text_block(template: Node, children: Vec<Node>) -> Vec<Node> {
    if children.iter().all(Node::is_inline) {
        let mut n = template;
        n.content = children;
        normalize_text(&mut n.content);
        return vec![n];
    }
    let mut out = Vec::new();
    let mut run = Vec::new();
    for c in children {
        if c.is_inline() {
            run.push(c);
        } else {
            if run.iter().any(|n: &Node| !n.is_text() || n.text.as_deref().is_some_and(|t| !t.trim().is_empty())) {
                let mut n = template.clone();
                n.content = std::mem::take(&mut run);
                normalize_text(&mut n.content);
                out.push(n);
            }
            run.clear();
            out.push(c);
        }
    }
    if run.iter().any(|n| !n.is_text() || n.text.as_deref().is_some_and(|t| !t.trim().is_empty())) {
        let mut n = template;
        n.content = run;
        normalize_text(&mut n.content);
        out.push(n);
    }
    out
}

fn callout_type_for_macro(name: &str) -> &'static str {
    match name {
        "tip" => "success",
        "note" => "warning",
        "warning" => "error",
        "panel" => "note",
        _ => "info",
    }
}

fn status_color(c: &str) -> &'static str {
    match c.to_ascii_lowercase().as_str() {
        "red" => "red",
        "yellow" => "yellow",
        "green" => "green",
        "blue" => "blue",
        "purple" => "purple",
        _ => "grey",
    }
}

impl Conv<'_> {
    fn children(&mut self, children: &[Dom], marks: &[Mark], pre: bool) -> Vec<Node> {
        let mut out = Vec::new();
        for c in children {
            match c {
                Dom::Text(t) => {
                    let text = if pre { t.clone() } else { collapse_ws(t) };
                    if !text.is_empty() {
                        out.push(Node::text_marked(text, marks.to_vec()));
                    }
                }
                Dom::Element(e) => out.extend(self.element(e, marks, pre)),
            }
        }
        out
    }

    fn blocks(&mut self, children: &[Dom]) -> Vec<Node> {
        let c = self.children(children, &[], false);
        wrap_inline_runs(c, true)
    }

    fn element(&mut self, el: &Element, marks: &[Mark], pre: bool) -> Vec<Node> {
        let name = el.name.as_str();
        match name {
            "p" => {
                let mut tpl = Node::new(nodes::PARAGRAPH);
                if let Some(a) = style_prop(el, "text-align").filter(|a| matches!(*a, "left" | "center" | "right" | "justify")) {
                    tpl = tpl.attr("textAlign", a);
                }
                let c = self.children(&el.children, marks, pre);
                split_text_block(tpl, c)
            }
            "h1" | "h2" | "h3" | "h4" | "h5" | "h6" => {
                let level: u8 = name[1..].parse().unwrap_or(1);
                let c = self.children(&el.children, marks, pre);
                split_text_block(Node::heading(level, vec![]), c)
            }
            "ul" | "ol" => {
                let items: Vec<Node> = el
                    .children
                    .iter()
                    .filter_map(|c| match c {
                        Dom::Element(li) if li.name == "li" => Some(Node::new(nodes::LIST_ITEM).with_content(self.blocks(&li.children))),
                        Dom::Element(other) => {
                            let inner = self.element(other, &[], false);
                            (!inner.is_empty()).then(|| Node::new(nodes::LIST_ITEM).with_content(wrap_inline_runs(inner, true)))
                        }
                        Dom::Text(t) if !t.trim().is_empty() => {
                            Some(Node::new(nodes::LIST_ITEM).with_content(vec![Node::paragraph(vec![Node::text(collapse_ws(t).trim().to_string())])]))
                        }
                        _ => None,
                    })
                    .collect();
                if items.is_empty() {
                    return vec![];
                }
                if name == "ol" {
                    let start = el.attr("start").and_then(|s| s.parse::<i64>().ok()).unwrap_or(1);
                    vec![Node::new(nodes::ORDERED_LIST).attr("start", start).with_content(items)]
                } else {
                    vec![Node::new(nodes::BULLET_LIST).with_content(items)]
                }
            }
            "li" => vec![Node::new(nodes::LIST_ITEM).with_content(self.blocks(&el.children))],
            "table" => vec![self.table(el)],
            "pre" => {
                let text = el.text();
                vec![Node::new(nodes::CODE_BLOCK).with_content(if text.is_empty() { vec![] } else { vec![Node::text(text)] })]
            }
            "blockquote" => vec![Node::new(nodes::BLOCKQUOTE).with_content(self.blocks(&el.children))],
            "hr" => vec![Node::new(nodes::HORIZONTAL_RULE)],
            "br" => vec![Node::new(nodes::HARD_BREAK)],
            "strong" | "b" => self.children(&el.children, &add_mark(marks, Mark::new(marks::BOLD)), pre),
            "em" | "i" | "cite" | "dfn" => self.children(&el.children, &add_mark(marks, Mark::new(marks::ITALIC)), pre),
            "u" | "ins" => self.children(&el.children, &add_mark(marks, Mark::new(marks::UNDERLINE)), pre),
            "s" | "del" | "strike" => self.children(&el.children, &add_mark(marks, Mark::new(marks::STRIKE)), pre),
            "code" | "tt" | "kbd" | "samp" | "var" => self.children(&el.children, &add_mark(marks, Mark::new(marks::CODE)), pre),
            "sub" => self.children(&el.children, &add_mark(marks, Mark::new(marks::SUBSCRIPT)), pre),
            "sup" => self.children(&el.children, &add_mark(marks, Mark::new(marks::SUPERSCRIPT)), pre),
            "span" | "font" | "small" | "big" | "abbr" | "q" | "label" | "mark" => {
                let mut m = marks.to_vec();
                let color = style_prop(el, "color").or_else(|| el.attr("color"));
                if let Some(c) = color {
                    m = add_mark(&m, Mark::new(marks::TEXT_STYLE).attr("color", c));
                }
                let bg = el.attr("data-highlight-colour").or_else(|| style_prop(el, "background-color"));
                if let Some(c) = bg {
                    m = add_mark(&m, Mark::new(marks::HIGHLIGHT).attr("color", c));
                } else if name == "mark" {
                    m = add_mark(&m, Mark::new(marks::HIGHLIGHT));
                }
                if matches!(style_prop(el, "text-decoration"), Some(d) if d.contains("line-through")) {
                    m = add_mark(&m, Mark::new(marks::STRIKE));
                }
                if matches!(style_prop(el, "text-decoration"), Some(d) if d.contains("underline")) {
                    m = add_mark(&m, Mark::new(marks::UNDERLINE));
                }
                self.children(&el.children, &m, pre)
            }
            "a" => {
                let href = el.attr("href").unwrap_or("");
                if href.is_empty() {
                    if let Some(n) = el.attr("name").or(el.attr("id")) {
                        let mut v = vec![Node::new(nodes::ANCHOR).attr("name", n)];
                        v.extend(self.children(&el.children, marks, pre));
                        return v;
                    }
                    return self.children(&el.children, marks, pre);
                }
                // a full URL of a page / attachment of this Confluence site: an internal link
                if let Some(internal) = self.site_link(href) {
                    let m = add_mark(marks, Mark::link(internal));
                    let c = self.children(&el.children, &m, pre);
                    return if c.is_empty() { vec![Node::text_marked(href, m)] } else { c };
                }
                // smart links shown as a card or an embed (videos, documents): an embed block
                if matches!(el.attr("data-card-appearance"), Some("block" | "embed")) && (href.starts_with("https://") || href.starts_with("http://")) {
                    self.stats.macro_seen("smart-link", Support::Native);
                    return vec![Node::new(nodes::EMBED).attr("url", href)];
                }
                let m = add_mark(marks, Mark::link(href));
                let c = self.children(&el.children, &m, pre);
                if c.is_empty() { vec![Node::text_marked(href, m)] } else { c }
            }
            "img" => {
                self.stats.images += 1;
                let src = el.attr("src").unwrap_or("");
                let src = self.site_link(src).filter(|s| s.starts_with("wcimport:attachment/")).unwrap_or_else(|| src.to_string());
                vec![Node::new(nodes::IMAGE).attr("src", src).attr("alt", el.attr("alt").map(String::from)).attr("title", el.attr("title").map(String::from))]
            }
            "time" => {
                let d = el.attr("datetime").map(String::from).unwrap_or_else(|| el.text());
                vec![Node::new(nodes::DATE).attr("date", d)]
            }
            "ac:structured-macro" | "ac:macro" => self.macro_(el, marks),
            "ac:link" => self.link(el, marks),
            "ac:image" => self.image(el, None),
            "ac:task-list" => vec![self.task_list(el)],
            "ac:layout" => {
                self.stats.layouts += 1;
                let mut out = Vec::new();
                for section in el.children_named("ac:layout-section") {
                    let cells: Vec<&Element> = section.children_named("ac:layout-cell").collect();
                    if cells.len() <= 1 {
                        for c in cells {
                            out.extend(self.blocks(&c.children));
                        }
                    } else {
                        let cols = cells.iter().map(|c| Node::new(nodes::COLUMN).with_content(self.blocks(&c.children))).collect();
                        out.push(Node::new(nodes::COLUMNS).attr("layout", section.attr("ac:type").map(String::from)).with_content(cols));
                    }
                }
                out
            }
            "ac:emoticon" => {
                self.stats.emoticons += 1;
                // Confluence's own emoticons are drawn by our icon set
                let name = el.attr("ac:name").unwrap_or("");
                if wikicraft_doc_model::icons::is_emoticon(name) {
                    return vec![Node::new(nodes::EMOTICON).attr("name", name)];
                }
                // any other emoji a Cloud user inserted is their content: keep the character
                let emoji = el
                    .attr("ac:emoji-fallback")
                    .filter(|f| !f.starts_with(':'))
                    .map(String::from)
                    .or_else(|| {
                        el.attr("ac:emoji-id").and_then(|id| {
                            let chars: Option<String> = id.split('-').map(|p| u32::from_str_radix(p, 16).ok().and_then(char::from_u32)).collect();
                            chars
                        })
                    })
                    .unwrap_or_default();
                if emoji.is_empty() { vec![] } else { vec![Node::text_marked(emoji, marks.to_vec())] }
            }
            // instructional text for authors: kept, but never shown to readers or exported
            "ac:placeholder" => {
                let t = el.text().split_whitespace().collect::<Vec<_>>().join(" ");
                if t.is_empty() { vec![] } else { vec![Node::new(nodes::PLACEHOLDER).attr("text", t)] }
            }
            "ac:inline-comment-marker" => {
                self.stats.inline_comment_markers += 1;
                let m = match el.attr("ac:ref") {
                    Some(r) => add_mark(marks, Mark::new(marks::COMMENT).attr("commentId", format!("wcimport:inline/{r}"))),
                    None => marks.to_vec(),
                };
                self.children(&el.children, &m, pre)
            }
            "ac:adf-extension" => self.adf_extension(el),
            "ac:adf-node" => self.adf_node(el),
            "ac:adf-content" | "ac:adf-fallback" | "ac:rich-text-body" | "ac:link-body" | "div" | "section" | "article" | "header" | "footer" | "main"
            | "nav" | "aside" | "center" | "figure" | "colgroup" | "tbody" | "thead" | "tfoot" | "wc-root" => self.children(&el.children, marks, pre),
            "figcaption" => {
                let c = self.children(&el.children, &add_mark(marks, Mark::new(marks::ITALIC)), pre);
                vec![Node::paragraph(c)]
            }
            "dl" => self.children(&el.children, marks, pre),
            "dt" => vec![Node::paragraph(self.children(&el.children, &add_mark(marks, Mark::new(marks::BOLD)), pre))],
            "dd" => vec![Node::new(nodes::BLOCKQUOTE).with_content(self.blocks(&el.children))],
            "ac:parameter" | "ac:task-id" | "ac:task-uuid" | "ac:task-status" | "col" | "script" | "style" | "ac:caption" => vec![],
            "ac:plain-text-body" | "ac:plain-text-link-body" => {
                let t = el.text();
                if t.is_empty() { vec![] } else { vec![Node::text_marked(t, marks.to_vec())] }
            }
            n if n.starts_with("ri:") => vec![],
            n if n.starts_with("at:") => self.children(&el.children, marks, pre),
            _ => self.children(&el.children, marks, pre),
        }
    }

    fn table(&mut self, el: &Element) -> Node {
        let mut widths: Vec<Option<i64>> = Vec::new();
        for cg in el.children_named("colgroup") {
            for col in cg.children_named("col") {
                let w =
                    style_prop(col, "width").or(col.attr("width")).and_then(|w| w.trim_end_matches("px").trim().parse::<f64>().ok()).map(|f| f.round() as i64);
                widths.push(w);
            }
        }
        let mut rows = Vec::new();
        let collect_rows = |container: &Element, rows: &mut Vec<Node>, this: &mut Self| {
            for tr in container.children_named("tr") {
                let mut cells = Vec::new();
                let mut col_idx = 0usize;
                for cell in tr.elements() {
                    if cell.name != "td" && cell.name != "th" {
                        continue;
                    }
                    let colspan = cell.attr("colspan").and_then(|v| v.parse::<i64>().ok()).unwrap_or(1).max(1);
                    let rowspan = cell.attr("rowspan").and_then(|v| v.parse::<i64>().ok()).unwrap_or(1).max(1);
                    let mut n =
                        Node::new(if cell.name == "th" { nodes::TABLE_HEADER } else { nodes::TABLE_CELL }).attr("colspan", colspan).attr("rowspan", rowspan);
                    let cw: Vec<i64> = (0..colspan as usize).filter_map(|i| widths.get(col_idx + i).copied().flatten()).collect();
                    if cw.len() == colspan as usize {
                        n = n.attr("colwidth", json!(cw));
                    }
                    if let Some(bg) = cell.attr("data-highlight-colour").or_else(|| style_prop(cell, "background-color")) {
                        n = n.attr("background", bg);
                    }
                    col_idx += colspan as usize;
                    n.content = this.blocks(&cell.children);
                    cells.push(n);
                }
                if !cells.is_empty() {
                    rows.push(Node::new(nodes::TABLE_ROW).with_content(cells));
                }
            }
        };
        collect_rows(el, &mut rows, self);
        for section in ["thead", "tbody", "tfoot"] {
            for s in el.children_named(section) {
                collect_rows(s, &mut rows, self);
            }
        }
        let mut t = Node::new(nodes::TABLE).with_content(rows);
        if let Some(l) = el.attr("data-layout") {
            t = t.attr("layout", l);
        }
        t
    }

    fn task_list(&mut self, el: &Element) -> Node {
        let mut items = Vec::new();
        for task in el.children_named("ac:task") {
            self.stats.tasks += 1;
            let done = task.child("ac:task-status").map(Element::text).is_some_and(|s| s.trim() == "complete");
            let mut content = Vec::new();
            if let Some(body) = task.child("ac:task-body") {
                content.extend(self.children(&body.children, &[], false));
            }
            for nested in task.children_named("ac:task-list") {
                content.push(self.task_list(nested));
            }
            let mut item = Node::new(nodes::TASK_ITEM).attr("checked", done).with_content(wrap_inline_runs(content, true));
            if let Some(id) = task.child("ac:task-uuid").map(Element::text).filter(|s| !s.is_empty()) {
                item = item.attr("taskId", id);
            }
            items.push(item);
        }
        Node::new(nodes::TASK_LIST).with_content(items)
    }

    /// An absolute URL into the Confluence site this export comes from, as a placeholder for
    /// the page or attachment it points at; None when it isn't one or doesn't resolve (then
    /// the URL stays as it is: another site, or a space that isn't migrated).
    fn site_link(&mut self, href: &str) -> Option<String> {
        let (host, target) = crate::urls::parse(href)?;
        let placeholder = match target {
            crate::urls::Target::PageId { id, title, anchor } => {
                if !self.r.content_exists(&id) {
                    return None;
                }
                placeholder_page(&id, heading_anchor(anchor, title.as_deref()).as_deref())
            }
            crate::urls::Target::Tiny { id } => self.r.content_exists(&id).then(|| placeholder_page(&id, None))?,
            crate::urls::Target::Title { space, title, anchor } => {
                if !self.r.site_host(&host) {
                    return None;
                }
                let id = self.r.page(&space, &title)?;
                placeholder_page(&id, heading_anchor(anchor, Some(&title)).as_deref())
            }
            crate::urls::Target::Attachment { page_id, filename } => format!("wcimport:attachment/{}", self.r.attachment(&page_id, &filename)?),
        };
        self.stats.site_links_rewritten += 1;
        Some(placeholder)
    }

    fn resolve_page_ref(&mut self, page: &Element) -> String {
        self.stats.page_links += 1;
        if let Some(id) = page.attr("ri:content-id").filter(|id| self.r.content_exists(id)) {
            self.stats.page_links_resolved += 1;
            return placeholder_page(id, None);
        }
        let space = page.attr("ri:space-key").unwrap_or(self.r.space_key()).to_string();
        let title = page.attr("ri:content-title").unwrap_or("").to_string();
        if let Some(id) = self.r.page(&space, &title) {
            self.stats.page_links_resolved += 1;
            return placeholder_page(&id, None);
        }
        self.stats.page_links_missing += 1;
        if self.stats.missing_link_samples.len() < 50 {
            self.stats.missing_link_samples.push(format!("{space}/{title}"));
        }
        format!("wcimport:missing/{}/{}", enc(&space), enc(&title))
    }

    /// Resolves `<ri:attachment>` (optionally scoped by a nested `<ri:page>`/`<ri:blog-post>`).
    fn resolve_attachment_ref(&mut self, att: &Element) -> (String, String) {
        self.stats.attachment_refs += 1;
        let filename = att.attr("ri:filename").unwrap_or("").to_string();
        let container = att
            .elements()
            .find(|e| e.name == "ri:page" || e.name == "ri:blog-post")
            .and_then(|p| {
                let space = p.attr("ri:space-key").unwrap_or(self.r.space_key()).to_string();
                self.r.page(&space, p.attr("ri:content-title").unwrap_or(""))
            })
            .unwrap_or_else(|| self.r.content_id().to_string());
        match self.r.attachment(&container, &filename) {
            Some(id) => {
                self.stats.attachment_refs_resolved += 1;
                (format!("wcimport:attachment/{id}"), filename)
            }
            None => {
                if self.stats.missing_attachment_samples.len() < 50 {
                    self.stats.missing_attachment_samples.push(filename.clone());
                }
                (format!("wcimport:missing-attachment/{}", enc(&filename)), filename)
            }
        }
    }

    fn user(&mut self, u: &Element) -> Node {
        self.stats.user_refs += 1;
        let key = u.attr("ri:account-id").or(u.attr("ri:userkey")).or(u.attr("ri:username")).unwrap_or("").to_string();
        let label = match self.r.user_name(&key) {
            Some(n) => {
                self.stats.user_refs_resolved += 1;
                n
            }
            None => u.attr("ri:username").map(String::from).unwrap_or_else(|| "Unknown user".into()),
        };
        Node::new(nodes::MENTION).attr("id", format!("wcimport:user/{key}")).attr("label", label)
    }

    fn link(&mut self, el: &Element, marks: &[Mark]) -> Vec<Node> {
        let anchor = el.attr("ac:anchor");
        let body_children = el.child("ac:link-body").map(|b| b.children.clone());
        let plain = el.child("ac:plain-text-link-body").map(Element::text);
        let target = el.elements().find(|e| e.name.starts_with("ri:"));
        let (href, default_text) = match target {
            Some(t) if t.name == "ri:user" => return vec![self.user(t)],
            Some(t) if t.name == "ri:page" || t.name == "ri:blog-post" || t.name == "ri:content-entity" => {
                let mut href = self.resolve_page_ref(t);
                if let Some(a) = anchor.filter(|a| !a.is_empty())
                    && !href.contains('#')
                    && href.starts_with("wcimport:page/")
                {
                    href = format!("{href}#{}", wikicraft_doc_model::text::slugify(a));
                }
                (href, t.attr("ri:content-title").unwrap_or("link").to_string())
            }
            Some(t) if t.name == "ri:attachment" => self.resolve_attachment_ref(t),
            Some(t) if t.name == "ri:url" => {
                let v = t.attr("ri:value").unwrap_or("").to_string();
                (v.clone(), v)
            }
            Some(t) if t.name == "ri:space" => {
                let k = t.attr("ri:space-key").unwrap_or("").to_string();
                (format!("wcimport:space/{}", enc(&k)), k)
            }
            Some(t) if t.name == "ri:shortcut" => {
                let k = t.attr("ri:key").unwrap_or("");
                let p = t.attr("ri:parameter").unwrap_or("");
                (String::new(), format!("{k}:{p}"))
            }
            _ => match anchor {
                Some(a) => (format!("#{}", wikicraft_doc_model::text::slugify(a)), a.to_string()),
                None => (String::new(), String::new()),
            },
        };
        let m = if href.is_empty() { marks.to_vec() } else { add_mark(marks, Mark::link(href)) };
        let mut out = if let Some(children) = body_children {
            self.children(&children, &m, false)
        } else if let Some(p) = plain.filter(|p| !p.is_empty()) {
            vec![Node::text_marked(p, m.clone())]
        } else {
            vec![]
        };
        if out.iter().all(|n| n.is_text() && n.text.as_deref().is_some_and(|t| t.trim().is_empty())) {
            out = if default_text.is_empty() { vec![] } else { vec![Node::text_marked(default_text, m)] };
        }
        out
    }

    fn image(&mut self, el: &Element, _marks: Option<&[Mark]>) -> Vec<Node> {
        self.stats.images += 1;
        let src = if let Some(att) = el.child("ri:attachment") {
            self.resolve_attachment_ref(att).0
        } else if let Some(u) = el.child("ri:url") {
            u.attr("ri:value").unwrap_or("").to_string()
        } else {
            String::new()
        };
        let caption = el.child("ac:caption").map(|c| c.text().trim().to_string()).filter(|c| !c.is_empty());
        let width = el.attr("ac:width").or(el.attr("ac:original-width")).and_then(|w| w.parse::<f64>().ok()).map(|f| f.round() as i64);
        let align = el.attr("ac:align").or(el.attr("ac:layout")).map(|a| match a {
            "center" | "align-center" => "center",
            "right" | "align-end" | "wrap-right" => "right",
            "left" | "align-start" | "wrap-left" => "left",
            _ => "center",
        });
        vec![
            Node::new(nodes::IMAGE)
                .attr("src", src)
                .attr("alt", el.attr("ac:alt").map(String::from).or_else(|| caption.clone()))
                .attr("title", el.attr("ac:title").map(String::from))
                .attr("caption", caption)
                .attr("width", width)
                .attr("align", align),
        ]
    }

    fn adf_extension(&mut self, el: &Element) -> Vec<Node> {
        if let Some(node) = el.child("ac:adf-node") {
            let converted = self.adf_node(node);
            if !converted.is_empty() {
                return converted;
            }
        }
        match el.child("ac:adf-fallback") {
            Some(f) => self.children(&f.children, &[], false),
            None => vec![],
        }
    }

    fn adf_attr(node: &Element, key: &str) -> Option<String> {
        node.children_named("ac:adf-attribute").find(|a| a.attr("key") == Some(key)).map(Element::text)
    }

    fn adf_node(&mut self, node: &Element) -> Vec<Node> {
        let ty = node.attr("type").unwrap_or("");
        match ty {
            "decision-list" => {
                self.stats.macro_seen("adf:decision-list", Support::Native);
                let items = node
                    .children_named("ac:adf-node")
                    .filter(|n| n.attr("type") == Some("decision-item"))
                    .map(|item| {
                        let state = Self::adf_attr(item, "state").unwrap_or_else(|| "DECIDED".into());
                        let content = item.child("ac:adf-content").map(|c| self.children(&c.children, &[], false)).unwrap_or_default();
                        Node::new(nodes::DECISION_ITEM).attr("state", state).with_content(wrap_inline_runs(content, true))
                    })
                    .collect();
                vec![Node::new(nodes::DECISION_LIST).with_content(items)]
            }
            "panel" => {
                self.stats.macro_seen("adf:panel", Support::Native);
                let kind = match Self::adf_attr(node, "panel-type").as_deref() {
                    Some("note") => "note",
                    Some("success") => "success",
                    Some("warning") => "warning",
                    Some("error") => "error",
                    _ => "info",
                };
                let content = node.child("ac:adf-content").map(|c| self.blocks(&c.children)).unwrap_or_default();
                let emoji = Self::adf_attr(node, "panel-icon-text");
                vec![Node::new(nodes::CALLOUT).attr("type", kind).attr("emoji", emoji).with_content(content)]
            }
            "expand" | "nested-expand" => {
                self.stats.macro_seen("adf:expand", Support::Native);
                let title = Self::adf_attr(node, "title").unwrap_or_default();
                let content = node.child("ac:adf-content").map(|c| self.blocks(&c.children)).unwrap_or_default();
                vec![details(title, content)]
            }
            _ => {
                self.stats.macro_seen(&format!("adf:{ty}"), Support::Placeholder);
                vec![]
            }
        }
    }

    fn macro_(&mut self, el: &Element, marks: &[Mark]) -> Vec<Node> {
        let name = el.attr("ac:name").unwrap_or("unknown").to_ascii_lowercase();
        let rich = el.child("ac:rich-text-body");
        let plain = el.child("ac:plain-text-body").map(Element::text);
        let param = |k: &str| el.macro_param(k).filter(|v| !v.is_empty());
        match name.as_str() {
            "info" | "tip" | "note" | "warning" | "panel" | "information" => {
                self.stats.macro_seen(&name, Support::Native);
                let mut content = Vec::new();
                if let Some(t) = param("title") {
                    content.push(Node::paragraph(vec![Node::text_marked(t, vec![Mark::new(marks::BOLD)])]));
                }
                if let Some(r) = rich {
                    content.extend(self.blocks(&r.children));
                }
                vec![Node::new(nodes::CALLOUT).attr("type", callout_type_for_macro(&name)).with_content(wrap_inline_runs(content, true))]
            }
            "code" | "noformat" | "code-block" | "sourcecode" => {
                self.stats.macro_seen(&name, Support::Native);
                let lang = param("language").map(|l| l.to_ascii_lowercase());
                // Confluence doesn't show the newline before `]]>`; the editor would (an empty last line)
                let text = plain.map(|t| t.trim_end_matches(['\n', '\r']).to_string()).unwrap_or_default();
                let mut v = Vec::new();
                if let Some(t) = param("title") {
                    v.push(Node::paragraph(vec![Node::text_marked(t, vec![Mark::new(marks::BOLD)])]));
                }
                v.push(Node::new(nodes::CODE_BLOCK).attr("language", lang).with_content(if text.is_empty() { vec![] } else { vec![Node::text(text)] }));
                v
            }
            "mermaid" | "mermaid-cloud" | "mermaid-macro" | "plantuml" | "plantumlrender" => {
                self.stats.macro_seen(&name, Support::Native);
                let lang = if name.starts_with("plantuml") { "plantuml" } else { "mermaid" };
                let text = plain.or_else(|| rich.map(Element::text)).unwrap_or_default();
                vec![Node::new(nodes::CODE_BLOCK).attr("language", lang).with_content(if text.is_empty() { vec![] } else { vec![Node::text(text)] })]
            }
            "html" | "html-include" => {
                self.stats.macro_seen(&name, Support::Partial);
                let text = plain.unwrap_or_default();
                vec![Node::new(nodes::CODE_BLOCK).attr("language", "html").with_content(if text.is_empty() { vec![] } else { vec![Node::text(text)] })]
            }
            "expand" | "ui-expand" => {
                self.stats.macro_seen(&name, Support::Native);
                let title = param("title").unwrap_or_else(|| "Click here to expand...".into());
                let content = rich.map(|r| self.blocks(&r.children)).unwrap_or_default();
                vec![details(title, content)]
            }
            "toc" | "toc-zone" => {
                self.stats.macro_seen(&name, Support::Native);
                let mut n = Node::new(nodes::TOC);
                if let Some(v) = param("minLevel").and_then(|v| v.parse::<i64>().ok()) {
                    n = n.attr("minLevel", v);
                }
                if let Some(v) = param("maxLevel").and_then(|v| v.parse::<i64>().ok()) {
                    n = n.attr("maxLevel", v);
                }
                for key in ["include", "exclude"] {
                    if let Some(p) = param(key).filter(|p| !p.trim().is_empty()) {
                        n = n.attr(key, p);
                    }
                }
                if param("type").as_deref() == Some("flat") {
                    n = n.attr("style", "flat");
                }
                if param("outline").as_deref() == Some("true") {
                    n = n.attr("numbered", true);
                }
                let mut v = vec![n];
                if name == "toc-zone"
                    && let Some(r) = rich
                {
                    v.extend(self.blocks(&r.children));
                }
                v
            }
            "anchor" => {
                self.stats.macro_seen(&name, Support::Native);
                let a = param("").or_else(|| el.macro_params().into_iter().next().map(|p| p.1)).unwrap_or_default();
                vec![Node::new(nodes::ANCHOR).attr("name", a)]
            }
            "status" => {
                self.stats.macro_seen(&name, Support::Native);
                let text = param("title").unwrap_or_default();
                let color = status_color(&param("colour").or_else(|| param("color")).unwrap_or_default());
                vec![Node::new(nodes::STATUS).attr("text", text).attr("color", color)]
            }
            "view-file" | "viewpdf" | "viewdoc" | "viewppt" | "viewxls" | "view-file-macro" | "multimedia" | "viewfile" => {
                self.stats.macro_seen(&name, Support::Native);
                let att = el
                    .children_named("ac:parameter")
                    .find_map(|p| p.child("ri:attachment").cloned())
                    .or_else(|| param("name").map(|n| Element { name: "ri:attachment".into(), attrs: vec![("ri:filename".into(), n)], children: vec![] }));
                match att {
                    Some(a) => {
                        let (href, filename) = self.resolve_attachment_ref(&a);
                        vec![Node::new(nodes::ATTACHMENT).attr("href", href).attr("name", filename).attr("view", "preview")]
                    }
                    None => vec![],
                }
            }
            "children" | "pagetree" | "pagetreesearch" => {
                self.stats.macro_seen(&name, Support::Native);
                let mut n = Node::new(nodes::CHILD_PAGES);
                if let Some(d) = param("depth").and_then(|v| v.parse::<i64>().ok()) {
                    n = n.attr("depth", d);
                }
                if param("all").is_some_and(|v| v == "true") || name == "pagetree" {
                    n = n.attr("depth", 0);
                }
                if let Some(s) = param("sort") {
                    n = n.attr("sort", s);
                }
                if param("reverse").is_some_and(|v| v == "true") {
                    n = n.attr("reverse", true);
                }
                if let Some(s) = param("style") {
                    n = n.attr("style", s);
                }
                if let Some(f) = param("first").and_then(|v| v.parse::<i64>().ok()) {
                    n = n.attr("limit", f);
                }
                if name == "pagetreesearch" {
                    n = n.attr("search", true);
                }
                vec![n]
            }
            "include" | "excerpt-include" => {
                self.stats.macro_seen(&name, Support::Native);
                let page = el
                    .children_named("ac:parameter")
                    .find_map(|p| p.child("ac:link").and_then(|l| l.elements().find(|e| e.name == "ri:page" || e.name == "ri:blog-post")).cloned());
                match page {
                    Some(p) => {
                        let title = p.attr("ri:content-title").unwrap_or("").to_string();
                        let href = self.resolve_page_ref(&p);
                        vec![Node::new(nodes::INCLUDE).attr("pageRef", href).attr("title", title).attr("excerpt", name == "excerpt-include")]
                    }
                    None => vec![],
                }
            }
            "excerpt" => {
                self.stats.macro_seen(&name, Support::Native);
                rich.map(|r| self.blocks(&r.children)).unwrap_or_default()
            }
            "details" => {
                // page properties: keep the table; the properties report can query it later
                self.stats.macro_seen(&name, Support::Partial);
                rich.map(|r| self.blocks(&r.children)).unwrap_or_default()
            }
            "section" => {
                self.stats.macro_seen(&name, Support::Native);
                let cols: Vec<Node> = rich
                    .map(|r| {
                        r.children_named("ac:structured-macro")
                            .filter(|m| m.attr("ac:name") == Some("column"))
                            .map(|c| Node::new(nodes::COLUMN).with_content(c.child("ac:rich-text-body").map(|b| self.blocks(&b.children)).unwrap_or_default()))
                            .collect()
                    })
                    .unwrap_or_default();
                if cols.is_empty() { rich.map(|r| self.blocks(&r.children)).unwrap_or_default() } else { vec![Node::new(nodes::COLUMNS).with_content(cols)] }
            }
            "column" | "div" | "span" | "center" | "auto-cursor-target" | "panelbox" | "ui-tab" | "ui-tabs" | "ui-steps" | "ui-step" => {
                self.stats.macro_seen(&name, Support::Partial);
                rich.map(|r| self.children(&r.children, marks, false)).unwrap_or_default()
            }
            "widget" | "iframe" | "widget-connector" => {
                self.stats.macro_seen(&name, Support::Native);
                let url = el
                    .children_named("ac:parameter")
                    .find(|p| p.attr("ac:name") == Some("url") || p.attr("ac:name") == Some("src"))
                    .map(|p| p.child("ri:url").and_then(|u| u.attr("ri:value").map(String::from)).unwrap_or_else(|| p.text()))
                    .unwrap_or_default();
                if url.is_empty() { vec![] } else { vec![Node::new(nodes::EMBED).attr("url", url)] }
            }
            "jira" | "jiraissues" => {
                self.stats.macro_seen(&name, Support::Partial);
                let key = param("key");
                let jql = param("jqlQuery");
                let server = param("server").unwrap_or_else(|| "Jira".into());
                match (key, jql) {
                    (Some(k), _) => vec![Node::text_marked(k.to_string(), add_mark(marks, Mark::new(marks::CODE)))],
                    (None, Some(q)) => vec![placeholder_macro(&name, el, vec![Node::paragraph(vec![Node::text(format!("{server} query: {q}"))])])],
                    _ => vec![placeholder_macro(&name, el, vec![])],
                }
            }
            "unmigrated-wiki-markup" | "unmigrated-inline-wiki-markup" => {
                self.stats.macro_seen(&name, Support::Partial);
                let mut doc = crate::wiki::wiki_to_pm(&plain.unwrap_or_default());
                resolve_wiki_refs(&mut doc, self.r, self.stats);
                doc.content
            }
            "mathblock" | "mathjax-block" | "latex" | "latex-formatting" | "math" | "eazy-math" => {
                self.stats.macro_seen(&name, Support::Native);
                let latex = plain.or_else(|| param("body")).or_else(|| rich.map(Element::text)).unwrap_or_default();
                vec![Node::new(nodes::MATH_BLOCK).attr("latex", latex.trim().to_string())]
            }
            "mathinline" | "mathjax-inline-macro" | "latex-inline" => {
                self.stats.macro_seen(&name, Support::Native);
                let latex = param("body").or(plain).or_else(|| rich.map(Element::text)).unwrap_or_default();
                vec![Node::new(nodes::MATH_INLINE).attr("latex", latex.trim().to_string())]
            }
            "drawio" | "inc-drawio" | "drawio-sketch" | "gliffy" => {
                // show the PNG preview the apps store as `<diagramName>.png` and keep the macro info
                self.stats.macro_seen(&name, Support::Partial);
                let diagram = param("diagramName").or_else(|| param("name")).unwrap_or_default();
                let mut v = Vec::new();
                if !diagram.is_empty() {
                    let png = Element { name: "ri:attachment".into(), attrs: vec![("ri:filename".into(), format!("{diagram}.png"))], children: vec![] };
                    let (href, _) = self.resolve_attachment_ref(&png);
                    if !href.starts_with("wcimport:missing") {
                        v.push(Node::paragraph(vec![
                            Node::new(nodes::IMAGE).attr("src", href).attr("alt", diagram.clone()).attr("title", format!("{name}: {diagram}")),
                        ]));
                    }
                }
                v.push(placeholder_macro(&name, el, vec![]));
                v
            }
            // queries rendered live by the editor from the preserved parameters
            "contentbylabel" | "recently-updated" | "detailssummary" | "content-report-table" | "tasks-report-macro" | "attachments" => {
                self.stats.macro_seen(&name, Support::Native);
                vec![placeholder_macro(&name, el, vec![])]
            }
            _ => {
                self.stats.macro_seen(&name, Support::Placeholder);
                let content = rich.map(|r| self.blocks(&r.children)).unwrap_or_default();
                vec![placeholder_macro(&name, el, if rich.is_some() { content } else { vec![] })]
            }
        }
    }
}

fn details(summary: String, content: Vec<Node>) -> Node {
    Node::new(nodes::DETAILS).with_content(vec![
        Node::new(nodes::DETAILS_SUMMARY).with_content(if summary.is_empty() { vec![] } else { vec![Node::text(summary)] }),
        Node::new(nodes::DETAILS_CONTENT).with_content(wrap_inline_runs(content, true)),
    ])
}

/// An inert macro node preserving name, parameters and plain body for later mapping.
fn placeholder_macro(name: &str, el: &Element, content: Vec<Node>) -> Node {
    let mut params = Map::new();
    for (k, v) in el.macro_params() {
        params.insert(if k.is_empty() { "default".into() } else { k }, Value::String(v));
    }
    let body_text = el.child("ac:plain-text-body").map(Element::text).filter(|t| !t.is_empty());
    let mut n = Node::new(nodes::MACRO).attr("name", name).attr("params", Value::Object(params)).attr("bodyText", body_text);
    if !content.is_empty() {
        n.content = content;
    }
    n
}

#[cfg(test)]
mod tests {
    use super::*;
    use pretty_assertions::assert_eq;

    struct R;
    impl Resolver for R {
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

    fn conv(body: &str) -> (Node, Stats) {
        let mut s = Stats::default();
        (convert(body, &R, &mut s), s)
    }

    fn json(n: &Node) -> String {
        serde_json::to_string(n).unwrap()
    }

    #[test]
    fn instructional_text_becomes_placeholders() {
        let (doc, _) = conv(r#"<p><ac:placeholder>Describe the   goal here</ac:placeholder></p><p><ac:placeholder ac:type="mention"/></p>"#);
        let p = &doc.content[0].content[0];
        assert_eq!((p.kind.as_str(), p.attr_str("text")), (nodes::PLACEHOLDER, Some("Describe the goal here")));
        assert!(doc.content.get(1).is_none_or(|p| p.content.is_empty()), "empty placeholders are dropped: {}", json(&doc));
    }

    #[test]
    fn emoticons_become_icons_not_emoji() {
        let (doc, stats) = conv(
            r#"<p>done <ac:emoticon ac:name="tick"/> and <ac:emoticon ac:name="unknown-thing"/> <ac:emoticon ac:name="rocket" ac:emoji-fallback="🚀"/></p>"#,
        );
        let p = &doc.content[0];
        assert_eq!(stats.emoticons, 3);
        assert!(p.content.iter().any(|n| n.kind == nodes::EMOTICON && n.attr_str("name") == Some("tick")));
        // an emoji a Cloud user typed is their content and stays text; unknown names are dropped
        let text: String = p.content.iter().filter_map(|n| n.text.clone()).collect();
        assert!(text.contains('\u{1F680}') && !text.contains("unknown"), "{text}");
    }

    #[test]
    fn basic_formatting() {
        let (doc, _) = conv("<h1>Title</h1><p>Hello <strong>bold</strong> and <em>it</em><br/>line&nbsp;2</p>");
        assert_eq!(doc.content[0], Node::heading(1, vec![Node::text("Title")]));
        let p = &doc.content[1];
        assert_eq!(p.content[1], Node::text_marked("bold", vec![Mark::new(marks::BOLD)]));
        assert_eq!(p.content[4].kind, "hardBreak");
        assert_eq!(p.content[5].text.as_deref(), Some("line\u{a0}2"));
    }

    #[test]
    fn links_users_and_attachments() {
        let body = r#"<p><ac:link><ri:page ri:content-title="Other Page"/><ac:plain-text-link-body><![CDATA[see other]]></ac:plain-text-link-body></ac:link> <ac:link><ri:page ri:content-title="Gone"/></ac:link> <ac:link><ri:user ri:userkey="abc"/></ac:link></p><p><ac:image ac:width="200"><ri:attachment ri:filename="diagram.png"/></ac:image></p>"#;
        let (doc, stats) = conv(body);
        let s = json(&doc);
        assert!(s.contains("wcimport:page/200"), "{s}");
        assert!(s.contains("see other"));
        assert!(s.contains("wcimport:missing/DOC/Gone"));
        assert!(s.contains("\"label\":\"Ada Lovelace\""));
        assert!(s.contains("wcimport:attachment/300"));
        assert_eq!(stats.page_links, 2);
        assert_eq!(stats.page_links_resolved, 1);
        assert_eq!(stats.user_refs_resolved, 1);
        assert_eq!(stats.attachment_refs_resolved, 1);
    }

    #[test]
    fn macros_map_to_native_nodes() {
        let body = r#"<ac:structured-macro ac:name="info"><ac:parameter ac:name="title">Heads up</ac:parameter><ac:rich-text-body><p>body</p></ac:rich-text-body></ac:structured-macro>
<ac:structured-macro ac:name="code"><ac:parameter ac:name="language">java</ac:parameter><ac:plain-text-body><![CDATA[class A {}]]></ac:plain-text-body></ac:structured-macro>
<p><ac:structured-macro ac:name="toc"><ac:parameter ac:name="type">flat</ac:parameter><ac:parameter ac:name="outline">true</ac:parameter><ac:parameter ac:name="exclude">^Appendix.*</ac:parameter><ac:parameter ac:name="maxLevel">3</ac:parameter></ac:structured-macro></p>
<ac:structured-macro ac:name="expand"><ac:parameter ac:name="title">More</ac:parameter><ac:rich-text-body><p>hidden</p></ac:rich-text-body></ac:structured-macro>
<p>State: <ac:structured-macro ac:name="status"><ac:parameter ac:name="colour">Green</ac:parameter><ac:parameter ac:name="title">DONE</ac:parameter></ac:structured-macro></p>
<ac:structured-macro ac:name="mystery-app"><ac:parameter ac:name="x">1</ac:parameter><ac:rich-text-body><p>kept</p></ac:rich-text-body></ac:structured-macro>"#;
        let (doc, stats) = conv(body);
        let kinds: Vec<&str> = doc.content.iter().map(|n| n.kind.as_str()).collect();
        assert_eq!(kinds, vec!["callout", "codeBlock", "toc", "details", "paragraph", "macro"]);
        assert_eq!(doc.content[0].attr_str("type"), Some("info"));
        assert_eq!(doc.content[1].attr_str("language"), Some("java"));
        let toc = &doc.content[2];
        assert_eq!(
            (toc.attr_str("style"), toc.attr_bool("numbered"), toc.attr_str("exclude"), toc.attr_i64("maxLevel")),
            (Some("flat"), Some(true), Some("^Appendix.*"), Some(3))
        );
        assert_eq!(doc.content[4].content[1].kind, "status");
        assert_eq!(doc.content[5].attr_str("name"), Some("mystery-app"));
        assert_eq!(json(&doc.content[5]).contains("kept"), true);
        assert_eq!(stats.macros.get("mystery-app").map(|m| m.1), Some(Some(Support::Placeholder)));
    }

    #[test]
    fn tasks_layouts_and_tables() {
        let body = r##"<ac:task-list><ac:task><ac:task-id>1</ac:task-id><ac:task-status>complete</ac:task-status><ac:task-body>done <time datetime="2024-01-02" /></ac:task-body></ac:task><ac:task><ac:task-id>2</ac:task-id><ac:task-status>incomplete</ac:task-status><ac:task-body>todo</ac:task-body></ac:task></ac:task-list>
<ac:layout><ac:layout-section ac:type="two_equal"><ac:layout-cell><p>left</p></ac:layout-cell><ac:layout-cell><p>right</p></ac:layout-cell></ac:layout-section></ac:layout>
<table data-layout="wide"><colgroup><col style="width: 100.0px;"/><col style="width: 50.0px;"/></colgroup><tbody><tr><th><p>H</p></th><th>H2</th></tr><tr><td data-highlight-colour="#e3fcef" colspan="2">cell</td></tr></tbody></table>"##;
        let (doc, stats) = conv(body);
        assert_eq!(doc.content[0].kind, "taskList");
        assert_eq!(doc.content[0].content[0].attr_bool("checked"), Some(true));
        assert_eq!(doc.content[0].content[0].content[0].content[1].kind, "date");
        assert_eq!(doc.content[1].kind, "columns");
        assert_eq!(doc.content[1].content.len(), 2);
        let table = &doc.content[2];
        assert_eq!(table.content[0].content[0].kind, "tableHeader");
        assert_eq!(table.content[0].content[0].attrs.get("colwidth"), Some(&json!([100])));
        assert_eq!(table.content[1].content[0].attr_str("background"), Some("#e3fcef"));
        assert_eq!(table.content[1].content[0].attrs.get("colwidth"), Some(&json!([100, 50])));
        assert_eq!(stats.tasks, 2);
    }

    #[test]
    fn inline_comments_and_adf_decisions() {
        let body = r#"<p>a <ac:inline-comment-marker ac:ref="uuid-1">marked</ac:inline-comment-marker> b</p><ac:adf-extension><ac:adf-node type="decision-list"><ac:adf-node type="decision-item"><ac:adf-attribute key="state">DECIDED</ac:adf-attribute><ac:adf-content>Ship it</ac:adf-content></ac:adf-node></ac:adf-node><ac:adf-fallback><ul><li>Ship it</li></ul></ac:adf-fallback></ac:adf-extension>"#;
        let (doc, _) = conv(body);
        let marked = &doc.content[0].content[1];
        assert_eq!(marked.marks[0].attr_str("commentId"), Some("wcimport:inline/uuid-1"));
        assert_eq!(doc.content[1].kind, "decisionList");
        assert_eq!(wikicraft_doc_model::text::plain_text(&doc.content[1]), "Ship it");
    }

    #[test]
    fn block_macro_inside_paragraph_is_lifted() {
        let (doc, _) = conv(r#"<p>before<ac:structured-macro ac:name="info"><ac:rich-text-body><p>x</p></ac:rich-text-body></ac:structured-macro>after</p>"#);
        let kinds: Vec<&str> = doc.content.iter().map(|n| n.kind.as_str()).collect();
        assert_eq!(kinds, vec!["paragraph", "callout", "paragraph"]);
    }
}
