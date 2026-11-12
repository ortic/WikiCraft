//! Included pages in exports: every include block gets the content it shows in the page (the
//! other page, or its excerpt), for the person exporting. Pages they can't open stay a link;
//! loops and more than [`MAX_DEPTH`] levels stop; at most [`MAX_PAGES`] pages per export.

use std::collections::HashMap;

use uuid::Uuid;
use wikicraft_doc_model::{Node, nodes};

use crate::app::AppState;
use crate::authz::{Need, Principal, require_page};

/// Levels of pages inside pages (the editor stops at the same depth).
pub const MAX_DEPTH: usize = 4;
/// Included pages read per export.
pub const MAX_PAGES: usize = 50;

/// What an include shows of its page: everything, or an excerpt (`true`/`"true"`: the first,
/// a name: that one, else the first).
fn selector(n: &Node) -> Option<Option<String>> {
    match n.attrs.get("excerpt") {
        None | Some(serde_json::Value::Null) | Some(serde_json::Value::Bool(false)) => None,
        Some(serde_json::Value::String(s)) if s != "true" && !s.trim().is_empty() => Some(Some(s.trim().to_lowercase())),
        Some(_) => Some(None),
    }
}

/// The blocks an include of `doc` shows.
pub fn shown(doc: &Node, excerpt: Option<Option<String>>) -> Vec<Node> {
    let Some(name) = excerpt else { return doc.content.clone() };
    let mut all: Vec<&Node> = Vec::new();
    fn walk<'a>(n: &'a Node, out: &mut Vec<&'a Node>) {
        if n.kind == nodes::EXCERPT {
            out.push(n);
        } else {
            n.content.iter().for_each(|c| walk(c, out));
        }
    }
    walk(doc, &mut all);
    let named = name.as_deref().and_then(|w| all.iter().find(|e| e.attr_str("name").is_some_and(|x| x.trim().to_lowercase() == w)));
    if let Some(e) = named.or(all.first()) {
        return e.content.clone();
    }
    // no excerpt marked: the first paragraph with text stands in (as in the editor)
    let has_text = |p: &Node| !wikicraft_doc_model::text::plain_text(p).trim().is_empty();
    doc.content.iter().find(|p| p.kind == nodes::PARAGRAPH && has_text(p)).cloned().into_iter().collect()
}

fn targets(doc: &Node) -> Vec<Uuid> {
    let mut ids = Vec::new();
    doc.walk(&mut |n| {
        if n.kind == nodes::INCLUDE
            && let Some(id) = n.attr_str("pageId").and_then(|s| s.parse::<Uuid>().ok())
            && !ids.contains(&id)
        {
            ids.push(id);
        }
    });
    ids
}

async fn load(state: &AppState, id: Uuid) -> Option<Node> {
    if let Some(r) = state.rooms.get(id) {
        return Some(r.snapshot());
    }
    let content: serde_json::Value =
        sqlx::query_scalar("SELECT content FROM pages WHERE id = $1 AND deleted_at IS NULL").bind(id).fetch_optional(&state.db).await.ok()??;
    Node::from_json(&content).ok()
}

/// Fills the include blocks of `doc` (the content of `page`) with what they show to `p`.
pub async fn expand(state: &AppState, p: &Principal, page: Uuid, doc: &mut Node) {
    let mut budget = MAX_PAGES;
    let mut cache: HashMap<Uuid, Option<Node>> = HashMap::new();
    expand_in(state, p, &[page], doc, &mut budget, &mut cache).await;
}

fn expand_in<'a>(
    state: &'a AppState,
    p: &'a Principal,
    chain: &'a [Uuid],
    doc: &'a mut Node,
    budget: &'a mut usize,
    cache: &'a mut HashMap<Uuid, Option<Node>>,
) -> std::pin::Pin<Box<dyn std::future::Future<Output = ()> + Send + 'a>> {
    Box::pin(async move {
        if chain.len() > MAX_DEPTH {
            return;
        }
        // each included page once, already expanded for its own level
        let mut pages: HashMap<Uuid, Node> = HashMap::new();
        for id in targets(doc) {
            if chain.contains(&id) {
                continue;
            }
            let loaded = match cache.get(&id) {
                Some(hit) => hit.clone(),
                None => {
                    let allowed = require_page(state, p, id, Need::View).await.is_ok();
                    let node = if allowed && *budget > 0 {
                        *budget -= 1;
                        load(state, id).await
                    } else {
                        None
                    };
                    cache.insert(id, node.clone());
                    node
                }
            };
            let Some(mut inner) = loaded else { continue };
            let deeper: Vec<Uuid> = chain.iter().copied().chain([id]).collect();
            expand_in(state, p, &deeper, &mut inner, budget, cache).await;
            pages.insert(id, inner);
        }
        doc.walk_mut(&mut |n| {
            if n.kind != nodes::INCLUDE || !n.content.is_empty() {
                return;
            }
            let Some(inner) = n.attr_str("pageId").and_then(|s| s.parse::<Uuid>().ok()).and_then(|id| pages.get(&id)) else { return };
            n.content = shown(inner, selector(n));
        });
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn doc(v: serde_json::Value) -> Node {
        Node::from_json(&v).unwrap()
    }

    #[test]
    fn picks_the_excerpt() {
        let page = doc(json!({"type": "doc", "content": [
            {"type": "paragraph", "content": [{"type": "text", "text": "Intro"}]},
            {"type": "excerpt", "content": [{"type": "paragraph", "content": [{"type": "text", "text": "First"}]}]},
            {"type": "excerpt", "attrs": {"name": "Pricing"}, "content": [{"type": "paragraph", "content": [{"type": "text", "text": "Five"}]}]}
        ]}));
        let text = |blocks: Vec<Node>| blocks.iter().map(wikicraft_doc_model::text::plain_text).collect::<Vec<_>>().join("|");
        assert_eq!(text(shown(&page, None)).matches('|').count(), 2, "the whole page");
        assert_eq!(text(shown(&page, Some(None))), "First");
        assert_eq!(text(shown(&page, Some(Some("pricing".into())))), "Five");
        assert_eq!(text(shown(&page, Some(Some("nope".into())))), "First");
        let plain = doc(
            json!({"type": "doc", "content": [{"type": "heading", "content": [{"type": "text", "text": "H"}]}, {"type": "paragraph"}, {"type": "paragraph", "content": [{"type": "text", "text": "Lead"}]}]}),
        );
        assert_eq!(text(shown(&plain, Some(None))), "Lead");
        let n = |v: serde_json::Value| selector(&Node::new(nodes::INCLUDE).attr("excerpt", v));
        assert_eq!((n(json!(true)), n(json!("true")), n(json!(" Pricing ")), n(json!(false))), (Some(None), Some(None), Some(Some("pricing".into())), None));
    }
}
