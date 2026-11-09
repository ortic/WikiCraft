//! Multi-replica collaboration (`WIKICRAFT_CLUSTER=postgres`).
//!
//! Each replica keeps its own in-memory rooms. Local document and presence updates are
//! published through Postgres `NOTIFY` (inline when small, via the unlogged `collab_updates`
//! table otherwise); every other replica applies them to its room for that page and fans them out
//! to its WebSocket clients. Persistence merges the stored Yjs state under a row lock (see
//! `collab::persist_with`), so missed notifications heal on the next save and no replica can
//! overwrite another's edits.

use std::sync::Mutex;
use std::time::Duration;

use base64::Engine;
use sqlx::postgres::PgListener;
use tokio::sync::mpsc;
use uuid::Uuid;

use crate::app::AppState;

pub const CHANNEL: &str = "wikicraft_collab";
/// Presence entries owned by other replicas are attributed to this pseudo connection id.
pub const REMOTE_CONN: u64 = u64::MAX;
/// NOTIFY payloads are limited to 8000 bytes; larger updates go through the table.
const INLINE_MAX: usize = 5000;

pub const KIND_DOC: i16 = 0;
pub const KIND_AWARENESS: i16 = 1;

pub struct Outgoing {
    pub page: Uuid,
    pub kind: i16,
    pub payload: Vec<u8>,
}

pub struct Relay {
    pub tx: mpsc::UnboundedSender<Outgoing>,
    rx: Mutex<Option<mpsc::UnboundedReceiver<Outgoing>>>,
}

impl Relay {
    pub fn new() -> Self {
        let (tx, rx) = mpsc::unbounded_channel();
        Self { tx, rx: Mutex::new(Some(rx)) }
    }
}

impl Default for Relay {
    fn default() -> Self {
        Self::new()
    }
}

/// Starts the publisher, listener and cleanup tasks (no-op unless clustering is enabled).
pub fn spawn(state: &AppState) {
    let Some(relay) = &state.relay else { return };
    let Some(rx) = relay.rx.lock().expect("relay lock").take() else { return };
    tokio::spawn(publisher(state.clone(), rx));
    tokio::spawn(listener(state.clone()));
    tokio::spawn(cleanup(state.clone()));
    tracing::info!(node = %state.node_id, "cluster relay enabled (postgres LISTEN/NOTIFY)");
}

async fn publisher(state: AppState, mut rx: mpsc::UnboundedReceiver<Outgoing>) {
    while let Some(out) = rx.recv().await {
        let res = if out.payload.len() <= INLINE_MAX {
            let msg = format!("i|{}|{}|{}|{}", out.page, state.node_id, out.kind, base64::engine::general_purpose::STANDARD.encode(&out.payload));
            sqlx::query("SELECT pg_notify($1, $2)").bind(CHANNEL).bind(msg).execute(&state.db).await.map(|_| ())
        } else {
            sqlx::query(
                "WITH ins AS (INSERT INTO collab_updates (page_id, origin, kind, payload) VALUES ($1, $2, $3, $4) RETURNING id)
                 SELECT pg_notify($5, 'r|' || id) FROM ins",
            )
            .bind(out.page)
            .bind(state.node_id)
            .bind(out.kind)
            .bind(&out.payload)
            .bind(CHANNEL)
            .execute(&state.db)
            .await
            .map(|_| ())
        };
        if let Err(e) = res {
            tracing::warn!("cluster publish failed: {e}");
        }
    }
}

async fn listener(state: AppState) {
    loop {
        match PgListener::connect_with(&state.db).await {
            Ok(mut l) => {
                if let Err(e) = l.listen(CHANNEL).await {
                    tracing::warn!("cluster listen failed: {e}");
                } else {
                    loop {
                        match l.recv().await {
                            Ok(n) => handle(&state, n.payload()).await,
                            Err(e) => {
                                tracing::warn!("cluster listener lost connection: {e}");
                                break;
                            }
                        }
                    }
                }
            }
            Err(e) => tracing::warn!("cluster listener connect failed: {e}"),
        }
        tokio::time::sleep(Duration::from_secs(2)).await;
    }
}

async fn handle(state: &AppState, payload: &str) {
    let (page, origin, kind, bytes) = if let Some(rest) = payload.strip_prefix("i|") {
        let mut it = rest.splitn(4, '|');
        let (Some(page), Some(origin), Some(kind), Some(data)) = (it.next(), it.next(), it.next(), it.next()) else { return };
        let (Ok(page), Ok(origin), Ok(kind), Ok(bytes)) =
            (page.parse::<Uuid>(), origin.parse::<Uuid>(), kind.parse::<i16>(), base64::engine::general_purpose::STANDARD.decode(data))
        else {
            return;
        };
        (page, origin, kind, bytes)
    } else if let Some(id) = payload.strip_prefix("r|").and_then(|i| i.parse::<i64>().ok()) {
        let row: Option<(Uuid, Uuid, i16, Vec<u8>)> =
            sqlx::query_as("SELECT page_id, origin, kind, payload FROM collab_updates WHERE id = $1").bind(id).fetch_optional(&state.db).await.ok().flatten();
        let Some(r) = row else { return };
        r
    } else {
        return;
    };
    if origin == state.node_id {
        return;
    }
    let Some(room) = state.rooms.get(page) else { return };
    match kind {
        KIND_DOC => room.apply_remote_update(&bytes),
        KIND_AWARENESS => room.apply_remote_awareness(&bytes),
        _ => {}
    }
}

async fn cleanup(state: AppState) {
    let mut tick = tokio::time::interval(Duration::from_secs(60));
    loop {
        tick.tick().await;
        let _ = sqlx::query("DELETE FROM collab_updates WHERE created_at < now() - interval '5 minutes'").execute(&state.db).await;
    }
}
