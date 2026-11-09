//! A tiny DOM for Confluence storage-format (XHTML + `ac:`/`ri:` elements).
//!
//! Parsing is strict XML after normalization (see [`normalize`]). The verification run on two
//! real exports showed 100% of bodies parse after normalization; if parsing still fails we fall
//! back to a lenient tag-soup parse so that content is never lost.

use quick_xml::Reader;
use quick_xml::events::Event;

use crate::entities;

#[derive(Debug, Clone, PartialEq)]
pub enum Dom {
    Element(Element),
    Text(String),
}

#[derive(Debug, Clone, PartialEq, Default)]
pub struct Element {
    /// Qualified name, e.g. `p`, `ac:structured-macro`, `ri:page`.
    pub name: String,
    pub attrs: Vec<(String, String)>,
    pub children: Vec<Dom>,
}

impl Element {
    pub fn attr(&self, name: &str) -> Option<&str> {
        self.attrs.iter().find(|(k, _)| k == name).map(|(_, v)| v.as_str())
    }

    pub fn child(&self, name: &str) -> Option<&Element> {
        self.children.iter().find_map(|c| match c {
            Dom::Element(e) if e.name == name => Some(e),
            _ => None,
        })
    }

    pub fn children_named<'a>(&'a self, name: &'a str) -> impl Iterator<Item = &'a Element> + 'a {
        self.children.iter().filter_map(move |c| match c {
            Dom::Element(e) if e.name == name => Some(e),
            _ => None,
        })
    }

    pub fn elements(&self) -> impl Iterator<Item = &Element> {
        self.children.iter().filter_map(|c| match c {
            Dom::Element(e) => Some(e),
            _ => None,
        })
    }

    /// Concatenated text of all descendants.
    pub fn text(&self) -> String {
        let mut s = String::new();
        fn go(e: &Element, s: &mut String) {
            for c in &e.children {
                match c {
                    Dom::Text(t) => s.push_str(t),
                    Dom::Element(e) => go(e, s),
                }
            }
        }
        go(self, &mut s);
        s
    }

    /// Value of `<ac:parameter ac:name="...">` children of a macro.
    pub fn macro_param(&self, name: &str) -> Option<String> {
        self.children_named("ac:parameter").find(|p| p.attr("ac:name") == Some(name)).map(Element::text)
    }

    pub fn macro_params(&self) -> Vec<(String, String)> {
        self.children_named("ac:parameter").map(|p| (p.attr("ac:name").unwrap_or("").to_string(), p.text())).collect()
    }
}

/// Makes a storage-format body strict-XML parseable:
/// 1. undo Confluence's CDATA terminator escape (`]]>` inside CDATA is exported as `]] >`),
/// 2. replace HTML named entities with numeric character references,
/// 3. (done by [`parse`]) wrap everything in a root element declaring the namespaces.
pub fn normalize(body: &str) -> String {
    let body = body.replace("]] >", "]]>");
    let mut out = String::with_capacity(body.len() + 16);
    let bytes = body.as_bytes();
    let mut i = 0;
    let mut in_cdata = false;
    while i < body.len() {
        if !in_cdata && body[i..].starts_with("<![CDATA[") {
            in_cdata = true;
            out.push_str("<![CDATA[");
            i += 9;
            continue;
        }
        if in_cdata && body[i..].starts_with("]]>") {
            in_cdata = false;
            out.push_str("]]>");
            i += 3;
            continue;
        }
        if !in_cdata && bytes[i] == b'&' {
            // find entity end
            let window_end = (i + 34).min(body.len());
            let window = body.get(i + 1..window_end).unwrap_or("");
            if let Some(end) = window.find(';') {
                let name = &body[i + 1..i + 1 + end];
                if matches!(name, "amp" | "lt" | "gt" | "quot" | "apos") || name.starts_with('#') {
                    out.push_str(&body[i..i + end + 2]);
                } else if let Some(ch) = entities::lookup(name) {
                    out.push_str(&format!("&#{};", ch as u32));
                } else {
                    out.push_str("&amp;");
                    out.push_str(&body[i + 1..i + end + 2]);
                }
                i += end + 2;
                continue;
            }
            out.push_str("&amp;");
            i += 1;
            continue;
        }
        let ch = body[i..].chars().next().expect("char boundary");
        out.push(ch);
        i += ch.len_utf8();
    }
    out
}

const ROOT_OPEN: &str =
    "<wc-root xmlns:ac=\"http://atlassian.com/content\" xmlns:ri=\"http://atlassian.com/resource/identifier\" xmlns:at=\"http://atlassian.com/template\">";
const ROOT_CLOSE: &str = "</wc-root>";

/// Whether the body parsed strictly (`true`) or needed the lenient fallback.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ParseMode {
    Strict,
    Lenient,
}

/// Parses a storage-format body into a root element (named `wc-root`).
pub fn parse(body: &str) -> (Element, ParseMode) {
    let normalized = normalize(body);
    let wrapped = format!("{ROOT_OPEN}{normalized}{ROOT_CLOSE}");
    match parse_xml(&wrapped, true) {
        Ok(root) => (root, ParseMode::Strict),
        Err(_) => (parse_xml(&wrapped, false).unwrap_or_default(), ParseMode::Lenient),
    }
}

/// Parses a standalone XML document (e.g. an Office Open XML part) and returns its root
/// element. Same depth limits as [`parse`].
pub fn parse_document(src: &str) -> Option<Element> {
    parse_xml(src, true).ok()
}

const MAX_DEPTH: usize = 200;

const VOID: &[&str] = &["br", "hr", "img", "col", "input", "meta", "link", "wbr", "area", "base", "source", "track", "param", "embed"];

fn parse_xml(src: &str, strict: bool) -> Result<Element, quick_xml::Error> {
    let mut reader = Reader::from_str(src);
    {
        let cfg = reader.config_mut();
        cfg.check_end_names = strict;
        cfg.allow_unmatched_ends = !strict;
        cfg.allow_dangling_amp = !strict;
        cfg.expand_empty_elements = false;
    }
    let mut stack: Vec<Element> = vec![Element { name: "#document".into(), ..Default::default() }];
    // elements nested deeper than MAX_DEPTH are not materialized (their text is kept), so the
    // recursive converter and `Drop` can't overflow the stack on hostile input
    let mut skipped = 0usize;
    loop {
        match reader.read_event()? {
            Event::Start(_) if stack.len() >= MAX_DEPTH => skipped += 1,
            Event::End(_) if skipped > 0 => skipped -= 1,
            Event::Start(e) => {
                let name = e.name().as_ref().to_string();
                let attrs = read_attrs(&e);
                let el = Element { name: name.clone(), attrs, children: vec![] };
                if !strict && VOID.contains(&name.as_str()) {
                    push_child(&mut stack, Dom::Element(el));
                } else {
                    stack.push(el);
                }
            }
            Event::Empty(e) => {
                let name = e.name().as_ref().to_string();
                let attrs = read_attrs(&e);
                push_child(&mut stack, Dom::Element(Element { name, attrs, children: vec![] }));
            }
            Event::End(e) => {
                let name = e.name().as_ref().to_string();
                if !strict {
                    if VOID.contains(&name.as_str()) {
                        continue;
                    }
                    // close up to the matching element if present, otherwise ignore
                    if !stack.iter().skip(1).any(|el| el.name == name) {
                        continue;
                    }
                    while stack.len() > 1 {
                        let el = stack.pop().expect("non-empty");
                        let matched = el.name == name;
                        push_child(&mut stack, Dom::Element(el));
                        if matched {
                            break;
                        }
                    }
                } else if stack.len() > 1 {
                    let el = stack.pop().expect("non-empty");
                    push_child(&mut stack, Dom::Element(el));
                }
            }
            Event::Text(t) => push_text(&mut stack, &t.into_inner()),
            Event::CData(t) => push_text(&mut stack, &t.into_inner()),
            Event::GeneralRef(r) => {
                let s = if let Ok(Some(ch)) = r.resolve_char_ref() {
                    ch.to_string()
                } else {
                    match r.into_inner().as_ref() {
                        "amp" => "&".into(),
                        "lt" => "<".into(),
                        "gt" => ">".into(),
                        "quot" => "\"".into(),
                        "apos" => "'".into(),
                        other => entities::lookup(other).map(String::from).unwrap_or_else(|| format!("&{other};")),
                    }
                };
                push_text(&mut stack, &s);
            }
            Event::Eof => break,
            _ => {}
        }
    }
    while stack.len() > 1 {
        let el = stack.pop().expect("non-empty");
        push_child(&mut stack, Dom::Element(el));
    }
    let doc = stack.pop().expect("document");
    // return the wc-root element (first element child)
    Ok(doc
        .children
        .into_iter()
        .find_map(|c| match c {
            Dom::Element(e) => Some(e),
            _ => None,
        })
        .unwrap_or_default())
}

fn read_attrs(e: &quick_xml::events::BytesStart<'_>) -> Vec<(String, String)> {
    e.attributes()
        .with_checks(false)
        .flatten()
        .map(|a| {
            let k = a.key.as_ref().to_string();
            let v = a.normalized_value(quick_xml::XmlVersion::Implicit1_0).map(|v| v.to_string()).unwrap_or_else(|_| a.value.to_string());
            (k, unescape_named(&v))
        })
        .collect()
}

/// Resolve numeric refs that survived in attribute values.
fn unescape_named(v: &str) -> String {
    if !v.contains('&') {
        return v.to_string();
    }
    let mut out = String::new();
    let mut rest = v;
    while let Some(i) = rest.find('&') {
        out.push_str(&rest[..i]);
        let after = &rest[i + 1..];
        let window = after.char_indices().take_while(|(i, _)| *i <= 32).map(|(i, c)| i + c.len_utf8()).last().unwrap_or(0);
        if let Some(end) = after[..window].find(';') {
            let name = &after[..end];
            let ch = if let Some(num) = name.strip_prefix("#x").or_else(|| name.strip_prefix("#X")) {
                u32::from_str_radix(num, 16).ok().and_then(char::from_u32)
            } else if let Some(num) = name.strip_prefix('#') {
                num.parse().ok().and_then(char::from_u32)
            } else {
                entities::lookup(name)
            };
            if let Some(ch) = ch {
                out.push(ch);
                rest = &after[end + 1..];
                continue;
            }
        }
        out.push('&');
        rest = after;
    }
    out.push_str(rest);
    out
}

fn push_child(stack: &mut [Element], child: Dom) {
    if let Some(top) = stack.last_mut() {
        top.children.push(child);
    }
}

fn push_text(stack: &mut [Element], s: &str) {
    if s.is_empty() {
        return;
    }
    if let Some(top) = stack.last_mut() {
        if let Some(Dom::Text(prev)) = top.children.last_mut() {
            prev.push_str(s);
        } else {
            top.children.push(Dom::Text(s.to_string()));
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn normalizes_entities_and_cdata() {
        // entities.xml escapes every `]]>` of the storage body as `]] >`; a literal `]]>` inside code is
        // stored by Confluence as `]]]]><![CDATA[>` (standard CDATA split).
        let body = "<p>a&nbsp;b &mdash; &amp; &unknown;</p><ac:plain-text-body><![CDATA[x ]]]] ><![CDATA[> y &nbsp;]] ></ac:plain-text-body>";
        let (root, mode) = parse(body);
        assert_eq!(mode, ParseMode::Strict);
        let p = root.child("p").unwrap();
        assert_eq!(p.text(), "a\u{a0}b \u{2014} & &unknown;");
        let code = root.child("ac:plain-text-body").unwrap();
        assert_eq!(code.text(), "x ]]> y &nbsp;");
    }

    #[test]
    fn lenient_fallback_keeps_text() {
        let (root, mode) = parse("<p>unclosed <b>bold</p><p>next");
        assert_eq!(mode, ParseMode::Lenient);
        assert!(root.text().contains("unclosed bold"));
        assert!(root.text().contains("next"));
    }

    #[test]
    fn deep_nesting_is_capped() {
        let body = format!("{}text{}", "<div>".repeat(50_000), "</div>".repeat(50_000));
        let (root, _) = std::thread::Builder::new()
            .stack_size(2 << 20)
            .spawn(move || {
                let r = parse(&body);
                let t = r.0.text();
                (t, r.1)
            })
            .unwrap()
            .join()
            .unwrap();
        assert_eq!(root, "text");
    }

    #[test]
    fn attributes_are_unescaped() {
        let (root, _) = parse("<ac:link><ri:page ri:content-title=\"A &amp; B&nbsp;C\" /></ac:link>");
        let page = root.child("ac:link").unwrap().child("ri:page").unwrap();
        assert_eq!(page.attr("ri:content-title"), Some("A & B\u{a0}C"));
    }
}
