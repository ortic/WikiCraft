//! Shared by the export tests.

use wikicraft_doc_model::{Node, nodes};

/// Structural checks the editor schema relies on.
pub fn validate(n: &Node, path: &str, errors: &mut Vec<String>) {
    let kind = n.kind.as_str();
    match kind {
        nodes::PARAGRAPH | nodes::HEADING => {
            if let Some(bad) = n.content.iter().find(|c| !c.is_inline()) {
                errors.push(format!("{path}: block '{}' inside {kind}", bad.kind));
            }
        }
        nodes::BULLET_LIST | nodes::ORDERED_LIST => {
            if let Some(bad) = n.content.iter().find(|c| c.kind != nodes::LIST_ITEM) {
                errors.push(format!("{path}: '{}' inside list", bad.kind));
            }
        }
        nodes::TABLE => {
            if let Some(bad) = n.content.iter().find(|c| c.kind != nodes::TABLE_ROW) {
                errors.push(format!("{path}: '{}' inside table", bad.kind));
            }
        }
        nodes::DOC | nodes::BLOCKQUOTE | nodes::LIST_ITEM | nodes::TABLE_CELL | nodes::TABLE_HEADER | nodes::CALLOUT => {
            if let Some(bad) = n.content.iter().find(|c| c.is_inline()) {
                errors.push(format!("{path}: inline '{}' directly inside {kind}", bad.kind));
            }
            if n.content.is_empty() {
                errors.push(format!("{path}: empty {kind}"));
            }
        }
        nodes::TEXT if n.text.as_deref().is_none_or(str::is_empty) => {
            errors.push(format!("{path}: empty text node"));
        }
        _ => {}
    }
    for (i, c) in n.content.iter().enumerate() {
        validate(c, &format!("{path}/{i}:{}", c.kind), errors);
    }
}
