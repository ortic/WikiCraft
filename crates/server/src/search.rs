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

#[derive(Clone)]
pub struct SearchIndex {
    inner: Arc<Inner>,
}

struct Inner {
    index: Index,
    reader: IndexReader,
    writer: Mutex<IndexWriter>,
    f: Fields,
    tx: mpsc::UnboundedSender<Uuid>,
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
}

impl SearchIndex {
    pub fn open(dir: &Path, db: PgPool) -> anyhow::Result<Self> {
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
                // schema changed between versions: rebuild from scratch
                tracing::warn!("search index incompatible ({e}); recreating");
                std::fs::remove_dir_all(dir)?;
                std::fs::create_dir_all(dir)?;
                Index::open_or_create(tantivy::directory::MmapDirectory::open(dir)?, schema)?
            }
        };
        let writer = index.writer(64 * 1024 * 1024)?;
        let reader = index.reader_builder().reload_policy(ReloadPolicy::OnCommitWithDelay).try_into()?;
        let (tx, rx) = mpsc::unbounded_channel();
        let s = SearchIndex { inner: Arc::new(Inner { index, reader, writer: Mutex::new(writer), f: Fields { id, space, title, body, labels }, tx }) };
        tokio::spawn(s.clone().worker(rx, db));
        Ok(s)
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
        let rows = sqlx::query_as::<_, IndexRow>(
            "SELECT p.id, p.space_id, p.title, p.text_content, p.deleted_at IS NOT NULL AS deleted,
                    (SELECT string_agg(name, ' ') FROM labels l WHERE l.page_id = p.id) AS labels,
                    (SELECT string_agg(filename || ' ' || coalesce(left(text_content, 20000), ''), ' ') FROM attachments a WHERE a.page_id = p.id AND a.deleted_at IS NULL) AS attachments
             FROM pages p WHERE p.id = ANY($1)",
        )
        .bind(&ids)
        .fetch_all(db)
        .await?;
        let f = self.inner.f;
        let this = self.clone();
        tokio::task::spawn_blocking(move || -> anyhow::Result<()> {
            let mut w = this.inner.writer.lock().map_err(|_| anyhow::anyhow!("writer poisoned"))?;
            for id in &ids {
                w.delete_term(Term::from_field_text(f.id, &id.to_string()));
            }
            for r in rows {
                if r.deleted {
                    continue;
                }
                let mut body = r.text_content;
                if let Some(a) = r.attachments {
                    body.push('\n');
                    body.push_str(&a);
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
        {
            let this = self.clone();
            tokio::task::spawn_blocking(move || -> anyhow::Result<()> {
                let mut w = this.inner.writer.lock().map_err(|_| anyhow::anyhow!("writer poisoned"))?;
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
    pub fn search(&self, q: &str, spaces: &[Uuid], limit: usize) -> anyhow::Result<Vec<Hit>> {
        if spaces.is_empty() || q.trim().is_empty() {
            return Ok(vec![]);
        }
        let f = self.inner.f;
        let searcher = self.inner.reader.searcher();
        let mut parser = QueryParser::for_index(&self.inner.index, vec![f.title, f.body, f.labels]);
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
        let query =
            BooleanQuery::new(vec![(Occur::Must, Box::new(BooleanQuery::new(text_clauses))), (Occur::Must, Box::new(BooleanQuery::new(space_clauses)))]);
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
}
