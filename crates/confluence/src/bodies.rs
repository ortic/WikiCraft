//! Page, version and comment bodies of an export, kept in a temporary file instead of memory.
//! A Data Center export with full history can hold gigabytes of storage-format XML; reading it
//! must not need that much RAM. Only an index (content id → where the body is) stays in
//! memory, a few dozen bytes per body. The file is anonymous: the OS removes it when the store
//! is dropped, also if the process dies.

use std::collections::{HashMap, HashSet};
use std::fs::File;
use std::io::{self, BufWriter, Write};
use std::sync::Mutex;

use crate::model::{Body, BodyKind};

#[derive(Debug, Clone, Copy)]
struct Slot {
    offset: u64,
    len: u64,
    kind: BodyKind,
}

#[derive(Default)]
struct Spill {
    writer: Option<BufWriter<File>>,
    reader: Option<File>,
    end: u64,
}

/// The bodies of an export (see the module docs).
#[derive(Default)]
pub struct BodyStore {
    index: HashMap<String, Slot>,
    spill: Mutex<Spill>,
    /// the first write error (reported after reading: the export can't be imported completely)
    error: Option<io::Error>,
    /// (host, page id) of absolute links into a Confluence site, collected while the text is at
    /// hand: the planner works out from them which site the export comes from
    site_links: HashSet<(String, String)>,
}

impl std::fmt::Debug for BodyStore {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("BodyStore").field("bodies", &self.index.len()).finish()
    }
}

/// At most this many distinct site links are remembered (plenty to recognise the site).
const MAX_SITE_LINKS: usize = 100_000;

impl BodyStore {
    pub fn len(&self) -> usize {
        self.index.len()
    }

    pub fn is_empty(&self) -> bool {
        self.index.is_empty()
    }

    pub fn contains_key(&self, content_id: &str) -> bool {
        self.index.contains_key(content_id)
    }

    pub fn kind(&self, content_id: &str) -> Option<BodyKind> {
        self.index.get(content_id).map(|s| s.kind)
    }

    pub fn ids(&self) -> impl Iterator<Item = &String> {
        self.index.keys()
    }

    pub fn remove(&mut self, content_id: &str) {
        self.index.remove(content_id);
    }

    /// Stores a body (replacing an earlier one of the same content).
    pub fn insert(&mut self, body: Body) {
        if body.kind == BodyKind::Storage && self.site_links.len() < MAX_SITE_LINKS {
            collect_site_links(&body.body, &mut self.site_links);
        }
        let spill = self.spill.get_mut().expect("lock");
        let res = (|| -> io::Result<u64> {
            if spill.writer.is_none() {
                let file = tempfile::tempfile()?;
                spill.reader = Some(file.try_clone()?);
                spill.writer = Some(BufWriter::with_capacity(1 << 20, file));
            }
            let w = spill.writer.as_mut().expect("created above");
            w.write_all(body.body.as_bytes())?;
            let offset = spill.end;
            spill.end += body.body.len() as u64;
            Ok(offset)
        })();
        match res {
            Ok(offset) => {
                self.index.insert(body.content_id, Slot { offset, len: body.body.len() as u64, kind: body.kind });
            }
            Err(e) => {
                if self.error.is_none() {
                    self.error = Some(e);
                }
            }
        }
    }

    /// Reads a body back.
    pub fn get(&self, content_id: &str) -> Option<Body> {
        let slot = *self.index.get(content_id)?;
        let mut spill = self.spill.lock().expect("lock");
        if let Some(w) = spill.writer.as_mut()
            && w.flush().is_err()
        {
            return None;
        }
        let reader = spill.reader.as_ref()?;
        let mut buf = vec![0u8; slot.len as usize];
        read_at(reader, &mut buf, slot.offset).ok()?;
        Some(Body { content_id: content_id.to_string(), kind: slot.kind, body: String::from_utf8(buf).ok()? })
    }

    /// The first error writing the store, if any.
    pub fn take_error(&mut self) -> Option<io::Error> {
        self.error.take()
    }

    /// (host, page id) pairs of absolute links seen in storage-format bodies.
    pub fn site_links(&self) -> &HashSet<(String, String)> {
        &self.site_links
    }
}

#[cfg(unix)]
fn read_at(f: &File, buf: &mut [u8], offset: u64) -> io::Result<()> {
    std::os::unix::fs::FileExt::read_exact_at(f, buf, offset)
}

#[cfg(windows)]
fn read_at(f: &File, mut buf: &mut [u8], mut offset: u64) -> io::Result<()> {
    while !buf.is_empty() {
        let n = std::os::windows::fs::FileExt::seek_read(f, buf, offset)?;
        if n == 0 {
            return Err(io::ErrorKind::UnexpectedEof.into());
        }
        buf = &mut buf[n..];
        offset += n as u64;
    }
    Ok(())
}

/// Page-id links (`viewpage.action?pageId=`, `/spaces/KEY/pages/ID`, tiny links) in a body.
fn collect_site_links(body: &str, out: &mut HashSet<(String, String)>) {
    let mut rest = body;
    while let Some(i) = rest.find("href=\"http") {
        rest = &rest[i + 6..];
        let Some(end) = rest.find('"') else { break };
        let url = rest[..end].replace("&amp;", "&");
        rest = &rest[end..];
        if let Some((host, crate::urls::Target::PageId { id, .. } | crate::urls::Target::Tiny { id })) = crate::urls::parse(&url) {
            out.insert((host, id));
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn bodies_round_trip_through_the_file() {
        let mut s = BodyStore::default();
        for i in 0..1000 {
            s.insert(Body { content_id: i.to_string(), kind: BodyKind::Storage, body: format!("<p>body {i} – ünïcode {}</p>", "x".repeat(i)) });
        }
        // replaced: the newer one counts
        s.insert(Body { content_id: "7".into(), kind: BodyKind::WikiMarkup, body: "h1. seven".into() });
        assert_eq!(s.len(), 1000);
        let b = s.get("500").unwrap();
        assert_eq!(b.body, format!("<p>body 500 – ünïcode {}</p>", "x".repeat(500)));
        assert_eq!(s.get("7").unwrap().body, "h1. seven");
        assert_eq!(s.kind("7"), Some(BodyKind::WikiMarkup));
        assert!(s.get("nope").is_none());
        // writes after reads work too
        s.insert(Body { content_id: "late".into(), kind: BodyKind::Storage, body: "<p>late</p>".into() });
        assert_eq!(s.get("late").unwrap().body, "<p>late</p>");
        assert_eq!(s.get("0").unwrap().body, "<p>body 0 – ünïcode </p>");
        assert!(s.take_error().is_none());
    }

    #[test]
    fn collects_page_id_links() {
        let mut s = BodyStore::default();
        s.insert(Body {
            content_id: "1".into(),
            kind: BodyKind::Storage,
            body: r#"<a href="https://acme.atlassian.net/wiki/pages/viewpage.action?spaceKey=X&amp;pageId=42">x</a> <a href="https://acme.atlassian.net/wiki/display/X/Y">y</a>"#.into(),
        });
        assert_eq!(s.site_links().iter().collect::<Vec<_>>(), [&("acme.atlassian.net".to_string(), "42".to_string())]);
    }
}
