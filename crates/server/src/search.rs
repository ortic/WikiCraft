//! Full-text search with tantivy (embedded). Pages are (re)indexed asynchronously through a
//! channel; queries are filtered by the spaces a user may see and then by page restrictions.
//!
//! Text is cut into words ([`WordTokenizer`]: letters and digits; Chinese, Japanese and Korean,
//! written without spaces, as overlapping character pairs), lower-cased and folded to ASCII
//! (`cafe` finds `Café`). Titles and bodies are indexed a second time with a stemmer for the
//! workspace's language (`Seiten` finds `Seite`). Words of a query that occur nowhere in the
//! index are matched with typos allowed (one edit from 5 letters, two from 9), and the last word
//! also as a prefix (search as you type). The index rebuilds itself in the background when its
//! layout ([`INDEX_FORMAT`]) or language changes.

use std::collections::HashSet;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use serde::{Deserialize, Serialize};
use sqlx::PgPool;
use tantivy::collector::TopDocs;
use tantivy::query::{BooleanQuery, BoostQuery, FuzzyTermQuery, Occur, Query, QueryParser, TermQuery};
use tantivy::schema::{Field, IndexRecordOption, STORED, STRING, Schema, TextFieldIndexing, TextOptions, Value};
use tantivy::snippet::SnippetGenerator;
use tantivy::tokenizer::{AsciiFoldingFilter, Language, LowerCaser, RemoveLongFilter, Stemmer, TextAnalyzer, Token, TokenStream, Tokenizer};
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
    /// to recognise absolute links into this wiki ([`crate::links`])
    base_url: String,
    /// `WIKICRAFT_SEARCH_LANGUAGE`: `auto` follows the workspace's default language
    language_setting: String,
    /// a rebuild of the tantivy index is running
    rebuilding: AtomicBool,
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
    dir: PathBuf,
    /// language code whose stemmer the index uses (`none`: no stemming)
    language: Mutex<String>,
}

#[derive(Clone, Copy)]
struct Fields {
    id: Field,
    space: Field,
    title: Field,
    body: Field,
    labels: Field,
    /// title and body again, stemmed
    title_stem: Field,
    body_stem: Field,
}

/// Bumped when the index's fields or text analysis change: the index is rebuilt at startup.
const INDEX_FORMAT: u32 = 2;
/// text analyzers: words, and words reduced to their stem
const WORDS: &str = "wc_words";
const STEMS: &str = "wc_stems";

/// What an index on disk was built with (`wikicraft-index.json`, written once it is complete).
#[derive(Serialize, Deserialize, PartialEq, Debug)]
struct IndexMeta {
    format: u32,
    language: String,
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
    /// Opens the index (or starts a new, empty one if it was built differently); the flag says
    /// whether it needs to be filled again.
    fn open(dir: &Path, language: &str) -> anyhow::Result<(Self, bool)> {
        let want = IndexMeta { format: INDEX_FORMAT, language: language.to_string() };
        let have: Option<IndexMeta> = std::fs::read(dir.join(META_FILE)).ok().and_then(|b| serde_json::from_slice(&b).ok());
        let mut rebuild = have.as_ref() != Some(&want);
        if rebuild && dir.exists() {
            tracing::info!("search index built differently ({have:?}, now {want:?}); rebuilding it");
            std::fs::remove_dir_all(dir)?;
        }
        std::fs::create_dir_all(dir)?;
        let text = |tokenizer: &str, stored: bool| {
            let o = TextOptions::default()
                .set_indexing_options(TextFieldIndexing::default().set_tokenizer(tokenizer).set_index_option(IndexRecordOption::WithFreqsAndPositions));
            if stored { o.set_stored() } else { o }
        };
        let mut sb = Schema::builder();
        let id = sb.add_text_field("id", STRING | STORED);
        let space = sb.add_text_field("space", STRING | STORED);
        let title = sb.add_text_field("title", text(WORDS, true));
        let body = sb.add_text_field("body", text(WORDS, true));
        let labels = sb.add_text_field("labels", text(WORDS, false));
        let title_stem = sb.add_text_field("title_stem", text(STEMS, false));
        let body_stem = sb.add_text_field("body_stem", text(STEMS, false));
        let schema = sb.build();
        let mmap = tantivy::directory::MmapDirectory::open(dir)?;
        let index = match Index::open_or_create(mmap, schema.clone()) {
            Ok(i) => i,
            Err(e) => {
                tracing::warn!("search index incompatible ({e}); recreating");
                rebuild = true;
                std::fs::remove_dir_all(dir)?;
                std::fs::create_dir_all(dir)?;
                Index::open_or_create(tantivy::directory::MmapDirectory::open(dir)?, schema)?
            }
        };
        index.tokenizers().register(WORDS, words_analyzer());
        index.tokenizers().register(STEMS, stems_analyzer(language));
        let writer = index.writer(64 * 1024 * 1024)?;
        let reader = index.reader_builder().reload_policy(ReloadPolicy::OnCommitWithDelay).try_into()?;
        let f = Fields { id, space, title, body, labels, title_stem, body_stem };
        Ok((Tantivy { index, reader, writer: Mutex::new(writer), f, dir: dir.to_path_buf(), language: Mutex::new(language.to_string()) }, rebuild))
    }

    fn language(&self) -> String {
        self.language.lock().map(|l| l.clone()).unwrap_or_default()
    }

    /// Notes on disk that the index is complete for its language.
    fn write_meta(&self) -> anyhow::Result<()> {
        let meta = IndexMeta { format: INDEX_FORMAT, language: self.language() };
        std::fs::write(self.dir.join(META_FILE), serde_json::to_vec(&meta)?)?;
        Ok(())
    }
}

const META_FILE: &str = "wikicraft-index.json";

/// Marks the local index as outdated (after a restore): it is rebuilt at the next start.
pub fn forget_index(config: &crate::config::Config) {
    let _ = std::fs::remove_file(config.index_dir().join(META_FILE));
}

/// The stemmer's language code for a setting or workspace language (`de`, `german`, …), `none`
/// for languages without one.
pub fn search_language(code: &str) -> String {
    let code = code.trim().to_ascii_lowercase();
    let code = code.split(['-', '_']).next().unwrap_or("");
    let canonical = match code {
        "en" | "english" => "en",
        "de" | "german" => "de",
        "fr" | "french" => "fr",
        "es" | "spanish" => "es",
        "pt" | "portuguese" => "pt",
        "it" | "italian" => "it",
        "nl" | "dutch" => "nl",
        "da" | "danish" => "da",
        "sv" | "swedish" => "sv",
        "no" | "nb" | "norwegian" => "no",
        "fi" | "finnish" => "fi",
        "ru" | "russian" => "ru",
        "ar" | "arabic" => "ar",
        _ => "none",
    };
    canonical.to_string()
}

fn stemmer_for(code: &str) -> Option<Language> {
    Some(match code {
        "en" => Language::English,
        "de" => Language::German,
        "fr" => Language::French,
        "es" => Language::Spanish,
        "pt" => Language::Portuguese,
        "it" => Language::Italian,
        "nl" => Language::Dutch,
        "da" => Language::Danish,
        "sv" => Language::Swedish,
        "no" => Language::Norwegian,
        "fi" => Language::Finnish,
        "ru" => Language::Russian,
        "ar" => Language::Arabic,
        _ => return None,
    })
}

fn words_analyzer() -> TextAnalyzer {
    TextAnalyzer::builder(WordTokenizer).filter(RemoveLongFilter::limit(48)).filter(LowerCaser).filter(AsciiFoldingFilter).build()
}

fn stems_analyzer(language: &str) -> TextAnalyzer {
    match stemmer_for(language) {
        // stem before folding: the stemmers know their language's accents (Häuser → haus)
        Some(l) => TextAnalyzer::builder(WordTokenizer)
            .filter(RemoveLongFilter::limit(48))
            .filter(LowerCaser)
            .filter(Stemmer::new(l))
            .filter(AsciiFoldingFilter)
            .build(),
        None => words_analyzer(),
    }
}

impl SearchIndex {
    pub async fn open(config: &crate::config::Config, db: PgPool) -> anyhow::Result<Self> {
        let mut rebuild = false;
        let backend = match config.search.as_str() {
            "postgres" => Backend::Postgres,
            "tantivy" | "" => {
                let language = match config.search_language.trim() {
                    "" | "auto" => search_language(&workspace_language(&db).await),
                    other => search_language(other),
                };
                let (t, r) = Tantivy::open(&config.index_dir(), &language)?;
                rebuild = r;
                Backend::Tantivy(Box::new(t))
            }
            other => anyhow::bail!("unknown search backend {other:?} (use tantivy or postgres)"),
        };
        let (tx, rx) = mpsc::unbounded_channel();
        let s = SearchIndex {
            inner: Arc::new(Inner {
                backend,
                db: db.clone(),
                tx,
                base_url: config.base_url.clone(),
                language_setting: config.search_language.trim().to_ascii_lowercase(),
                rebuilding: AtomicBool::new(false),
            }),
        };
        if rebuild {
            s.spawn_rebuild();
        }
        tokio::spawn(s.clone().worker(rx, db.clone()));
        crate::retrieval::spawn_backfill(db.clone());
        crate::links::spawn_backfill(db.clone(), config.base_url.clone());
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

    /// Fills a new (or differently built) tantivy index from the database in the background, and
    /// notes it as complete when done.
    fn spawn_rebuild(&self) {
        if self.inner.rebuilding.swap(true, Ordering::SeqCst) {
            return;
        }
        let this = self.clone();
        tokio::spawn(async move {
            let started = std::time::Instant::now();
            match this.rebuild().await {
                Ok(n) => tracing::info!("search index rebuilt: {n} pages in {:.1?}", started.elapsed()),
                Err(e) => tracing::error!("rebuilding the search index failed: {e:#}"),
            }
            this.inner.rebuilding.store(false, Ordering::SeqCst);
        });
    }

    async fn rebuild(&self) -> anyhow::Result<usize> {
        let Backend::Tantivy(t) = &self.inner.backend else { return Ok(0) };
        let db = &self.inner.db;
        let this = self.clone();
        tokio::task::spawn_blocking(move || -> anyhow::Result<()> {
            let Backend::Tantivy(t) = &this.inner.backend else { return Ok(()) };
            let mut w = t.writer.lock().map_err(|_| anyhow::anyhow!("writer poisoned"))?;
            w.delete_all_documents()?;
            w.commit()?;
            Ok(())
        })
        .await??;
        let ids: Vec<Uuid> = sqlx::query_scalar("SELECT id FROM pages WHERE deleted_at IS NULL ORDER BY id").fetch_all(db).await?;
        for chunk in ids.chunks(500) {
            self.index_batch_search(db, chunk).await?;
        }
        t.write_meta()?;
        Ok(ids.len())
    }

    /// The workspace's default language changed: with `WIKICRAFT_SEARCH_LANGUAGE=auto`, the index
    /// follows it (rebuilt in the background).
    pub fn workspace_language_changed(&self, code: &str) {
        let Backend::Tantivy(t) = &self.inner.backend else { return };
        if !matches!(self.inner.language_setting.as_str(), "" | "auto") {
            return;
        }
        let want = search_language(if code.trim().is_empty() { "en" } else { code });
        if t.language() == want {
            return;
        }
        if let Ok(mut l) = t.language.lock() {
            l.clone_from(&want);
        }
        t.index.tokenizers().register(STEMS, stems_analyzer(&want));
        let _ = std::fs::remove_file(t.dir.join(META_FILE));
        self.spawn_rebuild();
    }

    /// The language whose word forms search matches (tantivy; `none` without stemming), and
    /// whether the index is being rebuilt.
    pub fn language(&self) -> Option<String> {
        match &self.inner.backend {
            Backend::Tantivy(t) => Some(t.language()),
            Backend::Postgres => None,
        }
    }

    pub fn rebuilding(&self) -> bool {
        self.inner.rebuilding.load(Ordering::SeqCst)
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
        let indexed = self.index_batch_search(db, &ids).await;
        // the passages for retrieval (RAG) follow the same changes
        if let Err(e) = crate::retrieval::rechunk(db, &ids).await {
            tracing::error!("cutting pages into passages failed: {e:#}");
        }
        // and so does the link graph
        if let Err(e) = crate::links::relink(db, &ids, &self.inner.base_url).await {
            tracing::error!("extracting page links failed: {e:#}");
        }
        indexed
    }

    async fn index_batch_search(&self, db: &PgPool, ids: &[Uuid]) -> anyhow::Result<()> {
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
                .bind(ids)
                .execute(db)
                .await?;
                Ok(())
            }
            Backend::Tantivy(_) => self.tantivy_index_batch(db, ids.to_vec()).await,
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
                    f.title_stem => r.title.clone(),
                    f.title => r.title,
                    f.body_stem => body.clone(),
                    f.body => body,
                    f.labels => r.labels.unwrap_or_default(),
                ))?;
            }
            w.commit()?;
            drop(w);
            // searches see the change right away, not when the reader's file watch notices it
            // (retrieval and "search right after saving" depend on it)
            t.reader.reload()?;
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
    let text_query: Box<dyn Query> = if has_syntax(q) {
        // "phrases", OR, -exclude, +require: tantivy's query language over words and stems
        let mut parser = QueryParser::for_index(&t.index, vec![f.title, f.body, f.labels, f.title_stem, f.body_stem]);
        parser.set_field_boost(f.title, 4.0);
        parser.set_field_boost(f.labels, 2.0);
        parser.set_field_boost(f.title_stem, 3.0);
        parser.set_field_boost(f.body_stem, 0.8);
        parser.set_conjunction_by_default();
        parser.parse_query_lenient(q).0
    } else {
        match words_query(t, &searcher, q) {
            Some(query) => query,
            None => return Ok(vec![]),
        }
    };
    let text_clauses: Vec<(Occur, Box<dyn Query>)> = vec![(Occur::Must, text_query)];
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

/// Whether a query uses the query language (phrases, OR, required or excluded words).
fn has_syntax(q: &str) -> bool {
    q.contains('"') || q.split_whitespace().any(|w| w == "OR" || w == "AND" || ((w.starts_with('-') || w.starts_with('+')) && w.len() > 1))
}

/// Every word of the query must match: as written, by its stem, with typos (only words found
/// nowhere in the index), and the last one also as the start of a word.
fn words_query(t: &Tantivy, searcher: &tantivy::Searcher, q: &str) -> Option<Box<dyn Query>> {
    let f = t.f;
    let mut words = words_analyzer();
    let mut stems = t.index.tokenizers().get(STEMS).unwrap_or_else(words_analyzer);
    let mut found: Vec<(String, String)> = Vec::new();
    let mut stream = words.token_stream(q);
    while let Some(tok) = stream.next() {
        let original = &q[tok.offset_from..tok.offset_to];
        let stem = stems.token_stream(original).next().map(|s| s.text.clone()).unwrap_or_else(|| tok.text.clone());
        found.push((tok.text.clone(), stem));
        if found.len() >= 16 {
            break;
        }
    }
    let n = found.len();
    if n == 0 {
        return None;
    }
    let term = |field: Field, text: &str, boost: f32| -> (Occur, Box<dyn Query>) {
        let q: Box<dyn Query> = Box::new(TermQuery::new(Term::from_field_text(field, text), IndexRecordOption::WithFreqs));
        (Occur::Should, if boost == 1.0 { q } else { Box::new(BoostQuery::new(q, boost)) })
    };
    let automaton = |q: FuzzyTermQuery, boost: f32| -> (Occur, Box<dyn Query>) { (Occur::Should, Box::new(BoostQuery::new(Box::new(q), boost))) };
    let known = |field: Field, text: &str| searcher.doc_freq(&Term::from_field_text(field, text)).unwrap_or(0) > 0;
    let clauses: Vec<(Occur, Box<dyn Query>)> = found
        .iter()
        .enumerate()
        .map(|(i, (word, stem))| {
            let mut alts =
                vec![term(f.title, word, 4.0), term(f.body, word, 1.0), term(f.labels, word, 2.0), term(f.title_stem, stem, 3.0), term(f.body_stem, stem, 0.8)];
            let chars = word.chars().count();
            let cjk = word.chars().any(is_cjk);
            let unknown = !(known(f.body, word) || known(f.title, word) || known(f.labels, word) || known(f.body_stem, stem));
            if !cjk && chars >= 5 && unknown {
                let distance = if chars >= 9 { 2 } else { 1 };
                alts.push(automaton(FuzzyTermQuery::new(Term::from_field_text(f.title, word), distance, true), 1.5));
                alts.push(automaton(FuzzyTermQuery::new(Term::from_field_text(f.body, word), distance, true), 0.3));
            }
            if i == n - 1 && !cjk && chars >= 2 {
                alts.push(automaton(FuzzyTermQuery::new_prefix(Term::from_field_text(f.title, word), 0, false), 2.0));
                if chars >= 3 {
                    alts.push(automaton(FuzzyTermQuery::new_prefix(Term::from_field_text(f.body, word), 0, false), 0.5));
                }
            }
            (Occur::Must, Box::new(BooleanQuery::new(alts)) as Box<dyn Query>)
        })
        .collect();
    Some(Box::new(BooleanQuery::new(clauses)))
}

/// The workspace's default interface language ("" when unset or before setup).
async fn workspace_language(db: &PgPool) -> String {
    sqlx::query_scalar::<_, Option<String>>("SELECT NULLIF(settings->>'defaultLanguage', '') FROM workspaces ORDER BY created_at LIMIT 1")
        .fetch_optional(db)
        .await
        .ok()
        .flatten()
        .flatten()
        .unwrap_or_else(|| "en".into())
}

/// Chinese, Japanese and Korean characters (written without spaces between words).
fn is_cjk(c: char) -> bool {
    matches!(c as u32,
        0x1100..=0x11FF | 0x2E80..=0x2FDF | 0x3040..=0x30FF | 0x3100..=0x31FF | 0x3400..=0x4DBF | 0x4E00..=0x9FFF
        | 0xA960..=0xA97F | 0xAC00..=0xD7FF | 0xF900..=0xFAFF | 0xFF66..=0xFF9F | 0x20000..=0x2FA1F)
}

/// Words: runs of letters and digits; runs of Chinese, Japanese or Korean characters become
/// overlapping pairs (日本語 → 日本, 本語), the usual way to search text without spaces.
#[derive(Clone, Default)]
pub struct WordTokenizer;

pub struct WordTokens {
    tokens: Vec<Token>,
    next: usize,
}

impl Tokenizer for WordTokenizer {
    type TokenStream<'a> = WordTokens;
    fn token_stream<'a>(&'a mut self, text: &'a str) -> WordTokens {
        WordTokens { tokens: words(text), next: 0 }
    }
}

impl TokenStream for WordTokens {
    fn advance(&mut self) -> bool {
        if self.next < self.tokens.len() {
            self.next += 1;
            true
        } else {
            false
        }
    }

    fn token(&self) -> &Token {
        &self.tokens[self.next - 1]
    }

    fn token_mut(&mut self) -> &mut Token {
        &mut self.tokens[self.next - 1]
    }
}

fn words(text: &str) -> Vec<Token> {
    let mut out: Vec<Token> = Vec::new();
    let mut push = |text: &str, from: usize, to: usize| {
        let position = out.len();
        out.push(Token { offset_from: from, offset_to: to, position, text: text[from..to].to_string(), position_length: 1 });
    };
    let mut chars = text.char_indices().peekable();
    while let Some((start, c)) = chars.next() {
        if is_cjk(c) {
            let mut run = vec![(start, c)];
            while let Some(&(i, d)) = chars.peek() {
                if !is_cjk(d) {
                    break;
                }
                run.push((i, d));
                chars.next();
            }
            if run.len() == 1 {
                push(text, start, start + c.len_utf8());
            }
            for pair in run.windows(2) {
                push(text, pair[0].0, pair[1].0 + pair[1].1.len_utf8());
            }
        } else if c.is_alphanumeric() {
            let mut end = start + c.len_utf8();
            while let Some(&(i, d)) = chars.peek() {
                if !d.is_alphanumeric() || is_cjk(d) {
                    break;
                }
                end = i + d.len_utf8();
                chars.next();
            }
            push(text, start, end);
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tokens(analyzer: &mut TextAnalyzer, text: &str) -> Vec<String> {
        let mut out = Vec::new();
        let mut s = analyzer.token_stream(text);
        while let Some(t) = s.next() {
            out.push(t.text.clone());
        }
        out
    }

    #[test]
    fn words_split_cjk_into_pairs_and_fold_accents() {
        let mut a = words_analyzer();
        assert_eq!(tokens(&mut a, "Café crème, v2.0!"), ["cafe", "creme", "v2", "0"]);
        assert_eq!(tokens(&mut a, "日本語の文書"), ["日本", "本語", "語の", "の文", "文書"]);
        assert_eq!(tokens(&mut a, "iPhone15日本 한국어"), ["iphone15", "日本", "한국", "국어"]);
        assert_eq!(tokens(&mut a, "字"), ["字"]);
        let w = words("ab 日本");
        assert_eq!((w[1].offset_from, w[1].offset_to, w[1].position), (3, 9, 1));
    }

    #[test]
    fn stems_follow_the_language() {
        let mut de = stems_analyzer("de");
        assert_eq!(tokens(&mut de, "Seiten Seite Häuser"), tokens(&mut de, "seite seite haus"));
        let mut en = stems_analyzer("en");
        assert_eq!(tokens(&mut en, "deployments deploying"), ["deploy", "deploy"]);
        let mut none = stems_analyzer("none");
        assert_eq!(tokens(&mut none, "Deployments"), ["deployments"]);
        assert_eq!(search_language("de-CH"), "de");
        assert_eq!(search_language("German"), "de");
        assert_eq!(search_language("ja"), "none");
    }

    #[test]
    fn query_syntax_is_recognised() {
        assert!(has_syntax("\"release notes\""));
        assert!(has_syntax("deploy -staging"));
        assert!(has_syntax("a OR b"));
        assert!(!has_syntax("release notes 2026"));
        assert!(!has_syntax("e-mail - draft"));
    }

    #[test]
    fn tsquery_prefix_is_sanitized() {
        assert_eq!(tsquery_prefix("Release pro"), "release & pro:*");
        assert_eq!(tsquery_prefix("a'); DROP TABLE x; --"), "a & drop & table & x:*");
        assert_eq!(tsquery_prefix("   "), "");
    }
}
