//! Word (.docx) import: one document becomes one page.
//!
//! Reads `word/document.xml` with its styles, numbering, relationships and footnotes:
//! headings (by style name or outline level, so localized styles work), bullet and numbered
//! lists (nesting from the list level, numbering that continues across interruptions),
//! check-box tasks, tables with merged cells, column widths and shading, images (returned
//! as files for the caller to store as attachments), hyperlinks (also as fields), code
//! (monospace paragraphs), quotes, rules, a table of contents and footnotes. Single-cell
//! shaded tables — how WikiCraft exports callouts — come back as callouts.

use std::collections::HashMap;
use std::io::{Cursor, Read};

use wikicraft_confluence::dom::{self, Dom, Element};
use wikicraft_doc_model::{Mark, Node, marks, nodes, text};

pub struct Imported {
    /// from the Title style or the document properties
    pub title: Option<String>,
    pub doc: Node,
    /// images as (file name, bytes); the document refers to them as `{media_prefix}{name}`
    pub media: Vec<(String, Vec<u8>)>,
}

/// limits for untrusted input: XML parts, images, text produced, footnote text
const MAX_PART: u64 = 24 << 20;
const MAX_IMAGE: u64 = 25 << 20;
const MAX_MEDIA_TOTAL: u64 = 256 << 20;
const MAX_IMAGES: usize = 500;
const MAX_TEXT: usize = 16 << 20;
const MAX_FOOTNOTE: usize = 20_000;
const MAX_DEPTH: usize = 12;
const MONO_FONTS: &[&str] = &[
    "consolas",
    "courier",
    "courier new",
    "menlo",
    "monaco",
    "lucida console",
    "source code pro",
    "cascadia code",
    "cascadia mono",
    "dejavu sans mono",
    "liberation mono",
    "roboto mono",
    "fira code",
    "fira mono",
    "jetbrains mono",
    "sf mono",
];

fn local(name: &str) -> &str {
    name.rsplit(':').next().unwrap_or(name)
}

trait El {
    fn is(&self, l: &str) -> bool;
    fn at(&self, l: &str) -> Option<&str>;
    fn kid(&self, l: &str) -> Option<&Element>;
    fn kids<'a>(&'a self, l: &'a str) -> Box<dyn Iterator<Item = &'a Element> + 'a>;
    fn find(&self, l: &str) -> Option<&Element>;
    /// `<w:b/>`-style toggles: present and not switched off
    fn on(&self, l: &str) -> bool;
}

impl El for Element {
    fn is(&self, l: &str) -> bool {
        local(&self.name) == l
    }
    fn at(&self, l: &str) -> Option<&str> {
        self.attrs.iter().find(|(k, _)| local(k) == l).map(|(_, v)| v.as_str())
    }
    fn kid(&self, l: &str) -> Option<&Element> {
        self.elements().find(|e| e.is(l))
    }
    fn kids<'a>(&'a self, l: &'a str) -> Box<dyn Iterator<Item = &'a Element> + 'a> {
        Box::new(self.elements().filter(move |e| e.is(l)))
    }
    fn find(&self, l: &str) -> Option<&Element> {
        let mut stack: Vec<&Element> = vec![self];
        while let Some(e) = stack.pop() {
            for c in e.elements() {
                if c.is(l) {
                    return Some(c);
                }
                stack.push(c);
            }
        }
        None
    }
    fn on(&self, l: &str) -> bool {
        self.kid(l).is_some_and(|e| !matches!(e.at("val"), Some("0" | "false" | "off" | "none")))
    }
}

#[derive(Default, Clone)]
struct StyleInfo {
    name: String,
    based_on: Option<String>,
    outline: Option<u8>,
    num: Option<(String, u32)>,
    mono: bool,
}

struct Reader<'a> {
    zip: zip::ZipArchive<Cursor<&'a [u8]>>,
    main_dir: String,
    rels: HashMap<String, (String, bool)>,
    styles: HashMap<String, StyleInfo>,
    numbering: HashMap<String, NumDef>,
    /// (numId, level) pairs already used: a start override applies to the first use only
    seen_num: std::collections::HashSet<(String, u32)>,
    footnotes: HashMap<String, String>,
    footnote_order: Vec<String>,
    /// bytes of text still allowed; the import fails when it runs out
    text_budget: usize,
    overflow: bool,
    image_count: usize,
    media_names: std::collections::HashSet<String>,
    media: Vec<(String, Vec<u8>)>,
    media_by_path: HashMap<String, String>,
    media_total: u64,
    prefix: String,
    title: Option<String>,
    toc_done: bool,
    started: bool,
}

/// Converts a .docx file. Images are named uniquely and referenced as `{media_prefix}{name}`.
pub fn convert(data: &[u8], media_prefix: &str) -> anyhow::Result<Imported> {
    let zip = zip::ZipArchive::new(Cursor::new(data))?;
    let mut r = Reader {
        zip,
        main_dir: "word".into(),
        rels: HashMap::new(),
        styles: HashMap::new(),
        numbering: HashMap::new(),
        seen_num: Default::default(),
        footnotes: HashMap::new(),
        footnote_order: Vec::new(),
        text_budget: MAX_TEXT,
        overflow: false,
        image_count: 0,
        media_names: Default::default(),
        media: Vec::new(),
        media_by_path: HashMap::new(),
        media_total: 0,
        prefix: media_prefix.to_string(),
        title: None,
        toc_done: false,
        started: false,
    };
    // the main part, from the package relationships
    let mut main = "word/document.xml".to_string();
    if let Some(rels) = r.part("_rels/.rels").and_then(|x| dom::parse_document(&x))
        && let Some(t) = rels.kids("Relationship").find(|e| e.at("Type").is_some_and(|t| t.ends_with("/officeDocument"))).and_then(|e| e.at("Target"))
    {
        main = t.trim_start_matches('/').to_string();
    }
    r.main_dir = main.rsplit_once('/').map(|(d, _)| d.to_string()).unwrap_or_default();
    let file = main.rsplit('/').next().unwrap_or("document.xml").to_string();
    let xml = r.part(&main).ok_or_else(|| anyhow::anyhow!("not a Word document (no {main})"))?;
    let root = dom::parse_document(&xml).ok_or_else(|| anyhow::anyhow!("document.xml is not valid XML"))?;
    drop(xml);
    r.load_rels(&format!("{}/_rels/{file}.rels", r.main_dir));
    r.load_styles();
    r.load_numbering();
    r.load_footnotes();
    if let Some(core) = r.part("docProps/core.xml").and_then(|x| dom::parse_document(&x)) {
        r.title = core.kid("title").map(|t| t.text().trim().to_string()).filter(|t| !t.is_empty());
    }
    let core_title = r.title.take();

    let body = root.find("body").ok_or_else(|| anyhow::anyhow!("document has no body"))?;
    let mut blocks = r.blocks(body, 0);
    if r.overflow {
        anyhow::bail!("the document is too large to import (more than {} MB of text)", MAX_TEXT >> 20);
    }
    if !r.footnote_order.is_empty() {
        blocks.push(Node::new(nodes::HORIZONTAL_RULE));
        for (i, id) in r.footnote_order.clone().iter().enumerate() {
            let text = r.footnotes.get(id).cloned().unwrap_or_default();
            blocks.push(Node::paragraph(vec![Node::text_marked(format!("{}", i + 1), vec![Mark::new(marks::SUPERSCRIPT)]), Node::text(format!(" {text}"))]));
        }
    }
    if blocks.is_empty() {
        blocks.push(Node::paragraph(vec![]));
    }
    Ok(Imported { title: r.title.take().or(core_title), doc: Node::doc(blocks), media: std::mem::take(&mut r.media) })
}

/// Paragraph classification before grouping.
enum Para {
    Block(Node),
    List(Item),
    Task {
        checked: bool,
        content: Vec<Node>,
    },
    Code(String),
    Quote(Node),
    /// a caption paragraph: belongs to the image just before it
    Caption(String),
}

impl Reader<'_> {
    fn part(&mut self, name: &str) -> Option<String> {
        let f = self.zip.by_name(name).ok()?;
        let mut s = String::new();
        f.take(MAX_PART).read_to_string(&mut s).ok()?;
        Some(s)
    }

    fn load_rels(&mut self, path: &str) {
        let Some(rels) = self.part(path).and_then(|x| dom::parse_document(&x)) else { return };
        for e in rels.kids("Relationship") {
            if let (Some(id), Some(target)) = (e.at("Id"), e.at("Target")) {
                self.rels.insert(id.to_string(), (target.to_string(), e.at("TargetMode") == Some("External")));
            }
        }
    }

    fn load_styles(&mut self) {
        let path = format!("{}/styles.xml", self.main_dir);
        let Some(styles) = self.part(&path).and_then(|x| dom::parse_document(&x)) else { return };
        for s in styles.kids("style") {
            let Some(id) = s.at("styleId") else { continue };
            let ppr = s.kid("pPr");
            let info = StyleInfo {
                name: s.kid("name").and_then(|n| n.at("val")).unwrap_or(id).to_lowercase(),
                based_on: s.kid("basedOn").and_then(|b| b.at("val")).map(str::to_string),
                outline: ppr.and_then(|p| p.kid("outlineLvl")).and_then(|o| o.at("val")).and_then(|v| v.parse().ok()),
                num: ppr.and_then(|p| p.kid("numPr")).and_then(|n| {
                    let id = n.kid("numId")?.at("val")?.to_string();
                    Some((id, n.kid("ilvl").and_then(|l| l.at("val")).and_then(|v| v.parse().ok()).unwrap_or(0)))
                }),
                mono: s.kid("rPr").and_then(|r| r.kid("rFonts")).and_then(|f| f.at("ascii")).is_some_and(is_mono),
            };
            self.styles.insert(id.to_string(), info);
        }
    }

    fn load_numbering(&mut self) {
        let path = format!("{}/numbering.xml", self.main_dir);
        let Some(num) = self.part(&path).and_then(|x| dom::parse_document(&x)) else { return };
        let mut abstracts: HashMap<String, HashMap<u32, (bool, i64)>> = HashMap::new();
        for a in num.kids("abstractNum") {
            let Some(id) = a.at("abstractNumId") else { continue };
            let mut levels = HashMap::new();
            for l in a.kids("lvl") {
                let ilvl: u32 = l.at("ilvl").and_then(|v| v.parse().ok()).unwrap_or(0);
                let fmt = l.kid("numFmt").and_then(|f| f.at("val")).unwrap_or("decimal");
                let start = l.kid("start").and_then(|s| s.at("val")).and_then(|v| v.parse().ok()).unwrap_or(1);
                levels.insert(ilvl, (fmt == "bullet" || fmt == "none", start));
            }
            abstracts.insert(id.to_string(), levels);
        }
        for n in num.kids("num") {
            let (Some(id), Some(abs)) = (n.at("numId"), n.kid("abstractNumId").and_then(|a| a.at("val"))) else { continue };
            let mut def = NumDef { abs: abs.to_string(), levels: abstracts.get(abs).cloned().unwrap_or_default(), overrides: HashMap::new() };
            for o in n.kids("lvlOverride") {
                let ilvl: u32 = o.at("ilvl").and_then(|v| v.parse().ok()).unwrap_or(0);
                if let Some(start) = o.kid("startOverride").and_then(|s| s.at("val")).and_then(|v| v.parse().ok()) {
                    def.overrides.insert(ilvl, start);
                }
            }
            self.numbering.insert(id.to_string(), def);
        }
    }

    fn load_footnotes(&mut self) {
        let path = format!("{}/footnotes.xml", self.main_dir);
        let Some(f) = self.part(&path).and_then(|x| dom::parse_document(&x)) else { return };
        for n in f.kids("footnote") {
            if matches!(n.at("type"), Some("separator" | "continuationSeparator" | "continuationNotice")) {
                continue;
            }
            if let Some(id) = n.at("id") {
                let text: Vec<String> = n.kids("p").map(|p| text_of(p).trim().to_string()).filter(|t| !t.is_empty()).collect();
                let text = text.join(" ");
                let text = if text.chars().count() > MAX_FOOTNOTE { format!("{}…", text.chars().take(MAX_FOOTNOTE).collect::<String>()) } else { text };
                self.footnotes.insert(id.to_string(), text);
            }
        }
    }

    fn style(&self, id: &str) -> StyleInfo {
        // follow basedOn for the outline level / numbering a style inherits
        let mut info = self.styles.get(id).cloned().unwrap_or_default();
        let mut cur = info.based_on.clone();
        for _ in 0..6 {
            let Some(b) = cur.and_then(|b| self.styles.get(&b)) else { break };
            if info.outline.is_none() {
                info.outline = b.outline;
            }
            if info.num.is_none() {
                info.num = b.num.clone();
            }
            info.mono |= b.mono;
            if heading_level(&info.name).is_none() && heading_level(&b.name).is_some() && info.outline.is_none() {
                info.outline = heading_level(&b.name).map(|l| l - 1);
            }
            cur = b.based_on.clone();
        }
        info
    }

    /// Converts the block-level children of `parent` (body, cell, content control).
    fn blocks(&mut self, parent: &Element, depth: usize) -> Vec<Node> {
        let mut paras: Vec<Para> = Vec::new();
        self.collect(parent, depth, &mut paras);
        group(paras)
    }

    fn collect(&mut self, parent: &Element, depth: usize, out: &mut Vec<Para>) {
        for e in parent.elements() {
            match local(&e.name) {
                "p" => {
                    if let Some(p) = self.paragraph(e) {
                        self.started = true;
                        out.push(p);
                    }
                }
                "tbl" if depth < MAX_DEPTH => {
                    self.started = true;
                    out.push(Para::Block(self.table(e, depth + 1)));
                }
                "sdt" => {
                    let gallery = e.kid("sdtPr").and_then(|p| p.find("docPartGallery")).and_then(|g| g.at("val")).unwrap_or("");
                    if gallery.eq_ignore_ascii_case("Table of Contents") {
                        if !self.toc_done {
                            self.toc_done = true;
                            out.push(Para::Block(Node::new(nodes::TOC)));
                        }
                    } else if let Some(c) = e.kid("sdtContent") {
                        self.collect(c, depth, out);
                    }
                }
                "customXml" | "ins" | "smartTag" => self.collect(e, depth, out),
                _ => {}
            }
        }
    }

    fn paragraph(&mut self, p: &Element) -> Option<Para> {
        let ppr = p.kid("pPr");
        let style_id = ppr.and_then(|x| x.kid("pStyle")).and_then(|s| s.at("val")).unwrap_or("");
        let style = self.style(style_id);
        let name = style.name.as_str();
        // table of contents entries: one toc node instead
        if name.starts_with("toc ") || (name.starts_with("toc") && name[3..].trim().parse::<u8>().is_ok()) {
            if self.toc_done {
                return None;
            }
            self.toc_done = true;
            return Some(Para::Block(Node::new(nodes::TOC)));
        }
        let mut checkbox: Option<bool> = None;
        let mut content = self.inlines(p, &mut checkbox);
        merge_text(&mut content);
        let plain: String = content.iter().map(text::inline_text).collect();
        let has_media = content.iter().any(|n| n.kind != nodes::TEXT);

        // horizontal rule: an empty paragraph with a border
        if plain.trim().is_empty() && !has_media {
            let bordered = ppr.and_then(|x| x.kid("pBdr")).is_some_and(|b| b.kid("bottom").or(b.kid("top")).is_some());
            return bordered.then(|| Para::Block(Node::new(nodes::HORIZONTAL_RULE)));
        }
        let align = ppr.and_then(|x| x.kid("jc")).and_then(|j| j.at("val")).and_then(|v| match v {
            "center" => Some("center"),
            "right" | "end" => Some("right"),
            "both" | "distribute" => Some("justify"),
            _ => None,
        });
        let outline: Option<u8> = ppr.and_then(|x| x.kid("outlineLvl")).and_then(|o| o.at("val")).and_then(|v| v.parse().ok()).or(style.outline);
        // left indentation in half inches (720 twips) = indentation levels
        let indent: i64 = ppr
            .and_then(|x| x.kid("ind"))
            .and_then(|i| i.at("left").or_else(|| i.at("start")))
            .and_then(|v| v.parse::<f64>().ok())
            .map(|tw| ((tw / 720.0).round() as i64).clamp(0, wikicraft_doc_model::node::MAX_INDENT))
            .unwrap_or(0);

        if name == "title" {
            if self.title.is_none() && !self.started {
                self.title = Some(plain.trim().to_string());
                return None;
            }
            return Some(Para::Block(Node::heading(1, content)));
        }
        if let Some(level) = heading_level(name).or_else(|| outline.filter(|o| *o < 6).map(|o| o + 1)) {
            let mut h = Node::heading(level.min(6), strip_marks(content, &[marks::BOLD]));
            if let Some(a) = align {
                h = h.attr("textAlign", a);
            }
            if indent > 0 {
                h = h.attr("indent", indent);
            }
            return Some(Para::Block(h));
        }
        // tasks: a check-box content control or a ballot box character first
        if checkbox.is_none()
            && let Some(first) = content.first_mut().filter(|n| n.kind == nodes::TEXT)
        {
            let t = first.text.clone().unwrap_or_default();
            for (prefix, checked) in [("\u{2610}", false), ("\u{2611}", true), ("\u{2612}", true), ("[ ]", false), ("[x]", true), ("[X]", true)] {
                if let Some(rest) = t.trim_start().strip_prefix(prefix) {
                    checkbox = Some(checked);
                    first.text = Some(rest.trim_start().to_string());
                    break;
                }
            }
            content.retain(|n| n.kind != nodes::TEXT || n.text.as_deref().is_some_and(|t| !t.is_empty()));
        }
        if let Some(checked) = checkbox {
            return Some(Para::Task { checked, content });
        }
        // lists: numbering on the paragraph or its style
        let num = ppr
            .and_then(|x| x.kid("numPr"))
            .and_then(|n| {
                let id = n.kid("numId")?.at("val")?.to_string();
                Some((id, n.kid("ilvl").and_then(|l| l.at("val")).and_then(|v| v.parse().ok()).unwrap_or(0)))
            })
            .or(style.num.clone())
            .filter(|(id, _)| id != "0");
        if let Some((num, level)) = num {
            // numIds sharing an abstract list continue one list (LibreOffice writes them so)
            let def = self.numbering.get(&num);
            let (bullet, start) = def.and_then(|d| d.levels.get(&level)).copied().unwrap_or((true, 1));
            let first_use = self.seen_num.insert((num.clone(), level));
            let restart = def.and_then(|d| d.overrides.get(&level)).copied().filter(|_| first_use);
            let key = def.map(|d| format!("a{}", d.abs)).unwrap_or(num);
            return Some(Para::List(Item { key, level: level.min(8), bullet, start, restart, content }));
        }
        if name.starts_with("list bullet") || name.starts_with("list number") {
            return Some(Para::List(Item { key: name.to_string(), level: 0, bullet: name.starts_with("list bullet"), start: 1, restart: None, content }));
        }
        // code: a code style, or monospace throughout
        let mono_runs = !content.is_empty() && content.iter().all(|n| n.kind == nodes::TEXT && n.marks.iter().any(|m| m.kind == marks::CODE));
        if name.contains("code") || name.contains("source") || name == "html preformatted" || name == "plain text" || style.mono || mono_runs {
            return Some(Para::Code(plain));
        }
        if matches!(name, "caption" | "image caption" | "figure caption" | "table caption") {
            return Some(Para::Caption(plain.trim().to_string()));
        }
        if name.contains("quote")
            || name == "block text"
            || ppr.and_then(|x| x.kid("pBdr")).is_some_and(|b| b.kid("left").is_some() && b.kid("top").is_none() && b.kid("bottom").is_none())
        {
            return Some(Para::Quote(Node::paragraph(content)));
        }
        let mut node = Node::paragraph(content);
        if let Some(a) = align {
            node = node.attr("textAlign", a);
        }
        if indent > 0 {
            node = node.attr("indent", indent);
        }
        Some(Para::Block(node))
    }

    /// Inline content of a paragraph (runs, links, fields, content controls).
    fn inlines(&mut self, p: &Element, checkbox: &mut Option<bool>) -> Vec<Node> {
        let mut out = Vec::new();
        let mut field = FieldState::default();
        self.inline_children(p, &[], &mut field, checkbox, &mut out, 0);
        out
    }

    fn inline_children(&mut self, parent: &Element, link: &[Mark], field: &mut FieldState, checkbox: &mut Option<bool>, out: &mut Vec<Node>, depth: usize) {
        if depth > MAX_DEPTH {
            return;
        }
        for e in parent.elements() {
            match local(&e.name) {
                "r" => self.run(e, link, field, out),
                "hyperlink" => {
                    let href = match (e.at("id").and_then(|id| self.rels.get(id)), e.at("anchor")) {
                        (Some((target, true)), _) => Some(target.clone()),
                        (_, Some(anchor)) => Some(format!("#{}", text::slugify(anchor))),
                        _ => None,
                    };
                    let marks: Vec<Mark> = href.filter(|h| safe_href(h)).map(|h| vec![Mark::link(h)]).unwrap_or_else(|| link.to_vec());
                    self.inline_children(e, &marks, field, checkbox, out, depth + 1);
                }
                "fldSimple" => {
                    let marks = e.at("instr").and_then(hyperlink_instr).map(|h| vec![Mark::link(h)]).unwrap_or_else(|| link.to_vec());
                    self.inline_children(e, &marks, field, checkbox, out, depth + 1);
                }
                "sdt" => {
                    if let Some(cb) = e.kid("sdtPr").and_then(|p| p.find("checkbox")) {
                        *checkbox = Some(cb.kid("checked").is_some_and(|c| matches!(c.at("val"), Some("1" | "true"))));
                        continue;
                    }
                    if let Some(c) = e.kid("sdtContent") {
                        self.inline_children(c, link, field, checkbox, out, depth + 1);
                    }
                }
                "ins" | "smartTag" | "customXml" | "moveTo" => self.inline_children(e, link, field, checkbox, out, depth + 1),
                _ => {}
            }
        }
    }

    fn run(&mut self, r: &Element, link: &[Mark], field: &mut FieldState, out: &mut Vec<Node>) {
        let mut ms = self.run_marks(r.kid("rPr"));
        if let Some(h) = &field.link {
            ms.push(Mark::link(h.clone()));
        } else {
            ms.extend(link.iter().cloned());
        }
        for c in r.elements() {
            match local(&c.name) {
                "t" if !field.in_instr => {
                    let t = c.text();
                    if !t.is_empty() && self.spend(t.len()) {
                        out.push(Node::text_marked(t, ms.clone()));
                    }
                }
                "tab" | "ptab" => out.push(Node::text_marked(" ", ms.clone())),
                "noBreakHyphen" => out.push(Node::text_marked("-", ms.clone())),
                "br" | "cr" if !matches!(c.at("type"), Some("page" | "column")) => out.push(Node::new(nodes::HARD_BREAK)),
                "fldChar" => match c.at("fldCharType") {
                    Some("begin") => {
                        field.in_instr = true;
                        field.instr.clear();
                    }
                    Some("separate") => {
                        field.in_instr = false;
                        field.link = hyperlink_instr(&field.instr);
                    }
                    Some("end") => {
                        field.in_instr = false;
                        field.link = None;
                    }
                    _ => {}
                },
                "instrText" => field.instr.push_str(&c.text()),
                "drawing" | "pict" | "object" => {
                    if let Some(img) = self.image(c) {
                        out.push(img);
                    }
                }
                "footnoteReference" => {
                    if let Some(id) = c.at("id") {
                        // each footnote is listed once; repeated references reuse its number
                        let n = match self.footnote_order.iter().position(|f| f == id) {
                            Some(i) => i + 1,
                            None if self.footnote_order.len() < 5_000 => {
                                self.footnote_order.push(id.to_string());
                                self.footnote_order.len()
                            }
                            None => continue,
                        };
                        if self.spend(8) {
                            out.push(Node::text_marked(format!("{n}"), vec![Mark::new(marks::SUPERSCRIPT)]));
                        }
                    }
                }
                "sym" => {
                    let font = c.at("font").unwrap_or("").to_lowercase();
                    if !font.contains("wingdings")
                        && !font.contains("symbol")
                        && !font.contains("webdings")
                        && let Some(ch) = c.at("char").and_then(|h| u32::from_str_radix(h, 16).ok()).and_then(char::from_u32)
                    {
                        out.push(Node::text_marked(ch.to_string(), ms.clone()));
                    }
                }
                _ => {}
            }
        }
    }

    /// Takes `n` bytes from the text budget; false (and the import fails) once it is spent.
    fn spend(&mut self, n: usize) -> bool {
        match self.text_budget.checked_sub(n) {
            Some(left) => {
                self.text_budget = left;
                true
            }
            None => {
                self.overflow = true;
                false
            }
        }
    }

    fn run_marks(&self, rpr: Option<&Element>) -> Vec<Mark> {
        let mut ms = Vec::new();
        let Some(rpr) = rpr else { return ms };
        let style = rpr.kid("rStyle").and_then(|s| s.at("val")).map(|s| self.style(s)).unwrap_or_default();
        if rpr.on("b") || style.name == "strong" {
            ms.push(Mark::new(marks::BOLD));
        }
        if rpr.on("i") || style.name == "emphasis" {
            ms.push(Mark::new(marks::ITALIC));
        }
        if rpr.on("u") {
            ms.push(Mark::new(marks::UNDERLINE));
        }
        if rpr.on("strike") || rpr.on("dstrike") {
            ms.push(Mark::new(marks::STRIKE));
        }
        match rpr.kid("vertAlign").and_then(|v| v.at("val")) {
            Some("superscript") => ms.push(Mark::new(marks::SUPERSCRIPT)),
            Some("subscript") => ms.push(Mark::new(marks::SUBSCRIPT)),
            _ => {}
        }
        let mono = rpr.kid("rFonts").and_then(|f| f.at("ascii").or(f.at("hAnsi"))).is_some_and(is_mono) || style.mono || style.name.contains("code");
        if mono {
            ms.push(Mark::new(marks::CODE));
            return ms;
        }
        if let Some(c) = rpr.kid("color").and_then(|c| c.at("val")).filter(|c| c.len() == 6 && c.chars().all(|h| h.is_ascii_hexdigit())) {
            // black and the usual hyperlink blue carry no meaning of their own
            if !matches!(c.to_ascii_uppercase().as_str(), "000000" | "0563C1" | "0000FF" | "1D4ED8") {
                ms.push(Mark::new(marks::TEXT_STYLE).attr("color", format!("#{}", c.to_ascii_lowercase())));
            }
        }
        let highlight = rpr.kid("highlight").and_then(|h| h.at("val")).and_then(highlight_hex).or_else(|| {
            rpr.kid("shd")
                .and_then(|s| s.at("fill"))
                .filter(|f| f.len() == 6 && !f.eq_ignore_ascii_case("FFFFFF") && f.chars().all(|h| h.is_ascii_hexdigit()))
                .map(|f| format!("#{}", f.to_ascii_lowercase()))
        });
        if let Some(h) = highlight {
            ms.push(Mark::new(marks::HIGHLIGHT).attr("color", h));
        }
        ms
    }

    fn image(&mut self, e: &Element) -> Option<Node> {
        let (rid, alt, width) = if let Some(blip) = e.find("blip") {
            let rid = blip.at("embed")?.to_string();
            let doc_pr = e.find("docPr");
            let alt = doc_pr.and_then(|d| d.at("descr").filter(|s| !s.is_empty()).or(d.at("title"))).unwrap_or("").to_string();
            let width = e.find("extent").and_then(|x| x.at("cx")).and_then(|v| v.parse::<u64>().ok()).map(|emu| (emu / 9525).clamp(1, 1600));
            (rid, alt, width)
        } else {
            let data = e.find("imagedata")?;
            (data.at("id")?.to_string(), data.at("title").unwrap_or("").to_string(), None)
        };
        let (target, external) = self.rels.get(&rid)?.clone();
        if external {
            return None;
        }
        let path = resolve(&self.main_dir, &target);
        let name = match self.media_by_path.get(&path) {
            Some(n) => n.clone(),
            None => {
                if self.media_total > MAX_MEDIA_TOTAL || self.image_count >= MAX_IMAGES {
                    return None;
                }
                self.image_count += 1;
                let f = self.zip.by_name(&path).ok()?;
                let mut buf = Vec::new();
                let read = f.take(MAX_IMAGE + 1).read_to_end(&mut buf).ok()? as u64;
                if read > MAX_IMAGE {
                    return None;
                }
                self.media_total += read;
                let base = path.rsplit('/').next().unwrap_or("image").to_string();
                let mut name = base.clone();
                let mut i = 1;
                while self.media_names.contains(&name) {
                    i += 1;
                    name = format!("{i}-{base}");
                }
                self.media_names.insert(name.clone());
                self.media.push((name.clone(), buf));
                self.media_by_path.insert(path, name.clone());
                name
            }
        };
        let mut n = Node::new(nodes::IMAGE).attr("src", format!("{}{name}", self.prefix)).attr("alt", alt);
        if let Some(w) = width {
            n = n.attr("width", w);
        }
        Some(n)
    }

    fn table(&mut self, t: &Element, depth: usize) -> Node {
        let grid: Vec<u64> = t.kid("tblGrid").map(|g| g.kids("gridCol").filter_map(|c| c.at("w").and_then(|w| w.parse().ok())).collect()).unwrap_or_default();
        let rows: Vec<&Element> = t.kids("tr").collect();
        // a single shaded cell is a callout (that's how WikiCraft exports them)
        if rows.len() == 1 {
            let cells: Vec<&Element> = rows[0].kids("tc").collect();
            if cells.len() == 1
                && let Some(fill) =
                    cells[0].kid("tcPr").and_then(|p| p.kid("shd")).and_then(|s| s.at("fill")).filter(|f| f.len() == 6 && !f.eq_ignore_ascii_case("FFFFFF"))
            {
                let mut content = self.blocks(cells[0], depth);
                // drop the leading icon our export draws
                if let Some(first) = content.first_mut()
                    && first.content.first().is_some_and(|n| n.kind == nodes::IMAGE && n.attr_i64("width").is_some_and(|w| w <= 24))
                {
                    first.content.remove(0);
                    if let Some(t) = first.content.first_mut().and_then(|n| n.text.as_mut()) {
                        *t = t.trim_start().to_string();
                    }
                    first.content.retain(|n| n.kind != nodes::TEXT || n.text.as_deref().is_some_and(|t| !t.is_empty()));
                }
                if content.is_empty() {
                    content.push(Node::paragraph(vec![]));
                }
                return Node::new(nodes::CALLOUT).attr("type", callout_kind(fill)).with_content(content);
            }
        }
        // cells by grid column, merging vertical spans into the starting cell's rowspan
        let mut out_rows: Vec<Vec<Node>> = Vec::new();
        let mut open: HashMap<usize, (usize, usize)> = HashMap::new(); // column → (row, cell index)
        for (ri, row) in rows.iter().enumerate() {
            let header_row = row.kid("trPr").is_some_and(|p| p.kid("tblHeader").is_some());
            let mut col = row.kid("trPr").and_then(|p| p.kid("gridBefore")).and_then(|g| g.at("val")).and_then(|v| v.parse().ok()).unwrap_or(0usize);
            let mut cells = Vec::new();
            for tc in row.kids("tc") {
                let pr = tc.kid("tcPr");
                let span: usize = pr.and_then(|p| p.kid("gridSpan")).and_then(|g| g.at("val")).and_then(|v| v.parse().ok()).unwrap_or(1).clamp(1, 63);
                let vmerge = pr.and_then(|p| p.kid("vMerge")).map(|v| v.at("val").unwrap_or("continue").to_string());
                if vmerge.as_deref() == Some("continue") {
                    if let Some((r, c)) = open.get(&col).copied()
                        && let Some(cell) = out_rows.get_mut(r).and_then(|row| row.get_mut(c))
                    {
                        let rs = cell.attr_i64("rowspan").unwrap_or(1) + 1;
                        cell.attrs.insert("rowspan".into(), rs.into());
                    }
                    col += span;
                    continue;
                }
                let mut content = self.blocks(tc, depth);
                if content.is_empty() {
                    content.push(Node::paragraph(vec![]));
                }
                let header = header_row;
                let mut cell = Node::new(if header { nodes::TABLE_HEADER } else { nodes::TABLE_CELL }).with_content(content);
                if span > 1 {
                    cell = cell.attr("colspan", span as i64);
                }
                if let Some(fill) = pr
                    .and_then(|p| p.kid("shd"))
                    .and_then(|s| s.at("fill"))
                    .filter(|f| f.len() == 6 && !f.eq_ignore_ascii_case("FFFFFF") && f.chars().all(|h| h.is_ascii_hexdigit()))
                {
                    cell = cell.attr("background", format!("#{}", fill.to_ascii_lowercase()));
                }
                if ri == 0 && !grid.is_empty() {
                    let widths: Vec<serde_json::Value> =
                        (col..col + span).map(|c| grid.get(c).map(|w| serde_json::Value::from((w / 15).max(30))).unwrap_or(serde_json::Value::Null)).collect();
                    if widths.iter().all(|w| !w.is_null()) {
                        cell = cell.attr("colwidth", widths);
                    }
                }
                if vmerge.as_deref() == Some("restart") {
                    open.insert(col, (out_rows.len(), cells.len()));
                } else {
                    open.remove(&col);
                }
                cells.push(cell);
                col += span;
            }
            out_rows.push(cells);
        }
        // a first row that is bold throughout is a header row
        if let Some(first) = out_rows.first_mut()
            && first.iter().all(|c| c.kind == nodes::TABLE_CELL)
            && rows.len() > 1
            && first.iter().all(|c| {
                let mut texts = Vec::new();
                c.walk(&mut |n| {
                    if n.kind == nodes::TEXT && n.text.as_deref().is_some_and(|t| !t.trim().is_empty()) {
                        texts.push(n.marks.iter().any(|m| m.kind == marks::BOLD));
                    }
                });
                !texts.is_empty() && texts.iter().all(|b| *b)
            })
        {
            for c in first.iter_mut() {
                c.kind = nodes::TABLE_HEADER.into();
                c.walk_mut(&mut |n| n.marks.retain(|m| m.kind != marks::BOLD));
            }
        }
        let rows: Vec<Node> = out_rows.into_iter().filter(|r| !r.is_empty()).map(|cells| Node::new(nodes::TABLE_ROW).with_content(cells)).collect();
        if rows.is_empty() {
            return Node::paragraph(vec![]);
        }
        Node::new(nodes::TABLE).with_content(rows)
    }
}

#[derive(Default)]
struct FieldState {
    in_instr: bool,
    instr: String,
    link: Option<String>,
}

/// `HYPERLINK "https://…"` / `HYPERLINK \l "anchor"` field instructions.
fn hyperlink_instr(instr: &str) -> Option<String> {
    let t = instr.trim();
    let rest = t.strip_prefix("HYPERLINK").or_else(|| t.strip_prefix("hyperlink"))?.trim();
    let anchor = rest.starts_with("\\l");
    let q = rest.find('"')?;
    let end = rest[q + 1..].find('"')?;
    let target = &rest[q + 1..q + 1 + end];
    let href = if anchor { format!("#{}", text::slugify(target)) } else { target.to_string() };
    safe_href(&href).then_some(href)
}

fn safe_href(h: &str) -> bool {
    let l = h.trim().to_ascii_lowercase();
    l.starts_with("http://") || l.starts_with("https://") || l.starts_with("mailto:") || l.starts_with("tel:") || l.starts_with('#')
}

fn resolve(base_dir: &str, target: &str) -> String {
    if let Some(abs) = target.strip_prefix('/') {
        return abs.to_string();
    }
    let mut parts: Vec<&str> = if base_dir.is_empty() { vec![] } else { base_dir.split('/').collect() };
    for seg in target.split('/') {
        match seg {
            ".." => {
                parts.pop();
            }
            "." | "" => {}
            s => parts.push(s),
        }
    }
    parts.join("/")
}

fn is_mono(font: &str) -> bool {
    let f = font.trim().to_lowercase();
    MONO_FONTS.contains(&f.as_str()) || f.ends_with(" mono") || f.contains("code")
}

/// "heading 2" (and Word's localized style ids map to these names) → 2.
fn heading_level(name: &str) -> Option<u8> {
    let rest = name.strip_prefix("heading")?.trim();
    rest.parse::<u8>().ok().filter(|l| (1..=9).contains(l)).map(|l| l.min(6))
}

fn highlight_hex(name: &str) -> Option<String> {
    let hex = match name {
        "yellow" => "#fef08a",
        "green" | "darkGreen" => "#bbf7d0",
        "cyan" | "darkCyan" => "#a5f3fc",
        "magenta" | "darkMagenta" => "#f5d0fe",
        "blue" | "darkBlue" => "#bfdbfe",
        "red" | "darkRed" => "#fecaca",
        "lightGray" | "darkGray" => "#e5e7eb",
        _ => return None,
    };
    Some(hex.into())
}

/// Our callout types by the hue of the cell shading.
fn callout_kind(fill: &str) -> &'static str {
    let v = u32::from_str_radix(fill, 16).unwrap_or(0xEFF6FF);
    let (r, g, b) = (((v >> 16) & 255) as f32 / 255.0, ((v >> 8) & 255) as f32 / 255.0, (v & 255) as f32 / 255.0);
    let max = r.max(g).max(b);
    let min = r.min(g).min(b);
    if max - min < 0.02 {
        return "info";
    }
    let d = max - min;
    let h = if max == r {
        60.0 * (((g - b) / d) % 6.0)
    } else if max == g {
        60.0 * ((b - r) / d + 2.0)
    } else {
        60.0 * ((r - g) / d + 4.0)
    };
    let h = if h < 0.0 { h + 360.0 } else { h };
    match h {
        h if !(15.0..=340.0).contains(&h) => "error",
        h if h < 75.0 => "warning",
        h if h < 170.0 => "success",
        h if h < 240.0 => "info",
        _ => "note",
    }
}

fn text_of(e: &Element) -> String {
    let mut s = String::new();
    fn go(e: &Element, s: &mut String) {
        for c in &e.children {
            if let Dom::Element(c) = c {
                match local(&c.name) {
                    "t" => s.push_str(&c.text()),
                    "tab" => s.push(' '),
                    "instrText" | "delText" => {}
                    _ => go(c, s),
                }
            }
        }
    }
    go(e, &mut s);
    s
}

fn strip_marks(mut content: Vec<Node>, kinds: &[&str]) -> Vec<Node> {
    for n in content.iter_mut() {
        n.marks.retain(|m| !kinds.contains(&m.kind.as_str()));
    }
    content
}

/// Joins adjacent text nodes with the same marks.
fn merge_text(content: &mut Vec<Node>) {
    let mut out: Vec<Node> = Vec::with_capacity(content.len());
    for n in content.drain(..) {
        if n.kind == nodes::TEXT
            && let Some(last) = out.last_mut()
            && last.kind == nodes::TEXT
            && last.marks == n.marks
        {
            last.text.get_or_insert_with(String::new).push_str(n.text.as_deref().unwrap_or(""));
            continue;
        }
        out.push(n);
    }
    *content = out;
}

/// Groups list items, tasks, code lines and quotes into their container nodes.
fn group(paras: Vec<Para>) -> Vec<Node> {
    let mut out = Vec::new();
    let mut counters: HashMap<String, i64> = HashMap::new();
    let mut it = paras.into_iter().peekable();
    while let Some(p) = it.next() {
        match p {
            Para::Block(n) => out.push(n),
            Para::Caption(c) => {
                let image = out
                    .last_mut()
                    .filter(|n| n.kind == nodes::PARAGRAPH)
                    .and_then(|n| n.content.iter_mut().rev().find(|i| i.kind == nodes::IMAGE))
                    .filter(|i| i.attr_str("caption").is_none());
                match image {
                    Some(img) => {
                        img.attrs.insert("caption".into(), c.into());
                    }
                    None => out.push(Node::paragraph(vec![Node::text_marked(c, vec![Mark::new(marks::ITALIC)])])),
                }
            }
            Para::Code(first) => {
                let mut lines = vec![first];
                while let Some(Para::Code(_)) = it.peek() {
                    if let Some(Para::Code(l)) = it.next() {
                        lines.push(l);
                    }
                }
                out.push(Node::new(nodes::CODE_BLOCK).with_content(vec![Node::text(lines.join("\n"))]));
            }
            Para::Quote(first) => {
                let mut ps = vec![first];
                while let Some(Para::Quote(_)) = it.peek() {
                    if let Some(Para::Quote(q)) = it.next() {
                        ps.push(q);
                    }
                }
                out.push(Node::new(nodes::BLOCKQUOTE).with_content(ps));
            }
            Para::Task { checked, content } => {
                let mut items = vec![(checked, content)];
                while let Some(Para::Task { .. }) = it.peek() {
                    if let Some(Para::Task { checked, content }) = it.next() {
                        items.push((checked, content));
                    }
                }
                let items =
                    items.into_iter().map(|(c, content)| Node::new(nodes::TASK_ITEM).attr("checked", c).with_content(vec![Node::paragraph(content)])).collect();
                out.push(Node::new(nodes::TASK_LIST).with_content(items));
            }
            Para::List(first) => {
                let mut items = vec![first];
                while let Some(Para::List(_)) = it.peek() {
                    if let Some(Para::List(item)) = it.next() {
                        items.push(item);
                    }
                }
                let base = items.iter().map(|i| i.level).min().unwrap_or(0);
                let mut pos = 0;
                while pos < items.len() {
                    let level = items[pos].level.max(base);
                    out.push(build_list(&items, &mut pos, level, &mut counters));
                }
            }
        }
    }
    out
}

struct NumDef {
    abs: String,
    /// level → (bullet, start)
    levels: HashMap<u32, (bool, i64)>,
    /// level → start override
    overrides: HashMap<u32, i64>,
}

struct Item {
    /// which list the item belongs to (the abstract numbering)
    key: String,
    level: u32,
    bullet: bool,
    start: i64,
    /// numbering restarts here
    restart: Option<i64>,
    content: Vec<Node>,
}

/// One list at `level` from `items[*pos..]`, with deeper items nested into the previous item.
fn build_list(items: &[Item], pos: &mut usize, level: u32, counters: &mut HashMap<String, i64>) -> Node {
    let first = &items[*pos];
    let (key, bullet) = (first.key.clone(), first.bullet);
    let ckey = format!("{key}/{level}");
    let first_number = match first.restart {
        Some(s) if first.level == level => {
            counters.insert(ckey.clone(), s - 1);
            s
        }
        _ => counters.get(&ckey).map(|c| c + 1).unwrap_or(first.start),
    };
    let mut list_items: Vec<Node> = Vec::new();
    while *pos < items.len() && items[*pos].level >= level {
        if items[*pos].level > level {
            // deeper item without a parent at this level: nest under the last (or an empty) item
            let nested = build_list(items, pos, items[*pos].level, counters);
            match list_items.last_mut() {
                Some(li) => li.content.push(nested),
                None => list_items.push(Node::new(nodes::LIST_ITEM).with_content(vec![Node::paragraph(vec![]), nested])),
            }
            continue;
        }
        let it = &items[*pos];
        if it.bullet != bullet || (!list_items.is_empty() && (it.key != key || it.restart.is_some())) {
            break;
        }
        *pos += 1;
        *counters.entry(ckey.clone()).or_insert(first_number - 1) += 1;
        list_items.push(Node::new(nodes::LIST_ITEM).with_content(vec![Node::paragraph(it.content.clone())]));
    }
    // deeper levels restart when their parent level continues
    let prefix = format!("{key}/");
    counters.retain(|k, _| !k.starts_with(&prefix) || k.rsplit('/').next().and_then(|l| l.parse::<u32>().ok()).is_some_and(|l| l <= level));
    if bullet {
        Node::new(nodes::BULLET_LIST).with_content(list_items)
    } else {
        let mut n = Node::new(nodes::ORDERED_LIST).with_content(list_items);
        if first_number != 1 {
            n = n.attr("start", first_number);
        }
        n
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn helpers() {
        assert_eq!(hyperlink_instr(" HYPERLINK \"https://example.com/x\" \\o \"tip\" ").as_deref(), Some("https://example.com/x"));
        assert_eq!(hyperlink_instr("HYPERLINK \\l \"_Toc123 Intro\"").as_deref(), Some("#toc123-intro"));
        assert_eq!(hyperlink_instr("HYPERLINK \"javascript:alert(1)\""), None);
        assert_eq!(resolve("word", "media/image1.png"), "word/media/image1.png");
        assert_eq!(resolve("word", "../customXml/x.png"), "customXml/x.png");
        assert_eq!(resolve("word", "/word/media/a.png"), "word/media/a.png");
        assert_eq!(heading_level("heading 2"), Some(2));
        assert_eq!(heading_level("heading 9"), Some(6));
        assert_eq!(callout_kind("FFFBEB"), "warning");
        assert_eq!(callout_kind("FEF2F2"), "error");
        assert_eq!(callout_kind("F0FDF4"), "success");
        assert_eq!(callout_kind("EFF6FF"), "info");
        assert_eq!(callout_kind("F5F3FF"), "note");
        assert!(is_mono("Consolas") && is_mono("Ubuntu Mono") && !is_mono("Calibri"));
    }

    fn docx(document: &str, footnotes: &str) -> Vec<u8> {
        use std::io::Write;
        let mut w = zip::ZipWriter::new(std::io::Cursor::new(Vec::new()));
        let o = zip::write::SimpleFileOptions::default();
        w.start_file("word/document.xml", o).unwrap();
        w.write_all(document.as_bytes()).unwrap();
        w.start_file("word/footnotes.xml", o).unwrap();
        w.write_all(footnotes.as_bytes()).unwrap();
        w.finish().unwrap().into_inner()
    }

    #[test]
    fn repeated_footnote_references_are_listed_once() {
        let refs = "<w:r><w:footnoteReference w:id=\"1\"/></w:r>".repeat(20_000);
        let doc = format!("<w:document xmlns:w=\"w\"><w:body><w:p><w:r><w:t>x</w:t></w:r>{refs}</w:p></w:body></w:document>");
        let big = "lorem ipsum ".repeat(50_000);
        let notes = format!("<w:footnotes xmlns:w=\"w\"><w:footnote w:id=\"1\"><w:p><w:r><w:t>{big}</w:t></w:r></w:p></w:footnote></w:footnotes>");
        let out = convert(&docx(&doc, &notes), "m/").unwrap();
        let text = wikicraft_doc_model::text::plain_text(&out.doc);
        assert!(text.len() < 200_000, "footnote text appears once and is capped: {}", text.len());
    }

    #[test]
    fn too_much_text_is_refused() {
        let run = format!("<w:r><w:t>{}</w:t></w:r>", "a".repeat(1 << 20));
        let doc = format!("<w:document xmlns:w=\"w\"><w:body>{}</w:body></w:document>", format!("<w:p>{run}</w:p>").repeat(17));
        assert!(convert(&docx(&doc, "<w:footnotes xmlns:w=\"w\"/>"), "m/").is_err());
    }

    #[test]
    fn lists_nest_and_continue() {
        let item = |num: &str, level: u32, bullet: bool, t: &str| {
            Para::List(Item { key: num.into(), level, bullet, start: 1, restart: None, content: vec![Node::text(t)] })
        };
        let paras = vec![
            item("1", 0, false, "one"),
            item("1", 1, true, "nested"),
            item("1", 0, false, "two"),
            Para::Block(Node::paragraph(vec![Node::text("break")])),
            item("1", 0, false, "three"),
        ];
        let out = group(paras);
        assert_eq!(out.len(), 3);
        assert_eq!(out[0].kind, nodes::ORDERED_LIST);
        assert_eq!(out[0].content.len(), 2);
        assert_eq!(out[0].content[0].content[1].kind, nodes::BULLET_LIST);
        assert_eq!(out[2].attr_i64("start"), Some(3), "numbering continues after the interruption");
    }
}
