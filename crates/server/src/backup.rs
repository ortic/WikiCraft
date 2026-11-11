//! Backup and restore of a whole installation, for disaster recovery and for moving to another
//! server or storage (local disk ⇄ S3):
//!
//! * `wikicraft backup <file>` writes one zip: a consistent snapshot of every table (`COPY` inside
//!   one repeatable-read, read-only transaction, so it can run while people work) and every
//!   stored file the snapshot refers to (content-addressed, so files added later don't matter).
//! * `wikicraft restore <file>` loads it into an empty database and the configured file storage:
//!   it creates the schema the backup was made with, loads the tables (foreign keys are dropped
//!   while loading, since tables refer to each other in cycles, and checked again afterwards),
//!   resets sequences, then upgrades the schema to this version.
//!
//! The zip holds `manifest.json`, `db/<table>.copy` (PostgreSQL's binary COPY format: restore into
//! the same PostgreSQL major version or a newer one) and `blobs/<sha256>`. Semantic-search
//! vectors (`chunk_vectors`, pgvector) travel as text; without pgvector on the target they are
//! left out and computed again once embeddings are configured. The search index isn't included:
//! it rebuilds itself on the first start after a restore.

use std::io::{Read, Write};
use std::path::Path;

use futures::StreamExt;
use serde::{Deserialize, Serialize};
use sqlx::{Connection, PgConnection};
use zip::write::SimpleFileOptions;

use crate::blobs::BlobStore;
use crate::config::Config;

/// Version of the archive layout.
const FORMAT: u32 = 1;
/// pgvector table, created at runtime (see [`crate::embeddings`])
const VECTORS: &str = "chunk_vectors";
const MIGRATIONS: &str = "_sqlx_migrations";

#[derive(Serialize, Deserialize, Debug, Clone)]
#[serde(rename_all = "camelCase")]
pub struct Manifest {
    pub format: u32,
    /// WikiCraft version that wrote it
    pub version: String,
    /// the last database migration applied
    pub schema: i64,
    pub created_at: chrono::DateTime<chrono::Utc>,
    pub tables: Vec<TableInfo>,
    pub files: u64,
    pub file_bytes: u64,
    /// files the database refers to that the storage didn't have
    pub missing_files: Vec<String>,
    /// dimensions of the semantic-search vectors, when they are included
    pub vector_dims: Option<i32>,
}

#[derive(Serialize, Deserialize, Debug, Clone)]
pub struct TableInfo {
    pub name: String,
    pub rows: i64,
}

/// What a restore did.
#[derive(Serialize, Debug)]
#[serde(rename_all = "camelCase")]
pub struct Restored {
    pub manifest: Manifest,
    pub rows: i64,
    pub files: u64,
    /// whether the semantic-search vectors came back (else they are computed again)
    pub vectors: bool,
}

fn ident(name: &str) -> String {
    format!("\"{}\"", name.replace('"', "\"\""))
}

fn is_content_key(key: &str) -> bool {
    key.len() == 64 && key.bytes().all(|b| b.is_ascii_hexdigit())
}

/// Writes a backup of the database and stored files to `out` (through `<out>.partial`, renamed
/// when complete).
pub async fn backup(config: &Config, out: &Path) -> anyhow::Result<Manifest> {
    let mut conn = PgConnection::connect(&config.database_url).await?;
    let store = BlobStore::new(config).await?;
    let partial = out.with_extension("partial");
    let mut zip = zip::ZipWriter::new(std::io::BufWriter::new(std::fs::File::create(&partial)?));
    let deflate = SimpleFileOptions::default().compression_method(zip::CompressionMethod::Deflated).large_file(true);
    // most stored files are compressed already
    let stored = SimpleFileOptions::default().compression_method(zip::CompressionMethod::Stored).large_file(true);

    let mut tx = conn.begin().await?;
    sqlx::query("SET TRANSACTION ISOLATION LEVEL REPEATABLE READ, READ ONLY").execute(&mut *tx).await?;
    let schema: i64 = sqlx::query_scalar("SELECT coalesce(max(version), 0) FROM _sqlx_migrations WHERE success").fetch_one(&mut *tx).await?;
    let names: Vec<String> =
        sqlx::query_scalar("SELECT tablename::text FROM pg_tables WHERE schemaname = current_schema() ORDER BY tablename").fetch_all(&mut *tx).await?;
    let vector_dims: Option<i32> = if names.iter().any(|n| n == VECTORS) {
        sqlx::query_scalar("SELECT dims FROM retrieval_state").fetch_optional(&mut *tx).await?.flatten()
    } else {
        None
    };
    let mut tables = Vec::new();
    for name in &names {
        if name == VECTORS && vector_dims.is_none() {
            continue;
        }
        let rows: i64 = sqlx::query_scalar(sqlx::AssertSqlSafe(format!("SELECT count(*) FROM {}", ident(name)))).fetch_one(&mut *tx).await?;
        let (entry, sql) = if name == VECTORS {
            (format!("db/{name}.txt"), format!("COPY {} TO STDOUT", ident(name)))
        } else {
            (format!("db/{name}.copy"), format!("COPY {} TO STDOUT (FORMAT binary)", ident(name)))
        };
        zip.start_file(entry, deflate)?;
        let mut data = tx.copy_out_raw(&sql).await?;
        while let Some(chunk) = data.next().await {
            zip.write_all(&chunk?)?;
        }
        drop(data);
        tables.push(TableInfo { name: name.clone(), rows });
    }
    let keys: Vec<String> = sqlx::query_scalar("SELECT key FROM blobs ORDER BY key").fetch_all(&mut *tx).await?;
    tx.commit().await?;

    let (mut files, mut file_bytes, mut missing_files) = (0u64, 0u64, Vec::new());
    for key in keys {
        if !is_content_key(&key) {
            continue;
        }
        let mut data = match store.stream(&key).await {
            Ok(s) => s,
            Err(e) => {
                tracing::warn!("file {key} is missing from storage: {e:#}");
                missing_files.push(key);
                continue;
            }
        };
        zip.start_file(format!("blobs/{key}"), stored)?;
        while let Some(chunk) = data.next().await {
            let chunk = chunk?;
            file_bytes += chunk.len() as u64;
            zip.write_all(&chunk)?;
        }
        files += 1;
    }

    let manifest = Manifest {
        format: FORMAT,
        version: env!("CARGO_PKG_VERSION").to_string(),
        schema,
        created_at: chrono::Utc::now(),
        tables,
        files,
        file_bytes,
        missing_files,
        vector_dims,
    };
    zip.start_file("manifest.json", deflate)?;
    zip.write_all(&serde_json::to_vec_pretty(&manifest)?)?;
    zip.finish()?.flush()?;
    std::fs::rename(&partial, out)?;
    Ok(manifest)
}

/// Restores a backup into the (empty) database and the file storage of `config`.
pub async fn restore(config: &Config, input: &Path) -> anyhow::Result<Restored> {
    let mut zip = zip::ZipArchive::new(std::io::BufReader::new(std::fs::File::open(input)?))?;
    let manifest: Manifest = serde_json::from_reader(zip.by_name("manifest.json").map_err(|_| anyhow::anyhow!("not a WikiCraft backup (no manifest.json)"))?)?;
    anyhow::ensure!(manifest.format == FORMAT, "unknown backup format {} (this version reads {FORMAT})", manifest.format);
    let migrator = sqlx::migrate!("./migrations");
    let latest = migrator.iter().map(|m| m.version).max().unwrap_or(0);
    anyhow::ensure!(
        manifest.schema <= latest,
        "the backup comes from a newer WikiCraft ({}, schema {}); restore it with that version or a newer one",
        manifest.version,
        manifest.schema
    );
    anyhow::ensure!(migrator.version_exists(manifest.schema), "the backup's schema version {} is unknown to this WikiCraft", manifest.schema);

    let mut conn = PgConnection::connect(&config.database_url).await?;
    let used: bool = sqlx::query_scalar("SELECT EXISTS (SELECT 1 FROM pg_tables WHERE schemaname = current_schema())").fetch_one(&mut conn).await?;
    anyhow::ensure!(!used, "the database isn't empty: restore into a new, empty database");
    migrator.run_to(manifest.schema, &mut conn).await?;

    let mut tx = conn.begin().await?;
    let here: Vec<String> =
        sqlx::query_scalar("SELECT tablename::text FROM pg_tables WHERE schemaname = current_schema() AND tablename <> $1 ORDER BY tablename")
            .bind(MIGRATIONS)
            .fetch_all(&mut *tx)
            .await?;
    for t in manifest.tables.iter().filter(|t| t.name != MIGRATIONS && t.name != VECTORS) {
        anyhow::ensure!(here.contains(&t.name), "table {} of the backup doesn't exist in schema {}", t.name, manifest.schema);
    }
    // tables refer to each other in cycles (spaces ⇄ pages): load without foreign keys, then
    // put them back (which checks every row)
    let fks: Vec<(String, String, String)> = sqlx::query_as(
        "SELECT conrelid::regclass::text, conname::text, pg_get_constraintdef(oid) FROM pg_constraint
         WHERE contype = 'f' AND connamespace = current_schema()::regnamespace ORDER BY conrelid::regclass::text, conname",
    )
    .fetch_all(&mut *tx)
    .await?;
    for (table, name, _) in &fks {
        sqlx::query(sqlx::AssertSqlSafe(format!("ALTER TABLE {table} DROP CONSTRAINT {}", ident(name)))).execute(&mut *tx).await?;
    }
    // rows the migrations seeded (settings rows) come from the backup instead
    if !here.is_empty() {
        let list: Vec<String> = here.iter().map(|t| ident(t)).collect();
        sqlx::query(sqlx::AssertSqlSafe(format!("TRUNCATE {}", list.join(", ")))).execute(&mut *tx).await?;
    }
    let mut rows = 0i64;
    let mut buf = vec![0u8; 1 << 20];
    for t in manifest.tables.iter().filter(|t| t.name != MIGRATIONS && t.name != VECTORS) {
        let mut entry = zip.by_name(&format!("db/{}.copy", t.name))?;
        let mut copy = tx.copy_in_raw(&format!("COPY {} FROM STDIN (FORMAT binary)", ident(&t.name))).await?;
        loop {
            let n = entry.read(&mut buf)?;
            if n == 0 {
                break;
            }
            copy.send(&buf[..n]).await?;
        }
        let n = copy.finish().await? as i64;
        anyhow::ensure!(n == t.rows, "table {}: {n} rows loaded, the backup lists {}", t.name, t.rows);
        rows += n;
    }
    for (table, name, def) in &fks {
        sqlx::query(sqlx::AssertSqlSafe(format!("ALTER TABLE {table} ADD CONSTRAINT {} {def}", ident(name)))).execute(&mut *tx).await?;
    }
    // sequences continue after the restored rows
    let sequences: Vec<(String, String, String)> = sqlx::query_as(
        "SELECT format('%I', table_name), format('%I', column_name), pg_get_serial_sequence(format('%I.%I', table_schema, table_name), column_name)
         FROM information_schema.columns
         WHERE table_schema = current_schema() AND pg_get_serial_sequence(format('%I.%I', table_schema, table_name), column_name) IS NOT NULL",
    )
    .fetch_all(&mut *tx)
    .await?;
    for (table, column, sequence) in &sequences {
        sqlx::query(sqlx::AssertSqlSafe(format!("SELECT setval($1::regclass, (SELECT coalesce(max({column}), 0) + 1 FROM {table}), false)")))
            .bind(sequence)
            .execute(&mut *tx)
            .await?;
    }
    tx.commit().await?;

    let vectors = match (manifest.vector_dims, manifest.tables.iter().find(|t| t.name == VECTORS)) {
        (Some(dims), Some(t)) => match restore_vectors(&mut conn, &mut zip, dims, t.rows).await {
            Ok(()) => true,
            Err(e) => {
                tracing::warn!("semantic-search vectors left out ({e:#}); they are computed again once embeddings are configured");
                sqlx::query("UPDATE retrieval_state SET model = NULL, dims = NULL").execute(&mut conn).await?;
                false
            }
        },
        _ => false,
    };
    // the rest of the way to this version
    migrator.run(&mut conn).await?;

    let store = BlobStore::new(config).await?;
    let mut files = 0u64;
    for i in 0..zip.len() {
        let entry = zip.by_index(i)?;
        let name = entry.name()?.to_string();
        let Some(key) = name.strip_prefix("blobs/") else { continue };
        anyhow::ensure!(is_content_key(key), "unexpected file in the backup: {name}");
        if !store.has_blob(key).await {
            store.put_blob_from(key, entry).await?;
        }
        files += 1;
    }
    crate::search::forget_index(config);
    Ok(Restored { manifest, rows, files, vectors })
}

async fn restore_vectors(
    conn: &mut PgConnection,
    zip: &mut zip::ZipArchive<std::io::BufReader<std::fs::File>>,
    dims: i32,
    expected: i64,
) -> anyhow::Result<()> {
    anyhow::ensure!((1..=16000).contains(&dims), "unexpected vector size {dims}");
    let mut tx = conn.begin().await?;
    sqlx::query("CREATE EXTENSION IF NOT EXISTS vector").execute(&mut *tx).await?;
    sqlx::query(sqlx::AssertSqlSafe(format!("CREATE TABLE {VECTORS} (hash text PRIMARY KEY, embedding halfvec({dims}) NOT NULL)"))).execute(&mut *tx).await?;
    let mut entry = zip.by_name(&format!("db/{VECTORS}.txt"))?;
    let mut copy = tx.copy_in_raw(&format!("COPY {VECTORS} FROM STDIN")).await?;
    let mut buf = vec![0u8; 1 << 20];
    loop {
        let n = entry.read(&mut buf)?;
        if n == 0 {
            break;
        }
        copy.send(&buf[..n]).await?;
    }
    let n = copy.finish().await? as i64;
    drop(entry);
    anyhow::ensure!(n == expected, "{n} vectors loaded, the backup lists {expected}");
    sqlx::query(sqlx::AssertSqlSafe(format!("CREATE INDEX {VECTORS}_hnsw ON {VECTORS} USING hnsw (embedding halfvec_cosine_ops)"))).execute(&mut *tx).await?;
    tx.commit().await?;
    Ok(())
}
