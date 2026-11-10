//! Absolute links into a Confluence site, in the shapes Confluence writes them. Pages often
//! link to each other with full URLs (pasted from the browser); after a migration those point at
//! a site that is gone (`*.atlassian.net` can't be pointed at the new wiki), so the converter
//! turns the ones it can resolve into internal links.

use base64::Engine;

/// What an absolute Confluence URL points at.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Target {
    /// `/pages/viewpage.action?pageId=N`, `/spaces/KEY/pages/N/Title`, `/spaces/KEY/blog/…/N/Title`
    PageId { id: String, title: Option<String>, anchor: Option<String> },
    /// `/display/KEY/Title` (blog posts: `/display/KEY/YYYY/MM/DD/Title`)
    Title { space: String, title: String, anchor: Option<String> },
    /// `/x/AbCd` (tiny link: the page id, encoded)
    Tiny { id: String },
    /// `/download/attachments/N/file.pdf`
    Attachment { page_id: String, filename: String },
}

/// Splits `https://host/path?query#fragment` and recognises Confluence's URL shapes (also below
/// a context path such as `/wiki` or `/confluence`). Returns the host and the target.
pub fn parse(url: &str) -> Option<(String, Target)> {
    let rest = url.strip_prefix("https://").or_else(|| url.strip_prefix("http://"))?;
    let (host, path) = rest.split_once('/').map_or((rest, ""), |(h, p)| (h, p));
    let host = host.to_ascii_lowercase();
    let (path, fragment) = path.split_once('#').map_or((path, None), |(p, f)| (p, Some(f)));
    let (path, query) = path.split_once('?').map_or((path, None), |(p, q)| (p, Some(q)));
    let anchor = fragment.map(decode).filter(|a| !a.is_empty());
    let segs: Vec<&str> = path.split('/').filter(|s| !s.is_empty()).collect();
    let digits = |s: &str| !s.is_empty() && s.len() <= 20 && s.bytes().all(|b| b.is_ascii_digit());
    for (i, seg) in segs.iter().enumerate() {
        let after = &segs[i + 1..];
        match (*seg, after) {
            ("pages", ["viewpage.action"]) => {
                let id = query?.split('&').find_map(|kv| kv.strip_prefix("pageId="))?;
                return digits(id).then(|| (host, Target::PageId { id: id.to_string(), title: None, anchor }));
            }
            ("spaces", [_key, "pages", id, title @ ..]) if digits(id) => {
                return Some((host, Target::PageId { id: id.to_string(), title: title.first().map(|t| decode(t)), anchor }));
            }
            ("spaces", [_key, "blog", y, m, d, id, title @ ..]) if digits(y) && digits(m) && digits(d) && digits(id) => {
                return Some((host, Target::PageId { id: id.to_string(), title: title.first().map(|t| decode(t)), anchor }));
            }
            ("display", [space, y, m, d, title]) if digits(y) && digits(m) && digits(d) => {
                return Some((host, Target::Title { space: decode(space), title: decode(title), anchor }));
            }
            ("display", [space, title]) => return Some((host, Target::Title { space: decode(space), title: decode(title), anchor })),
            ("x", [code]) => return decode_tiny(code).map(|id| (host, Target::Tiny { id: id.to_string() })),
            ("download", ["attachments", page_id, filename]) if digits(page_id) => {
                return Some((host, Target::Attachment { page_id: page_id.to_string(), filename: decode(filename) }));
            }
            _ => {}
        }
    }
    None
}

/// URL decoding as Confluence writes titles: `+` is a space, `%XX` a byte.
fn decode(s: &str) -> String {
    let b = s.as_bytes();
    let mut out = Vec::with_capacity(b.len());
    let mut i = 0;
    while i < b.len() {
        match b[i] {
            b'+' => out.push(b' '),
            b'%' if i + 2 < b.len() => match std::str::from_utf8(&b[i + 1..i + 3]).ok().and_then(|h| u8::from_str_radix(h, 16).ok()) {
                Some(v) => {
                    out.push(v);
                    i += 3;
                    continue;
                }
                None => out.push(b'%'),
            },
            c => out.push(c),
        }
        i += 1;
    }
    String::from_utf8_lossy(&out).into_owned()
}

/// Decodes a Confluence tiny-link id (little-endian page id, base64 with `-`/`_` substitutions
/// and stripped trailing `A`s / padding).
pub fn decode_tiny(tiny: &str) -> Option<u64> {
    let mut s: String = tiny
        .chars()
        .map(|c| match c {
            '-' => '/',
            '_' => '+',
            c => c,
        })
        .collect();
    if s.is_empty() || s.len() > 11 || !s.chars().all(|c| c.is_ascii_alphanumeric() || c == '/' || c == '+') {
        return None;
    }
    while s.len() < 11 {
        s.push('A');
    }
    let bytes = base64::engine::general_purpose::STANDARD_NO_PAD.decode(s.as_bytes()).ok()?;
    let mut buf = [0u8; 8];
    for (i, b) in bytes.iter().take(8).enumerate() {
        buf[i] = *b;
    }
    Some(u64::from_le_bytes(buf))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn target(url: &str) -> Option<Target> {
        parse(url).map(|(_, t)| t)
    }

    #[test]
    fn confluence_url_shapes() {
        assert_eq!(
            parse("https://Acme.atlassian.net/wiki/spaces/HOME/pages/32473104/High+Assurance#Scope"),
            Some(("acme.atlassian.net".into(), Target::PageId { id: "32473104".into(), title: Some("High Assurance".into()), anchor: Some("Scope".into()) }))
        );
        assert_eq!(
            target("https://acme.atlassian.net/wiki/pages/viewpage.action?spaceKey=X&pageId=22982892"),
            Some(Target::PageId { id: "22982892".into(), title: None, anchor: None })
        );
        assert_eq!(
            target("https://wiki.acme.com/confluence/display/HOME/Calendar+of+ToIP+Meetings%3A+2024"),
            Some(Target::Title { space: "HOME".into(), title: "Calendar of ToIP Meetings: 2024".into(), anchor: None })
        );
        assert_eq!(
            target("https://wiki.acme.com/display/ENG/2024/01/31/Release+notes"),
            Some(Target::Title { space: "ENG".into(), title: "Release notes".into(), anchor: None })
        );
        assert_eq!(
            target("https://acme.atlassian.net/wiki/spaces/ENG/blog/2024/01/31/991/Release+notes"),
            Some(Target::PageId { id: "991".into(), title: Some("Release notes".into()), anchor: None })
        );
        assert_eq!(target("https://acme.atlassian.net/wiki/x/AgAB"), Some(Target::Tiny { id: "65538".into() }));
        assert_eq!(
            target("https://acme.atlassian.net/wiki/download/attachments/22970599/Minutes%20May.pdf?version=1&api=v2"),
            Some(Target::Attachment { page_id: "22970599".into(), filename: "Minutes May.pdf".into() })
        );
        assert_eq!(target("https://acme.atlassian.net/wiki/people/712020:abc?ref=confluence"), None);
        assert_eq!(target("https://github.com/acme/repo/pages/123"), None, "only Confluence shapes");
        assert_eq!(target("mailto:x@y"), None);
    }
}
