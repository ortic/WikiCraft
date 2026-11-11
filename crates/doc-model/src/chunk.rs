//! Passages for retrieval (RAG): a document cut along its structure.
//!
//! A new passage starts at every heading, so a passage stays inside one section and knows its
//! heading path (`["Setup", "Database"]`). Long sections are cut between blocks once a passage
//! reaches [`TARGET_CHARS`]; a single block longer than [`MAX_CHARS`] (a big table, code block or
//! list) is cut between lines, repeating a table's header row and a code block's fence. Each
//! passage is Markdown (tables, lists and code keep their shape for a language model) and
//! remembers the `blockId` it starts at, for links straight to the passage.

use crate::blocks;
use crate::markdown::pm_to_markdown;
use crate::node::{Node, nodes};
use crate::text::inline_text;

/// A passage grows block by block up to about this many characters.
pub const TARGET_CHARS: usize = 1200;
/// Longer blocks are cut into pieces of at most this many characters.
pub const MAX_CHARS: usize = 2400;

#[derive(Debug, Clone, PartialEq)]
pub struct Chunk {
    /// the first block of the passage that has an id (link target `#block-<id>`)
    pub block_id: Option<String>,
    /// the headings the passage sits under, outermost first
    pub headings: Vec<String>,
    pub markdown: String,
}

/// Cuts a document into passages.
pub fn chunk_doc(doc: &Node) -> Vec<Chunk> {
    let mut c = Chunker::default();
    for block in &doc.content {
        c.block(block);
    }
    c.flush();
    c.out
}

/// Cuts plain text (the extracted text of an attachment) into passages along paragraphs.
pub fn chunk_text(text: &str) -> Vec<String> {
    let mut out = Vec::new();
    let mut cur = String::new();
    for para in text.split('\n').map(str::trim).filter(|p| !p.is_empty()) {
        if !cur.is_empty() && cur.len() + para.len() + 1 > TARGET_CHARS {
            out.push(std::mem::take(&mut cur));
        }
        if para.len() > MAX_CHARS {
            if !cur.is_empty() {
                out.push(std::mem::take(&mut cur));
            }
            out.extend(split_long_line(para, MAX_CHARS));
            continue;
        }
        if !cur.is_empty() {
            cur.push('\n');
        }
        cur.push_str(para);
    }
    if !cur.is_empty() {
        out.push(cur);
    }
    out
}

#[derive(Default)]
struct Chunker {
    out: Vec<Chunk>,
    /// open headings: (level, text)
    stack: Vec<(i64, String)>,
    /// blocks of the passage being built, rendered
    parts: Vec<String>,
    len: usize,
    /// the passage holds only headings so far
    only_headings: bool,
    block_id: Option<String>,
    headings: Vec<String>,
}

impl Chunker {
    fn block(&mut self, n: &Node) {
        if n.kind == nodes::HEADING {
            let level = n.attr_i64("level").unwrap_or(1);
            let text = inline_text(n).split_whitespace().collect::<Vec<_>>().join(" ");
            if text.is_empty() {
                return;
            }
            // a heading right under another one (no text between) joins its passage
            if !self.parts.is_empty() && !self.only_headings {
                self.flush();
            }
            while self.stack.last().is_some_and(|(l, _)| *l >= level) {
                self.stack.pop();
            }
            self.stack.push((level, text));
            self.headings = self.stack.iter().map(|(_, t)| t.clone()).collect();
            let md = render(n);
            self.push(md, n, true);
            return;
        }
        let md = render(n);
        if md.is_empty() {
            return;
        }
        if md.len() > MAX_CHARS {
            // headings waiting for their text go with the first piece
            let lead = if self.only_headings { std::mem::take(&mut self.parts).join("\n\n") } else { String::new() };
            let lead_id = self.block_id.take();
            if !self.parts.is_empty() {
                self.flush();
            }
            self.parts.clear();
            self.len = 0;
            let id = lead_id.or_else(|| block_id(n));
            for (i, piece) in split_block(&md, MAX_CHARS).into_iter().enumerate() {
                let markdown = if i == 0 && !lead.is_empty() { format!("{lead}\n\n{piece}") } else { piece };
                self.out.push(Chunk { block_id: id.clone(), headings: self.headings.clone(), markdown });
            }
            self.only_headings = false;
            return;
        }
        if !self.parts.is_empty() && !self.only_headings && self.len + md.len() > TARGET_CHARS {
            self.flush();
        }
        self.push(md, n, false);
    }

    fn push(&mut self, md: String, n: &Node, heading: bool) {
        if self.parts.is_empty() {
            self.only_headings = true;
            self.headings = self.stack.iter().map(|(_, t)| t.clone()).collect();
        }
        if self.block_id.is_none() {
            self.block_id = block_id(n);
        }
        self.only_headings &= heading;
        self.len += md.len() + 2;
        self.parts.push(md);
    }

    fn flush(&mut self) {
        if !self.parts.is_empty() {
            let markdown = std::mem::take(&mut self.parts).join("\n\n");
            self.out.push(Chunk { block_id: self.block_id.take(), headings: self.headings.clone(), markdown });
        }
        self.len = 0;
        self.only_headings = false;
        self.block_id = None;
    }
}

fn block_id(n: &Node) -> Option<String> {
    n.attr_str(blocks::ATTR).filter(|s| !s.is_empty()).map(str::to_string)
}

/// One block as Markdown, images reduced to their description (their URLs mean nothing to a
/// reader of the passage).
fn render(n: &Node) -> String {
    let doc = Node::new(nodes::DOC).with_content(vec![n.clone()]);
    strip_images(pm_to_markdown(&doc).trim())
}

/// `![alt](src "title")` → `[Image: alt]` (dropped without a description).
fn strip_images(md: &str) -> String {
    let mut out = String::with_capacity(md.len());
    let mut rest = md;
    while let Some(start) = rest.find("![") {
        let after = &rest[start + 2..];
        let Some(close) = after.find("](") else { break };
        let alt = &after[..close];
        let tail = &after[close + 2..];
        // the destination ends at the first ')' outside quotes
        let mut end = None;
        let mut quoted = false;
        for (i, ch) in tail.char_indices() {
            match ch {
                '"' => quoted = !quoted,
                ')' if !quoted => {
                    end = Some(i);
                    break;
                }
                '\n' => break,
                _ => {}
            }
        }
        let Some(end) = end else { break };
        out.push_str(&rest[..start]);
        let alt = alt.replace('\\', "");
        if !alt.trim().is_empty() {
            out.push_str(&format!("[Image: {}]", alt.trim()));
        }
        rest = &tail[end + 1..];
    }
    out.push_str(rest);
    out.trim().to_string()
}

/// Cuts one long block between lines into pieces of at most `max` characters. Tables repeat
/// their header row; code blocks get their fences back.
fn split_block(md: &str, max: usize) -> Vec<String> {
    let lines: Vec<&str> = md.lines().collect();
    let (head, foot, body): (Vec<&str>, Option<&str>, &[&str]) = if md.starts_with('|') && lines.len() > 2 {
        (lines[..2].to_vec(), None, &lines[2..])
    } else if (md.starts_with("```") || md.starts_with("~~~")) && lines.len() > 2 {
        let fence: &str = if md.starts_with("```") { "```" } else { "~~~" };
        let close = if lines.last().is_some_and(|l| l.trim_start().starts_with(fence)) { lines.len() - 1 } else { lines.len() };
        (vec![lines[0]], Some(fence), &lines[1..close])
    } else {
        (vec![], None, &lines[..])
    };
    let head_len: usize = head.iter().map(|l| l.len() + 1).sum::<usize>() + foot.map_or(0, |f| f.len() + 1);
    let room = max.saturating_sub(head_len).max(max / 2);
    let mut pieces: Vec<Vec<String>> = Vec::new();
    let mut cur: Vec<String> = Vec::new();
    let mut len = 0;
    for line in body {
        let parts = if line.len() > room { split_long_line(line, room) } else { vec![line.to_string()] };
        for p in parts {
            if !cur.is_empty() && len + p.len() + 1 > room {
                pieces.push(std::mem::take(&mut cur));
                len = 0;
            }
            len += p.len() + 1;
            cur.push(p);
        }
    }
    if !cur.is_empty() {
        pieces.push(cur);
    }
    pieces
        .into_iter()
        .map(|p| {
            let mut s: Vec<String> = head.iter().map(|l| l.to_string()).collect();
            s.extend(p);
            if let Some(f) = foot {
                s.push(f.to_string());
            }
            s.join("\n").trim().to_string()
        })
        .filter(|s| !s.is_empty())
        .collect()
}

/// Cuts a long line at sentence ends (else spaces, else anywhere) into pieces of at most `max`
/// bytes, on character boundaries.
fn split_long_line(line: &str, max: usize) -> Vec<String> {
    let mut out = Vec::new();
    let mut rest = line.trim();
    while rest.len() > max {
        let mut cut = max;
        while !rest.is_char_boundary(cut) {
            cut -= 1;
        }
        let window = &rest[..cut];
        let at = window
            .rfind(". ")
            .or_else(|| window.rfind("? "))
            .or_else(|| window.rfind("! "))
            .filter(|i| *i > max / 3)
            .map(|i| i + 1)
            .or_else(|| window.rfind(' ').filter(|i| *i > max / 3))
            .unwrap_or(cut);
        out.push(rest[..at].trim().to_string());
        rest = rest[at..].trim_start();
    }
    if !rest.is_empty() {
        out.push(rest.to_string());
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::markdown::markdown_to_pm;

    fn doc(md: &str) -> Node {
        let mut d = markdown_to_pm(md).doc;
        blocks::ensure_ids(&mut d);
        d
    }

    #[test]
    fn passages_follow_headings() {
        let d = doc("Intro text.\n\n# Setup\n\n## Database\n\nCreate the database.\n\n## Mail\n\nSet SMTP_URL.\n\n# Use\n\nOpen the wiki.");
        let c = chunk_doc(&d);
        let got: Vec<(Vec<&str>, &str)> = c.iter().map(|c| (c.headings.iter().map(String::as_str).collect(), c.markdown.as_str())).collect();
        assert_eq!(
            got,
            vec![
                (vec![], "Intro text."),
                (vec!["Setup", "Database"], "# Setup\n\n## Database\n\nCreate the database."),
                (vec!["Setup", "Mail"], "## Mail\n\nSet SMTP\\_URL."),
                (vec!["Use"], "# Use\n\nOpen the wiki."),
            ]
        );
        // every passage links to the block it starts at
        let first_ids: Vec<Option<String>> = d.content.iter().map(|b| b.attr_str(blocks::ATTR).map(str::to_string)).collect();
        assert_eq!(c[0].block_id, first_ids[0]);
        assert_eq!(c[1].block_id, first_ids[1], "the passage starts at the Setup heading");
        assert!(c.iter().all(|c| c.block_id.is_some()));
    }

    #[test]
    fn long_sections_are_cut_between_blocks() {
        let para = "Lorem ipsum dolor sit amet, consectetur adipiscing elit. ".repeat(6);
        let md = format!("# Policy\n\n{}", [para.trim(); 8].join("\n\n"));
        let c = chunk_doc(&doc(&md));
        assert!(c.len() >= 3, "{c:?}");
        assert!(c.iter().all(|c| c.headings == ["Policy"] && c.markdown.len() <= TARGET_CHARS + para.len() + 2));
        assert!(c[0].markdown.starts_with("# Policy"));
        let ids: std::collections::HashSet<_> = c.iter().map(|c| c.block_id.clone()).collect();
        assert_eq!(ids.len(), c.len(), "each piece links to its own first block");
    }

    #[test]
    fn big_tables_repeat_their_header() {
        let rows: String = (0..200).map(|i| format!("| host-{i} | 10.0.0.{i} | running |\n")).collect();
        let md = format!("## Servers\n\n| Name | IP | State |\n|---|---|---|\n{rows}");
        let c = chunk_doc(&doc(&md));
        assert!(c.len() > 2);
        for p in &c {
            assert!(p.markdown.len() <= MAX_CHARS + 40, "{}", p.markdown.len());
            assert!(p.markdown.contains("| Name | IP | State |"), "{}", p.markdown);
            assert_eq!(p.headings, ["Servers"]);
        }
        assert!(c[0].markdown.starts_with("## Servers"), "the heading goes with the first piece");
        assert!(c.iter().any(|p| p.markdown.contains("host-199")));
    }

    #[test]
    fn big_code_blocks_keep_fences() {
        let code: String = (0..300).map(|i| format!("let x{i} = compute({i});\n")).collect();
        let c = chunk_doc(&doc(&format!("```rust\n{code}```")));
        assert!(c.len() > 1);
        for p in &c {
            assert!(p.markdown.starts_with("```rust\n") && p.markdown.ends_with("\n```"), "{}", p.markdown);
        }
    }

    #[test]
    fn images_become_descriptions() {
        assert_eq!(strip_images("See ![Network diagram](/api/attachments/1/content \"Fig (1)\") here"), "See [Image: Network diagram] here");
        assert_eq!(strip_images("![](x.png)\n\ntext"), "text");
        assert_eq!(strip_images("no ![image"), "no ![image");
    }

    #[test]
    fn plain_text_passages() {
        let text = format!("Title\n\n{}\n{}", "a".repeat(1000), "b ".repeat(2000));
        let c = chunk_text(&text);
        assert!(c.iter().all(|c| c.len() <= MAX_CHARS));
        assert!(c[0].starts_with("Title"));
        assert_eq!(c.concat().matches('b').count(), 2000);
        assert!(chunk_text("  \n ").is_empty());
    }
}
