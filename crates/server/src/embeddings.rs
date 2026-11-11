//! Passage vectors for semantic retrieval, from any OpenAI-compatible embeddings API (OpenAI,
//! Azure OpenAI, Ollama, vLLM, LM Studio, Voyage, Mistral, …), stored with pgvector.
//!
//! Off unless `WIKICRAFT_EMBEDDINGS_URL` and `WIKICRAFT_EMBEDDINGS_MODEL` are set. A background
//! worker embeds every passage whose text has no vector yet; vectors are addressed by the hash
//! of the embedded text, so editing one section of a page only re-embeds that section, and
//! identical passages share one vector. The `chunk_vectors` table is created on the first
//! batch (its size comes from the model) and dropped when the model changes. With several
//! replicas, an advisory lock lets one of them do the embedding.

use std::collections::HashMap;
use std::sync::{LazyLock, Mutex};
use std::time::{Duration, Instant};

use serde_json::{Value, json};
use sqlx::{Connection, PgPool};
use tokio::sync::Notify;
use uuid::Uuid;

use crate::app::AppState;
use crate::config::Config;

/// passages per request to the embeddings API
const BATCH: usize = 64;
/// characters of a passage sent to be embedded
const MAX_INPUT_CHARS: usize = 8000;
/// largest vector the HNSW index takes (pgvector `halfvec`)
const MAX_DIMS: usize = 4000;
/// question embeddings per person per window (per replica); past it, keyword retrieval only
const QUERY_RATE: usize = 300;
const QUERY_WINDOW: Duration = Duration::from_secs(600);
const LOCK_KEY: i64 = 0x7769_6b69_656d_6264; // "wikiembd"

static WAKE: LazyLock<Notify> = LazyLock::new(Notify::new);

#[derive(Default)]
struct Status {
    error: Option<String>,
    error_at: Option<chrono::DateTime<chrono::Utc>>,
}

static STATUS: LazyLock<Mutex<Status>> = LazyLock::new(Default::default);
static QUERY_CACHE: LazyLock<Mutex<HashMap<String, Vec<f32>>>> = LazyLock::new(Default::default);
static RECENT: LazyLock<Mutex<HashMap<Uuid, Vec<Instant>>>> = LazyLock::new(Default::default);

fn set_error(e: Option<String>) {
    let mut s = STATUS.lock().unwrap_or_else(|e| e.into_inner());
    s.error_at = e.as_ref().map(|_| chrono::Utc::now());
    s.error = e;
}

/// Whether an embeddings API is configured.
pub fn configured(config: &Config) -> bool {
    config.embeddings_url.as_deref().is_some_and(|u| !u.trim().is_empty()) && config.embeddings_model.as_deref().is_some_and(|m| !m.trim().is_empty())
}

/// What the stored vectors depend on: a different value means they must be made again.
fn model_key(config: &Config) -> String {
    format!(
        "{}|{}|{}",
        config.embeddings_model.as_deref().unwrap_or("").trim(),
        config.embeddings_dimensions.map(|d| d.to_string()).unwrap_or_default(),
        config.embeddings_document_prefix
    )
}

/// Wakes the worker (new passages to embed).
pub fn wake() {
    WAKE.notify_one();
}

/// `[0.1,0.2,…]`, the text form pgvector reads.
pub fn vector_literal(v: &[f32]) -> String {
    let mut s = String::with_capacity(v.len() * 10 + 2);
    s.push('[');
    for (i, x) in v.iter().enumerate() {
        if i > 0 {
            s.push(',');
        }
        s.push_str(&format!("{x}"));
    }
    s.push(']');
    s
}

/// Whether semantic retrieval can run: configured, and vectors exist for the current model.
pub async fn ready(state: &AppState) -> bool {
    if !configured(&state.config) {
        return false;
    }
    sqlx::query_scalar::<_, bool>("SELECT model = $1 AND dims IS NOT NULL FROM retrieval_state")
        .bind(model_key(&state.config))
        .fetch_one(&state.db)
        .await
        .unwrap_or(false)
}

/// Why semantic retrieval isn't available (for the `note` of a response).
pub async fn unavailable_reason(state: &AppState) -> String {
    if !configured(&state.config) {
        return "semantic retrieval is off on this server (no embeddings API configured); keyword results only".into();
    }
    match STATUS.lock().unwrap_or_else(|e| e.into_inner()).error.clone() {
        Some(e) => format!("semantic retrieval isn't available ({e}); keyword results only"),
        None => "the wiki's passages are still being embedded; keyword results only for now".into(),
    }
}

/// Better recall for filtered nearest-neighbour searches (pgvector ≥ 0.8 keeps scanning the
/// index until enough rows pass the filter).
pub async fn tune_search(tx: &mut sqlx::Transaction<'_, sqlx::Postgres>) -> Result<(), sqlx::Error> {
    sqlx::query("SET LOCAL hnsw.ef_search = 200").execute(&mut **tx).await?;
    let v: Option<String> = sqlx::query_scalar("SELECT extversion FROM pg_extension WHERE extname = 'vector'").fetch_optional(&mut **tx).await?;
    if v.as_deref().is_some_and(|v| version_at_least(v, 0, 8)) {
        sqlx::query("SET LOCAL hnsw.iterative_scan = relaxed_order").execute(&mut **tx).await?;
    }
    Ok(())
}

fn version_at_least(v: &str, major: u32, minor: u32) -> bool {
    let mut it = v.split('.').map(|p| p.parse::<u32>().unwrap_or(0));
    let (a, b) = (it.next().unwrap_or(0), it.next().unwrap_or(0));
    (a, b) >= (major, minor)
}

fn allow_query(user: Option<Uuid>) -> bool {
    let mut m = RECENT.lock().unwrap_or_else(|e| e.into_inner());
    let q = m.entry(user.unwrap_or_default()).or_default();
    let now = Instant::now();
    q.retain(|t| now.duration_since(*t) < QUERY_WINDOW);
    if q.len() >= QUERY_RATE {
        return false;
    }
    q.push(now);
    true
}

/// The vector of a question.
pub async fn embed_query(state: &AppState, user: Option<Uuid>, query: &str) -> anyhow::Result<Vec<f32>> {
    let input = format!("{}{}", state.config.embeddings_query_prefix, query.trim());
    if let Some(v) = QUERY_CACHE.lock().unwrap_or_else(|e| e.into_inner()).get(&input) {
        return Ok(v.clone());
    }
    if !allow_query(user) {
        anyhow::bail!("too many questions in a short time");
    }
    let v = embed(&state.config, std::slice::from_ref(&input)).await?.pop().ok_or_else(|| anyhow::anyhow!("no vector returned"))?;
    let mut cache = QUERY_CACHE.lock().unwrap_or_else(|e| e.into_inner());
    if cache.len() >= 500 {
        cache.clear();
    }
    cache.insert(input, v.clone());
    Ok(v)
}

/// One call to the embeddings API, with retries for rate limits and server errors.
async fn embed(config: &Config, inputs: &[String]) -> anyhow::Result<Vec<Vec<f32>>> {
    let base = config.embeddings_url.as_deref().unwrap_or("").trim().trim_end_matches('/');
    let url = if base.ends_with("/embeddings") { base.to_string() } else { format!("{base}/embeddings") };
    let mut body = json!({ "model": config.embeddings_model.as_deref().unwrap_or("").trim(), "input": inputs, "encoding_format": "float" });
    if let Some(d) = config.embeddings_dimensions {
        body["dimensions"] = json!(d);
    }
    let client = openidconnect::reqwest::Client::builder().timeout(Duration::from_secs(120)).build()?;
    let body = serde_json::to_vec(&body)?;
    let mut attempt = 0u64;
    let resp: Value = loop {
        attempt += 1;
        let mut req = client.post(&url).header("content-type", "application/json").body(body.clone());
        if let Some(k) = config.embeddings_api_key.as_deref().filter(|k| !k.is_empty()) {
            req = req.bearer_auth(k);
        }
        let r = match req.send().await {
            Ok(r) => r,
            Err(e) if attempt < 3 => {
                tracing::warn!("embeddings: request failed ({e}), retrying");
                tokio::time::sleep(Duration::from_millis(800 * attempt)).await;
                continue;
            }
            Err(e) => anyhow::bail!("embeddings API unreachable: {e}"),
        };
        let status = r.status().as_u16();
        if (status == 429 || status >= 500) && attempt < 4 {
            let wait = r.headers().get("retry-after").and_then(|v| v.to_str().ok()).and_then(|v| v.parse::<u64>().ok()).unwrap_or(attempt * 2).min(30);
            tracing::warn!(status, "embeddings API busy, retrying in {wait}s");
            tokio::time::sleep(Duration::from_secs(wait)).await;
            continue;
        }
        let v: Value = r.bytes().await.ok().and_then(|b| serde_json::from_slice(&b).ok()).unwrap_or(Value::Null);
        if !(200..300).contains(&status) {
            let msg = v["error"]["message"].as_str().or(v["error"].as_str()).or(v["detail"].as_str()).unwrap_or("unknown error");
            anyhow::bail!("embeddings API answered HTTP {status}: {msg}");
        }
        break v;
    };
    let mut data: Vec<(usize, Vec<f32>)> = resp["data"]
        .as_array()
        .ok_or_else(|| anyhow::anyhow!("embeddings API returned no data"))?
        .iter()
        .enumerate()
        .map(|(i, d)| {
            let idx = d["index"].as_u64().map(|x| x as usize).unwrap_or(i);
            let v: Vec<f32> = d["embedding"].as_array().map(|a| a.iter().filter_map(|x| x.as_f64()).map(|x| x as f32).collect()).unwrap_or_default();
            (idx, v)
        })
        .collect();
    data.sort_by_key(|d| d.0);
    if data.len() != inputs.len() || data.iter().any(|d| d.1.is_empty()) {
        anyhow::bail!("embeddings API returned {} vectors for {} inputs", data.len(), inputs.len());
    }
    let dims = data[0].1.len();
    if data.iter().any(|d| d.1.len() != dims) {
        anyhow::bail!("embeddings API returned vectors of different sizes");
    }
    Ok(data.into_iter().map(|d| d.1).collect())
}

/// Starts the embedding worker (when configured).
pub fn spawn_worker(state: &AppState) {
    if !configured(&state.config) {
        return;
    }
    let state = state.clone();
    tokio::spawn(async move {
        let mut backoff = 30u64;
        loop {
            let started = Instant::now();
            match run(&state).await {
                Ok(()) => return,
                Err(e) => {
                    tracing::error!("embedding passages failed: {e:#}");
                    set_error(Some(format!("{e:#}")));
                    // a failure after a long good run starts the back-off over
                    if started.elapsed() > Duration::from_secs(1800) {
                        backoff = 30;
                    }
                    tokio::time::sleep(Duration::from_secs(backoff)).await;
                    backoff = (backoff * 2).min(1800);
                }
            }
        }
    });
}

/// Holds the advisory lock and embeds until the process ends (errors return for a retry).
async fn run(state: &AppState) -> anyhow::Result<()> {
    let mut lock_conn = sqlx::PgConnection::connect(&state.config.database_url).await?;
    loop {
        let got: bool = sqlx::query_scalar("SELECT pg_try_advisory_lock($1)").bind(LOCK_KEY).fetch_one(&mut lock_conn).await?;
        if got {
            break;
        }
        // another replica embeds; check back later
        tokio::time::sleep(Duration::from_secs(60)).await;
    }
    prepare(state).await?;
    let mut last_gc = Instant::now();
    let mut ok_since_error = false;
    loop {
        let n = embed_pending(state).await?;
        if n > 0 && !ok_since_error {
            set_error(None);
            ok_since_error = true;
        }
        if n == 0 {
            if last_gc.elapsed() > Duration::from_secs(600) {
                gc(&state.db).await?;
                last_gc = Instant::now();
            }
            // keep the lock connection alive and notice new passages (also from other replicas)
            let _ = tokio::time::timeout(Duration::from_secs(60), WAKE.notified()).await;
            sqlx::query("SELECT 1").execute(&mut lock_conn).await?;
        }
    }
}

/// pgvector present (installing it if allowed), and vectors of another model dropped.
async fn prepare(state: &AppState) -> anyhow::Result<()> {
    let db = &state.db;
    let mut version: Option<String> = sqlx::query_scalar("SELECT extversion FROM pg_extension WHERE extname = 'vector'").fetch_optional(db).await?;
    if version.is_none() {
        if let Err(e) = sqlx::query("CREATE EXTENSION IF NOT EXISTS vector").execute(db).await {
            anyhow::bail!(
                "the database lacks the pgvector extension ({e}); install pgvector and run CREATE EXTENSION vector; as a superuser (Docker: the pgvector/pgvector image)"
            );
        }
        version = sqlx::query_scalar("SELECT extversion FROM pg_extension WHERE extname = 'vector'").fetch_optional(db).await?;
    }
    let version = version.unwrap_or_default();
    if !version_at_least(&version, 0, 7) {
        anyhow::bail!("pgvector {version} is too old: semantic retrieval needs 0.7 or later");
    }
    let key = model_key(&state.config);
    let stored: Option<String> = sqlx::query_scalar("SELECT model FROM retrieval_state").fetch_one(db).await?;
    if stored.as_deref() != Some(key.as_str()) {
        if stored.is_some() {
            tracing::info!("embedding model changed: all passages will be embedded again");
        }
        sqlx::query("DROP TABLE IF EXISTS chunk_vectors").execute(db).await?;
        sqlx::query("UPDATE retrieval_state SET model = $1, dims = NULL").bind(&key).execute(db).await?;
    }
    Ok(())
}

async fn table_exists(db: &PgPool) -> anyhow::Result<bool> {
    Ok(sqlx::query_scalar("SELECT to_regclass('chunk_vectors') IS NOT NULL").fetch_one(db).await?)
}

/// Embeds one batch of passages without a vector; returns how many.
async fn embed_pending(state: &AppState) -> anyhow::Result<usize> {
    let db = &state.db;
    let exists = table_exists(db).await?;
    let sql = if exists {
        "SELECT c.hash, p.title, c.headings, c.content FROM page_chunks c JOIN pages p ON p.id = c.page_id
         WHERE NOT EXISTS (SELECT 1 FROM chunk_vectors v WHERE v.hash = c.hash) LIMIT $1"
    } else {
        "SELECT c.hash, p.title, c.headings, c.content FROM page_chunks c JOIN pages p ON p.id = c.page_id LIMIT $1"
    };
    let rows: Vec<(String, String, Vec<String>, String)> = sqlx::query_as(sql).bind(BATCH as i64).fetch_all(db).await?;
    let mut seen = std::collections::HashSet::new();
    let rows: Vec<_> = rows.into_iter().filter(|r| seen.insert(r.0.clone())).collect();
    if rows.is_empty() {
        return Ok(0);
    }
    let prefix = &state.config.embeddings_document_prefix;
    let inputs: Vec<String> = rows
        .iter()
        .map(|(_, title, headings, content)| {
            let text = crate::retrieval::embed_input(title, headings, content);
            format!("{prefix}{}", text.chars().take(MAX_INPUT_CHARS).collect::<String>())
        })
        .collect();
    let vectors = embed(&state.config, &inputs).await?;
    let dims = vectors[0].len();
    if !exists {
        if dims > MAX_DIMS {
            anyhow::bail!("the model's vectors have {dims} dimensions; at most {MAX_DIMS} are supported (set WIKICRAFT_EMBEDDINGS_DIMENSIONS)");
        }
        // dims is a number we checked: safe to put into the DDL
        sqlx::query(sqlx::AssertSqlSafe(format!("CREATE TABLE IF NOT EXISTS chunk_vectors (hash text PRIMARY KEY, embedding halfvec({dims}) NOT NULL)")))
            .execute(db)
            .await?;
        sqlx::query("CREATE INDEX IF NOT EXISTS chunk_vectors_hnsw ON chunk_vectors USING hnsw (embedding halfvec_cosine_ops)").execute(db).await?;
        sqlx::query("UPDATE retrieval_state SET dims = $1").bind(dims as i32).execute(db).await?;
    } else {
        let stored: Option<i32> = sqlx::query_scalar("SELECT dims FROM retrieval_state").fetch_one(db).await?;
        if stored.is_some_and(|d| d as usize != dims) {
            anyhow::bail!("the embeddings API now returns {dims}-dimensional vectors instead of {}", stored.unwrap_or(0));
        }
    }
    let hashes: Vec<String> = rows.iter().map(|r| r.0.clone()).collect();
    let literals: Vec<String> = vectors.iter().map(|v| vector_literal(v)).collect();
    sqlx::query(
        "INSERT INTO chunk_vectors (hash, embedding) SELECT h, e::halfvec FROM unnest($1::text[], $2::text[]) AS t(h, e) ON CONFLICT (hash) DO NOTHING",
    )
    .bind(&hashes)
    .bind(&literals)
    .execute(db)
    .await?;
    Ok(rows.len())
}

/// Drops vectors no passage uses any more.
async fn gc(db: &PgPool) -> anyhow::Result<()> {
    if table_exists(db).await? {
        sqlx::query("DELETE FROM chunk_vectors v WHERE NOT EXISTS (SELECT 1 FROM page_chunks c WHERE c.hash = v.hash)").execute(db).await?;
    }
    Ok(())
}

/// For Admin → Overview.
pub async fn status(state: &AppState) -> anyhow::Result<Value> {
    let db = &state.db;
    let (passages, distinct, pages): (i64, i64, i64) =
        sqlx::query_as("SELECT count(*), count(DISTINCT hash), count(DISTINCT page_id) FROM page_chunks").fetch_one(db).await?;
    let pending_pages: i64 = sqlx::query_scalar("SELECT count(*) FROM pages WHERE chunked_at IS NULL AND deleted_at IS NULL").fetch_one(db).await?;
    let mut semantic = json!({ "configured": configured(&state.config) });
    if configured(&state.config) {
        let dims: Option<i32> =
            sqlx::query_scalar("SELECT dims FROM retrieval_state WHERE model = $1").bind(model_key(&state.config)).fetch_optional(db).await?.flatten();
        let embedded: i64 = if dims.is_some() && table_exists(db).await? {
            sqlx::query_scalar("SELECT count(*) FROM chunk_vectors v WHERE EXISTS (SELECT 1 FROM page_chunks c WHERE c.hash = v.hash)").fetch_one(db).await?
        } else {
            0
        };
        let s = STATUS.lock().unwrap_or_else(|e| e.into_inner());
        semantic = json!({
            "configured": true,
            "model": state.config.embeddings_model,
            "dims": dims,
            "embedded": embedded,
            "pending": (distinct - embedded).max(0),
            "error": s.error,
            "errorAt": s.error_at,
        });
    }
    Ok(json!({ "passages": passages, "pages": pages, "pagesPending": pending_pages, "semantic": semantic }))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn literals_and_versions() {
        assert_eq!(vector_literal(&[0.5, -1.0, 0.25]), "[0.5,-1,0.25]");
        assert!(version_at_least("0.8.7", 0, 8) && version_at_least("1.0", 0, 8) && !version_at_least("0.7.4", 0, 8));
    }
}
