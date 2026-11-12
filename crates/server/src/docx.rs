//! Word (.docx) export of a page: headings (bookmarked, so the table of contents links work),
//! lists and tasks, tables with merged cells, callouts, code, quotes, columns, images
//! (attachments embedded as PNG, SVG rasterised), links and the WikiCraft icon set rasterised
//! for emoticons, callouts, decisions and checkboxes — never emoji.

use std::collections::HashMap;
use std::io::Cursor;

use docx_rs::{
    AbstractNumbering, AlignmentType, BorderType, BreakType, Docx, Hyperlink, HyperlinkType, IndentLevel, Level, LevelJc, LevelOverride, LevelText,
    LineSpacing, NumberFormat, Numbering, NumberingId, PageMargin, Paragraph, ParagraphBorder, ParagraphBorderPosition, ParagraphBorders, Pic, Run, RunFonts,
    Shading, SpecialIndentType, Start, Style, StyleType, Table, TableBorder, TableBorderPosition, TableCell, TableCellBorder, TableCellBorderPosition,
    TableCellMargins, TableRow, VMergeType, VertAlignType, WidthType,
};
use uuid::Uuid;
use wikicraft_doc_model::html::safe_url;
use wikicraft_doc_model::{Mark, Node, icons, marks, nodes, text};

use crate::app::AppState;
use crate::authz::{Need, Principal, require_page};
use crate::routes::reports::Snapshot;

/// An image ready to embed: PNG bytes plus its natural display size in CSS pixels.
#[derive(Clone)]
pub struct DocxImage {
    pub(crate) png: Vec<u8>,
    pub(crate) w: u32,
    pub(crate) h: u32,
}

/// Images referenced by the document, keyed by their `src`.
/// Images for one export, keyed by attachment id (`?query` variants of a URL are one image).
pub type Images = HashMap<Uuid, DocxImage>;

/// What the export needs besides the document itself.
pub struct DocxInput<'a> {
    pub title: &'a str,
    /// small grey line under the title (space, last update)
    pub subtitle: Option<String>,
    pub doc: &'a Node,
    pub images: &'a Images,
    /// public base URL, for links to pages and attachments
    pub base_url: &'a str,
    /// child pages (title, absolute URL) for the children macro
    pub children: &'a [(String, String)],
    /// evaluated report macros, one entry per `macro` node in document order
    pub macros: &'a [Option<Snapshot>],
    /// the page itself: links to pages of the same export become links inside the document
    pub page_id: Option<Uuid>,
}

/// per export (a page tree shares them): distinct images loaded, their total size, and how
/// many times images may be embedded (each embedding carries its own copy)
const MAX_IMAGES: usize = 100;
const MAX_IMAGE_BYTES: i64 = 25 << 20;
const MAX_LOADED_BYTES: usize = 150 << 20;
const MAX_EMBEDS: usize = 400;
const MAX_EMBEDDED_BYTES: usize = 300 << 20;
/// A4 with 2.54 cm margins: 9026 twips = 6.27 in ≈ 600 CSS px
const TEXT_WIDTH_DXA: i32 = 9026;
const CONTENT_WIDTH_PX: u32 = 600;
const EMU_PER_PX: u32 = 9525;
const ICON_PX: u32 = 15;
const MONO: &str = "Consolas";
const LINK_COLOR: &str = "1D4ED8";
const MUTED: &str = "64748B";

pub fn attachment_id(src: &str) -> Option<Uuid> {
    src.strip_prefix("/api/attachments/")?.split(['/', '?']).next()?.parse().ok()
}

/// Loads the attachment images of `doc` that the principal may see (an attachment of another
/// page only when that page is visible to them). External images are never fetched; they
/// become links in the document.
pub async fn load_images(state: &AppState, p: &Principal, doc: &Node, out: &mut Images) {
    let mut ids: Vec<Uuid> = Vec::new();
    doc.walk(&mut |n| {
        let url = match n.kind.as_str() {
            nodes::IMAGE => n.attr_str("src"),
            nodes::DRAWING => n.attr_str("preview"),
            _ => None,
        };
        if let Some(id) = url.and_then(attachment_id)
            && !ids.contains(&id)
        {
            ids.push(id);
        }
    });
    let mut loaded: usize = out.values().map(|i| i.png.len()).sum();
    let mut allowed: HashMap<Uuid, bool> = HashMap::new();
    for id in ids {
        if out.contains_key(&id) {
            continue;
        }
        if out.len() >= MAX_IMAGES || loaded >= MAX_LOADED_BYTES {
            break;
        }
        let row: Option<(Uuid, Option<String>, i64, String)> =
            sqlx::query_as("SELECT page_id, blob_key, size, media_type FROM attachments WHERE id = $1 AND deleted_at IS NULL")
                .bind(id)
                .fetch_optional(&state.db)
                .await
                .ok()
                .flatten();
        let Some((page_id, Some(key), size, mime)) = row else { continue };
        if size > MAX_IMAGE_BYTES {
            continue;
        }
        let ok = match allowed.get(&page_id) {
            Some(v) => *v,
            None => {
                let v = require_page(state, p, page_id, Need::View).await.is_ok();
                allowed.insert(page_id, v);
                v
            }
        };
        if !ok {
            continue;
        }
        let Ok(bytes) = state.blobs.get(&key).await else { continue };
        let svg = mime.contains("svg");
        let prepared = tokio::task::spawn_blocking(move || if svg { rasterize_svg(&bytes, None) } else { prepare_image(&bytes) }).await;
        if let Ok(Some(img)) = prepared {
            loaded += img.png.len();
            out.insert(id, img);
        }
    }
}

/// Decodes an uploaded raster image for embedding: PNG as is, everything else re-encoded
/// (docx-rs embeds PNG only); large images are scaled down. `None` if it can't be decoded.
pub fn prepare_image(bytes: &[u8]) -> Option<DocxImage> {
    let reader = image::ImageReader::new(Cursor::new(bytes)).with_guessed_format().ok()?;
    if reader.format() == Some(image::ImageFormat::Png) {
        let (w, h) = reader.into_dimensions().ok()?;
        return (w > 0 && h > 0).then(|| DocxImage { png: bytes.to_vec(), w, h });
    }
    let mut reader = reader;
    let mut limits = image::Limits::default();
    limits.max_image_width = Some(12_000);
    limits.max_image_height = Some(12_000);
    limits.max_alloc = Some(512 << 20);
    reader.limits(limits);
    let img = reader.decode().ok()?;
    let img = if img.width() > 1600 { img.resize(1600, u32::MAX, image::imageops::FilterType::Triangle) } else { img };
    let mut buf = Cursor::new(Vec::new());
    img.write_to(&mut buf, image::ImageFormat::Png).ok()?;
    let (w, h) = (img.width(), img.height());
    (w > 0 && h > 0).then(|| DocxImage { png: buf.into_inner(), w, h })
}

fn svg_options() -> resvg::usvg::Options<'static> {
    // never resolve `<image href>` (the default reads local files)
    resvg::usvg::Options {
        image_href_resolver: resvg::usvg::ImageHrefResolver { resolve_data: Box::new(|_, _, _| None), resolve_string: Box::new(|_, _| None) },
        ..Default::default()
    }
}

/// Rasterises an SVG at 2x (or to `display_px` square at 4x for icons).
pub fn rasterize_svg(data: &[u8], display_px: Option<u32>) -> Option<DocxImage> {
    let tree = resvg::usvg::Tree::from_data(data, &svg_options()).ok()?;
    let size = tree.size();
    let (sw, sh) = (size.width(), size.height());
    if !(sw > 0.0 && sh > 0.0) {
        return None;
    }
    let (dw, dh, scale) = match display_px {
        Some(px) => (px, px, px as f32 * 4.0 / sw.max(sh)),
        None => {
            let dw = (sw.round() as u32).clamp(1, 1600);
            let dh = ((sh * dw as f32 / sw).round() as u32).max(1);
            (dw, dh, (2.0 * dw as f32 / sw).min(2400.0 / sw.max(sh)))
        }
    };
    let (pw, ph) = (((sw * scale).ceil() as u32).max(1), ((sh * scale).ceil() as u32).max(1));
    let mut pixmap = resvg::tiny_skia::Pixmap::new(pw, ph)?;
    resvg::render(&tree, resvg::tiny_skia::Transform::from_scale(scale, scale), &mut pixmap.as_mut());
    Some(DocxImage { png: pixmap.encode_png().ok()?, w: dw, h: dh })
}

pub(crate) fn line_icon_svg(group: &str, name: &str, color: &str) -> Option<String> {
    let inner = icons::inner_markup(group, name)?.replace("currentColor", color);
    Some(format!(
        "<svg xmlns=\"http://www.w3.org/2000/svg\" viewBox=\"0 0 24 24\" width=\"24\" height=\"24\" fill=\"none\" stroke=\"{color}\" stroke-width=\"1.75\" stroke-linecap=\"round\" stroke-linejoin=\"round\">{inner}</svg>"
    ))
}

pub(crate) fn checkbox_svg(checked: bool) -> String {
    if checked {
        "<svg xmlns=\"http://www.w3.org/2000/svg\" viewBox=\"0 0 24 24\" width=\"24\" height=\"24\"><rect x=\"3\" y=\"3\" width=\"18\" height=\"18\" rx=\"4\" fill=\"#2563EB\"/><path d=\"M7.5 12.5l3 3 6-6.5\" fill=\"none\" stroke=\"#fff\" stroke-width=\"2.2\" stroke-linecap=\"round\" stroke-linejoin=\"round\"/></svg>".into()
    } else {
        "<svg xmlns=\"http://www.w3.org/2000/svg\" viewBox=\"0 0 24 24\" width=\"24\" height=\"24\"><rect x=\"3.75\" y=\"3.75\" width=\"16.5\" height=\"16.5\" rx=\"3.5\" fill=\"#fff\" stroke=\"#64748B\" stroke-width=\"1.5\"/></svg>".into()
    }
}

/// `#rgb`, `#rrggbb(aa)`, `rgb(r, g, b)` and a few colour names as Word's `RRGGBB`.
pub(crate) fn hex_color(c: &str) -> Option<String> {
    let c = c.trim().to_ascii_lowercase();
    if let Some(h) = c.strip_prefix('#') {
        if !h.chars().all(|ch| ch.is_ascii_hexdigit()) {
            return None;
        }
        return match h.len() {
            3 | 4 => Some(h.chars().take(3).flat_map(|ch| [ch, ch]).collect::<String>().to_uppercase()),
            6 | 8 => Some(h[..6].to_uppercase()),
            _ => None,
        };
    }
    if let Some(args) = c.strip_prefix("rgba(").or_else(|| c.strip_prefix("rgb(")).and_then(|r| r.strip_suffix(')')) {
        let v: Vec<u8> = args.split(',').take(3).filter_map(|x| x.trim().parse::<f32>().ok()).map(|x| x.clamp(0.0, 255.0) as u8).collect();
        return (v.len() == 3).then(|| format!("{:02X}{:02X}{:02X}", v[0], v[1], v[2]));
    }
    let named = match c.as_str() {
        "black" => "000000",
        "white" => "FFFFFF",
        "red" => "DC2626",
        "green" => "16A34A",
        "blue" => "2563EB",
        "yellow" => "FACC15",
        "orange" => "EA580C",
        "purple" => "7C3AED",
        "gray" | "grey" => "6B7280",
        _ => return None,
    };
    Some(named.into())
}

fn spacing(before: u32, after: u32) -> LineSpacing {
    LineSpacing::new().before(before).after(after)
}

fn mono() -> RunFonts {
    RunFonts::new().ascii(MONO).hi_ansi(MONO).cs(MONO)
}

fn pic(img: &DocxImage, w: u32, h: u32) -> Pic {
    Pic::new_with_dimensions(img.png.clone(), w, h).size(w * EMU_PER_PX, h * EMU_PER_PX)
}

enum Block {
    P(Box<Paragraph>),
    T(Box<Table>),
}

/// How the blocks being rendered are nested.
#[derive(Clone, Copy, Default)]
struct Ctx {
    /// left indent (twips) for paragraphs that aren't list items themselves
    indent: i32,
    /// list nesting depth
    depth: usize,
    /// width available (twips), for tables and columns
    width: i32,
    /// runs are bold (table headers, details summaries)
    bold: bool,
}

struct Renderer<'a> {
    input: &'a DocxInput<'a>,
    /// section index (one per page in a page-tree export) and the bookmark of every page
    section: usize,
    pages: &'a HashMap<Uuid, String>,
    /// heading anchor id → bookmark name, in document order
    headings: Vec<(u8, String, String)>,
    heading_idx: usize,
    next_bookmark: usize,
    anchors: HashMap<String, String>,
    numberings: Vec<Numbering>,
    icons: HashMap<String, Option<DocxImage>>,
    macro_idx: usize,
    /// images embedded so far in this export: (count, bytes, icons)
    embeds: (usize, usize, usize),
}

// docx-rs always writes its own decimal list as abstract/num 1, so ours start above it
const BULLET_NUM: usize = 2;
const BULLET_ABS: usize = 10;
const ORDERED_ABS: usize = 11;

pub fn render(input: &DocxInput) -> anyhow::Result<Vec<u8>> {
    render_many(std::slice::from_ref(input))
}

/// Several pages in one document (a page and its descendants), each starting on a new page.
pub fn render_many(inputs: &[DocxInput]) -> anyhow::Result<Vec<u8>> {
    let pages: HashMap<Uuid, String> = inputs.iter().enumerate().filter_map(|(i, d)| d.page_id.map(|id| (id, format!("p{i}")))).collect();
    let mut blocks: Vec<Block> = Vec::new();
    let mut numberings = vec![Numbering::new(BULLET_NUM, BULLET_ABS)];
    let mut icons: HashMap<String, Option<DocxImage>> = HashMap::new();
    let mut next_bookmark = 1;
    let mut embeds = (0usize, 0usize, 0usize);
    for (section, input) in inputs.iter().enumerate() {
        let headings = text::headings(input.doc);
        let prefix = if section == 0 { String::new() } else { format!("s{section}") };
        let anchors: HashMap<String, String> = headings.iter().enumerate().map(|(i, h)| (h.2.clone(), format!("{prefix}h{i}"))).collect();
        let mut r = Renderer {
            input,
            section,
            pages: &pages,
            headings,
            heading_idx: 0,
            next_bookmark,
            anchors,
            numberings: std::mem::take(&mut numberings),
            icons: std::mem::take(&mut icons),
            macro_idx: 0,
            embeds,
        };
        let ctx = Ctx { width: TEXT_WIDTH_DXA, ..Default::default() };
        let id = r.bookmark();
        let mut title = Paragraph::new().style("Title").keep_next(true);
        if section > 0 {
            title = title.page_break_before(true);
        }
        let title = title.add_bookmark_start(id, format!("p{}", r.section)).add_run(Run::new().add_text(input.title)).add_bookmark_end(id);
        blocks.push(Block::P(Box::new(title)));
        if let Some(sub) = &input.subtitle {
            blocks.push(Block::P(Box::new(Paragraph::new().line_spacing(spacing(0, 240)).add_run(Run::new().add_text(sub).size(18).color(MUTED)))));
        }
        blocks.extend(r.blocks(&input.doc.content, ctx));
        numberings = std::mem::take(&mut r.numberings);
        icons = std::mem::take(&mut r.icons);
        next_bookmark = r.next_bookmark;
        embeds = r.embeds;
    }

    let mut docx = Docx::new()
        .page_size(11906, 16838)
        .page_margin(PageMargin::new().top(1440).bottom(1440).left(1440).right(1440))
        .default_fonts(RunFonts::new().ascii("Calibri").hi_ansi("Calibri").cs("Calibri").east_asia("Calibri"))
        .default_size(22)
        .add_abstract_numbering(bullets())
        .add_abstract_numbering(ordered());
    for s in styles() {
        docx = docx.add_style(s);
    }
    for n in numberings {
        docx = docx.add_numbering(n);
    }
    for b in blocks {
        docx = match b {
            Block::P(p) => docx.add_paragraph(*p),
            Block::T(t) => docx.add_table(*t),
        };
    }
    let mut buf = Cursor::new(Vec::new());
    docx.build().pack(&mut buf)?;
    Ok(buf.into_inner())
}

fn bullets() -> AbstractNumbering {
    let mut a = AbstractNumbering::new(BULLET_ABS);
    for l in 0..9 {
        let sym = ["\u{2022}", "\u{25E6}", "\u{2023}"][l % 3];
        a = a.add_level(Level::new(l, Start::new(1), NumberFormat::new("bullet"), LevelText::new(sym), LevelJc::new("left")).indent(
            Some(360 * (l as i32 + 1) + 360),
            Some(SpecialIndentType::Hanging(360)),
            None,
            None,
        ));
    }
    a
}

fn ordered() -> AbstractNumbering {
    let mut a = AbstractNumbering::new(ORDERED_ABS);
    for l in 0..9 {
        let fmt = ["decimal", "lowerLetter", "lowerRoman"][l % 3];
        a = a.add_level(Level::new(l, Start::new(1), NumberFormat::new(fmt), LevelText::new(format!("%{}.", l + 1)), LevelJc::new("left")).indent(
            Some(360 * (l as i32 + 1) + 360),
            Some(SpecialIndentType::Hanging(360)),
            None,
            None,
        ));
    }
    a
}

fn styles() -> Vec<Style> {
    let heading = |lvl: usize, size: usize, color: &str| {
        Style::new(format!("Heading{lvl}"), StyleType::Paragraph)
            .name(format!("heading {lvl}"))
            .based_on("Normal")
            .next("Normal")
            .size(size)
            .bold()
            .color(color)
            .outline_lvl(lvl - 1)
            .line_spacing(spacing(if lvl <= 2 { 360 } else { 240 }, 120))
    };
    vec![
        Style::new("Normal", StyleType::Paragraph).name("Normal").line_spacing(spacing(0, 120).line(276)),
        Style::new("Title", StyleType::Paragraph).name("Title").based_on("Normal").next("Normal").size(52).bold().color("0F172A").line_spacing(spacing(0, 80)),
        heading(1, 36, "0F172A"),
        heading(2, 30, "0F172A"),
        heading(3, 26, "1E293B"),
        heading(4, 24, "1E293B"),
        heading(5, 22, "334155"),
        heading(6, 22, "475569"),
        Style::new("Caption", StyleType::Paragraph).name("caption").based_on("Normal").size(18).italic().color(MUTED),
    ]
}

impl Renderer<'_> {
    fn bookmark(&mut self) -> usize {
        self.next_bookmark += 1;
        self.next_bookmark
    }

    fn icon(&mut self, key: &str, svg: impl FnOnce() -> Option<String>) -> Option<DocxImage> {
        if let Some(v) = self.icons.get(key) {
            return v.clone();
        }
        let img = svg().and_then(|s| rasterize_svg(s.as_bytes(), Some(ICON_PX)));
        self.icons.insert(key.to_string(), img.clone());
        img
    }

    fn icon_run(&mut self, key: &str, svg: impl FnOnce() -> Option<String>) -> Option<Run> {
        // icons are small, but each one is embedded: bounded too (text fallbacks after that)
        if self.embeds.2 >= 3_000 {
            return None;
        }
        self.embeds.2 += 1;
        self.icon(key, svg).map(|img| Run::new().add_image(pic(&img, ICON_PX, ICON_PX)))
    }

    /// The bookmark of a page included in this export that `href` points to.
    fn page_bookmark(&self, href: &str) -> Option<String> {
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

    fn blocks(&mut self, content: &[Node], ctx: Ctx) -> Vec<Block> {
        let mut out = Vec::new();
        for n in content {
            self.block(n, ctx, &mut out);
        }
        out
    }

    /// The context's indentation plus the block's own levels (half an inch each).
    fn own_indent(ctx: Ctx, n: &Node) -> Ctx {
        Ctx { indent: ctx.indent + 720 * wikicraft_doc_model::node::indent_level(n) as i32, ..ctx }
    }

    fn indented(p: Paragraph, ctx: Ctx) -> Paragraph {
        if ctx.indent > 0 { p.indent(Some(ctx.indent), None, None, None) } else { p }
    }

    fn block(&mut self, n: &Node, ctx: Ctx, out: &mut Vec<Block>) {
        match n.kind.as_str() {
            nodes::PARAGRAPH => {
                let p = Self::indented(self.aligned(Paragraph::new(), n), Self::own_indent(ctx, n));
                self.paragraph(n, p, ctx, out);
            }
            nodes::HEADING => {
                let lvl = n.attr_i64("level").unwrap_or(1).clamp(1, 6);
                let name = self.headings.get(self.heading_idx).and_then(|h| self.anchors.get(&h.2)).cloned();
                self.heading_idx += 1;
                let mut p = Self::indented(self.aligned(Paragraph::new().style(&format!("Heading{lvl}")).keep_next(true), n), Self::own_indent(ctx, n));
                let bm = name.map(|name| (self.bookmark(), name));
                if let Some((id, name)) = &bm {
                    p = p.add_bookmark_start(*id, name);
                }
                let mut captions = Vec::new();
                p = self.inlines(&n.content, p, ctx, &mut captions);
                if let Some((id, _)) = bm {
                    p = p.add_bookmark_end(id);
                }
                out.push(Block::P(Box::new(p)));
            }
            nodes::BULLET_LIST | nodes::ORDERED_LIST => {
                let num = if n.kind == nodes::BULLET_LIST {
                    BULLET_NUM
                } else {
                    // every ordered list restarts its numbering (at `start`)
                    let id = BULLET_NUM + self.numberings.len();
                    let start = n.attr_i64("start").unwrap_or(1).clamp(0, 32_767) as usize;
                    self.numberings.push(Numbering::new(id, ORDERED_ABS).add_override(LevelOverride::new(ctx.depth.min(8)).start(start)));
                    id
                };
                let level = ctx.depth.min(8);
                let item_indent = 360 * (level as i32 + 1) + 360;
                for item in n.content.iter() {
                    let inner = Ctx { indent: item_indent, depth: ctx.depth + 1, ..ctx };
                    let mut first = true;
                    for child in &item.content {
                        if first && child.kind == nodes::PARAGRAPH {
                            first = false;
                            let p = Paragraph::new().numbering(NumberingId::new(num), IndentLevel::new(level)).line_spacing(spacing(0, 60));
                            self.paragraph(child, p, inner, out);
                        } else {
                            first = false;
                            self.block(child, inner, out);
                        }
                    }
                }
            }
            nodes::TASK_LIST => {
                for item in &n.content {
                    let checked = item.attr_bool("checked").unwrap_or(false);
                    let left = ctx.indent + 120;
                    let inner = Ctx { indent: left + 360, depth: ctx.depth + 1, ..ctx };
                    let mut first = true;
                    for child in &item.content {
                        if first && child.kind == nodes::PARAGRAPH {
                            first = false;
                            let mut p =
                                Paragraph::new().indent(Some(left + 360), Some(SpecialIndentType::Hanging(360)), None, None).line_spacing(spacing(0, 60));
                            if let Some(run) = self.icon_run(if checked { "task:on" } else { "task:off" }, || Some(checkbox_svg(checked))) {
                                p = p.add_run(run).add_run(Run::new().add_tab());
                            }
                            let mut captions = Vec::new();
                            let p = self.inlines(&child.content, p, Ctx { bold: false, ..inner }, &mut captions);
                            let p = if checked { strike_all(p) } else { p };
                            out.push(Block::P(Box::new(p)));
                            self.captions(captions, inner, out);
                        } else {
                            first = false;
                            self.block(child, inner, out);
                        }
                    }
                }
            }
            nodes::DECISION_LIST => {
                for item in &n.content {
                    let mut p = Self::indented(Paragraph::new(), ctx);
                    if let Some(run) = self.icon_run("decision", || line_icon_svg("symbols", "decision", "#16A34A")) {
                        p = p.add_run(run).add_run(Run::new().add_text(" "));
                    }
                    self.paragraph(item, p, ctx, out);
                }
            }
            nodes::CODE_BLOCK => {
                let code = text::inline_text(n);
                let lines: Vec<&str> = code.trim_end_matches('\n').split('\n').collect();
                let border = |pos| ParagraphBorder::new(pos).val(BorderType::Single).size(4).space(4).color("E2E8F0");
                let borders = ParagraphBorders::with_empty()
                    .set(border(ParagraphBorderPosition::Top))
                    .set(border(ParagraphBorderPosition::Bottom))
                    .set(border(ParagraphBorderPosition::Left))
                    .set(border(ParagraphBorderPosition::Right));
                let last = lines.len().saturating_sub(1);
                for (i, line) in lines.iter().enumerate() {
                    let mut p = Paragraph::new().line_spacing(spacing(if i == 0 { 120 } else { 0 }, if i == last { 160 } else { 0 }).line(240));
                    p.property = p.property.shading(Shading::new().fill("F6F8FA")).set_borders(borders.clone());
                    let p = Self::indented(p, Ctx { indent: ctx.indent + 80, ..ctx });
                    let text = line.replace('\t', "    ");
                    out.push(Block::P(Box::new(p.add_run(Run::new().add_text(text).fonts(mono()).size(19).color("0F172A")))));
                }
            }
            nodes::BLOCKQUOTE => {
                let inner = Ctx { indent: ctx.indent + 240, ..ctx };
                for b in self.blocks(&n.content, inner) {
                    out.push(match b {
                        Block::P(mut p) => {
                            p.property = p.property.clone().set_borders(
                                ParagraphBorders::with_empty()
                                    .set(ParagraphBorder::new(ParagraphBorderPosition::Left).val(BorderType::Single).size(18).space(8).color("CBD5E1")),
                            );
                            Block::P(Box::new(p.color("475569")))
                        }
                        t => t,
                    });
                }
            }
            nodes::HORIZONTAL_RULE => {
                let mut p = Paragraph::new().line_spacing(spacing(0, 240));
                p.property = p.property.set_borders(
                    ParagraphBorders::with_empty()
                        .set(ParagraphBorder::new(ParagraphBorderPosition::Bottom).val(BorderType::Single).size(6).space(1).color("CBD5E1")),
                );
                out.push(Block::P(Box::new(p)));
            }
            nodes::TABLE => {
                let t = self.table(n, ctx);
                out.push(Block::T(Box::new(t)));
                out.push(Block::P(Box::new(Paragraph::new().line_spacing(spacing(0, 0)))));
            }
            nodes::CALLOUT => {
                let kind = n.attr_str("type").filter(|k| matches!(*k, "info" | "note" | "success" | "warning" | "error")).unwrap_or("info");
                let (fill, edge) = match kind {
                    "warning" => ("FFFBEB", "F59E0B"),
                    "error" => ("FEF2F2", "EF4444"),
                    "success" => ("F0FDF4", "22C55E"),
                    "note" => ("F5F3FF", "8B5CF6"),
                    _ => ("EFF6FF", "3B82F6"),
                };
                let width = ctx.width - ctx.indent;
                let inner = Ctx { indent: 0, depth: 0, width: width - 400, bold: false };
                let mut blocks = self.blocks(&n.content, inner);
                let custom = n.attr_str("icon").and_then(icons::parse_symbol);
                let icon_key = custom.map(|(name, colour)| format!("symbol:{name}:{colour}")).unwrap_or_else(|| format!("callout:{kind}"));
                let draw = || match custom {
                    Some((name, colour)) => line_icon_svg("symbols", name, colour),
                    None => line_icon_svg("callout", kind, &format!("#{edge}")),
                };
                if let Some(run) = self.icon_run(&icon_key, draw) {
                    let icon = run;
                    match blocks.first_mut() {
                        Some(Block::P(p)) => {
                            let mut children = std::mem::take(&mut p.children);
                            let mut np = Paragraph::new().add_run(icon).add_run(Run::new().add_text(" "));
                            np.children.append(&mut children);
                            p.children = np.children;
                        }
                        _ => blocks.insert(0, Block::P(Box::new(Paragraph::new().add_run(icon)))),
                    }
                }
                let cell = fill_cell(TableCell::new(), blocks)
                    .shading(Shading::new().fill(fill))
                    .width(width as usize, WidthType::Dxa)
                    .set_border(TableCellBorder::new(TableCellBorderPosition::Left).border_type(BorderType::Single).size(24).color(edge))
                    .set_border(TableCellBorder::new(TableCellBorderPosition::Top).border_type(BorderType::Nil))
                    .set_border(TableCellBorder::new(TableCellBorderPosition::Bottom).border_type(BorderType::Nil))
                    .set_border(TableCellBorder::new(TableCellBorderPosition::Right).border_type(BorderType::Nil));
                let t = Table::without_borders(vec![TableRow::new(vec![cell])])
                    .set_grid(vec![width as usize])
                    .width(width as usize, WidthType::Dxa)
                    .indent(ctx.indent)
                    .margins(TableCellMargins::new().margin(100, 160, 100, 200));
                out.push(Block::T(Box::new(t)));
                out.push(Block::P(Box::new(Paragraph::new().line_spacing(spacing(0, 0)))));
            }
            nodes::DETAILS => {
                for c in &n.content {
                    match c.kind.as_str() {
                        nodes::DETAILS_SUMMARY => {
                            let p = Self::indented(Paragraph::new().keep_next(true), ctx);
                            self.paragraph(c, p, Ctx { bold: true, ..ctx }, out);
                        }
                        nodes::DETAILS_CONTENT => {
                            let blocks = self.blocks(&c.content, Ctx { indent: ctx.indent + 240, ..ctx });
                            out.extend(blocks);
                        }
                        _ => self.block(c, ctx, out),
                    }
                }
            }
            nodes::TOC => {
                let entries = wikicraft_doc_model::toc::entries(self.input.doc, n);
                let label = |e: &wikicraft_doc_model::toc::TocEntry| if e.number.is_empty() { e.text.clone() } else { format!("{} {}", e.number, e.text) };
                if wikicraft_doc_model::toc::is_flat(n) {
                    // one paragraph, entries separated by a middle dot
                    let mut p = Self::indented(Paragraph::new(), ctx).line_spacing(spacing(0, 80));
                    for (i, e) in entries.iter().enumerate() {
                        let Some(bm) = self.anchors.get(&e.id).cloned() else { continue };
                        if i > 0 {
                            p = p.add_run(Run::new().add_text(" · ").color(MUTED));
                        }
                        p = p.add_hyperlink(Hyperlink::new(bm, HyperlinkType::Anchor).add_run(Run::new().add_text(label(e)).color(LINK_COLOR)));
                    }
                    out.push(Block::P(Box::new(p)));
                } else {
                    for e in &entries {
                        let Some(bm) = self.anchors.get(&e.id).cloned() else { continue };
                        let p = Paragraph::new().indent(Some(ctx.indent + 240 * i32::from(e.depth)), None, None, None).line_spacing(spacing(0, 40));
                        let link = Hyperlink::new(bm, HyperlinkType::Anchor).add_run(Run::new().add_text(label(e)).color(LINK_COLOR));
                        out.push(Block::P(Box::new(p.add_hyperlink(link))));
                    }
                }
            }
            nodes::CHILD_PAGES => {
                let children = self.input.children;
                if children.is_empty() {
                    out.push(Block::P(Box::new(Self::indented(Paragraph::new(), ctx).add_run(Run::new().add_text("No child pages.").italic().color(MUTED)))));
                }
                for (title, url) in children {
                    let p = Paragraph::new().numbering(NumberingId::new(BULLET_NUM), IndentLevel::new(ctx.depth.min(8))).line_spacing(spacing(0, 60));
                    let link = match self.page_bookmark(url) {
                        Some(bm) => Hyperlink::new(bm, HyperlinkType::Anchor),
                        None => Hyperlink::new(url, HyperlinkType::External),
                    };
                    out.push(Block::P(Box::new(p.add_hyperlink(link.add_run(Run::new().add_text(title).color(LINK_COLOR).underline("single"))))));
                }
            }
            nodes::INCLUDE => {
                let title = n.attr_str("title").unwrap_or("page").to_string();
                let mut p = Self::indented(Paragraph::new(), ctx).add_run(Run::new().add_text("Included page: ").italic().color(MUTED));
                let run = Run::new().add_text(&title).italic().color(LINK_COLOR);
                p = match n.attr_str("pageId").and_then(|id| id.parse::<Uuid>().ok()) {
                    Some(id) => p.add_hyperlink(Hyperlink::new(format!("{}/pages/{id}", self.input.base_url), HyperlinkType::External).add_run(run)),
                    None => p.add_run(run),
                };
                out.push(Block::P(Box::new(p)));
            }
            nodes::ATTACHMENT => {
                let name = n.attr_str("name").unwrap_or("attachment").to_string();
                let mut p = Self::indented(Paragraph::new(), ctx);
                if let Some(run) = self.icon_run("paperclip", || line_icon_svg("ui", "paperclip", "#475569")) {
                    p = p.add_run(run).add_run(Run::new().add_text(" "));
                }
                let run = Run::new().add_text(&name).color(LINK_COLOR).underline("single");
                p = match n.attr_str("href").and_then(|h| self.url(h)) {
                    Some(url) => p.add_hyperlink(Hyperlink::new(url, HyperlinkType::External).add_run(run)),
                    None => p.add_run(run),
                };
                out.push(Block::P(Box::new(p)));
            }
            nodes::EMBED => {
                let p = Self::indented(Paragraph::new(), ctx);
                let p = match n.attr_str("url").and_then(|u| self.url(u)) {
                    Some(url) => {
                        p.add_hyperlink(Hyperlink::new(&url, HyperlinkType::External).add_run(Run::new().add_text(&url).color(LINK_COLOR).underline("single")))
                    }
                    None => p,
                };
                out.push(Block::P(Box::new(p)));
            }
            nodes::COLUMNS => {
                let cols: Vec<&Node> = n.content.iter().filter(|c| c.kind == nodes::COLUMN).collect();
                if cols.is_empty() {
                    return;
                }
                let width = ctx.width - ctx.indent;
                let w = width / cols.len() as i32;
                let cells = cols
                    .iter()
                    .map(|c| {
                        let blocks = self.blocks(&c.content, Ctx { indent: 0, depth: 0, width: w - 200, bold: false });
                        fill_cell(TableCell::new(), blocks).width(w as usize, WidthType::Dxa)
                    })
                    .collect();
                let t = Table::without_borders(vec![TableRow::new(cells)])
                    .set_grid(vec![w as usize; cols.len()])
                    .width(width as usize, WidthType::Dxa)
                    .indent(ctx.indent);
                out.push(Block::T(Box::new(t)));
            }
            nodes::DRAWING => {
                // drawn as its preview image, centred
                if let Some(src) = n.attr_str("preview") {
                    let img = Node::new(nodes::IMAGE).attr("src", src).attr("alt", n.attr_str("title").unwrap_or("Drawing")).attr("align", "center");
                    let wrapper = Node::paragraph(vec![img]);
                    let p = Self::indented(Paragraph::new(), ctx);
                    self.paragraph(&wrapper, p, ctx, out);
                }
            }
            nodes::MATH_BLOCK => {
                let latex = n.attr_str("latex").unwrap_or("");
                let fonts = RunFonts::new().ascii("Cambria Math").hi_ansi("Cambria Math");
                out.push(Block::P(Box::new(Paragraph::new().align(AlignmentType::Center).add_run(Run::new().add_text(latex).fonts(fonts).italic()))));
            }
            nodes::MACRO => {
                let idx = self.macro_idx;
                self.macro_idx += 1;
                let name = n.attr_str("name").unwrap_or("macro").to_string();
                match self.input.macros.get(idx).and_then(|m| m.as_ref()) {
                    Some(snap) => self.snapshot(snap, ctx, out),
                    None => {
                        let label = Run::new().add_text(format!("Macro: {name}")).size(18).italic().color(MUTED);
                        out.push(Block::P(Box::new(Self::indented(Paragraph::new(), ctx).add_run(label))));
                    }
                }
                let blocks = self.blocks(&n.content, ctx);
                out.extend(blocks);
            }
            _ => {
                if !n.content.is_empty() && n.content.iter().all(|c| c.is_inline()) {
                    let p = Self::indented(Paragraph::new(), ctx);
                    self.paragraph(n, p, ctx, out);
                } else if n.is_inline() {
                    let p = Self::indented(Paragraph::new(), ctx);
                    let wrapper = Node { content: vec![n.clone()], ..Node::new(nodes::PARAGRAPH) };
                    self.paragraph(&wrapper, p, ctx, out);
                } else {
                    let blocks = self.blocks(&n.content, ctx);
                    out.extend(blocks);
                }
            }
        }
    }

    /// A report macro's result: a small heading line and a table.
    fn snapshot(&mut self, snap: &Snapshot, ctx: Ctx, out: &mut Vec<Block>) {
        let label = Run::new().add_text(&snap.title).size(18).bold().color(MUTED);
        out.push(Block::P(Box::new(Self::indented(Paragraph::new().keep_next(true).line_spacing(spacing(120, 60)), ctx).add_run(label))));
        if snap.rows.is_empty() {
            out.push(Block::P(Box::new(Self::indented(Paragraph::new(), ctx).add_run(Run::new().add_text(snap.empty).italic().color(MUTED)))));
            return;
        }
        let cols = snap.header.len().max(1);
        let width = (ctx.width - ctx.indent).max(1440) as usize;
        let w = width / cols;
        let cell = |text: &str, link: Option<&String>, header: bool| {
            let mut run = Run::new().add_text(text).size(20);
            if header {
                run = run.bold();
            }
            let p = Paragraph::new().line_spacing(spacing(0, 0));
            let p = match link {
                Some(url) => p.add_hyperlink(Hyperlink::new(url, HyperlinkType::External).add_run(run.color(LINK_COLOR))),
                None => p.add_run(run),
            };
            let c = TableCell::new().add_paragraph(p).width(w, WidthType::Dxa);
            if header { c.shading(Shading::new().fill("F1F5F9")) } else { c }
        };
        let mut rows = vec![TableRow::new(snap.header.iter().map(|h| cell(h, None, true)).collect())];
        for r in &snap.rows {
            let mut cells: Vec<TableCell> = r.iter().take(cols).map(|(t, l)| cell(t, l.as_ref(), false)).collect();
            while cells.len() < cols {
                cells.push(cell("", None, false));
            }
            rows.push(TableRow::new(cells));
        }
        let border = |pos| TableBorder::new(pos).border_type(BorderType::Single).size(4).color("CBD5E1");
        let t = Table::new(rows)
            .set_grid(vec![w; cols])
            .width(width, WidthType::Dxa)
            .indent(ctx.indent)
            .margins(TableCellMargins::new().margin(40, 100, 40, 100))
            .set_border(border(TableBorderPosition::Top))
            .set_border(border(TableBorderPosition::Bottom))
            .set_border(border(TableBorderPosition::Left))
            .set_border(border(TableBorderPosition::Right))
            .set_border(border(TableBorderPosition::InsideH))
            .set_border(border(TableBorderPosition::InsideV));
        out.push(Block::T(Box::new(t)));
        out.push(Block::P(Box::new(Paragraph::new().line_spacing(spacing(0, 0)))));
    }

    fn aligned(&self, p: Paragraph, n: &Node) -> Paragraph {
        match n.attr_str("textAlign") {
            Some("center") => p.align(AlignmentType::Center),
            Some("right") => p.align(AlignmentType::Right),
            Some("justify") => p.align(AlignmentType::Both),
            _ => p,
        }
    }

    /// A paragraph from `n`'s inline content (plus image captions after it).
    fn paragraph(&mut self, n: &Node, p: Paragraph, ctx: Ctx, out: &mut Vec<Block>) {
        let mut captions = Vec::new();
        // a paragraph holding just an aligned image takes the image's alignment
        let mut p = p;
        if n.content.len() == 1 && n.content[0].kind == nodes::IMAGE {
            p = match n.content[0].attr_str("align") {
                Some("center") => p.align(AlignmentType::Center),
                Some("right") => p.align(AlignmentType::Right),
                _ => p,
            };
        }
        let p = self.inlines(&n.content, p, ctx, &mut captions);
        out.push(Block::P(Box::new(p)));
        self.captions(captions, ctx, out);
    }

    fn captions(&mut self, captions: Vec<(String, Option<&'static str>)>, ctx: Ctx, out: &mut Vec<Block>) {
        for (c, align) in captions {
            let p = Self::indented(Paragraph::new().style("Caption"), ctx);
            let p = match align {
                Some("center") => p.align(AlignmentType::Center),
                Some("right") => p.align(AlignmentType::Right),
                _ => p,
            };
            out.push(Block::P(Box::new(p.add_run(Run::new().add_text(c)))));
        }
    }

    fn inlines(&mut self, content: &[Node], mut p: Paragraph, ctx: Ctx, captions: &mut Vec<(String, Option<&'static str>)>) -> Paragraph {
        for n in content {
            p = self.inline(n, p, ctx, captions);
        }
        p
    }

    fn inline(&mut self, n: &Node, p: Paragraph, ctx: Ctx, captions: &mut Vec<(String, Option<&'static str>)>) -> Paragraph {
        match n.kind.as_str() {
            nodes::TEXT => {
                let t = n.text.as_deref().unwrap_or("");
                let mut run = styled(Run::new().add_text(t), &n.marks);
                if ctx.bold {
                    run = run.bold();
                }
                let link = n.marks.iter().find(|m| m.kind == marks::LINK).and_then(|m| m.attr_str("href"));
                match link {
                    Some(href) if href.starts_with('#') => match self.anchors.get(&href[1..]).cloned() {
                        Some(bm) => p.add_hyperlink(Hyperlink::new(bm, HyperlinkType::Anchor).add_run(run.color(LINK_COLOR))),
                        None => p.add_run(run),
                    },
                    Some(href) if self.page_bookmark(href).is_some() => {
                        let bm = self.page_bookmark(href).unwrap_or_default();
                        p.add_hyperlink(Hyperlink::new(bm, HyperlinkType::Anchor).add_run(run.color(LINK_COLOR).underline("single")))
                    }
                    Some(href) => match self.url(href) {
                        Some(url) => p.add_hyperlink(Hyperlink::new(url, HyperlinkType::External).add_run(run.color(LINK_COLOR).underline("single"))),
                        None => p.add_run(run),
                    },
                    None => p.add_run(run),
                }
            }
            nodes::HARD_BREAK => p.add_run(Run::new().add_break(BreakType::TextWrapping)),
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
                let img = attachment_id(src).and_then(|id| self.input.images.get(&id));
                // every embedding carries its own copy of the image: bounded per export
                let img = img.filter(|i| self.embeds.0 < MAX_EMBEDS && self.embeds.1 + i.png.len() <= MAX_EMBEDDED_BYTES);
                match img {
                    Some(img) => {
                        self.embeds = (self.embeds.0 + 1, self.embeds.1 + img.png.len(), self.embeds.2);
                        let max = (((ctx.width - ctx.indent).max(1440) as u32) / 15).min(CONTENT_WIDTH_PX);
                        let want = n.attr_i64("width").filter(|w| *w > 0).map(|w| w as u32).unwrap_or(img.w);
                        let w = want.min(max).max(8);
                        let h = ((img.h as u64 * w as u64) / img.w.max(1) as u64).max(1) as u32;
                        p.add_run(Run::new().add_image(pic(img, w, h)))
                    }
                    None => {
                        let label = if alt.is_empty() { "Image".to_string() } else { format!("Image: {alt}") };
                        let run = Run::new().add_text(format!("[{label}]")).italic().color(MUTED);
                        match self.url(src) {
                            Some(url) => p.add_hyperlink(Hyperlink::new(url, HyperlinkType::External).add_run(run.color(LINK_COLOR))),
                            None => p.add_run(run),
                        }
                    }
                }
            }
            nodes::MENTION => {
                let label = n.attr_str("label").or(n.attr_str("id")).unwrap_or("user");
                p.add_run(Run::new().add_text(format!("@{label}")).color(LINK_COLOR))
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
                let t = n.attr_str("text").unwrap_or("").to_uppercase();
                p.add_run(Run::new().add_text(format!("\u{00A0}{t}\u{00A0}")).bold().size(16).color(fg).shading(Shading::new().fill(bg)))
            }
            nodes::DATE => {
                let d = n.attr_str("date").unwrap_or("");
                let shown = chrono::NaiveDate::parse_from_str(d, "%Y-%m-%d").map(|d| d.format("%-d %b %Y").to_string()).unwrap_or_else(|_| d.to_string());
                p.add_run(Run::new().add_text(format!("\u{00A0}{shown}\u{00A0}")).shading(Shading::new().fill("F1F5F9")))
            }
            nodes::ISSUE => {
                let key = n.attr_str("key").unwrap_or("issue").to_string();
                match n.attr_str("url").and_then(|u| self.url(u)) {
                    Some(url) => {
                        p.add_hyperlink(Hyperlink::new(url, HyperlinkType::External).add_run(Run::new().add_text(key).color(LINK_COLOR).underline("single")))
                    }
                    None => p.add_run(Run::new().add_text(key).fonts(mono())),
                }
            }
            nodes::EMOTICON => {
                let name = n.attr_str("name").unwrap_or("");
                let key = format!("emoticon:{name}");
                let svg = || {
                    icons::inner_markup("emoticons", name)
                        .map(|inner| format!("<svg xmlns=\"http://www.w3.org/2000/svg\" viewBox=\"0 0 24 24\" width=\"24\" height=\"24\">{inner}</svg>"))
                };
                match self.icon_run(&key, svg) {
                    Some(run) => p.add_run(run),
                    None => p.add_run(Run::new().add_text(icons::emoticon_shortcut(name))),
                }
            }
            nodes::MATH_INLINE => {
                let fonts = RunFonts::new().ascii("Cambria Math").hi_ansi("Cambria Math");
                p.add_run(Run::new().add_text(n.attr_str("latex").unwrap_or("")).fonts(fonts).italic())
            }
            nodes::ANCHOR => {
                let name = text::slugify(n.attr_str("name").unwrap_or(""));
                if name.is_empty() {
                    return p;
                }
                let bm = format!("a{}", self.anchors.len());
                self.anchors.entry(name).or_insert(bm.clone());
                let id = self.bookmark();
                p.add_bookmark_start(id, bm).add_bookmark_end(id)
            }
            _ => {
                if let Some(t) = &n.text {
                    p.add_run(Run::new().add_text(t))
                } else {
                    self.inlines(&n.content, p, ctx, captions)
                }
            }
        }
    }

    fn table(&mut self, n: &Node, ctx: Ctx) -> Table {
        let rows: Vec<&Node> = n.content.iter().filter(|r| r.kind == nodes::TABLE_ROW).collect();
        // column count: the widest row, counting colspans
        let cols =
            rows.iter().map(|r| r.content.iter().map(|c| c.attr_i64("colspan").unwrap_or(1).clamp(1, 63) as usize).sum::<usize>()).max().unwrap_or(1).max(1);
        let width = (ctx.width - ctx.indent).max(1440);
        // column widths from the editor's `colwidth` (px) where known, scaled to the page
        let mut px: Vec<Option<u32>> = vec![None; cols];
        if let Some(first) = rows.first() {
            let mut col = 0;
            for c in &first.content {
                let span = c.attr_i64("colspan").unwrap_or(1).clamp(1, 63) as usize;
                if let Some(ws) = c.attrs.get("colwidth").and_then(|v| v.as_array()) {
                    for (i, w) in ws.iter().enumerate() {
                        if col + i < cols {
                            px[col + i] = w.as_u64().map(|w| w as u32);
                        }
                    }
                }
                col += span;
            }
        }
        let known: u32 = px.iter().flatten().sum();
        let unknown = px.iter().filter(|w| w.is_none()).count() as u32;
        let default_px = CONTENT_WIDTH_PX.saturating_sub(known).max(60 * unknown).checked_div(unknown).unwrap_or(0);
        let total_px: u32 = px.iter().map(|w| w.unwrap_or(default_px)).sum::<u32>().max(1);
        let grid: Vec<usize> = px.iter().map(|w| (w.unwrap_or(default_px) as i64 * width as i64 / total_px as i64).max(200) as usize).collect();

        // rowspans: columns still covered by a cell above (rows left, colspan)
        let mut covered: Vec<(usize, usize)> = vec![(0, 0); cols];
        let mut out_rows = Vec::new();
        for row in rows {
            let mut cells = Vec::new();
            let mut col = 0;
            let mut iter = row.content.iter();
            loop {
                if col < cols && covered[col].0 > 0 {
                    let span = covered[col].1.max(1);
                    covered[col].0 -= 1;
                    let w: usize = grid[col..(col + span).min(cols)].iter().sum();
                    cells.push(TableCell::new().vertical_merge(VMergeType::Continue).grid_span(span).width(w, WidthType::Dxa).add_paragraph(Paragraph::new()));
                    col += span;
                    continue;
                }
                let Some(c) = iter.next() else { break };
                let span = (c.attr_i64("colspan").unwrap_or(1).clamp(1, 63) as usize).min(cols.saturating_sub(col).max(1));
                let rowspan = c.attr_i64("rowspan").unwrap_or(1).clamp(1, 1000) as usize;
                let w: usize = grid[col.min(cols - 1)..(col + span).min(cols)].iter().sum();
                let header = c.kind == nodes::TABLE_HEADER;
                let inner = Ctx { indent: 0, depth: 0, width: w as i32 - 200, bold: header };
                let blocks = self.blocks(&c.content, inner);
                let mut cell = fill_cell(TableCell::new(), blocks).width(w, WidthType::Dxa);
                if span > 1 {
                    cell = cell.grid_span(span);
                }
                if rowspan > 1 {
                    cell = cell.vertical_merge(VMergeType::Restart);
                    if col < cols {
                        covered[col] = (rowspan - 1, span);
                    }
                }
                let bg = c.attr_str("background").and_then(hex_color).or_else(|| header.then(|| "F1F5F9".to_string()));
                if let Some(bg) = bg {
                    cell = cell.shading(Shading::new().fill(bg));
                }
                cells.push(cell);
                col += span;
            }
            if cells.is_empty() {
                continue;
            }
            out_rows.push(TableRow::new(cells));
        }
        let border = |pos| TableBorder::new(pos).border_type(BorderType::Single).size(4).color("CBD5E1");
        Table::new(out_rows)
            .set_grid(grid)
            .width(width as usize, WidthType::Dxa)
            .indent(ctx.indent)
            .margins(TableCellMargins::new().margin(60, 100, 60, 100))
            .set_border(border(TableBorderPosition::Top))
            .set_border(border(TableBorderPosition::Bottom))
            .set_border(border(TableBorderPosition::Left))
            .set_border(border(TableBorderPosition::Right))
            .set_border(border(TableBorderPosition::InsideH))
            .set_border(border(TableBorderPosition::InsideV))
    }
}

/// Puts blocks into a cell; a cell must end with a paragraph.
fn fill_cell(mut cell: TableCell, blocks: Vec<Block>) -> TableCell {
    let mut last_is_paragraph = false;
    for b in blocks {
        cell = match b {
            Block::P(p) => {
                last_is_paragraph = true;
                cell.add_paragraph(*p)
            }
            Block::T(t) => {
                last_is_paragraph = false;
                cell.add_table(*t)
            }
        };
    }
    if !last_is_paragraph {
        cell = cell.add_paragraph(Paragraph::new());
    }
    cell
}

fn styled(mut run: Run, ms: &[Mark]) -> Run {
    for m in ms {
        run = match m.kind.as_str() {
            marks::BOLD => run.bold(),
            marks::ITALIC => run.italic(),
            marks::UNDERLINE => run.underline("single"),
            marks::STRIKE => run.strike(),
            marks::CODE => run.fonts(mono()).size(20).shading(Shading::new().fill("F1F5F9")),
            marks::SUBSCRIPT => {
                run.run_property = run.run_property.vert_align(VertAlignType::SubScript);
                run
            }
            marks::SUPERSCRIPT => {
                run.run_property = run.run_property.vert_align(VertAlignType::SuperScript);
                run
            }
            marks::TEXT_STYLE => match m.attr_str("color").and_then(hex_color) {
                Some(c) => run.color(c),
                None => run,
            },
            marks::HIGHLIGHT => run.shading(Shading::new().fill(m.attr_str("color").and_then(hex_color).unwrap_or_else(|| "FEF08A".into()))),
            _ => run,
        };
    }
    run
}

/// Done tasks read as done: struck through and muted.
fn strike_all(mut p: Paragraph) -> Paragraph {
    use docx_rs::ParagraphChild;
    for c in p.children.iter_mut() {
        match c {
            ParagraphChild::Run(r) if !r.children.iter().any(|x| matches!(x, docx_rs::RunChild::Drawing(_))) => {
                **r = std::mem::take(&mut **r).strike().color(MUTED);
            }
            ParagraphChild::Hyperlink(h) => {
                for hc in h.children.iter_mut() {
                    if let ParagraphChild::Run(r) = hc {
                        **r = std::mem::take(&mut **r).strike().color(MUTED);
                    }
                }
            }
            _ => {}
        }
    }
    p
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn colors() {
        assert_eq!(hex_color("#abc").as_deref(), Some("AABBCC"));
        assert_eq!(hex_color("#1d4ed8ff").as_deref(), Some("1D4ED8"));
        assert_eq!(hex_color("rgb(255, 0, 10)").as_deref(), Some("FF000A"));
        assert_eq!(hex_color("red").as_deref(), Some("DC2626"));
        assert_eq!(hex_color("url(x)"), None);
        assert_eq!(hex_color("#zzz"), None);
    }

    #[test]
    fn icons_rasterise_and_svg_cannot_read_files() {
        let svg = line_icon_svg("callout", "info", "#3B82F6").unwrap();
        let img = rasterize_svg(svg.as_bytes(), Some(ICON_PX)).unwrap();
        assert!(img.png.starts_with(&[0x89, b'P', b'N', b'G']) && img.w == ICON_PX);
        // an <image> pointing at a local file is ignored (the image still renders, empty)
        let evil = "<svg xmlns=\"http://www.w3.org/2000/svg\" xmlns:xlink=\"http://www.w3.org/1999/xlink\" width=\"10\" height=\"10\"><image href=\"/etc/hostname\" width=\"10\" height=\"10\"/><image xlink:href=\"Cargo.toml\" width=\"10\" height=\"10\"/></svg>";
        assert!(rasterize_svg(evil.as_bytes(), None).is_some());
        assert!(prepare_image(b"not an image").is_none());
    }
}
