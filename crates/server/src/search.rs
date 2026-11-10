//! Full-text search with tantivy (embedded). Pages are (re)indexed asynchronously through a
//! channel; queries are filtered by the spaces a user may see and then by page restrictions.

use std::collections::HashSet;
use std::path::Path;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use serde::Serialize;
use sqlx::PgPool;
use tantivy::collector::TopDocs;
use tantivy::query::{BooleanQuery, Occur, Query, QueryParser, TermQuery};
use tantivy::schema::{Field, IndexRecordOption, STORED, STRING, Schema, TEXT, Value};
use tantivy::snippet::SnippetGenerator;
use tantivy::{Index, IndexReader, IndexWriter, ReloadPolicy, TantivyDocument, Term, doc};
use tokio::sync::mpsc;
use uuid::Uuid;

/// Search over pages. Backends:
/// * `tantivy` (default): embedded index on local disk — fastest, single node.
/// * `postgres`: full-text index in the database (`pages.search_vec`) — shared by all replicas,
///   no extra infrastructure; use it when running more than one instance.
#[derive(Clone)]
pub struct SearchIndex {
    inner: Arc<Inner>,
}

struct Inner {
    backend: Backend,
    db: PgPool,
    tx: mpsc::UnboundedSender<Uuid>,
}

enum Backend {
    Tantivy(Box<Tantivy>),
    Postgres,
}

struct Tantivy {
    index: Index,
    reader: IndexReader,
    writer: Mutex<IndexWriter>,
    f: Fields,
}

#[derive(Clone, Copy)]
struct Fields {
    id: Field,
    space: Field,
    title: Field,
    body: Field,
    labels: Field,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Hit {
    pub page_id: Uuid,
    pub space_id: Uuid,
    pub title: String,
    pub snippet: String,
    pub score: f32,
}

#[derive(sqlx::FromRow)]
struct IndexRow {
    id: Uuid,
    space_id: Uuid,
    title: String,
    text_content: String,
    deleted: bool,
    labels: Option<String>,
    attachments: Option<String>,
    comments: Option<String>,
}

impl Tantivy {
    fn open(dir: &Path) -> anyhow::Result<Self> {
        std::fs::create_dir_all(dir)?;
        let mut sb = Schema::builder();
        let id = sb.add_text_field("id", STRING | STORED);
        let space = sb.add_text_field("space", STRING | STORED);
        let title = sb.add_text_field("title", TEXT | STORED);
        let body = sb.add_text_field("body", TEXT | STORED);
        let labels = sb.add_text_field("labels", TEXT);
        let schema = sb.build();
        let mmap = tantivy::directory::MmapDirectory::open(dir)?;
        let index = match Index::open_or_create(mmap, schema.clone()) {
            Ok(i) => i,
            Err(e) => {
                tracing::warn!("search index incompatible ({e}); recreating");
                std::fs::remove_dir_all(dir)?;
                std::fs::create_dir_all(dir)?;
                Index::open_or_create(tantivy::directory::MmapDirectory::open(dir)?, schema)?
            }
        };
        let writer = index.writer(64 * 1024 * 1024)?;
        let reader = index.reader_builder().reload_policy(ReloadPolicy::OnCommitWithDelay).try_into()?;
        Ok(Tantivy { index, reader, writer: Mutex::new(writer), f: Fields { id, space, title, body, labels } })
    }
}

impl SearchIndex {
    pub fn open(config: &crate::config::Config, db: PgPool) -> anyhow::Result<Self> {
        let backend = match config.search.as_str() {
            "postgres" => Backend::Postgres,
            "tantivy" | "" => Backend::Tantivy(Box::new(Tantivy::open(&config.index_dir())?)),
            other => anyhow::bail!("unknown search backend {other:?} (use tantivy or postgres)"),
        };
        let (tx, rx) = mpsc::unbounded_channel();
        let s = SearchIndex { inner: Arc::new(Inner { backend, db: db.clone(), tx }) };
        tokio::spawn(s.clone().worker(rx, db.clone()));
        if matches!(s.inner.backend, Backend::Postgres) {
            // pages written while no indexer ran (fresh migration, crashed replica)
            let s2 = s.clone();
            tokio::spawn(async move {
                if let Ok(ids) =
                    sqlx::query_scalar::<_, Uuid>("SELECT id FROM pages WHERE search_vec IS NULL AND deleted_at IS NULL").fetch_all(&s2.inner.db).await
                {
                    for chunk in ids.chunks(500) {
                        let _ = s2.index_batch(&s2.inner.db, chunk.to_vec()).await;
                    }
                }
            });
        }
        Ok(s)
    }

    pub fn backend_name(&self) -> &'static str {
        match self.inner.backend {
            Backend::Tantivy(_) => "tantivy",
            Backend::Postgres => "postgres",
        }
    }

    /// How many pages the index holds (Postgres: pages with a search vector).
    pub async fn indexed_documents(&self, db: &PgPool) -> anyhow::Result<u64> {
        Ok(match &self.inner.backend {
            Backend::Tantivy(t) => t.reader.searcher().num_docs(),
            Backend::Postgres => {
                let n: i64 = sqlx::query_scalar("SELECT count(*) FROM pages WHERE search_vec IS NOT NULL AND deleted_at IS NULL").fetch_one(db).await?;
                n.max(0) as u64
            }
        })
    }

    /// Queues a page for (re)indexing.
    pub fn enqueue(&self, page_id: Uuid) {
        let _ = self.inner.tx.send(page_id);
    }

    async fn worker(self, mut rx: mpsc::UnboundedReceiver<Uuid>, db: PgPool) {
        loop {
            let Some(first) = rx.recv().await else { return };
            let mut batch: HashSet<Uuid> = HashSet::from([first]);
            // gather a burst
            tokio::time::sleep(Duration::from_millis(300)).await;
            while let Ok(id) = rx.try_recv() {
                batch.insert(id);
                if batch.len() >= 2000 {
                    break;
                }
            }
            if let Err(e) = self.index_batch(&db, batch.into_iter().collect()).await {
                tracing::error!("search indexing failed: {e:#}");
            }
        }
    }

    async fn index_batch(&self, db: &PgPool, ids: Vec<Uuid>) -> anyhow::Result<()> {
        match &self.inner.backend {
            Backend::Postgres => {
                // weighted: title (A) > labels (B) > body + attachment text (C); 'simple' config = language agnostic
                sqlx::query(
                    "UPDATE pages p SET search_vec =
                        setweight(to_tsvector('simple', p.title), 'A') ||
                        setweight(to_tsvector('simple', coalesce((SELECT string_agg(name, ' ') FROM labels l WHERE l.page_id = p.id), '') || ' ' ||
                            coalesce((SELECT string_agg(key || ' ' || left(text_value, 2000), ' ') FROM page_properties pp WHERE pp.page_id = p.id), '')), 'B') ||
                        setweight(to_tsvector('simple', left(p.text_content || ' ' || coalesce((SELECT string_agg(filename || ' ' || coalesce(left(text_content, 20000), ''), ' ')
                            FROM attachments a WHERE a.page_id = p.id AND a.deleted_at IS NULL), '') || ' ' || coalesce((SELECT string_agg(left(c.text_content, 5000), ' ')
                            FROM comments c WHERE c.page_id = p.id AND c.deleted_at IS NULL), ''), 300000)), 'C')
                     WHERE p.id = ANY($1)",
                )
                .bind(&ids)
                .execute(db)
                .await?;
                Ok(())
            }
            Backend::Tantivy(_) => self.tantivy_index_batch(db, ids).await,
        }
    }

    async fn tantivy_index_batch(&self, db: &PgPool, ids: Vec<Uuid>) -> anyhow::Result<()> {
        let rows = sqlx::query_as::<_, IndexRow>(
            "SELECT p.id, p.space_id, p.title, p.text_content, p.deleted_at IS NOT NULL AS deleted,
                    concat_ws(' ', (SELECT string_agg(name, ' ') FROM labels l WHERE l.page_id = p.id),
                        (SELECT string_agg(key || ' ' || left(text_value, 2000), ' ') FROM page_properties pp WHERE pp.page_id = p.id)) AS labels,
                    (SELECT string_agg(filename || ' ' || coalesce(left(text_content, 20000), ''), ' ') FROM attachments a WHERE a.page_id = p.id AND a.deleted_at IS NULL) AS attachments,
                    (SELECT string_agg(left(c.text_content, 5000), ' ') FROM comments c WHERE c.page_id = p.id AND c.deleted_at IS NULL) AS comments
             FROM pages p WHERE p.id = ANY($1)",
        )
        .bind(&ids)
        .fetch_all(db)
        .await?;
        let this = self.clone();
        tokio::task::spawn_blocking(move || -> anyhow::Result<()> {
            let Backend::Tantivy(t) = &this.inner.backend else { return Ok(()) };
            let f = t.f;
            let mut w = t.writer.lock().map_err(|_| anyhow::anyhow!("writer poisoned"))?;
            for id in &ids {
                w.delete_term(Term::from_field_text(f.id, &id.to_string()));
            }
            for r in rows {
                if r.deleted {
                    continue;
                }
                let mut body = r.text_content;
                // attachment text and comments are searchable too
                for extra in [r.attachments, r.comments].into_iter().flatten() {
                    body.push('\n');
                    body.push_str(&extra);
                }
                w.add_document(doc!(
                    f.id => r.id.to_string(),
                    f.space => r.space_id.to_string(),
                    f.title => r.title,
                    f.body => body,
                    f.labels => r.labels.unwrap_or_default(),
                ))?;
            }
            w.commit()?;
            Ok(())
        })
        .await??;
        Ok(())
    }

    /// Rebuilds the whole index from the database.
    pub async fn reindex_all(&self, db: &PgPool) -> anyhow::Result<usize> {
        if matches!(self.inner.backend, Backend::Tantivy(_)) {
            let this = self.clone();
            tokio::task::spawn_blocking(move || -> anyhow::Result<()> {
                let Backend::Tantivy(t) = &this.inner.backend else { return Ok(()) };
                let mut w = t.writer.lock().map_err(|_| anyhow::anyhow!("writer poisoned"))?;
                w.delete_all_documents()?;
                w.commit()?;
                Ok(())
            })
            .await??;
        }
        let ids: Vec<Uuid> = sqlx::query_scalar("SELECT id FROM pages WHERE deleted_at IS NULL").fetch_all(db).await?;
        for chunk in ids.chunks(1000) {
            self.index_batch(db, chunk.to_vec()).await?;
        }
        Ok(ids.len())
    }

    /// Searches pages in the given spaces. Returns up to `limit` hits.
    pub async fn search(&self, q: &str, spaces: &[Uuid], limit: usize) -> anyhow::Result<Vec<Hit>> {
        if spaces.is_empty() || q.trim().is_empty() {
            return Ok(vec![]);
        }
        match &self.inner.backend {
            Backend::Tantivy(t) => tantivy_search(t, q, spaces, limit),
            Backend::Postgres => postgres_search(&self.inner.db, q, spaces, limit).await,
        }
    }
}

/// Words of the query reduced to letters/digits (safe to splice into `to_tsquery`).
fn tsquery_prefix(q: &str) -> String {
    let words: Vec<String> =
        q.split_whitespace().map(|w| w.chars().filter(|c| c.is_alphanumeric()).collect::<String>().to_lowercase()).filter(|w| !w.is_empty()).collect();
    match words.split_last() {
        Some((last, rest)) => {
            let mut parts: Vec<String> = rest.to_vec();
            parts.push(format!("{last}:*"));
            parts.join(" & ")
        }
        None => String::new(),
    }
}

#[derive(sqlx::FromRow)]
struct PgHit {
    id: Uuid,
    space_id: Uuid,
    title: String,
    headline: String,
    rank: f32,
}

async fn postgres_search(db: &PgPool, q: &str, spaces: &[Uuid], limit: usize) -> anyhow::Result<Vec<Hit>> {
    let prefix = tsquery_prefix(q);
    // the user query (websearch syntax: "phrases", OR, -exclude) OR the same words with prefix
    // matching on the last one (search-as-you-type)
    let rows = sqlx::query_as::<_, PgHit>(
        "WITH q AS (SELECT websearch_to_tsquery('simple', $1) || CASE WHEN $2 = '' THEN ''::tsquery ELSE to_tsquery('simple', $2) END AS query)
         SELECT p.id, p.space_id, p.title, ts_rank_cd(p.search_vec, q.query)::real AS rank,
                ts_headline('simple', left(p.text_content, 100000), q.query, 'StartSel=' || chr(2) || ', StopSel=' || chr(3) || ', MaxFragments=1, MaxWords=32, MinWords=12') AS headline
         FROM pages p, q
         WHERE p.space_id = ANY($3) AND p.deleted_at IS NULL AND p.search_vec @@ q.query
         ORDER BY rank DESC, p.updated_at DESC LIMIT $4",
    )
    .bind(q)
    .bind(&prefix)
    .bind(spaces)
    .bind(limit as i64)
    .fetch_all(db)
    .await?;
    Ok(rows
        .into_iter()
        .map(|r| {
            // escape first, then turn the control-char markers into <b> (never trust page text as HTML)
            let snippet = wikicraft_doc_model::html::escape(&r.headline).replace('\u{2}', "<b>").replace('\u{3}', "</b>");
            Hit { page_id: r.id, space_id: r.space_id, title: r.title, snippet, score: r.rank }
        })
        .collect())
}

fn tantivy_search(t: &Tantivy, q: &str, spaces: &[Uuid], limit: usize) -> anyhow::Result<Vec<Hit>> {
    let f = t.f;
    let searcher = t.reader.searcher();
    let mut parser = QueryParser::for_index(&t.index, vec![f.title, f.body, f.labels]);
    parser.set_field_boost(f.title, 4.0);
    parser.set_field_boost(f.labels, 2.0);
    parser.set_conjunction_by_default();
    let (text_query, _errors) = parser.parse_query_lenient(q);
    // prefix match on the last word for search-as-you-type
    let last = q.split_whitespace().last().unwrap_or("").to_lowercase();
    let mut text_clauses: Vec<(Occur, Box<dyn Query>)> = vec![(Occur::Should, text_query)];
    if last.chars().count() >= 2 && last.chars().all(char::is_alphanumeric) {
        let prefix = tantivy::query::PhrasePrefixQuery::new(vec![Term::from_field_text(f.title, &last)]);
        text_clauses.push((Occur::Should, Box::new(prefix)));
    }
    let space_clauses: Vec<(Occur, Box<dyn Query>)> = spaces
        .iter()
        .map(|s| (Occur::Should, Box::new(TermQuery::new(Term::from_field_text(f.space, &s.to_string()), IndexRecordOption::Basic)) as Box<dyn Query>))
        .collect();
    let query = BooleanQuery::new(vec![(Occur::Must, Box::new(BooleanQuery::new(text_clauses))), (Occur::Must, Box::new(BooleanQuery::new(space_clauses)))]);
    let top = searcher.search(&query, &TopDocs::with_limit(limit).order_by_score())?;
    let mut snippets = SnippetGenerator::create(&searcher, &query, f.body)?;
    snippets.set_max_num_chars(220);
    let mut hits = Vec::with_capacity(top.len());
    for (score, addr) in top {
        let d: TantivyDocument = searcher.doc(addr)?;
        let get = |field: Field| d.get_first(field).and_then(|v| v.as_str()).unwrap_or("").to_string();
        let snippet = snippets.snippet_from_doc(&d);
        let mut html = snippet.to_html();
        if html.is_empty() {
            html = wikicraft_doc_model::html::escape(&get(f.body).chars().take(200).collect::<String>());
        }
        hits.push(Hit {
            page_id: get(f.id).parse().unwrap_or_default(),
            space_id: get(f.space).parse().unwrap_or_default(),
            title: get(f.title),
            snippet: html,
            score,
        });
    }
    Ok(hits)
}

#[cfg(test)]
mod tests {
    #[test]
    fn tsquery_prefix_is_sanitized() {
        assert_eq!(super::tsquery_prefix("Release pro"), "release & pro:*");
        assert_eq!(super::tsquery_prefix("a'); DROP TABLE x; --"), "a & drop & table & x:*");
        assert_eq!(super::tsquery_prefix("   "), "");
    }
}
