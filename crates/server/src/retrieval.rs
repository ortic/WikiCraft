//! Retrieval for AI applications (RAG): the passages of the wiki that best match a question,
//! taken only from pages the asker may open.
//!
//! Pages are cut into passages along their structure ([`wikicraft_doc_model::chunk`]) whenever
//! the search index picks them up; the extracted text of attachments adds passages of its own.
//! A question is answered from two rankings, fused with reciprocal rank fusion:
//! * **keyword**: the full-text search finds candidate pages (permission-filtered exactly like
//!   search), then BM25 over their passages finds the passages that match;
//! * **semantic** (with an embeddings API configured and pgvector installed, see
//!   [`crate::embeddings`]): the passages whose vectors are nearest to the question's, among
//!   the spaces the asker can see, then filtered by page restrictions, drafts and archiving.

use std::collections::{HashMap, HashSet};

use serde::Serialize;
use sha2::{Digest, Sha256};
use sqlx::PgPool;
use uuid::Uuid;
use wikicraft_doc_model::{Node, chunk};

use crate::app::AppState;
use crate::authz::{Principal, Role};
use crate::error::{ApiError, ApiResult};
use crate::routes::search::{filter_visible, hits_in, visible_spaces};
use crate::routes::spaces::SpaceRow;

/// Bumped when the chunker changes: every page is cut again at the next start.
pub const CHUNKER_VERSION: i32 = 1;
/// extracted attachment text beyond this is not cut into passages
const MAX_ATTACHMENT_TEXT: usize = 300_000;
/// candidates each ranking contributes before fusion
const CANDIDATES: usize = 60;
/// reciprocal rank fusion constant
const RRF_K: f64 = 60.0;

/// The text a passage is embedded as (and hashed by): page title, heading path, passage.
pub fn embed_input(title: &str, headings: &[String], content: &str) -> String {
    if headings.is_empty() { format!("{title}\n\n{content}") } else { format!("{title}\n{}\n\n{content}", headings.join(" > ")) }
}

fn hash(input: &str) -> String {
    hex::encode(Sha256::digest(input.as_bytes()))
}

// --------------------------------------------------------------------------------------------
// Cutting pages into passages

#[derive(sqlx::FromRow)]
struct PageRow {
    id: Uuid,
    title: String,
    content: serde_json::Value,
    gone: bool,
}

/// Cuts the given pages (and their attachments' text) into passages again; trashed or deleted
/// pages lose theirs.
pub async fn rechunk(db: &PgPool, ids: &[Uuid]) -> anyhow::Result<()> {
    for batch in ids.chunks(100) {
        rechunk_batch(db, batch).await?;
    }
    crate::embeddings::wake();
    Ok(())
}

async fn rechunk_batch(db: &PgPool, ids: &[Uuid]) -> anyhow::Result<()> {
    let pages: Vec<PageRow> =
        sqlx::query_as("SELECT id, title, content, deleted_at IS NOT NULL AS gone FROM pages WHERE id = ANY($1)").bind(ids).fetch_all(db).await?;
    let atts: Vec<(Uuid, Uuid, String, String)> = sqlx::query_as(
        "SELECT id, page_id, filename, left(text_content, $2) FROM attachments
         WHERE page_id = ANY($1) AND deleted_at IS NULL AND coalesce(text_content, '') <> '' ORDER BY page_id, lower(filename)",
    )
    .bind(ids)
    .bind(MAX_ATTACHMENT_TEXT as i32)
    .fetch_all(db)
    .await?;
    let mut by_page: HashMap<Uuid, Vec<(Uuid, String, String)>> = HashMap::new();
    for (id, page, name, text) in atts {
        by_page.entry(page).or_default().push((id, name, text));
    }
    // (page, ord, attachment, block, headings, content, hash)
    type Row = (Uuid, i32, Option<Uuid>, Option<String>, Vec<String>, String, String);
    let rows: Vec<Row> = tokio::task::spawn_blocking(move || {
        let mut rows: Vec<Row> = Vec::new();
        for p in pages.iter().filter(|p| !p.gone) {
            let doc = Node::from_json(&p.content).unwrap_or_else(|_| Node::empty_doc());
            let mut ord = 0;
            for c in chunk::chunk_doc(&doc) {
                let h = hash(&embed_input(&p.title, &c.headings, &c.markdown));
                rows.push((p.id, ord, None, c.block_id, c.headings, c.markdown, h));
                ord += 1;
            }
            for (att, name, text) in by_page.remove(&p.id).unwrap_or_default() {
                let headings = vec![name];
                for piece in chunk::chunk_text(&text) {
                    let h = hash(&embed_input(&p.title, &headings, &piece));
                    rows.push((p.id, ord, Some(att), None, headings.clone(), piece, h));
                    ord += 1;
                }
            }
        }
        rows
    })
    .await?;
    let mut tx = db.begin().await?;
    sqlx::query("DELETE FROM page_chunks WHERE page_id = ANY($1)").bind(ids).execute(&mut *tx).await?;
    for part in rows.chunks(1000) {
        let mut qb = sqlx::QueryBuilder::new("INSERT INTO page_chunks (page_id, ord, attachment_id, block_id, headings, content, hash) ");
        qb.push_values(part, |mut b, r| {
            b.push_bind(r.0).push_bind(r.1).push_bind(r.2).push_bind(&r.3).push_bind(&r.4).push_bind(&r.5).push_bind(&r.6);
        });
        // a concurrent re-cut of the same page (another replica) may have written first
        qb.push(
            " ON CONFLICT (page_id, ord) DO UPDATE SET attachment_id = EXCLUDED.attachment_id, block_id = EXCLUDED.block_id,
              headings = EXCLUDED.headings, content = EXCLUDED.content, hash = EXCLUDED.hash",
        );
        qb.build().execute(&mut *tx).await?;
    }
    sqlx::query("UPDATE pages SET chunked_at = now() WHERE id = ANY($1)").bind(ids).execute(&mut *tx).await?;
    tx.commit().await?;
    Ok(())
}

/// At startup: re-cut everything after a chunker change, then cut the pages that never were
/// (new installs, upgrades), in the background.
pub fn spawn_backfill(db: PgPool) {
    tokio::spawn(async move {
        if let Err(e) = backfill(&db).await {
            tracing::error!("cutting pages into passages failed: {e:#}");
        }
    });
}

async fn backfill(db: &PgPool) -> anyhow::Result<()> {
    let version: i32 = sqlx::query_scalar("SELECT chunker FROM retrieval_state").fetch_one(db).await?;
    if version != CHUNKER_VERSION {
        sqlx::query("UPDATE pages SET chunked_at = NULL").execute(db).await?;
        sqlx::query("UPDATE retrieval_state SET chunker = $1").bind(CHUNKER_VERSION).execute(db).await?;
    }
    let mut last = Uuid::nil();
    loop {
        let ids: Vec<Uuid> = sqlx::query_scalar("SELECT id FROM pages WHERE chunked_at IS NULL AND deleted_at IS NULL AND id > $1 ORDER BY id LIMIT 200")
            .bind(last)
            .fetch_all(db)
            .await?;
        let Some(l) = ids.last() else { return Ok(()) };
        last = *l;
        rechunk(db, &ids).await?;
    }
}

// --------------------------------------------------------------------------------------------
// Retrieving

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Mode {
    /// hybrid when semantic retrieval is available, else keyword
    Auto,
    Hybrid,
    Keyword,
    Semantic,
}

impl Mode {
    pub fn parse(s: Option<&str>) -> ApiResult<Self> {
        Ok(match s.map(str::trim).unwrap_or("") {
            "" | "auto" => Mode::Auto,
            "hybrid" => Mode::Hybrid,
            "keyword" => Mode::Keyword,
            "semantic" => Mode::Semantic,
            _ => return Err(ApiError::bad("mode must be auto, hybrid, keyword or semantic")),
        })
    }
}

pub struct Options {
    pub query: String,
    /// space keys to search in (empty: all the asker can see)
    pub spaces: Vec<String>,
    /// labels a page must have one of (empty: any page)
    pub labels: Vec<String>,
    pub limit: usize,
    /// at most this many passages from one page
    pub per_page: usize,
    pub mode: Mode,
    /// stop adding passages once their text reaches this many characters
    pub max_chars: Option<usize>,
}

#[derive(Debug, Serialize, Clone)]
#[serde(rename_all = "camelCase")]
pub struct AttachmentRef {
    pub id: Uuid,
    pub filename: String,
    pub url: String,
}

#[derive(Debug, Serialize, Clone)]
#[serde(rename_all = "camelCase")]
pub struct Passage {
    pub page_id: Uuid,
    pub title: String,
    pub space_key: String,
    pub space_name: String,
    /// titles of the pages above, outermost first
    pub path: Vec<String>,
    /// the headings the passage sits under, outermost first (an attachment's: its file name)
    pub headings: Vec<String>,
    pub block_id: Option<String>,
    /// link to the page, scrolled to the passage
    pub url: String,
    /// the passage as Markdown
    pub text: String,
    pub labels: Vec<String>,
    pub updated_at: chrono::DateTime<chrono::Utc>,
    pub version: i32,
    /// set when the passage comes from an attached file's text
    pub attachment: Option<AttachmentRef>,
    /// fused rank score: higher is better, comparable within one response only
    pub score: f64,
    /// `keyword` and/or `semantic`
    pub matched_by: Vec<&'static str>,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Retrieval {
    /// what actually ran: `keyword`, `semantic` or `hybrid`
    pub mode: &'static str,
    pub passages: Vec<Passage>,
    /// why semantic retrieval didn't run when it was asked for
    #[serde(skip_serializing_if = "Option::is_none")]
    pub note: Option<String>,
}

const STOP_WORDS: &[&str] = &[
    // English
    "the", "and", "for", "are", "was", "were", "how", "what", "which", "who", "whom", "where", "when", "why", "can", "could", "should", "would", "does", "did",
    "have", "has", "had", "with", "from", "about", "into", "this", "that", "these", "those", "there", "our", "you", "your", "get", "please", "tell", "any",
    "all", "not", "but", "use", "using", "need", "want", "know", "its", "they", "them", "their", "then", "than", "also", "will", "shall", "may", "might",
    "must", "been", "being", "some", "such", "here", "out", "over", "under", "again", "find", "show", "give", "explain", "describe", "do", "to", "is", "in",
    "on", "of", "at", "by", "or", "an", "as", "be", "if", "me", "my", "we", "us", "so", "no", "am", // German
    "wie", "was", "wer", "wann", "warum", "welche", "welcher", "welches", "ich", "wir", "der", "die", "das", "den", "dem", "des", "ein", "eine", "einen",
    "einem", "einer", "und", "oder", "ist", "sind", "mit", "für", "von", "auf", "kann", "können", "muss", "müssen", "gibt", "es", "wo", "im", "zum", "zur",
    "bei", "aus", "nach", "auch", "nicht", "sich", "man", "mein", "meine", "unser", "unsere", "bitte", "zu", "um", "ob", "da", "du", "er", "sie",
    // French
    "les", "des", "une", "est", "sont", "pour", "avec", "dans", "sur", "comment", "quoi", "quel", "quelle", "quels", "quelles", "qui", "que", "pourquoi",
    "est-ce", "nous", "vous", "mon", "mes", "notre", "nos", "votre", "vos", "aux", "par", "pas", "plus", "le", "la", "de", "du", "un", "et", "en", "je", "il",
    "ce", "ou", "où", "ne", "se", "sa", "son",
];

/// The words of a question worth searching for: stop words and one-letter words dropped,
/// lower-cased, each once, at most 16.
pub fn keywords(question: &str) -> Vec<String> {
    let mut words: Vec<String> = Vec::new();
    for w in question.split(|c: char| !c.is_alphanumeric()).map(|w| w.to_lowercase()) {
        if w.chars().count() > 1 && !STOP_WORDS.contains(&w.as_str()) && !words.contains(&w) {
            words.push(w);
        }
    }
    words.truncate(16);
    words
}

fn tokens(s: &str) -> Vec<String> {
    s.split(|c: char| !c.is_alphanumeric()).filter(|w| !w.is_empty()).map(|w| w.to_lowercase()).collect()
}

/// BM25 scores of `docs` for `terms`, statistics taken over `docs` themselves.
fn bm25(terms: &[String], docs: &[Vec<String>]) -> Vec<f64> {
    const K1: f64 = 1.2;
    const B: f64 = 0.75;
    let n = docs.len() as f64;
    if docs.is_empty() || terms.is_empty() {
        return vec![0.0; docs.len()];
    }
    let avg = docs.iter().map(|d| d.len() as f64).sum::<f64>() / n;
    let counts: Vec<HashMap<&str, usize>> = docs
        .iter()
        .map(|d| {
            let mut m: HashMap<&str, usize> = HashMap::new();
            for t in d {
                *m.entry(t.as_str()).or_default() += 1;
            }
            m
        })
        .collect();
    let idf: Vec<f64> = terms
        .iter()
        .map(|t| {
            let df = counts.iter().filter(|c| c.contains_key(t.as_str())).count() as f64;
            (1.0 + (n - df + 0.5) / (df + 0.5)).ln()
        })
        .collect();
    counts
        .iter()
        .zip(docs)
        .map(|(c, d)| {
            let len = d.len() as f64;
            terms
                .iter()
                .zip(&idf)
                .map(|(t, idf)| {
                    let tf = *c.get(t.as_str()).unwrap_or(&0) as f64;
                    if tf == 0.0 { 0.0 } else { idf * tf * (K1 + 1.0) / (tf + K1 * (1.0 - B + B * len / avg.max(1.0))) }
                })
                .sum()
        })
        .collect()
}

#[derive(sqlx::FromRow, Clone)]
struct ChunkRow {
    page_id: Uuid,
    ord: i32,
    attachment_id: Option<Uuid>,
    block_id: Option<String>,
    headings: Vec<String>,
    content: String,
}

const CHUNK_COLUMNS: &str = "c.page_id, c.ord, c.attachment_id, c.block_id, c.headings, c.content";

/// The spaces a question may be answered from: visible, not archived, optionally narrowed.
async fn spaces_for(state: &AppState, p: &Principal, keys: &[String]) -> ApiResult<Vec<(SpaceRow, Role)>> {
    let mut spaces = visible_spaces(state, p).await?;
    spaces.retain(|(s, _)| s.archived_at.is_none());
    if !keys.is_empty() {
        spaces.retain(|(s, _)| keys.iter().any(|k| s.key.eq_ignore_ascii_case(k.trim())));
    }
    Ok(spaces)
}

/// Keyword ranking: candidate pages from the full-text search, their passages by BM25 (a
/// better-ranked page lifts its passages a little).
async fn keyword_ranking(state: &AppState, p: &Principal, opts: &Options, spaces: &[(SpaceRow, Role)], labels: &[String]) -> ApiResult<Vec<(Uuid, i32, f64)>> {
    let mut terms = keywords(&opts.query);
    if terms.is_empty() {
        // a question made only of stop words: search for them after all
        terms = tokens(&opts.query).into_iter().filter(|t| t.chars().count() > 1).take(16).collect();
    }
    if terms.is_empty() {
        return Ok(vec![]);
    }
    let pages = if labels.is_empty() { 30 } else { 100 };
    let hits = hits_in(state, p, &terms.join(" OR "), spaces, pages).await?;
    let mut ids: Vec<Uuid> = hits.iter().map(|h| h.page_id).collect();
    if !labels.is_empty() {
        let keep: HashSet<Uuid> = sqlx::query_scalar("SELECT DISTINCT page_id FROM labels WHERE page_id = ANY($1) AND name = ANY($2)")
            .bind(&ids)
            .bind(labels)
            .fetch_all(&state.db)
            .await?
            .into_iter()
            .collect();
        ids.retain(|i| keep.contains(i));
        ids.truncate(30);
    }
    if ids.is_empty() {
        return Ok(vec![]);
    }
    let rank: HashMap<Uuid, usize> = ids.iter().enumerate().map(|(i, id)| (*id, i)).collect();
    let rows: Vec<(Uuid, i32, String, Vec<String>, String)> =
        sqlx::query_as("SELECT c.page_id, c.ord, p.title, c.headings, c.content FROM page_chunks c JOIN pages p ON p.id = c.page_id WHERE c.page_id = ANY($1)")
            .bind(&ids)
            .fetch_all(&state.db)
            .await?;
    let terms2 = terms.clone();
    let scored = tokio::task::spawn_blocking(move || {
        let docs: Vec<Vec<String>> = rows.iter().map(|(_, _, title, headings, content)| tokens(&embed_input(title, headings, content))).collect();
        let scores = bm25(&terms2, &docs);
        rows.into_iter().zip(scores).map(|((page, ord, ..), s)| (page, ord, s)).collect::<Vec<_>>()
    })
    .await
    .map_err(|e| ApiError::Internal(e.into()))?;
    let mut out: Vec<(Uuid, i32, f64)> = scored
        .into_iter()
        .filter(|(_, _, s)| *s > 0.0)
        .map(|(page, ord, s)| (page, ord, s * (1.0 + 1.0 / (2.0 + rank.get(&page).copied().unwrap_or(99) as f64))))
        .collect();
    out.sort_by(|a, b| b.2.total_cmp(&a.2));
    out.truncate(CANDIDATES);
    Ok(out)
}

/// Semantic ranking: nearest passage vectors in the spaces, then permission-filtered.
async fn semantic_ranking(state: &AppState, p: &Principal, vector: &[f32], spaces: &[(SpaceRow, Role)], labels: &[String]) -> ApiResult<Vec<(Uuid, i32, f64)>> {
    let space_ids: Vec<Uuid> = spaces.iter().map(|(s, _)| s.id).collect();
    let literal = crate::embeddings::vector_literal(vector);
    let mut tx = state.db.begin().await?;
    crate::embeddings::tune_search(&mut tx).await?;
    let rows: Vec<(Uuid, i32, Uuid, f64)> = sqlx::query_as(
        "SELECT c.page_id, c.ord, p.space_id, (v.embedding <=> $1::halfvec)::float8 AS dist
         FROM chunk_vectors v JOIN page_chunks c ON c.hash = v.hash JOIN pages p ON p.id = c.page_id
         WHERE p.space_id = ANY($2) AND p.deleted_at IS NULL AND p.archived_at IS NULL
           AND (cardinality($3::text[]) = 0 OR EXISTS (SELECT 1 FROM labels l WHERE l.page_id = p.id AND l.name = ANY($3)))
         ORDER BY v.embedding <=> $1::halfvec LIMIT $4",
    )
    .bind(&literal)
    .bind(&space_ids)
    .bind(labels)
    .bind((CANDIDATES * 2) as i64)
    .fetch_all(&mut *tx)
    .await?;
    tx.commit().await?;
    let pages: Vec<(Uuid, Uuid)> = rows.iter().map(|r| (r.0, r.2)).collect::<HashSet<_>>().into_iter().collect();
    let visible = filter_visible(state, p, spaces, &pages).await?;
    let archived = crate::authz::archived_among(state, &pages.iter().map(|x| x.0).collect::<Vec<_>>()).await?;
    let mut out: Vec<(Uuid, i32, f64)> =
        rows.into_iter().filter(|r| visible.contains(&r.0) && !archived.contains(&r.0)).map(|(page, ord, _, dist)| (page, ord, 1.0 - dist)).collect();
    // relaxed index order: sort exactly
    out.sort_by(|a, b| b.2.total_cmp(&a.2));
    out.truncate(CANDIDATES);
    Ok(out)
}

/// The passages that best match `opts.query` among the pages `p` may open.
pub async fn retrieve(state: &AppState, p: &Principal, opts: &Options) -> ApiResult<Retrieval> {
    let query = opts.query.trim();
    if query.is_empty() || query.chars().count() > 2000 {
        return Err(ApiError::bad("the query must have 1 to 2000 characters"));
    }
    let spaces = spaces_for(state, p, &opts.spaces).await?;
    let labels: Vec<String> = opts.labels.iter().filter_map(|l| crate::routes::pages::normalize_label(l)).collect();
    let mut note = None;
    let semantic_ready = crate::embeddings::ready(state).await;
    if !semantic_ready && matches!(opts.mode, Mode::Hybrid | Mode::Semantic) {
        note = Some(crate::embeddings::unavailable_reason(state).await);
    }
    let want_semantic = semantic_ready && opts.mode != Mode::Keyword;
    let mut keyword_ran = opts.mode != Mode::Semantic || !want_semantic;
    if spaces.is_empty() {
        let mode = if want_semantic && keyword_ran {
            "hybrid"
        } else if want_semantic {
            "semantic"
        } else {
            "keyword"
        };
        return Ok(Retrieval { mode, passages: vec![], note });
    }
    let mut keyword = if keyword_ran { keyword_ranking(state, p, opts, &spaces, &labels).await? } else { vec![] };
    let mut semantic = vec![];
    let mut semantic_ran = false;
    if want_semantic {
        match crate::embeddings::embed_query(state, p.user_id(), query).await {
            Ok(v) => {
                semantic = semantic_ranking(state, p, &v, &spaces, &labels).await?;
                semantic_ran = true;
            }
            Err(e) => {
                tracing::warn!("semantic retrieval skipped: {e:#}");
                note = Some(format!("semantic retrieval is unavailable right now ({e}); keyword results only"));
                if !keyword_ran {
                    keyword = keyword_ranking(state, p, opts, &spaces, &labels).await?;
                    keyword_ran = true;
                }
            }
        }
    }
    let mode = match (keyword_ran, semantic_ran) {
        (true, true) => "hybrid",
        (false, true) => "semantic",
        _ => "keyword",
    };

    // reciprocal rank fusion
    let mut fused: HashMap<(Uuid, i32), (f64, Vec<&'static str>)> = HashMap::new();
    for (list, name) in [(&keyword, "keyword"), (&semantic, "semantic")] {
        for (rank, (page, ord, _)) in list.iter().enumerate() {
            let e = fused.entry((*page, *ord)).or_insert((0.0, vec![]));
            e.0 += 1.0 / (RRF_K + rank as f64 + 1.0);
            e.1.push(name);
        }
    }
    let mut ranked: Vec<((Uuid, i32), (f64, Vec<&'static str>))> = fused.into_iter().collect();
    ranked.sort_by(|a, b| b.1.0.total_cmp(&a.1.0).then(a.0.cmp(&b.0)));
    let limit = opts.limit.clamp(1, 50);
    let per_page = opts.per_page.clamp(1, 50);
    let mut per: HashMap<Uuid, usize> = HashMap::new();
    let mut picked: Vec<((Uuid, i32), (f64, Vec<&'static str>))> = Vec::new();
    for r in ranked {
        let n = per.entry(r.0.0).or_default();
        if *n >= per_page {
            continue;
        }
        *n += 1;
        picked.push(r);
        if picked.len() >= limit {
            break;
        }
    }
    let passages = load_passages(state, picked, opts.max_chars).await?;
    Ok(Retrieval { mode, passages, note })
}

async fn load_passages(state: &AppState, picked: Vec<((Uuid, i32), (f64, Vec<&'static str>))>, max_chars: Option<usize>) -> ApiResult<Vec<Passage>> {
    if picked.is_empty() {
        return Ok(vec![]);
    }
    let pages: Vec<Uuid> = picked.iter().map(|p| p.0.0).collect();
    let ords: Vec<i32> = picked.iter().map(|p| p.0.1).collect();
    let chunks: HashMap<(Uuid, i32), ChunkRow> = sqlx::query_as::<_, ChunkRow>(sqlx::AssertSqlSafe(format!(
        "SELECT {CHUNK_COLUMNS} FROM page_chunks c JOIN unnest($1::uuid[], $2::int[]) AS k(page_id, ord) ON k.page_id = c.page_id AND k.ord = c.ord"
    )))
    .bind(&pages)
    .bind(&ords)
    .fetch_all(&state.db)
    .await?
    .into_iter()
    .map(|c| ((c.page_id, c.ord), c))
    .collect();
    #[derive(sqlx::FromRow)]
    struct Meta {
        id: Uuid,
        title: String,
        space_key: String,
        space_name: String,
        updated_at: chrono::DateTime<chrono::Utc>,
        version: i32,
        labels: Vec<String>,
        path: Vec<String>,
    }
    let metas: HashMap<Uuid, Meta> = sqlx::query_as::<_, Meta>(
        "WITH RECURSIVE anc(page_id, id, depth) AS (
             SELECT p.id, p.parent_id, 1 FROM pages p WHERE p.id = ANY($1) AND p.parent_id IS NOT NULL
             UNION ALL
             SELECT a.page_id, q.parent_id, a.depth + 1 FROM anc a JOIN pages q ON q.id = a.id WHERE q.parent_id IS NOT NULL AND a.depth < 50
         )
         SELECT p.id, p.title, s.key AS space_key, s.name AS space_name, p.updated_at, p.version,
                coalesce((SELECT array_agg(l.name ORDER BY l.name) FROM labels l WHERE l.page_id = p.id), '{}') AS labels,
                coalesce((SELECT array_agg(q.title ORDER BY a.depth DESC) FROM anc a JOIN pages q ON q.id = a.id WHERE a.page_id = p.id), '{}') AS path
         FROM pages p JOIN spaces s ON s.id = p.space_id WHERE p.id = ANY($1)",
    )
    .bind(&pages)
    .fetch_all(&state.db)
    .await?
    .into_iter()
    .map(|m| (m.id, m))
    .collect();
    let att_ids: Vec<Uuid> = chunks.values().filter_map(|c| c.attachment_id).collect();
    let att_names: HashMap<Uuid, String> = sqlx::query_as::<_, (Uuid, String)>("SELECT id, filename FROM attachments WHERE id = ANY($1)")
        .bind(&att_ids)
        .fetch_all(&state.db)
        .await?
        .into_iter()
        .collect();
    let base = state.config.base_url.trim_end_matches('/');
    let mut used = 0usize;
    let mut out = Vec::new();
    for ((page, ord), (score, matched_by)) in picked {
        let (Some(c), Some(m)) = (chunks.get(&(page, ord)), metas.get(&page)) else { continue };
        if max_chars.is_some_and(|max| !out.is_empty() && used + c.content.len() > max) {
            break;
        }
        used += c.content.len();
        let url = match &c.block_id {
            Some(b) => format!("{base}/pages/{page}#block-{b}"),
            None => format!("{base}/pages/{page}"),
        };
        let attachment = c.attachment_id.map(|id| AttachmentRef {
            id,
            filename: att_names.get(&id).cloned().unwrap_or_default(),
            url: format!("{base}/api/attachments/{id}/content"),
        });
        out.push(Passage {
            page_id: page,
            title: m.title.clone(),
            space_key: m.space_key.clone(),
            space_name: m.space_name.clone(),
            path: m.path.clone(),
            headings: c.headings.clone(),
            block_id: c.block_id.clone(),
            url,
            text: c.content.clone(),
            labels: m.labels.clone(),
            updated_at: m.updated_at,
            version: m.version,
            attachment,
            score: (score * 10_000.0).round() / 10_000.0,
            matched_by,
        });
    }
    Ok(out)
}

/// Passages as Markdown for a language model: numbered, each with where it comes from.
pub fn to_markdown(passages: &[Passage]) -> String {
    let mut s = String::new();
    for (i, p) in passages.iter().enumerate() {
        let mut place = p.title.clone();
        for h in &p.headings {
            place.push_str(" › ");
            place.push_str(h);
        }
        s.push_str(&format!("[{}] {place}\n", i + 1));
        let mut meta = format!("Space: {}", p.space_key);
        if !p.path.is_empty() {
            meta.push_str(&format!(" · Path: {}", p.path.join(" / ")));
        }
        if let Some(a) = &p.attachment {
            meta.push_str(&format!(" · From the attached file {}", a.filename));
        }
        meta.push_str(&format!(" · Updated {} · Page id {}", p.updated_at.format("%Y-%m-%d"), p.page_id));
        s.push_str(&meta);
        s.push_str(&format!("\nURL: {}\n\n{}\n\n---\n\n", p.url, p.text.trim()));
    }
    s.trim_end().trim_end_matches("---").trim_end().to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn question_keywords() {
        assert_eq!(keywords("How do I connect to the VPN?"), ["connect", "vpn"]);
        assert_eq!(keywords("Which IP does the CI use?"), ["ip", "ci"]);
        assert_eq!(keywords("Wie beantrage ich Ferien?"), ["beantrage", "ferien"]);
        assert!(keywords("??").is_empty());
    }

    #[test]
    fn bm25_prefers_rare_terms_and_short_passages() {
        let docs: Vec<Vec<String>> = [
            "the vpn uses wireguard and the company profile",
            "the vpn is down on sundays the vpn the vpn",
            "parking rules for the office and the garage and the bikes and more",
        ]
        .iter()
        .map(|d| tokens(d))
        .collect();
        let s = bm25(&["vpn".into(), "wireguard".into()], &docs);
        assert!(s[0] > s[1], "the rarer term wins: {s:?}");
        assert_eq!(s[2], 0.0);
    }

    #[test]
    fn embed_input_carries_title_and_headings() {
        assert_eq!(embed_input("VPN", &[], "Text"), "VPN\n\nText");
        assert_eq!(embed_input("VPN", &["Setup".into(), "Linux".into()], "Text"), "VPN\nSetup > Linux\n\nText");
    }
}
