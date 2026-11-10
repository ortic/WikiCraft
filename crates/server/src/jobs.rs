//! Background job queue in PostgreSQL. Any replica claims queued jobs (`FOR UPDATE SKIP LOCKED`),
//! keeps a heartbeat while working, and jobs of crashed workers are failed after a timeout.
//! Job inputs (uploaded files) live in the blob store, so they're reachable from every replica.

use std::time::Duration;

use serde_json::Value;
use uuid::Uuid;

use crate::app::AppState;
use crate::importer::{self, ImportOptions};

const STALE_AFTER_SECS: i64 = 120;

#[derive(sqlx::FromRow)]
struct Claimed {
    id: Uuid,
    workspace_id: Uuid,
    created_by: Option<Uuid>,
    kind: String,
    input: Option<Value>,
}

/// Starts the worker loop of this replica.
pub fn spawn_worker(state: &AppState) {
    let state = state.clone();
    tokio::spawn(async move {
        let mut tick = tokio::time::interval(Duration::from_secs(2));
        let mut last_purge: Option<std::time::Instant> = None;
        let mut last_schedule: Option<std::time::Instant> = None;
        let mut last_reminders: Option<std::time::Instant> = None;
        loop {
            tick.tick().await;
            if state.shutting_down.load(std::sync::atomic::Ordering::SeqCst) {
                return;
            }
            if let Err(e) = fail_stale(&state).await {
                tracing::warn!("job housekeeping failed: {e}");
            }
            if let Err(e) = expire_exports(&state).await {
                tracing::warn!("export cleanup failed: {e}");
            }
            // drafts whose publishing time has come, every 20 seconds
            if last_schedule.is_none_or(|t| t.elapsed() > Duration::from_secs(20)) {
                last_schedule = Some(std::time::Instant::now());
                match crate::routes::pages::publish_scheduled(&state).await {
                    Ok(0) => {}
                    Ok(n) => tracing::info!(pages = n, "published scheduled drafts"),
                    Err(e) => tracing::warn!("scheduled publishing failed: {e:#}"),
                }
            }
            // tasks due today, every 5 minutes
            if last_reminders.is_none_or(|t| t.elapsed() > Duration::from_secs(300)) {
                last_reminders = Some(std::time::Instant::now());
                match crate::routes::tasks::remind_due_tasks(&state).await {
                    Ok(0) => {}
                    Ok(n) => tracing::info!(reminders = n, "reminded people of tasks due"),
                    Err(e) => tracing::warn!("task reminders failed: {e}"),
                }
            }
            // trash past its retention period, every 10 minutes
            if last_purge.is_none_or(|t| t.elapsed() > Duration::from_secs(600)) {
                last_purge = Some(std::time::Instant::now());
                match crate::purge::purge_expired(&state).await {
                    Ok(0) => {}
                    Ok(n) => tracing::info!(pages = n, "purged pages past the trash retention period"),
                    Err(e) => tracing::warn!("trash retention failed: {e:#}"),
                }
            }
            match claim(&state).await {
                Ok(Some(job)) => run(&state, job).await,
                Ok(None) => {}
                Err(e) => tracing::warn!("job claim failed: {e}"),
            }
        }
    });
}

async fn fail_stale(state: &AppState) -> sqlx::Result<()> {
    sqlx::query(
        "UPDATE jobs SET status = 'failed', error = 'worker stopped while running the job (restart/crash); delete partial results and retry', finished_at = now()
         WHERE status = 'running' AND heartbeat_at < now() - make_interval(secs => $1)",
    )
    .bind(STALE_AFTER_SECS as f64)
    .execute(&state.db)
    .await?;
    Ok(())
}

/// Deletes export archives after [`crate::export::EXPORT_TTL_HOURS`].
async fn expire_exports(state: &AppState) -> anyhow::Result<()> {
    let old: Vec<(Uuid, String)> = sqlx::query_as(
        "SELECT id, result->>'file' FROM jobs WHERE kind = 'export.space' AND status = 'succeeded' AND result ? 'file'
         AND finished_at < now() - make_interval(hours => $1) LIMIT 20",
    )
    .bind(crate::export::EXPORT_TTL_HOURS as i32)
    .fetch_all(&state.db)
    .await?;
    for (id, key) in old {
        let _ = state.blobs.delete_key(&key).await;
        sqlx::query("UPDATE jobs SET result = (result - 'file') || '{\"expired\": true}' WHERE id = $1").bind(id).execute(&state.db).await?;
    }
    Ok(())
}

async fn claim(state: &AppState) -> sqlx::Result<Option<Claimed>> {
    sqlx::query_as::<_, Claimed>(
        "UPDATE jobs SET status = 'running', worker = $1, started_at = now(), heartbeat_at = now()
         WHERE id = (SELECT id FROM jobs WHERE status = 'queued' AND input IS NOT NULL ORDER BY created_at FOR UPDATE SKIP LOCKED LIMIT 1)
         RETURNING id, workspace_id, created_by, kind, input",
    )
    .bind(state.node_id.to_string())
    .fetch_optional(&state.db)
    .await
}

async fn run(state: &AppState, job: Claimed) {
    tracing::info!(job = %job.id, kind = %job.kind, "running job");
    // heartbeat while working
    let hb_state = state.clone();
    let id = job.id;
    let heartbeat = tokio::spawn(async move {
        let mut t = tokio::time::interval(Duration::from_secs(20));
        loop {
            t.tick().await;
            let _ = sqlx::query("UPDATE jobs SET heartbeat_at = now() WHERE id = $1").bind(id).execute(&hb_state.db).await;
        }
    });
    let input = job.input.unwrap_or_default();
    let upload_key = input.get("upload").and_then(Value::as_str).map(String::from);
    let result: anyhow::Result<()> = async {
        match job.kind.as_str() {
            "import.confluence" => {
                let key = upload_key.clone().ok_or_else(|| anyhow::anyhow!("job has no upload"))?;
                let local = state.config.upload_dir().join(format!("{}.job", job.id));
                state.blobs.download_to(&key, &local).await?;
                let opts: ImportOptions = serde_json::from_value(input.get("options").cloned().unwrap_or_default()).unwrap_or_default();
                let actor = job.created_by.ok_or_else(|| anyhow::anyhow!("job has no creator"))?;
                // run() records success/failure on the job row and removes the local file
                importer::run(state.clone(), job.id, job.workspace_id, actor, local, opts).await;
                Ok(())
            }
            "export.space" => {
                let actor = job.created_by.ok_or_else(|| anyhow::anyhow!("job has no creator"))?;
                let result = crate::export::run_export(state, job.id, actor, &input).await?;
                finish(state, job.id, result).await
            }
            "import.backup" => {
                let key = upload_key.clone().ok_or_else(|| anyhow::anyhow!("job has no upload"))?;
                let local = state.config.upload_dir().join(format!("{}.job", job.id));
                state.blobs.download_to(&key, &local).await?;
                let actor = job.created_by.ok_or_else(|| anyhow::anyhow!("job has no creator"))?;
                let opts: crate::export::RestoreOptions = serde_json::from_value(input.get("options").cloned().unwrap_or_default()).unwrap_or_default();
                let result = crate::export::run_restore(state, job.id, actor, &local, opts).await;
                let _ = tokio::fs::remove_file(&local).await;
                finish(state, job.id, result?).await
            }
            other => anyhow::bail!("unknown job kind {other}"),
        }
    }
    .await;
    heartbeat.abort();
    if let Err(e) = result {
        let _ = sqlx::query("UPDATE jobs SET status = 'failed', error = $2, finished_at = now() WHERE id = $1")
            .bind(job.id)
            .bind(format!("{e:#}"))
            .execute(&state.db)
            .await;
    }
    if let Some(k) = upload_key {
        let _ = state.blobs.delete_key(&k).await;
    }
}

async fn finish(state: &AppState, id: Uuid, result: Value) -> anyhow::Result<()> {
    sqlx::query("UPDATE jobs SET status = 'succeeded', result = $2, progress = '{\"phase\": \"done\"}', finished_at = now() WHERE id = $1")
        .bind(id)
        .bind(result)
        .execute(&state.db)
        .await?;
    Ok(())
}
