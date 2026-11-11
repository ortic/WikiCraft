//! The link graph: which pages each page links to, kept in `page_links`.
//!
//! Links are taken from the page body (link marks and link attributes pointing at
//! `/pages/<id>`, also as absolute URLs on this wiki, included pages) and from page-link
//! properties. Links that were made to pages which did not exist (Confluence imports, see
//! [`crate::importer`]: `/missing?space=KEY&title=T` and `/missing?source=confluence:ID`) keep
//! the space and title or the source id, so they count once such a page exists. A body that
//! lists its child pages (`childPages`) gets a `children` row, so those children aren't orphans.
//!
//! The graph is refreshed together with the search index ([`crate::search`]), which every change
//! to a page passes through, and filled in the background at startup for pages that were never
//! scanned (and for all pages after the extractor changes). It powers "Linked from" on pages and
//! the broken-link and orphaned-page reports of a space ([`crate::routes::links`]).

use std::collections::HashSet;

use serde_json::Value;
use sqlx::PgPool;
use uuid::Uuid;
use wikicraft_doc_model::{Node, marks, nodes};

/// Bumped when link extraction changes: every page is scanned again at the next start.
pub const LINKER_VERSION: i32 = 1;
/// links per page beyond this are not recorded
const MAX_LINKS: usize = 2000;
/// characters of link text kept for reports
const MAX_LABEL: usize = 200;

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub enum Target {
    Page(Uuid),
    /// a page that didn't exist when the link was made
    Missing {
        space: String,
        title: String,
    },
    /// a page of another system that wasn't imported (`confluence:<id>`)
    Source(String),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Kind {
    Link,
    Include,
    Property,
    /// the body lists the page's children (`childPages`); stored with the page itself as target
    Children,
}

impl Kind {
    pub fn as_str(self) -> &'static str {
        match self {
            Kind::Link => "link",
            Kind::Include => "include",
            Kind::Property => "property",
            Kind::Children => "children",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Link {
    pub kind: Kind,
    pub target: Target,
    pub block_id: Option<String>,
    pub label: String,
}

/// The page a URL points at, if it is a link into this wiki: `/pages/<id>[#…|?…|/…]`, the same
/// on `base_url`, or a `/missing?…` link.
pub fn parse_href(href: &str, base_url: &str) -> Option<Target> {
    let href = href.trim();
    let base = base_url.trim_end_matches('/');
    let path = match href.strip_prefix(base) {
        Some(rest) if !base.is_empty() && (rest.starts_with('/') || rest.is_empty()) => rest,
        _ => href,
    };
    if let Some(rest) = path.strip_prefix("/pages/") {
        let id = rest.split(['#', '?', '/']).next()?;
        return id.parse().ok().map(Target::Page);
    }
    let query = path.strip_prefix("/missing?")?;
    let query = query.split('#').next().unwrap_or(query);
    let (mut space, mut title, mut source) = (None, None, None);
    for pair in query.split('&') {
        let (k, v) = pair.split_once('=').unwrap_or((pair, ""));
        let v = crate::importer::decode_component(v);
        match k {
            "space" => space = Some(v),
            "title" => title = Some(v),
            "source" => source = Some(v),
            _ => {}
        }
    }
    match (source, space, title) {
        (Some(s), _, _) if !s.trim().is_empty() => Some(Target::Source(s.trim().to_string())),
        (_, Some(space), Some(title)) if !title.trim().is_empty() => Some(Target::Missing { space: space.trim().to_string(), title: title.trim().to_string() }),
        _ => None,
    }
}

/// The links in a document, in document order (duplicates included).
pub fn extract(doc: &Node, base_url: &str) -> Vec<Link> {
    let mut out = Vec::new();
    visit(doc, None, base_url, &mut out);
    out
}

fn visit(n: &Node, block: Option<&str>, base: &str, out: &mut Vec<Link>) {
    let block = n.attr_str("blockId").or(block);
    if n.kind == nodes::CHILD_PAGES {
        // the page's own id isn't known here: nil stands for it
        out.push(Link { kind: Kind::Children, target: Target::Page(Uuid::nil()), block_id: block.map(String::from), label: String::new() });
    }
    if n.kind == nodes::INCLUDE
        && let Some(id) = n.attr_str("pageId").and_then(|s| s.parse::<Uuid>().ok())
    {
        out.push(Link {
            kind: Kind::Include,
            target: Target::Page(id),
            block_id: block.map(String::from),
            label: n.attr_str("title").unwrap_or("").to_string(),
        });
    }
    // link cards, embeds, buttons
    if n.kind != nodes::IMAGE {
        for key in ["href", "url"] {
            if let Some(t) = n.attr_str(key).and_then(|u| parse_href(u, base)) {
                let label = n.attr_str("title").or(n.attr_str("text")).unwrap_or("").to_string();
                out.push(Link { kind: Kind::Link, target: t, block_id: block.map(String::from), label });
            }
        }
    }
    // link marks; a link spanning several text nodes (bold part, …) is one link
    let mut open: Option<usize> = None;
    for c in &n.content {
        let target = c.marks.iter().find(|m| m.kind == marks::LINK).and_then(|m| m.attr_str("href")).and_then(|h| parse_href(h, base));
        let text = c.text.as_deref().or(c.attr_str("alt")).unwrap_or("");
        match target {
            Some(t) => match open {
                Some(i) if out[i].target == t => out[i].label.push_str(text),
                _ => {
                    out.push(Link { kind: Kind::Link, target: t, block_id: c.attr_str("blockId").or(block).map(String::from), label: text.to_string() });
                    open = Some(out.len() - 1);
                }
            },
            None => open = None,
        }
        if !c.content.is_empty() || !c.attrs.is_empty() {
            open = None;
            visit(c, block, base, out);
        }
    }
}

/// One row of `page_links`: (ord, kind, target page, missing space, missing title, source, block, label).
type Row = (i32, &'static str, Option<Uuid>, Option<String>, Option<String>, Option<String>, Option<String>, String);

/// The rows to store for a page: one per distinct (kind, target), self-links left out.
fn rows_for(page: Uuid, links: Vec<Link>) -> Vec<Row> {
    let mut seen: HashSet<(Kind, Target)> = HashSet::new();
    let mut rows = Vec::new();
    for mut l in links {
        if l.kind == Kind::Children {
            l.target = Target::Page(page);
        } else if l.target == Target::Page(page) {
            continue;
        }
        if !seen.insert((l.kind, l.target.clone())) {
            continue;
        }
        let label: String = l.label.split_whitespace().collect::<Vec<_>>().join(" ").chars().take(MAX_LABEL).collect();
        let (id, space, title, source) = match l.target {
            Target::Page(id) => (Some(id), None, None, None),
            Target::Missing { space, title } => (None, Some(space), Some(title), None),
            Target::Source(s) => (None, None, None, Some(s)),
        };
        rows.push((rows.len() as i32, l.kind.as_str(), id, space, title, source, l.block_id, label));
        if rows.len() >= MAX_LINKS {
            break;
        }
    }
    rows
}

/// Extracts the links of the given pages again.
pub async fn relink(db: &PgPool, ids: &[Uuid], base_url: &str) -> anyhow::Result<()> {
    for batch in ids.chunks(100) {
        relink_batch(db, batch, base_url).await?;
    }
    Ok(())
}

async fn relink_batch(db: &PgPool, ids: &[Uuid], base_url: &str) -> anyhow::Result<()> {
    let pages: Vec<(Uuid, Value)> = sqlx::query_as("SELECT id, content FROM pages WHERE id = ANY($1)").bind(ids).fetch_all(db).await?;
    let props: Vec<(Uuid, String, Value)> = sqlx::query_as(
        "SELECT page_id, key, value FROM page_properties WHERE page_id = ANY($1) AND kind = 'page' AND value IS NOT NULL ORDER BY page_id, position",
    )
    .bind(ids)
    .fetch_all(db)
    .await?;
    let base = base_url.to_string();
    let rows: Vec<(Uuid, Vec<Row>)> = tokio::task::spawn_blocking(move || {
        pages
            .into_iter()
            .map(|(id, content)| {
                let doc = Node::from_json(&content).unwrap_or_else(|_| Node::empty_doc());
                let mut links = extract(&doc, &base);
                for (_, key, value) in props.iter().filter(|(p, _, _)| *p == id) {
                    if let Some(t) = value.as_str().and_then(|s| s.parse::<Uuid>().ok()) {
                        links.push(Link { kind: Kind::Property, target: Target::Page(t), block_id: None, label: key.clone() });
                    }
                }
                (id, rows_for(id, links))
            })
            .collect()
    })
    .await?;
    let mut tx = db.begin().await?;
    sqlx::query("DELETE FROM page_links WHERE source_id = ANY($1)").bind(ids).execute(&mut *tx).await?;
    let flat: Vec<(Uuid, &Row)> = rows.iter().flat_map(|(id, rs)| rs.iter().map(move |r| (*id, r))).collect();
    for part in flat.chunks(1000) {
        let mut qb =
            sqlx::QueryBuilder::new("INSERT INTO page_links (source_id, ord, kind, target_id, missing_space, missing_title, missing_source, block_id, label) ");
        qb.push_values(part, |mut b, (id, r)| {
            b.push_bind(*id).push_bind(r.0).push_bind(r.1).push_bind(r.2).push_bind(&r.3).push_bind(&r.4).push_bind(&r.5).push_bind(&r.6).push_bind(&r.7);
        });
        // a concurrent scan of the same page (another replica) may have written first
        qb.push(" ON CONFLICT (source_id, ord) DO NOTHING");
        qb.build().execute(&mut *tx).await?;
    }
    sqlx::query("UPDATE pages SET linked_at = now() WHERE id = ANY($1)").bind(ids).execute(&mut *tx).await?;
    tx.commit().await?;
    Ok(())
}

/// At startup: scan everything again after an extractor change, then the pages that never were
/// (new installs, upgrades), in the background.
pub fn spawn_backfill(db: PgPool, base_url: String) {
    tokio::spawn(async move {
        if let Err(e) = backfill(&db, &base_url).await {
            tracing::error!("extracting page links failed: {e:#}");
        }
    });
}

async fn backfill(db: &PgPool, base_url: &str) -> anyhow::Result<()> {
    let version: i32 = sqlx::query_scalar("SELECT linker FROM retrieval_state").fetch_one(db).await?;
    if version != LINKER_VERSION {
        sqlx::query("UPDATE pages SET linked_at = NULL").execute(db).await?;
        sqlx::query("UPDATE retrieval_state SET linker = $1").bind(LINKER_VERSION).execute(db).await?;
    }
    let mut last = Uuid::nil();
    loop {
        let ids: Vec<Uuid> =
            sqlx::query_scalar("SELECT id FROM pages WHERE linked_at IS NULL AND id > $1 ORDER BY id LIMIT 200").bind(last).fetch_all(db).await?;
        let Some(l) = ids.last() else { return Ok(()) };
        last = *l;
        relink(db, &ids, base_url).await?;
    }
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;

    const BASE: &str = "https://wiki.example.com";

    fn doc(v: Value) -> Node {
        Node::from_json(&v).unwrap()
    }

    #[test]
    fn parses_wiki_links() {
        let id = Uuid::now_v7();
        assert_eq!(parse_href(&format!("/pages/{id}"), BASE), Some(Target::Page(id)));
        assert_eq!(parse_href(&format!("/pages/{id}#block-x1"), BASE), Some(Target::Page(id)));
        assert_eq!(parse_href(&format!("/pages/{id}/history"), BASE), Some(Target::Page(id)));
        assert_eq!(parse_href(&format!("{BASE}/pages/{id}?v=2"), BASE), Some(Target::Page(id)));
        assert_eq!(parse_href(&format!("{BASE}/pages/{id}"), &format!("{BASE}/")), Some(Target::Page(id)));
        assert_eq!(parse_href(&format!("https://elsewhere.example.com/pages/{id}"), BASE), None);
        assert_eq!(parse_href("/pages/not-an-id", BASE), None);
        assert_eq!(parse_href("https://example.com", BASE), None);
        assert_eq!(
            parse_href("/missing?space=ENG&title=Release%20Notes+2", BASE),
            Some(Target::Missing { space: "ENG".into(), title: "Release Notes 2".into() })
        );
        assert_eq!(parse_href("/missing?source=confluence%3A123", BASE), Some(Target::Source("confluence:123".into())));
        assert_eq!(parse_href("/missing?space=ENG", BASE), None);
    }

    #[test]
    fn extracts_links_with_blocks_and_text() {
        let (a, b, c) = (Uuid::now_v7(), Uuid::now_v7(), Uuid::now_v7());
        let d = doc(json!({"type": "doc", "content": [
            {"type": "paragraph", "attrs": {"blockId": "p1"}, "content": [
                {"type": "text", "text": "see "},
                {"type": "text", "text": "the ", "marks": [{"type": "link", "attrs": {"href": format!("/pages/{a}")}}]},
                {"type": "text", "text": "guide", "marks": [{"type": "link", "attrs": {"href": format!("/pages/{a}")}}, {"type": "bold"}]},
                {"type": "text", "text": " and "},
                {"type": "text", "text": "elsewhere", "marks": [{"type": "link", "attrs": {"href": "https://example.com"}}]},
            ]},
            {"type": "bulletList", "attrs": {"blockId": "l1"}, "content": [{"type": "listItem", "content": [{"type": "paragraph", "content": [
                {"type": "text", "text": "Old page", "marks": [{"type": "link", "attrs": {"href": "/missing?space=OPS&title=Old%20page"}}]},
            ]}]}]},
            {"type": "include", "attrs": {"blockId": "i1", "pageId": b.to_string(), "title": "Shared"}},
            {"type": "embed", "attrs": {"blockId": "k1", "url": format!("{BASE}/pages/{c}"), "title": "Card"}},
            {"type": "childPages", "attrs": {"blockId": "c1", "depth": 1}},
        ]}));
        let links = extract(&d, BASE);
        assert_eq!(
            links,
            vec![
                Link { kind: Kind::Link, target: Target::Page(a), block_id: Some("p1".into()), label: "the guide".into() },
                Link {
                    kind: Kind::Link,
                    target: Target::Missing { space: "OPS".into(), title: "Old page".into() },
                    block_id: Some("l1".into()),
                    label: "Old page".into()
                },
                Link { kind: Kind::Include, target: Target::Page(b), block_id: Some("i1".into()), label: "Shared".into() },
                Link { kind: Kind::Link, target: Target::Page(c), block_id: Some("k1".into()), label: "Card".into() },
                Link { kind: Kind::Children, target: Target::Page(Uuid::nil()), block_id: Some("c1".into()), label: String::new() },
            ]
        );
    }

    #[test]
    fn rows_skip_duplicates_and_self_links() {
        let (me, a) = (Uuid::now_v7(), Uuid::now_v7());
        let l = |t: Target, kind: Kind| Link { kind, target: t, block_id: None, label: "x".into() };
        let rows = rows_for(
            me,
            vec![
                l(Target::Page(a), Kind::Link),
                l(Target::Page(me), Kind::Link),
                l(Target::Page(a), Kind::Link),
                l(Target::Page(a), Kind::Include),
                l(Target::Page(Uuid::nil()), Kind::Children),
                l(Target::Page(Uuid::nil()), Kind::Children),
            ],
        );
        assert_eq!(rows.len(), 3);
        assert_eq!((rows[0].0, rows[0].1, rows[0].2), (0, "link", Some(a)));
        assert_eq!((rows[1].0, rows[1].1, rows[1].2), (1, "include", Some(a)));
        assert_eq!((rows[2].0, rows[2].1, rows[2].2), (2, "children", Some(me)));
    }
}
