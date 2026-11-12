//! PDF export of a page or page tree, typeset with Typst: page titles and headings (PDF
//! bookmarks, links inside the document), an optional cover page and table of contents, a header
//! (space, title) and footer (who exported it, page x / y) on every page, lists and tasks, tables
//! with merged cells, callouts, code, quotes, columns, images and the WikiCraft icon set — never
//! emoji.
//!
//! The document becomes Typst markup in which every piece of page text is a Typst *string
//! literal* (`#"…";`), so nothing a page contains is ever read as Typst code. The compiler only
//! sees an in-memory world: the generated source, the images and icons of this export, and the
//! fonts (Noto Sans and Typst's own fonts built in, plus the fonts installed on the server for
//! other scripts).

use std::collections::HashMap;
use std::fmt::Write as _;
use std::path::{Path, PathBuf};
use std::sync::OnceLock;

use typst::diag::{FileError, FileResult};
use typst::foundations::{Bytes, Datetime, Duration};
use typst::syntax::{FileId, RootedPath, Source, VirtualPath, VirtualRoot};
use typst::text::{Font, FontBook, FontInfo};
use typst::utils::LazyHash;
use typst::{Library, LibraryExt, World};
use typst_layout::PagedDocument;
use uuid::Uuid;
use wikicraft_doc_model::html::safe_url;
use wikicraft_doc_model::{Mark, Node, icons, marks, nodes, text};

use crate::docx::{DocxInput, attachment_id, checkbox_svg, hex_color, line_icon_svg};
use crate::routes::reports::Snapshot;

/// What goes around the pages.
pub struct PdfOptions {
    /// a table of contents (page titles and headings) after the cover
    pub toc: bool,
    /// a cover page: title, space, who exported it and when
    pub cover: bool,
    /// left side of the page header (the space)
    pub header: String,
    /// left side of the footer ("Exported by Ann Lee on 11 Nov 2026")
    pub footer: String,
    pub date: chrono::NaiveDate,
    /// language of the text (hyphenation, quotes), e.g. `en`
    pub lang: String,
}

/// A4 with 2.2 cm side margins: 166 mm of text width.
const CONTENT_PT: f32 = 470.0;
const PX_TO_PT: f32 = 0.75;
const MUTED: &str = "#64748B";
const LINK: &str = "#1D4ED8";

/// A Typst string literal: page text is never Typst code.
fn lit(s: &str) -> String {
    let mut o = String::with_capacity(s.len() + 2);
    o.push('"');
    for c in s.chars() {
        match c {
            '\\' => o.push_str("\\\\"),
            '"' => o.push_str("\\\""),
            '\n' => o.push_str("\\n"),
            '\t' => o.push_str("\\t"),
            '\r' => {}
            c if c.is_control() => {
                let _ = write!(o, "\\u{{{:x}}}", c as u32);
            }
            c => o.push(c),
        }
    }
    o.push('"');
    o
}

/// Text as markup.
fn t(s: &str) -> String {
    format!("#{};", lit(s))
}

/// `#rgb("#RRGGBB")` from a validated colour.
fn rgb(hex: &str) -> String {
    format!("rgb(\"#{}\")", hex.trim_start_matches('#'))
}

fn muted(content: &str, size: Option<f32>) -> String {
    match size {
        Some(s) => format!("#text(size: {s}pt, fill: {})[{content}];", rgb(MUTED)),
        None => format!("#text(fill: {})[{content}];", rgb(MUTED)),
    }
}

/// How the blocks being rendered are nested.
#[derive(Clone, Copy)]
struct Ctx {
    /// width available, in points (images, tables, columns)
    width: f32,
    /// text is bold (table headers, details summaries)
    bold: bool,
}

struct Renderer<'a> {
    input: &'a DocxInput<'a>,
    section: usize,
    /// labels of the pages in this export
    pages: &'a HashMap<Uuid, String>,
    headings: Vec<(u8, String, String)>,
    heading_idx: usize,
    /// heading or anchor id → label
    anchors: HashMap<String, String>,
    macro_idx: usize,
    files: &'a mut HashMap<String, Vec<u8>>,
}

/// Renders pages (a page and its descendants) into one PDF.
pub fn render(inputs: &[DocxInput], opts: &PdfOptions, font_dirs: &[PathBuf]) -> anyhow::Result<Vec<u8>> {
    let mut files: HashMap<String, Vec<u8>> = HashMap::new();
    let source = source(inputs, opts, &mut files);
    compile(source, files, opts, font_dirs)
}

/// The Typst source of the export (and the images and icons it uses, into `files`).
fn source(inputs: &[DocxInput], opts: &PdfOptions, files: &mut HashMap<String, Vec<u8>>) -> String {
    let title = inputs.first().map(|i| i.title).unwrap_or("Export");
    let tree = inputs.len() > 1;
    let pages: HashMap<Uuid, String> = inputs.iter().enumerate().filter_map(|(i, d)| d.page_id.map(|id| (id, format!("wc-p{i}")))).collect();
    let mut src = String::new();
    let _ = write!(
        src,
        r##"#set document(title: {title})
#set text(font: ("Noto Sans", "DejaVu Sans", "Libertinus Serif"), size: 10pt, lang: {lang}, hyphenate: false)
#set par(leading: 0.62em, spacing: 0.9em, justify: false)
#set page(paper: "a4", margin: (x: 2.2cm, top: 2.4cm, bottom: 2.2cm),
  header: context {{ if counter(page).get().first() > 1 [#set text(size: 7.5pt, fill: {muted}); {header} #h(1fr) {title_text}] }},
  footer: context [#set text(size: 7.5pt, fill: {muted}); {footer} #h(1fr) #counter(page).display("1 / 1", both: true)])
#show heading: set block(above: 1.25em, below: 0.6em)
#show heading.where(level: 1): set text(size: 20pt)
#show heading.where(level: 1): set block(above: 0pt, below: 4pt)
#show heading.where(level: 2): set text(size: 15pt)
#show heading.where(level: 3): set text(size: 12.5pt)
#show heading.where(level: 4): set text(size: 11pt)
#show heading: set text(fill: rgb("#0F172A"))
#show raw: set text(font: ("DejaVu Sans Mono",), size: 8.5pt)
#show raw.where(block: false): box.with(fill: rgb("#F1F5F9"), inset: (x: 2pt), outset: (y: 2pt), radius: 2pt)
#show raw.where(block: true): block.with(fill: rgb("#F6F8FA"), inset: 8pt, radius: 3pt, width: 100%, stroke: 0.5pt + rgb("#E2E8F0"))
#show link: set text(fill: {link})
#set list(indent: 4pt, body-indent: 6pt)
#set enum(indent: 4pt, body-indent: 6pt)
#set table(stroke: 0.5pt + rgb("#CBD5E1"), inset: (x: 5pt, y: 4pt))
#show table: set text(size: 9pt)
#show outline.entry.where(level: 1): set block(above: 0.9em)
// a link to a label of this document; plain text when the target isn't in it
#let wclink(target, body) = context {{
  let found = query(target)
  if found.len() > 0 {{ link(found.first().location(), body) }} else {{ body }}
}}
"##,
        title = lit(title),
        lang = lit(&opts.lang),
        muted = rgb(MUTED),
        link = rgb(LINK),
        header = t(&opts.header),
        title_text = t(title),
        footer = t(&opts.footer),
    );
    if opts.cover {
        let date = opts.date.format("%-d %B %Y").to_string();
        let pages_line = if tree { format!("{} pages", inputs.len()) } else { String::new() };
        let _ = write!(
            src,
            "#page(header: none, footer: none)[#v(30%)\n#text(size: 10pt, fill: {m})[{header}]\n\n#text(size: 28pt, weight: \"bold\", fill: rgb(\"#0F172A\"))[{title}]\n\n#v(8pt)\n#text(size: 10pt, fill: {m})[{footer} #linebreak() {date} #linebreak() {pages}]\n]\n",
            m = rgb(MUTED),
            header = t(&opts.header),
            title = t(title),
            footer = t(&opts.footer),
            date = t(&date),
            pages = t(&pages_line),
        );
    }
    if opts.toc {
        let depth = if tree { 3 } else { 4 };
        let _ = writeln!(src, "#outline(title: {}, depth: {depth})\n#pagebreak()", lit("Contents"));
    }
    for (section, input) in inputs.iter().enumerate() {
        let headings = text::headings(input.doc);
        let anchors: HashMap<String, String> = headings.iter().enumerate().map(|(i, h)| (h.2.clone(), format!("wc-{section}-h{i}"))).collect();
        let mut r = Renderer { input, section, pages: &pages, headings, heading_idx: 0, anchors, macro_idx: 0, files };
        if section > 0 {
            src.push_str("#pagebreak(weak: true)\n");
        }
        // the page title is the level-1 heading (in the contents of a page tree); the page's own
        // headings go one level down
        let _ = writeln!(src, "#heading(level: 1, outlined: {tree})[{}]<wc-p{section}>", t(input.title));
        if let Some(sub) = &input.subtitle {
            let _ = writeln!(src, "{}\n\n#v(6pt)", muted(&t(sub), Some(8.5)));
        }
        let body = r.blocks(&input.doc.content, Ctx { width: CONTENT_PT, bold: false });
        src.push_str(&body);
        src.push('\n');
    }
    src
}

impl Renderer<'_> {
    fn label(&self, name: &str) -> String {
        format!("wc-{}-{name}", self.section)
    }

    /// A file for the compiler; returns its path.
    fn file(&mut self, path: String, bytes: impl FnOnce() -> Option<Vec<u8>>) -> Option<String> {
        if !self.files.contains_key(&path) {
            let b = bytes()?;
            self.files.insert(path.clone(), b);
        }
        Some(path)
    }

    /// An icon of the WikiCraft set (or a checkbox), as inline SVG one line high.
    fn icon(&mut self, key: &str, svg: impl FnOnce() -> Option<String>, size: &str) -> Option<String> {
        let name: String = key.chars().map(|c| if c.is_ascii_alphanumeric() || c == '-' { c } else { '_' }).collect();
        let path = self.file(format!("/icons/{name}.svg"), || svg().map(String::into_bytes))?;
        Some(format!("#box(baseline: 12%, image({}, height: {size}));", lit(&path)))
    }

    fn page_label(&self, href: &str) -> Option<String> {
        let path = href.strip_prefix(self.input.base_url).unwrap_or(href);
        let id: Uuid = path.strip_prefix("/pages/")?.split(['#', '?', '/']).next()?.parse().ok()?;
        self.pages.get(&id).cloned()
    }

    fn url(&self, href: &str) -> Option<String> {
        let href = safe_url(href)?;
        if href.starts_with('/') {
            return Some(format!("{}{href}", self.input.base_url));
        }
        let lower = href.to_ascii_lowercase();
        (lower.starts_with("http://") || lower.starts_with("https://") || lower.starts_with("mailto:") || lower.starts_with("tel:")).then(|| href.to_string())
    }

    /// A link around `body`: inside the export (a page or heading of it) or to the web.
    fn link(&self, href: &str, body: String) -> String {
        if let Some(anchor) = href.strip_prefix('#') {
            return match self.anchors.get(anchor) {
                Some(l) => format!("#wclink(<{l}>)[{body}];"),
                None => body,
            };
        }
        if let Some(l) = self.page_label(href) {
            return format!("#wclink(<{l}>)[{body}];");
        }
        match self.url(href) {
            Some(url) => format!("#link({})[{body}];", lit(&url)),
            None => body,
        }
    }

    fn blocks(&mut self, content: &[Node], ctx: Ctx) -> String {
        let parts: Vec<String> = content.iter().map(|n| self.block(n, ctx)).filter(|s| !s.is_empty()).collect();
        parts.join("\n\n")
    }

    fn indent(n: &Node, body: String) -> String {
        let lvl = wikicraft_doc_model::node::indent_level(n);
        if lvl > 0 { format!("#pad(left: {}pt)[{body}];", 24 * lvl) } else { body }
    }

    fn block(&mut self, n: &Node, ctx: Ctx) -> String {
        match n.kind.as_str() {
            nodes::PARAGRAPH => Self::indent(n, self.paragraph(n, ctx)),
            nodes::HEADING => {
                let lvl = n.attr_i64("level").unwrap_or(1).clamp(1, 6) + 1;
                let label = self.headings.get(self.heading_idx).and_then(|h| self.anchors.get(&h.2)).cloned();
                self.heading_idx += 1;
                let mut captions = Vec::new();
                let body = self.inlines(&n.content, ctx, &mut captions);
                let h = match label {
                    Some(l) => format!("#heading(level: {lvl})[{body}]<{l}>"),
                    None => format!("#heading(level: {lvl})[{body}];"),
                };
                Self::indent(n, Self::aligned(n, h))
            }
            nodes::BULLET_LIST | nodes::ORDERED_LIST => {
                let items: Vec<String> =
                    n.content.iter().map(|item| format!("[{}]", self.blocks(&item.content, Ctx { width: ctx.width - 18.0, ..ctx }))).collect();
                if n.kind == nodes::BULLET_LIST {
                    format!("#list(tight: true, {});", items.join(", "))
                } else {
                    let start = n.attr_i64("start").unwrap_or(1).clamp(0, 32_767);
                    format!("#enum(tight: true, start: {start}, {});", items.join(", "))
                }
            }
            nodes::TASK_LIST => {
                let mut cells = Vec::new();
                for item in &n.content {
                    let checked = item.attr_bool("checked").unwrap_or(false);
                    let key = if checked { "task-on" } else { "task-off" };
                    let icon = self.icon(key, || Some(checkbox_svg(checked)), "1em").unwrap_or_default();
                    let body = self.blocks(&item.content, Ctx { width: ctx.width - 18.0, ..ctx });
                    let body = if checked { format!("#text(fill: {})[#strike[{body}]];", rgb(MUTED)) } else { body };
                    cells.push(format!("[{icon}], [{body}]"));
                }
                format!("#grid(columns: (auto, 1fr), column-gutter: 6pt, row-gutter: 6pt, {});", cells.join(", "))
            }
            nodes::DECISION_LIST => {
                let mut cells = Vec::new();
                for item in &n.content {
                    let icon = self.icon("decision", || line_icon_svg("symbols", "decision", "#16A34A"), "1em").unwrap_or_default();
                    let mut captions = Vec::new();
                    let body = self.inlines(&item.content, ctx, &mut captions);
                    cells.push(format!("[{icon}], [{body}]"));
                }
                format!("#grid(columns: (auto, 1fr), column-gutter: 6pt, row-gutter: 6pt, {});", cells.join(", "))
            }
            nodes::CODE_BLOCK => {
                let code = text::inline_text(n);
                let lang = n
                    .attr_str("language")
                    .filter(|l| !l.is_empty() && l.len() <= 20 && l.chars().all(|c| c.is_ascii_alphanumeric() || matches!(c, '+' | '#' | '-' | '_')))
                    .map(|l| format!("lang: {}, ", lit(&l.to_ascii_lowercase())))
                    .unwrap_or_default();
                Self::indent(n, format!("#raw(block: true, {lang}{});", lit(code.trim_end_matches('\n'))))
            }
            nodes::BLOCKQUOTE => {
                let body = self.blocks(&n.content, Ctx { width: ctx.width - 14.0, ..ctx });
                format!("#block(inset: (left: 10pt, y: 2pt), stroke: (left: 2pt + rgb(\"#CBD5E1\")))[#set text(fill: rgb(\"#475569\"))\n{body}];")
            }
            nodes::HORIZONTAL_RULE => "#line(length: 100%, stroke: 0.6pt + rgb(\"#CBD5E1\"));".into(),
            nodes::TABLE => self.table(n, ctx),
            nodes::CALLOUT => {
                let kind = n.attr_str("type").filter(|k| matches!(*k, "info" | "note" | "success" | "warning" | "error")).unwrap_or("info");
                let (fill, edge) = match kind {
                    "warning" => ("FFFBEB", "F59E0B"),
                    "error" => ("FEF2F2", "EF4444"),
                    "success" => ("F0FDF4", "22C55E"),
                    "note" => ("F5F3FF", "8B5CF6"),
                    _ => ("EFF6FF", "3B82F6"),
                };
                let custom = n.attr_str("icon").and_then(icons::parse_symbol);
                let key = custom.map(|(name, colour)| format!("symbol-{name}-{}", colour.trim_start_matches('#'))).unwrap_or_else(|| format!("callout-{kind}"));
                let icon = self
                    .icon(
                        &key,
                        || match custom {
                            Some((name, colour)) => line_icon_svg("symbols", name, colour),
                            None => line_icon_svg("callout", kind, &format!("#{edge}")),
                        },
                        "1.1em",
                    )
                    .unwrap_or_default();
                let body = self.blocks(&n.content, Ctx { width: ctx.width - 40.0, bold: false });
                format!(
                    "#block(width: 100%, fill: {}, inset: (x: 10pt, y: 8pt), radius: 3pt, stroke: (left: 3pt + {}))[#grid(columns: (auto, 1fr), column-gutter: 8pt, [{icon}], [{body}])];",
                    rgb(fill),
                    rgb(edge)
                )
            }
            nodes::DETAILS => {
                let mut out = Vec::new();
                for c in &n.content {
                    match c.kind.as_str() {
                        nodes::DETAILS_SUMMARY => out.push(self.paragraph(c, Ctx { bold: true, ..ctx })),
                        nodes::DETAILS_CONTENT => out.push(format!("#pad(left: 12pt)[{}];", self.blocks(&c.content, Ctx { width: ctx.width - 12.0, ..ctx }))),
                        _ => out.push(self.block(c, ctx)),
                    }
                }
                out.join("\n\n")
            }
            nodes::TOC => {
                let entries = wikicraft_doc_model::toc::entries(self.input.doc, n);
                let label = |e: &wikicraft_doc_model::toc::TocEntry| if e.number.is_empty() { e.text.clone() } else { format!("{} {}", e.number, e.text) };
                let links: Vec<(u8, String)> =
                    entries.iter().filter_map(|e| self.anchors.get(&e.id).map(|l| (e.depth, format!("#wclink(<{l}>)[{}];", t(&label(e)))))).collect();
                if wikicraft_doc_model::toc::is_flat(n) {
                    links.into_iter().map(|(_, l)| l).collect::<Vec<_>>().join(&format!(" {} ", muted(&t("·"), None)))
                } else {
                    let lines: Vec<String> = links.into_iter().map(|(d, l)| format!("#pad(left: {}pt)[{l}];", 12 * u32::from(d))).collect();
                    format!("#block(spacing: 4pt)[{}];", lines.join(" #v(2pt, weak: true) "))
                }
            }
            nodes::CHILD_PAGES => {
                let children = self.input.children;
                if children.is_empty() {
                    return format!("#emph[{}];", muted(&t("No child pages."), None));
                }
                let items: Vec<String> = children.iter().map(|(title, url)| format!("[{}]", self.link(url, t(title)))).collect();
                format!("#list(tight: true, {});", items.join(", "))
            }
            nodes::INCLUDE => {
                let title = n.attr_str("title").unwrap_or("page");
                let link = match n.attr_str("pageId").and_then(|id| id.parse::<Uuid>().ok()) {
                    Some(id) => self.link(&format!("/pages/{id}"), t(title)),
                    None => t(title),
                };
                format!("#emph[{} {link}];", muted(&t("Included page:"), None))
            }
            nodes::ATTACHMENT => {
                let name = n.attr_str("name").unwrap_or("attachment");
                let icon = self.icon("paperclip", || line_icon_svg("ui", "paperclip", "#475569"), "1em").unwrap_or_default();
                let link = match n.attr_str("href") {
                    Some(h) => self.link(h, t(name)),
                    None => t(name),
                };
                format!("{icon} {link}")
            }
            nodes::EMBED => match n.attr_str("url").and_then(|u| self.url(u)) {
                Some(url) => format!("#link({})[{}];", lit(&url), t(&url)),
                None => String::new(),
            },
            nodes::COLUMNS => {
                let cols: Vec<&Node> = n.content.iter().filter(|c| c.kind == nodes::COLUMN).collect();
                if cols.is_empty() {
                    return String::new();
                }
                let w = (ctx.width - 16.0 * (cols.len() as f32 - 1.0)) / cols.len() as f32;
                let cells: Vec<String> = cols.iter().map(|c| format!("[{}]", self.blocks(&c.content, Ctx { width: w, bold: false }))).collect();
                format!("#grid(columns: {}, column-gutter: 16pt, {});", fr_columns(cols.len()), cells.join(", "))
            }
            nodes::DRAWING => match n.attr_str("preview") {
                Some(src) => {
                    let img = Node::new(nodes::IMAGE).attr("src", src).attr("alt", n.attr_str("title").unwrap_or("Drawing")).attr("align", "center");
                    self.paragraph(&Node::paragraph(vec![img]), ctx)
                }
                None => String::new(),
            },
            nodes::MATH_BLOCK => format!("#align(center)[#raw({})];", lit(n.attr_str("latex").unwrap_or(""))),
            nodes::MACRO => {
                let idx = self.macro_idx;
                self.macro_idx += 1;
                let name = n.attr_str("name").unwrap_or("macro").to_string();
                let head = match self.input.macros.get(idx).and_then(|m| m.as_ref()) {
                    Some(snap) => self.snapshot(snap),
                    None => format!("#emph[{}];", muted(&t(&format!("Macro: {name}")), Some(8.5))),
                };
                let body = self.blocks(&n.content, ctx);
                if body.is_empty() { head } else { format!("{head}\n\n{body}") }
            }
            _ => {
                if !n.content.is_empty() && n.content.iter().all(|c| c.is_inline()) {
                    self.paragraph(n, ctx)
                } else if n.is_inline() {
                    let wrapper = Node { content: vec![n.clone()], ..Node::new(nodes::PARAGRAPH) };
                    self.paragraph(&wrapper, ctx)
                } else {
                    self.blocks(&n.content, ctx)
                }
            }
        }
    }

    fn aligned(n: &Node, body: String) -> String {
        match n.attr_str("textAlign") {
            Some(a @ ("center" | "right")) => format!("#align({a})[{body}];"),
            _ => body,
        }
    }

    /// A paragraph from `n`'s inline content (plus image captions after it).
    fn paragraph(&mut self, n: &Node, ctx: Ctx) -> String {
        let mut captions = Vec::new();
        let body = self.inlines(&n.content, ctx, &mut captions);
        if body.trim().is_empty() {
            return "#v(0.3em)".into();
        }
        // a paragraph holding just an aligned image takes the image's alignment
        let image_align = (n.content.len() == 1 && n.content[0].kind == nodes::IMAGE).then(|| n.content[0].attr_str("align")).flatten();
        let mut out = match image_align {
            Some(a @ ("center" | "right")) => format!("#align({a})[{body}];"),
            _ => Self::aligned(n, body),
        };
        for (c, align) in captions {
            let cap = muted(&format!("#emph[{}];", t(&c)), Some(8.5));
            let _ = write!(
                out,
                "\n\n{}",
                match align {
                    Some(a) => format!("#align({a})[{cap}];"),
                    None => cap,
                }
            );
        }
        out
    }

    fn inlines(&mut self, content: &[Node], ctx: Ctx, captions: &mut Vec<(String, Option<&'static str>)>) -> String {
        content.iter().map(|n| self.inline(n, ctx, captions)).collect()
    }

    fn inline(&mut self, n: &Node, ctx: Ctx, captions: &mut Vec<(String, Option<&'static str>)>) -> String {
        match n.kind.as_str() {
            nodes::TEXT => {
                let s = n.text.as_deref().unwrap_or("");
                let mut out = if n.marks.iter().any(|m| m.kind == marks::CODE) { format!("#raw({});", lit(s)) } else { t(s) };
                out = styled(out, &n.marks);
                if ctx.bold {
                    out = format!("#strong[{out}];");
                }
                match n.marks.iter().find(|m| m.kind == marks::LINK).and_then(|m| m.attr_str("href")) {
                    Some(href) => {
                        let external = !href.starts_with('#') && self.page_label(href).is_none();
                        let body = if external { format!("#underline[{out}];") } else { out };
                        self.link(href, body)
                    }
                    None => out,
                }
            }
            nodes::HARD_BREAK => "#linebreak();".into(),
            nodes::IMAGE => {
                let src = n.attr_str("src").unwrap_or("");
                let alt = n.attr_str("alt").unwrap_or("");
                if let Some(c) = n.attr_str("caption").filter(|c| !c.trim().is_empty()) {
                    let align = match n.attr_str("align") {
                        Some("center") => Some("center"),
                        Some("right") => Some("right"),
                        _ => None,
                    };
                    captions.push((c.to_string(), align));
                }
                let img = attachment_id(src).and_then(|id| self.input.images.get(&id).map(|i| (id, i)));
                match img {
                    Some((id, img)) => {
                        let (png, natural) = (img.png.clone(), img.w);
                        let path = self.file(format!("/images/{id}.png"), || Some(png)).unwrap_or_default();
                        let want = n.attr_i64("width").filter(|w| *w > 0).map(|w| w as f32).unwrap_or(natural as f32) * PX_TO_PT;
                        let w = want.min(ctx.width.max(40.0)).max(6.0);
                        format!("#box(image({}, width: {w:.1}pt));", lit(&path))
                    }
                    None => {
                        let label = if alt.is_empty() { "[Image]".to_string() } else { format!("[Image: {alt}]") };
                        let body = format!("#emph[{}];", muted(&t(&label), None));
                        if src.is_empty() { body } else { self.link(src, body) }
                    }
                }
            }
            nodes::MENTION => {
                let label = n.attr_str("label").or(n.attr_str("id")).unwrap_or("user");
                format!("#text(fill: {})[{}];", rgb(LINK), t(&format!("@{label}")))
            }
            nodes::STATUS => {
                let (bg, fg) = match n.attr_str("color") {
                    Some("blue") => ("DEEBFF", "0747A6"),
                    Some("green") => ("E3FCEF", "006644"),
                    Some("yellow") => ("FFF0B3", "974F0C"),
                    Some("red") => ("FFEBE6", "BF2600"),
                    Some("purple") => ("EAE6FF", "403294"),
                    _ => ("DFE1E6", "42526E"),
                };
                let label = n.attr_str("text").unwrap_or("").to_uppercase();
                format!(
                    "#box(fill: {}, inset: (x: 3pt), outset: (y: 2pt), radius: 2pt)[#text(size: 7pt, weight: \"bold\", fill: {})[{}]];",
                    rgb(bg),
                    rgb(fg),
                    t(&label)
                )
            }
            nodes::ISSUE => {
                let key = t(n.attr_str("key").unwrap_or("issue"));
                match n.attr_str("url").filter(|u| u.starts_with("https://") || u.starts_with("http://")) {
                    Some(u) => self.link(u, key),
                    None => format!("#raw({});", lit(n.attr_str("key").unwrap_or("issue"))),
                }
            }
            nodes::DATE => {
                let d = n.attr_str("date").unwrap_or("");
                let shown = chrono::NaiveDate::parse_from_str(d, "%Y-%m-%d").map(|d| d.format("%-d %b %Y").to_string()).unwrap_or_else(|_| d.to_string());
                format!("#box(fill: rgb(\"#F1F5F9\"), inset: (x: 3pt), outset: (y: 2pt), radius: 2pt)[{}];", t(&shown))
            }
            nodes::EMOTICON => {
                let name = n.attr_str("name").unwrap_or("");
                let svg = || {
                    icons::inner_markup("emoticons", name)
                        .map(|inner| format!("<svg xmlns=\"http://www.w3.org/2000/svg\" viewBox=\"0 0 24 24\" width=\"24\" height=\"24\">{inner}</svg>"))
                };
                match self.icon(&format!("emoticon-{name}"), svg, "1em") {
                    Some(icon) => icon,
                    None => t(icons::emoticon_shortcut(name)),
                }
            }
            nodes::MATH_INLINE => format!("#raw({});", lit(n.attr_str("latex").unwrap_or(""))),
            nodes::ANCHOR => {
                let name = text::slugify(n.attr_str("name").unwrap_or(""));
                if name.is_empty() {
                    return String::new();
                }
                let label = self.label(&format!("a{}", self.anchors.len()));
                let label = self.anchors.entry(name).or_insert(label).clone();
                format!("#metadata(none)<{label}>")
            }
            _ => match &n.text {
                Some(s) => t(s),
                None => self.inlines(&n.content, ctx, captions),
            },
        }
    }

    fn table(&mut self, n: &Node, ctx: Ctx) -> String {
        let rows: Vec<&Node> = n.content.iter().filter(|r| r.kind == nodes::TABLE_ROW).collect();
        let span = |c: &Node, key: &str, max: i64| c.attr_i64(key).unwrap_or(1).clamp(1, max) as usize;
        let cols = rows.iter().map(|r| r.content.iter().map(|c| span(c, "colspan", 63)).sum::<usize>()).max().unwrap_or(1).max(1);
        // column widths from the editor's `colwidth` (px) where known
        let mut px: Vec<Option<u32>> = vec![None; cols];
        if let Some(first) = rows.first() {
            let mut col = 0;
            for c in &first.content {
                if let Some(ws) = c.attrs.get("colwidth").and_then(|v| v.as_array()) {
                    for (i, w) in ws.iter().enumerate() {
                        if col + i < cols {
                            px[col + i] = w.as_u64().map(|w| w.clamp(20, 2000) as u32);
                        }
                    }
                }
                col += span(c, "colspan", 63);
            }
        }
        let known: Vec<u32> = px.iter().flatten().copied().collect();
        let default_px = if known.is_empty() { 120 } else { known.iter().sum::<u32>() / known.len() as u32 };
        let widths: Vec<u32> = px.iter().map(|w| w.unwrap_or(default_px)).collect();
        let total: u32 = widths.iter().sum::<u32>().max(1);
        let columns: Vec<String> = widths.iter().map(|w| format!("{w}fr")).collect();
        // Typst places cells in order and skips the slots of rowspans above; short rows are padded
        let mut covered: Vec<usize> = vec![0; cols];
        let mut out_rows: Vec<String> = Vec::new();
        let mut header_row = false;
        for (ri, row) in rows.iter().enumerate() {
            let mut cells = Vec::new();
            let mut col = 0;
            let mut row_covered = covered.clone();
            for c in &row.content {
                while col < cols && covered[col] > 0 {
                    col += 1;
                }
                if col >= cols {
                    break;
                }
                let cs = span(c, "colspan", 63).min(cols - col);
                let rs = span(c, "rowspan", 1000).min(rows.len() - ri);
                let w: u32 = widths[col..col + cs].iter().sum();
                let header = c.kind == nodes::TABLE_HEADER;
                let inner = Ctx { width: (ctx.width * w as f32 / total as f32 - 10.0).max(20.0), bold: header };
                let body = self.blocks(&c.content, inner);
                let mut args = Vec::new();
                if cs > 1 {
                    args.push(format!("colspan: {cs}"));
                }
                if rs > 1 {
                    args.push(format!("rowspan: {rs}"));
                }
                if let Some(bg) = c.attr_str("background").and_then(hex_color).or_else(|| header.then(|| "F1F5F9".to_string())) {
                    args.push(format!("fill: {}", rgb(&bg)));
                }
                cells.push(if args.is_empty() { format!("[{body}]") } else { format!("table.cell({})[{body}]", args.join(", ")) });
                for slot in row_covered.iter_mut().skip(col).take(cs) {
                    *slot = rs;
                }
                col += cs;
            }
            // pad the row: the free slots left after its cells
            for c in col..cols {
                if covered[c] == 0 && row_covered[c] == 0 {
                    cells.push("[]".into());
                }
            }
            if ri == 0 {
                header_row = !row.content.is_empty() && row.content.iter().all(|c| c.kind == nodes::TABLE_HEADER && span(c, "rowspan", 1000) == 1);
            }
            // rows covered from above count down
            covered = row_covered.iter().map(|r| r.saturating_sub(1)).collect();
            out_rows.push(cells.join(", "));
        }
        if out_rows.is_empty() {
            return String::new();
        }
        let body = if header_row && out_rows.len() > 1 {
            let head = out_rows.remove(0);
            format!("table.header({head}), {}", out_rows.join(", "))
        } else {
            out_rows.join(", ")
        };
        format!("#table(columns: ({},), {body});", columns.join(", "))
    }

    /// A report macro's result: a small label and a table.
    fn snapshot(&mut self, snap: &Snapshot) -> String {
        let label = format!("#strong[{}];", muted(&t(&snap.title), Some(8.5)));
        if snap.rows.is_empty() {
            return format!("{label}\n\n#emph[{}];", muted(&t(snap.empty), None));
        }
        let cols = snap.header.len().max(1);
        let head: Vec<String> = snap.header.iter().map(|h| format!("table.cell(fill: rgb(\"#F1F5F9\"))[#strong[{}]]", t(h))).collect();
        let mut cells = Vec::new();
        for r in &snap.rows {
            for i in 0..cols {
                let cell = match r.get(i) {
                    Some((text, Some(url))) => format!("[{}]", self.link(url, t(text))),
                    Some((text, None)) => format!("[{}]", t(text)),
                    None => "[]".into(),
                };
                cells.push(cell);
            }
        }
        format!("{label}\n\n#table(columns: {}, table.header({}), {});", fr_columns(cols), head.join(", "), cells.join(", "))
    }
}

/// `(1fr, 1fr, …)`
fn fr_columns(n: usize) -> String {
    format!("({},)", vec!["1fr"; n.max(1)].join(", "))
}

/// Text with its marks (links are added by the caller).
fn styled(mut out: String, ms: &[Mark]) -> String {
    for m in ms {
        out = match m.kind.as_str() {
            marks::BOLD => format!("#strong[{out}];"),
            marks::ITALIC => format!("#emph[{out}];"),
            marks::UNDERLINE => format!("#underline[{out}];"),
            marks::STRIKE => format!("#strike[{out}];"),
            marks::SUBSCRIPT => format!("#sub[{out}];"),
            marks::SUPERSCRIPT => format!("#super[{out}];"),
            marks::TEXT_STYLE => match m.attr_str("color").and_then(hex_color) {
                Some(c) => format!("#text(fill: {})[{out}];", rgb(&c)),
                None => out,
            },
            marks::HIGHLIGHT => {
                let c = m.attr_str("color").and_then(hex_color).unwrap_or_else(|| "FEF08A".into());
                format!("#highlight(fill: {})[{out}];", rgb(&c))
            }
            _ => out,
        };
    }
    out
}

// ---------------------------------------------------------------------------------------------
// Fonts and the compiler's world

/// A font: built in (loaded), or a file on the server read when first used.
struct FontSlot {
    path: Option<PathBuf>,
    index: u32,
    font: OnceLock<Option<Font>>,
}

struct FontStore {
    book: FontBook,
    slots: Vec<FontSlot>,
}

static FONTS: OnceLock<FontStore> = OnceLock::new();

const NOTO_SANS: [&[u8]; 4] = [
    include_bytes!("../assets/fonts/NotoSans-Regular.ttf"),
    include_bytes!("../assets/fonts/NotoSans-Bold.ttf"),
    include_bytes!("../assets/fonts/NotoSans-Italic.ttf"),
    include_bytes!("../assets/fonts/NotoSans-BoldItalic.ttf"),
];

/// The built-in fonts plus the ones installed on the server (standard font folders and
/// `<data dir>/fonts`), for scripts the built-in ones don't cover (CJK, Arabic, …). Scanned once.
fn fonts(dirs: &[PathBuf]) -> &'static FontStore {
    FONTS.get_or_init(|| {
        let mut book = FontBook::new();
        let mut slots = Vec::new();
        for data in NOTO_SANS.into_iter().chain(typst_assets::fonts()) {
            for font in Font::iter(Bytes::new(data)) {
                book.push(font.info().clone());
                slots.push(FontSlot { path: None, index: font.index(), font: OnceLock::from(Some(font)) });
            }
        }
        let mut files = Vec::new();
        for dir in dirs {
            collect_font_files(dir, 0, &mut files);
        }
        for path in files {
            let Ok(data) = std::fs::read(&path) else { continue };
            for index in 0..64 {
                match FontInfo::new(&data, index) {
                    Some(info) => {
                        book.push(info);
                        slots.push(FontSlot { path: Some(path.clone()), index, font: OnceLock::new() });
                    }
                    None => break,
                }
            }
        }
        FontStore { book, slots }
    })
}

fn collect_font_files(dir: &Path, depth: usize, out: &mut Vec<PathBuf>) {
    if depth > 6 || out.len() > 3_000 {
        return;
    }
    let Ok(entries) = std::fs::read_dir(dir) else { return };
    for e in entries.flatten() {
        let path = e.path();
        let Ok(meta) = e.metadata() else { continue };
        if meta.is_dir() {
            collect_font_files(&path, depth + 1, out);
        } else if meta.len() < (96 << 20) {
            let ext = path.extension().and_then(|x| x.to_str()).map(str::to_ascii_lowercase);
            if matches!(ext.as_deref(), Some("ttf" | "otf" | "ttc" | "otc")) {
                out.push(path);
            }
        }
    }
}

/// Where the server looks for installed fonts.
pub fn font_dirs(data_dir: &Path) -> Vec<PathBuf> {
    ["/usr/share/fonts", "/usr/local/share/fonts"].iter().map(PathBuf::from).chain(std::iter::once(data_dir.join("fonts"))).collect()
}

struct ExportWorld {
    library: LazyHash<Library>,
    book: LazyHash<FontBook>,
    fonts: &'static FontStore,
    main: FileId,
    source: Source,
    files: HashMap<String, Bytes>,
    today: Option<Datetime>,
}

impl World for ExportWorld {
    fn library(&self) -> &LazyHash<Library> {
        &self.library
    }

    fn book(&self) -> &LazyHash<FontBook> {
        &self.book
    }

    fn main(&self) -> FileId {
        self.main
    }

    fn source(&self, id: FileId) -> FileResult<Source> {
        if id == self.main { Ok(self.source.clone()) } else { Err(FileError::AccessDenied) }
    }

    fn file(&self, id: FileId) -> FileResult<Bytes> {
        let path = id.get().vpath().get_with_slash();
        self.files.get(path).cloned().ok_or(FileError::AccessDenied)
    }

    fn font(&self, index: usize) -> Option<Font> {
        let slot = self.fonts.slots.get(index)?;
        slot.font
            .get_or_init(|| {
                let data = std::fs::read(slot.path.as_ref()?).ok()?;
                Font::new(Bytes::new(data), slot.index)
            })
            .clone()
    }

    fn today(&self, _offset: Option<Duration>) -> Option<Datetime> {
        self.today
    }
}

fn file_id(path: &str) -> anyhow::Result<FileId> {
    let vpath = VirtualPath::new(path).map_err(|e| anyhow::anyhow!("path {path}: {e:?}"))?;
    Ok(FileId::new(RootedPath::new(VirtualRoot::Project, vpath)))
}

fn compile(source: String, files: HashMap<String, Vec<u8>>, opts: &PdfOptions, font_dirs: &[PathBuf]) -> anyhow::Result<Vec<u8>> {
    use chrono::Datelike;
    let fonts = fonts(font_dirs);
    let main = file_id("/main.typ")?;
    let world = ExportWorld {
        library: LazyHash::new(<Library as LibraryExt>::default()),
        book: LazyHash::new(fonts.book.clone()),
        fonts,
        main,
        source: Source::new(main, source),
        files: files.into_iter().map(|(k, v)| (k, Bytes::new(v))).collect(),
        today: Datetime::from_ymd(opts.date.year(), opts.date.month() as u8, opts.date.day() as u8),
    };
    let doc: PagedDocument = typst::compile::<PagedDocument>(&world).output.map_err(|errors| {
        let msgs: Vec<String> = errors.iter().take(3).map(|e| e.message.to_string()).collect();
        anyhow::anyhow!("PDF typesetting failed: {}", msgs.join("; "))
    })?;
    typst_pdf::pdf(&doc, &typst_pdf::PdfOptions::default()).map_err(|errors| {
        let msgs: Vec<String> = errors.iter().take(3).map(|e| e.message.to_string()).collect();
        anyhow::anyhow!("PDF export failed: {}", msgs.join("; "))
    })
}

/// Labels used by the export, for tests: every `<wc-…>` the source defines exactly once.
#[cfg(test)]
fn defined_labels(src: &str) -> std::collections::HashSet<String> {
    let mut seen = std::collections::HashSet::new();
    for part in src.split("]<").skip(1).chain(src.split(")<").skip(1)) {
        if let Some(end) = part.find('>') {
            assert!(seen.insert(part[..end].to_string()), "label {} defined twice", &part[..end]);
        }
    }
    seen
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn doc(v: serde_json::Value) -> Node {
        Node::from_json(&v).unwrap()
    }

    fn input<'a>(title: &'a str, d: &'a Node, images: &'a crate::docx::Images) -> DocxInput<'a> {
        DocxInput {
            title,
            subtitle: Some("Space · Last updated".into()),
            doc: d,
            images,
            base_url: "https://wiki.acme.test",
            children: &[],
            macros: &[],
            page_id: None,
        }
    }

    fn opts() -> PdfOptions {
        PdfOptions {
            toc: true,
            cover: true,
            header: "Engineering".into(),
            footer: "Exported by Ann".into(),
            date: chrono::NaiveDate::from_ymd_opt(2026, 11, 11).unwrap(),
            lang: "en".into(),
        }
    }

    #[test]
    fn page_text_is_never_code() {
        assert_eq!(lit(r#"a "b" \c #d"#), r#""a \"b\" \\c #d""#);
        assert_eq!(lit("x\u{0007}y"), "\"x\\u{7}y\"");
        let d = doc(json!({"type": "doc", "content": [
            {"type": "paragraph", "content": [{"type": "text", "text": "#read(\"/etc/passwd\") ]; #panic() $x$ <lbl> @ref *b* _i_"}]}
        ]}));
        let images = crate::docx::Images::new();
        let mut files = HashMap::new();
        let src = source(&[input("T", &d, &images)], &opts(), &mut files);
        assert!(src.contains(r##"#"#read(\"/etc/passwd\") ]; #panic() $x$ <lbl> @ref *b* _i_";"##), "{src}");
        let pdf = compile(src, files, &opts(), &[]).unwrap();
        assert!(pdf.starts_with(b"%PDF"));
    }

    #[test]
    fn renders_the_whole_vocabulary() {
        let d = doc(json!({"type": "doc", "content": [
            {"type": "toc"},
            {"type": "heading", "attrs": {"level": 1}, "content": [{"type": "text", "text": "Intro"}]},
            {"type": "paragraph", "attrs": {"textAlign": "center"}, "content": [
                {"type": "text", "text": "bold", "marks": [{"type": "bold"}]},
                {"type": "text", "text": " code", "marks": [{"type": "code"}]},
                {"type": "text", "text": " link", "marks": [{"type": "link", "attrs": {"href": "https://example.com/?a=\"x\""}}]},
                {"type": "text", "text": " jump", "marks": [{"type": "link", "attrs": {"href": "#intro"}}]},
                {"type": "text", "text": " red", "marks": [{"type": "textStyle", "attrs": {"color": "#ff0000"}}, {"type": "highlight"}]},
                {"type": "hardBreak"},
                {"type": "mention", "attrs": {"id": "u1", "label": "Ann"}},
                {"type": "status", "attrs": {"text": "done", "color": "green"}},
                {"type": "date", "attrs": {"date": "2026-11-11"}},
                {"type": "emoticon", "attrs": {"name": "smile"}},
                {"type": "anchor", "attrs": {"name": "here"}},
                {"type": "inlineMath", "attrs": {"latex": "x^2"}}
            ]},
            {"type": "bulletList", "content": [{"type": "listItem", "content": [
                {"type": "paragraph", "content": [{"type": "text", "text": "one"}]},
                {"type": "orderedList", "attrs": {"start": 3}, "content": [{"type": "listItem", "content": [{"type": "paragraph", "content": [{"type": "text", "text": "three"}]}]}]}
            ]}]},
            {"type": "taskList", "content": [
                {"type": "taskItem", "attrs": {"checked": true}, "content": [{"type": "paragraph", "content": [{"type": "text", "text": "done"}]}]},
                {"type": "taskItem", "attrs": {"checked": false}, "content": [{"type": "paragraph", "content": [{"type": "text", "text": "open"}]}]}
            ]},
            {"type": "decisionList", "content": [{"type": "decisionItem", "content": [{"type": "text", "text": "We ship"}]}]},
            {"type": "codeBlock", "attrs": {"language": "rust"}, "content": [{"type": "text", "text": "fn main() {\n    println!(\"]\");\n}"}]},
            {"type": "blockquote", "content": [{"type": "paragraph", "content": [{"type": "text", "text": "quoted"}]}]},
            {"type": "horizontalRule"},
            {"type": "callout", "attrs": {"type": "warning"}, "content": [{"type": "paragraph", "content": [{"type": "text", "text": "careful"}]}]},
            {"type": "details", "content": [
                {"type": "detailsSummary", "content": [{"type": "text", "text": "More"}]},
                {"type": "detailsContent", "content": [{"type": "paragraph", "content": [{"type": "text", "text": "hidden"}]}]}
            ]},
            {"type": "table", "content": [
                {"type": "tableRow", "content": [
                    {"type": "tableHeader", "content": [{"type": "paragraph", "content": [{"type": "text", "text": "A"}]}]},
                    {"type": "tableHeader", "content": [{"type": "paragraph", "content": [{"type": "text", "text": "B"}]}]},
                    {"type": "tableHeader", "content": [{"type": "paragraph", "content": [{"type": "text", "text": "C"}]}]}
                ]},
                {"type": "tableRow", "content": [
                    {"type": "tableCell", "attrs": {"rowspan": 2}, "content": [{"type": "paragraph", "content": [{"type": "text", "text": "tall"}]}]},
                    {"type": "tableCell", "attrs": {"colspan": 2, "background": "#fef3c7"}, "content": [{"type": "paragraph", "content": [{"type": "text", "text": "wide"}]}]}
                ]},
                {"type": "tableRow", "content": [
                    {"type": "tableCell", "content": [{"type": "paragraph", "content": [{"type": "text", "text": "short row"}]}]}
                ]}
            ]},
            {"type": "columns", "content": [
                {"type": "column", "content": [{"type": "paragraph", "content": [{"type": "text", "text": "left"}]}]},
                {"type": "column", "content": [{"type": "heading", "attrs": {"level": 2}, "content": [{"type": "text", "text": "Right"}]}]}
            ]},
            {"type": "blockMath", "attrs": {"latex": "\\frac{a}{b}"}},
            {"type": "image", "attrs": {"src": "https://elsewhere.test/x.png", "alt": "remote"}},
            {"type": "embed", "attrs": {"url": "https://www.youtube.com/watch?v=x"}},
            {"type": "macro", "attrs": {"name": "unknown"}},
            {"type": "paragraph", "content": [{"type": "text", "text": "back", "marks": [{"type": "link", "attrs": {"href": "#here"}}]}]},
            {"type": "paragraph"}
        ]}));
        let images = crate::docx::Images::new();
        let mut files = HashMap::new();
        let first = input("Runbook", &d, &images);
        let second_doc = doc(json!({"type": "doc", "content": [{"type": "heading", "attrs": {"level": 1}, "content": [{"type": "text", "text": "Intro"}]}]}));
        let second = DocxInput { page_id: Some(Uuid::nil()), ..input("Child", &second_doc, &images) };
        let src = source(&[first, second], &opts(), &mut files);
        let labels = defined_labels(&src);
        assert!(labels.contains("wc-0-h0") && labels.contains("wc-1-h0") && labels.contains("wc-p1"), "{labels:?}");
        assert!(files.keys().any(|k| k.starts_with("/icons/task-on")), "{:?}", files.keys());
        let pdf = compile(src.clone(), files, &opts(), &[]).unwrap_or_else(|e| panic!("{e}\n{src}"));
        assert!(pdf.starts_with(b"%PDF") && pdf.len() > 10_000);
        // WIKICRAFT_PDF_SAMPLE=/path/sample.pdf keeps the result for a look
        if let Ok(path) = std::env::var("WIKICRAFT_PDF_SAMPLE") {
            std::fs::write(path, &pdf).unwrap();
        }
    }
}
