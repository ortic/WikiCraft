//! HTML pages → documents, for the folder/zip importer: plain HTML sites, Notion's HTML export
//! and Confluence's HTML space export (content in `#main-content`, title in `#title-text`,
//! hierarchy in `#breadcrumbs`, files listed under "Attachments:").
//!
//! Untrusted input: parsing is the tag-soup parser of the Confluence crate (depth-capped), raw
//! text elements (scripts, styles) are cut out before parsing, and nothing is fetched.

use wikicraft_confluence::dom::{self, Dom, Element};
use wikicraft_confluence::storage::{self, Resolver, Stats};
use wikicraft_doc_model::{Node, nodes};

pub struct HtmlPage {
    pub title: Option<String>,
    pub doc: Node,
    /// Confluence HTML export: the file the last breadcrumb points to (the parent page).
    pub parent_href: Option<String>,
    /// Confluence HTML export: files listed below the page, as (href, file name).
    pub attachments: Vec<(String, String)>,
}

/// Plain HTML has no Confluence references to resolve.
struct NoRefs;

impl Resolver for NoRefs {
    fn space_key(&self) -> &str {
        ""
    }
    fn content_id(&self) -> &str {
        ""
    }
    fn page(&self, _: &str, _: &str) -> Option<String> {
        None
    }
    fn content_exists(&self, _: &str) -> bool {
        false
    }
    fn attachment(&self, _: &str, _: &str) -> Option<String> {
        None
    }
    fn user_name(&self, _: &str) -> Option<String> {
        None
    }
}

/// Elements whose content is not markup: cut out before parsing, as their text may hold `<`.
const RAW: &[&str] = &["script", "style", "noscript", "template", "textarea", "iframe", "svg", "math", "object"];

/// Elements that are never page content.
const DROP: &[&str] = &["head", "title", "nav", "form", "button", "input", "select", "option", "embed", "canvas", "video", "audio", "link", "meta", "base"];

fn strip_raw(src: &str) -> String {
    let lower = src.to_ascii_lowercase(); // same byte offsets as `src`
    let mut out = String::with_capacity(src.len());
    let mut i = 0;
    while let Some(off) = lower[i..].find('<') {
        let at = i + off;
        out.push_str(&src[i..at]);
        let rest = &lower[at..];
        let skip_to = if rest.starts_with("<!--") {
            rest.find("-->").map(|e| at + e + 3)
        } else if rest.starts_with("<!") || rest.starts_with("<?") {
            rest.find('>').map(|e| at + e + 1)
        } else if let Some(tag) = RAW
            .iter()
            .find(|t| rest[1..].starts_with(**t) && rest[1 + t.len()..].chars().next().is_none_or(|c| c.is_ascii_whitespace() || c == '>' || c == '/'))
        {
            let close = format!("</{tag}");
            rest.find(&close).map(|e| rest[e..].find('>').map(|g| at + e + g + 1).unwrap_or(src.len()))
        } else {
            out.push('<');
            i = at + 1;
            continue;
        };
        i = skip_to.unwrap_or(src.len());
    }
    out.push_str(&src[i..]);
    out
}

fn has_class(el: &Element, class: &str) -> bool {
    el.attr("class").is_some_and(|c| c.split_ascii_whitespace().any(|x| x == class))
}

fn find<'a>(el: &'a Element, pred: &dyn Fn(&Element) -> bool) -> Option<&'a Element> {
    if pred(el) {
        return Some(el);
    }
    el.elements().find_map(|c| find(c, pred))
}

fn find_all<'a>(el: &'a Element, pred: &dyn Fn(&Element) -> bool, out: &mut Vec<&'a Element>) {
    if pred(el) {
        out.push(el);
    }
    for c in el.elements() {
        find_all(c, pred, out);
    }
}

fn text_of(el: &Element) -> String {
    el.text().split_whitespace().collect::<Vec<_>>().join(" ")
}

/// A copy without navigation, forms and (outside a known content element) page header/footer.
fn cleaned(el: &Element, drop_chrome: bool) -> Element {
    let children = el
        .children
        .iter()
        .filter_map(|c| match c {
            Dom::Element(e) => {
                let n = e.name.to_ascii_lowercase();
                let chrome = drop_chrome && matches!(n.as_str(), "header" | "footer" | "aside");
                let navish =
                    matches!(e.attr("id"), Some("breadcrumbs" | "breadcrumb-section" | "footer" | "navigation")) || e.attr("role") == Some("navigation");
                (!DROP.contains(&n.as_str()) && !chrome && !navish)
                    .then(|| Dom::Element(Element { name: n, attrs: e.attrs.clone(), children: cleaned(e, drop_chrome).children }))
            }
            t => Some(t.clone()),
        })
        .collect();
    Element { name: el.name.clone(), attrs: el.attrs.clone(), children }
}

/// Converts one HTML file.
pub fn convert(src: &str) -> HtmlPage {
    let (root, _) = dom::parse(&strip_raw(src));
    let lname = |e: &Element, n: &str| e.name.eq_ignore_ascii_case(n);
    let main_content = find(&root, &|e| e.attr("id") == Some("main-content"));
    let confluence = main_content.is_some();
    let (content_el, drop_chrome) = match main_content.or_else(|| find(&root, &|e| lname(e, "main"))) {
        Some(e) => (e, false),
        None => (find(&root, &|e| lname(e, "body")).unwrap_or(&root), true),
    };
    let content = cleaned(content_el, drop_chrome);
    let mut doc = storage::convert_element(&content, &NoRefs, &mut Stats::default());

    // title: Confluence's "Space : Page" heading, the document title, the first H1
    let title_text = find(&root, &|e| e.attr("id") == Some("title-text")).map(text_of).filter(|t| !t.is_empty());
    let doc_title = find(&root, &|e| lname(e, "title")).map(text_of).filter(|t| !t.is_empty());
    let h1 = find(&content, &|e| e.name == "h1").map(text_of).filter(|t| !t.is_empty());
    let title = match (title_text, doc_title) {
        (Some(t), _) => Some(t.split_once(" : ").map(|(_, p)| p.trim().to_string()).unwrap_or(t)),
        (None, Some(t)) if confluence => Some(t.split_once(" : ").map(|(_, p)| p.trim().to_string()).unwrap_or(t)),
        (None, Some(t)) => Some(t),
        (None, None) => h1,
    };
    // the heading that repeats the title is not content
    if let (Some(t), Some(first)) = (&title, doc.content.first())
        && first.kind == nodes::HEADING
        && first.attr_i64("level") == Some(1)
        && wikicraft_doc_model::text::plain_text(&Node::doc(vec![first.clone()])).trim().eq_ignore_ascii_case(t.trim())
    {
        doc.content.remove(0);
        if doc.content.is_empty() {
            doc.content.push(Node::paragraph(vec![]));
        }
    }

    let href = |e: &Element| e.attr("href").map(str::trim).filter(|h| !h.is_empty()).map(String::from);
    let parent_href = find(&root, &|e| e.attr("id") == Some("breadcrumbs")).and_then(|bc| {
        let mut links = Vec::new();
        find_all(bc, &|e| lname(e, "a"), &mut links);
        links.iter().filter_map(|a| href(a)).rfind(|h| {
            let file = h.rsplit('/').next().unwrap_or(h).to_ascii_lowercase();
            file != "index.html" && (file.ends_with(".html") || file.ends_with(".htm"))
        })
    });
    let mut attachments = Vec::new();
    if confluence {
        let mut sections = Vec::new();
        find_all(&root, &|e| has_class(e, "pageSection") && find(e, &|h| h.attr("id") == Some("attachments")).is_some(), &mut sections);
        for s in sections {
            let mut links = Vec::new();
            find_all(s, &|e| lname(e, "a"), &mut links);
            for a in links {
                if let Some(h) = href(a) {
                    let name = text_of(a);
                    attachments.push((h.clone(), if name.is_empty() { h.rsplit('/').next().unwrap_or(&h).to_string() } else { name }));
                }
            }
        }
    }
    HtmlPage { title: title.map(|t| t.chars().take(255).collect()), doc, parent_href, attachments }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn plain_html_page() {
        let p = convert(
            "<!DOCTYPE html><html><head><meta charset=utf-8><title>Guide</title><style>p { color: red }</style>\
             <script>if (a < b && c) { document.write('<p>x</p>') }</script></head>\
             <body><nav><a href=\"other.html\">Menu</a></nav><h1>Guide</h1><p>Hello <b>bold</b> &amp; <em>it</em> a &lt; b<br>next</p>\
             <ul><li>one<li>two</ul><table><tr><th>H</th></tr><tr><td>C</td></tr></table>\
             <pre><code>let x = 1;</code></pre><blockquote>q</blockquote><img src=\"img/a.png\" alt=\"A\"><p><a href=\"b.html#s\">B</a></p>\
             <footer>Copyright</footer></body></html>",
        );
        assert_eq!(p.title.as_deref(), Some("Guide"));
        let s = p.doc.to_json().to_string();
        for want in ["\"bold\"", "\"bulletList\"", "\"table\"", "\"codeBlock\"", "\"blockquote\"", "img/a.png", "b.html#s", "let x = 1;", "a < b"] {
            assert!(s.contains(want), "{want} missing: {s}");
        }
        for unwanted in ["Menu", "Copyright", "color: red", "document.write", "\"level\":1"] {
            assert!(!s.contains(unwanted), "{unwanted} kept: {s}");
        }
    }

    #[test]
    fn confluence_html_export_page() {
        let p = convert(
            "<html><head><title>Docs : Child</title></head><body><div id=\"page\"><div id=\"main-header\">\
             <div id=\"breadcrumb-section\"><ol id=\"breadcrumbs\"><li class=\"first\"><span><a href=\"index.html\">Docs</a></span></li>\
             <li><span><a href=\"Parent_123.html\">Parent</a></span></li></ol></div>\
             <h1 id=\"title-heading\" class=\"pagetitle\"><span id=\"title-text\"> Docs : Child </span></h1></div>\
             <div id=\"content\" class=\"view\"><div class=\"page-metadata\">Created by Ann</div>\
             <div id=\"main-content\" class=\"wiki-content group\"><p>Body <img class=\"confluence-embedded-image\" src=\"attachments/9/10.png?width=200\"></p></div>\
             <div class=\"pageSection group\"><div class=\"pageSectionHeader\"><h2 id=\"attachments\" class=\"pageSectionTitle\">Attachments:</h2></div>\
             <div class=\"greybox\"><img src=\"images/icons/bullet_blue.gif\"/> <a href=\"attachments/9/10.png\">diagram.png</a> (image/png)<br/></div></div>\
             </div></div><div id=\"footer\">Generated by Confluence</div></body></html>",
        );
        assert_eq!(p.title.as_deref(), Some("Child"));
        assert_eq!(p.parent_href.as_deref(), Some("Parent_123.html"));
        assert_eq!(p.attachments, vec![("attachments/9/10.png".to_string(), "diagram.png".to_string())]);
        let s = p.doc.to_json().to_string();
        assert!(s.contains("Body") && s.contains("attachments/9/10.png?width=200"), "{s}");
        assert!(!s.contains("Created by") && !s.contains("Generated by") && !s.contains("Attachments:"), "{s}");
    }
}
