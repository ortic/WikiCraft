//! Text extraction from attachments for full-text search (PDF, Office Open XML, OpenDocument,
//! plain text). Runs in the background with bounded concurrency; parser panics are contained.

use std::io::Read;

use tokio::sync::Semaphore;
use uuid::Uuid;

use crate::app::AppState;

const MAX_BYTES: usize = 50 * 1024 * 1024;
const MAX_TEXT: usize = 200_000;
static SLOTS: Semaphore = Semaphore::const_new(2);

/// Extracts searchable text; `None` if the type isn't supported or extraction failed.
pub fn extract(media_type: &str, filename: &str, data: &[u8]) -> Option<String> {
    if data.len() > MAX_BYTES {
        return None;
    }
    let lower = filename.to_lowercase();
    let ext = lower.rsplit('.').next().unwrap_or("");
    let text = if media_type == "application/pdf" || ext == "pdf" {
        // pdf parsers can panic on malformed files: contain it
        std::panic::catch_unwind(|| pdf_extract::extract_text_from_mem(data).ok()).ok().flatten()
    } else if matches!(ext, "docx" | "docm" | "dotx") {
        zip_xml(data, |n| n == "word/document.xml" || n.starts_with("word/header") || n.starts_with("word/footer"), &["w:t"], &["w:p"])
    } else if matches!(ext, "pptx" | "ppsx") {
        zip_xml(data, |n| n.starts_with("ppt/slides/slide") && n.ends_with(".xml"), &["a:t"], &["a:p"])
    } else if matches!(ext, "xlsx" | "xlsm") {
        zip_xml(data, |n| n == "xl/sharedStrings.xml", &["t"], &["si"])
    } else if matches!(ext, "odt" | "ods" | "odp") {
        zip_xml(data, |n| n == "content.xml", &["text:p", "text:h", "text:span"], &["text:p", "text:h"])
    } else if media_type.starts_with("text/")
        || matches!(media_type, "application/json" | "application/xml" | "application/x-yaml")
        || matches!(ext, "md" | "txt" | "csv" | "log" | "json" | "yaml" | "yml" | "xml")
    {
        Some(String::from_utf8_lossy(data).to_string())
    } else {
        None
    }?;
    let cleaned: String = text.split_whitespace().collect::<Vec<_>>().join(" ");
    Some(cleaned.chars().take(MAX_TEXT).collect())
}

/// Collects the text content of `text_tags` elements in matching zip entries; `break_tags` end a
/// line (paragraphs).
fn zip_xml(data: &[u8], want: impl Fn(&str) -> bool, text_tags: &[&str], break_tags: &[&str]) -> Option<String> {
    let mut zip = zip::ZipArchive::new(std::io::Cursor::new(data)).ok()?;
    let mut names: Vec<String> = zip.file_names().flatten().filter(|n| want(n)).map(|n| n.to_string()).collect();
    names.sort_by_key(|n| (n.len(), n.clone())); // slide2 before slide10
    let mut out = String::new();
    for name in names {
        let mut xml = String::new();
        let entry = zip.by_name(&name).ok()?;
        entry.take(MAX_BYTES as u64).read_to_string(&mut xml).ok()?;
        let mut reader = quick_xml::Reader::from_str(&xml);
        let mut depth_in_text = 0usize;
        loop {
            match reader.read_event() {
                Ok(quick_xml::events::Event::Start(e)) => {
                    if text_tags.iter().any(|t| e.name().as_ref() == *t) {
                        depth_in_text += 1;
                    }
                }
                Ok(quick_xml::events::Event::End(e)) => {
                    let n = e.name();
                    if text_tags.iter().any(|t| n.as_ref() == *t) {
                        depth_in_text = depth_in_text.saturating_sub(1);
                    }
                    if break_tags.iter().any(|t| n.as_ref() == *t) {
                        out.push('\n');
                    }
                }
                Ok(quick_xml::events::Event::Text(t)) if depth_in_text > 0 => out.push_str(&t.into_inner()),
                Ok(quick_xml::events::Event::GeneralRef(r)) if depth_in_text > 0 => {
                    if let Ok(Some(c)) = r.resolve_char_ref() {
                        out.push(c);
                    } else {
                        match r.into_inner().as_ref() {
                            "amp" => out.push('&'),
                            "lt" => out.push('<'),
                            "gt" => out.push('>'),
                            "quot" => out.push('"'),
                            "apos" => out.push('\''),
                            _ => {}
                        }
                    }
                }
                Ok(quick_xml::events::Event::Eof) | Err(_) => break,
                _ => {}
            }
            if out.len() > MAX_TEXT * 4 {
                break;
            }
        }
        out.push('\n');
    }
    Some(out)
}

/// Extracts text for an attachment in the background and reindexes its page.
pub fn spawn(state: &AppState, attachment_id: Uuid) {
    let state = state.clone();
    tokio::spawn(async move {
        let Ok(_slot) = SLOTS.acquire().await else { return };
        let row: Option<(Uuid, String, String, Option<String>)> =
            sqlx::query_as("SELECT page_id, filename, media_type, blob_key FROM attachments WHERE id = $1")
                .bind(attachment_id)
                .fetch_optional(&state.db)
                .await
                .ok()
                .flatten();
        let Some((page_id, filename, media_type, Some(key))) = row else { return };
        let Ok(bytes) = state.blobs.get(&key).await else { return };
        let text = tokio::task::spawn_blocking(move || extract(&media_type, &filename, &bytes)).await.ok().flatten();
        if let Some(text) = text {
            let _ = sqlx::query("UPDATE attachments SET text_content = $2 WHERE id = $1").bind(attachment_id).bind(text).execute(&state.db).await;
            state.search.enqueue(page_id);
        }
    });
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;

    #[test]
    fn extracts_docx_text() {
        let mut buf = std::io::Cursor::new(Vec::new());
        {
            let mut z = zip::ZipWriter::new(&mut buf);
            z.start_file("word/document.xml", zip::write::SimpleFileOptions::default()).unwrap();
            z.write_all(br#"<w:document><w:body><w:p><w:r><w:t>Quarterly</w:t></w:r><w:r><w:t xml:space="preserve"> report &amp; plan</w:t></w:r></w:p><w:p><w:r><w:t>Second</w:t></w:r></w:p></w:body></w:document>"#).unwrap();
            z.finish().unwrap();
        }
        let t = extract("application/vnd.openxmlformats-officedocument.wordprocessingml.document", "r.docx", &buf.into_inner()).unwrap();
        assert_eq!(t, "Quarterly report & plan Second");
    }

    #[test]
    fn garbage_pdf_does_not_panic() {
        // the result doesn't matter; the call must not panic or hang
        let _ = extract("application/pdf", "x.pdf", b"%PDF-1.4 garbage");
    }
}
