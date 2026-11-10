//! Real-time collaboration: a y-websocket compatible sync server built on `yrs`.
//!
//! One [`Room`] per page holds the live Yjs document plus awareness (cursors/presence).
//! Clients speak the standard y-websocket protocol (sync step 1/2, updates, awareness), so the
//! frontend uses Tiptap's Collaboration extension with `y-websocket` unchanged.
//!
//! Persistence: dirty rooms are flushed every ~2 s (Yjs state + derived ProseMirror JSON +
//! plain text for search). A version snapshot is created when the last editor leaves, and at
//! most every 10 minutes during long sessions.

use std::sync::atomic::{AtomicBool, AtomicU64, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use axum::extract::ws::{Message as WsMessage, WebSocket};
use axum::extract::{Path, State, WebSocketUpgrade};
use axum::http::{HeaderMap, header};
use axum::response::Response;
use dashmap::DashMap;
use futures::{SinkExt, StreamExt};
use tokio::sync::{broadcast, mpsc};
use uuid::Uuid;
use wikicraft_doc_model::{Node, blocks, text, ydoc};
use yrs::block::ClientID;
use yrs::sync::awareness::AwarenessUpdateEntry;
use yrs::sync::{AwarenessUpdate, Message, SyncMessage};
use yrs::updates::decoder::Decode;
use yrs::updates::encoder::Encode;
use yrs::{ReadTxn, StateVector, Transact, Update};

use crate::app::AppState;
use crate::auth::MaybeUser;
use crate::authz::{Principal, page_access};
use crate::error::{ApiError, ApiResult};

const VERSION_INTERVAL: Duration = Duration::from_secs(600);

#[derive(Default)]
pub struct Rooms {
    map: DashMap<Uuid, Arc<Room>>,
}

/// Presence (awareness) states per Yjs client: cursor, selection, user name/color.
/// (yrs' `Awareness` holds non-`Send` observer callbacks, so we keep our own map.)
const MAX_PRESENCE_CLIENTS: usize = 200;
const MAX_PRESENCE_JSON: usize = 8 * 1024;

#[derive(Default)]
struct Presence {
    states: std::collections::HashMap<ClientID, AwarenessUpdateEntry>,
    /// which connection a Yjs client id belongs to (no spoofing other users' presence)
    owner: std::collections::HashMap<ClientID, u64>,
}

impl Presence {
    /// Applies the entries a connection may set; returns the accepted subset (to broadcast).
    fn apply(&mut self, conn: u64, u: AwarenessUpdate) -> AwarenessUpdate {
        let mut accepted = std::collections::HashMap::new();
        for (id, e) in u.clients {
            if e.json.len() > MAX_PRESENCE_JSON {
                continue;
            }
            match self.owner.get(&id) {
                Some(owner) if *owner != conn => continue,
                None if self.owner.len() >= MAX_PRESENCE_CLIENTS => continue,
                None => {
                    self.owner.insert(id, conn);
                }
                _ => {}
            }
            let newer = self.states.get(&id).is_none_or(|cur| e.clock > cur.clock || (e.clock == cur.clock && &*e.json == "null"));
            if newer {
                self.states.insert(id, AwarenessUpdateEntry { clock: e.clock, json: e.json.clone() });
                accepted.insert(id, e);
            }
        }
        AwarenessUpdate { clients: accepted }
    }

    fn full(&self) -> AwarenessUpdate {
        AwarenessUpdate {
            clients: self
                .states
                .iter()
                .filter(|(_, e)| &*e.json != "null")
                .map(|(k, e)| (*k, AwarenessUpdateEntry { clock: e.clock, json: e.json.clone() }))
                .collect(),
        }
    }

    /// Marks clients offline (Yjs encodes removal as a `null` state with a bumped clock).
    fn remove(&mut self, conn: u64) -> AwarenessUpdate {
        let ids: Vec<ClientID> = self.owner.iter().filter(|(_, c)| **c == conn).map(|(id, _)| *id).collect();
        let mut clients = std::collections::HashMap::new();
        for id in &ids {
            self.owner.remove(id);
            let clock = self.states.get(id).map(|e| e.clock + 1).unwrap_or(1);
            let e = AwarenessUpdateEntry { clock, json: Arc::from("null") };
            self.states.insert(*id, AwarenessUpdateEntry { clock, json: e.json.clone() });
            clients.insert(*id, e);
        }
        AwarenessUpdate { clients }
    }
}

pub struct Room {
    pub page_id: Uuid,
    doc: Mutex<yrs::Doc>,
    presence: Mutex<Presence>,
    tx: broadcast::Sender<(u64, Arc<Vec<u8>>)>,
    conns: AtomicUsize,
    dirty: AtomicBool,
    edited_since_version: AtomicBool,
    last_editor: Mutex<Option<Uuid>>,
    last_version_at: Mutex<Instant>,
    next_conn: AtomicU64,
    /// set when the page was trashed / the room evicted: connections close, nothing is persisted
    closed: AtomicBool,
    closing: tokio::sync::Notify,
    /// cluster relay (other replicas), when clustering is enabled
    relay: Option<mpsc::UnboundedSender<crate::cluster::Outgoing>>,
}

impl Room {
    fn new(page_id: Uuid, doc: yrs::Doc, relay: Option<mpsc::UnboundedSender<crate::cluster::Outgoing>>) -> Self {
        let (tx, _) = broadcast::channel(1024);
        Self {
            page_id,
            doc: Mutex::new(doc),
            presence: Mutex::new(Presence::default()),
            tx,
            conns: AtomicUsize::new(0),
            dirty: AtomicBool::new(false),
            edited_since_version: AtomicBool::new(false),
            last_editor: Mutex::new(None),
            last_version_at: Mutex::new(Instant::now()),
            next_conn: AtomicU64::new(1),
            closed: AtomicBool::new(false),
            closing: tokio::sync::Notify::new(),
            relay,
        }
    }

    fn with_doc<R>(&self, f: impl FnOnce(&yrs::Doc) -> R) -> R {
        let d = self.doc.lock().expect("doc lock");
        f(&d)
    }

    /// Current document as ProseMirror JSON.
    pub fn snapshot(&self) -> Node {
        self.with_doc(ydoc::doc_to_pm)
    }

    /// Replaces the document content (REST edits, version restore) and broadcasts the change.
    /// Blocks that didn't change keep their ids (see [`blocks::adopt_ids`]).
    pub fn replace_content(&self, pm: &Node, editor: Option<Uuid>) {
        let update = self.with_doc(|doc| {
            let mut pm = pm.clone();
            blocks::adopt_ids(&ydoc::doc_to_pm(doc), &mut pm);
            let before: StateVector = doc.transact().state_vector();
            ydoc::write_pm(doc, &pm);
            doc.transact().encode_state_as_update_v1(&before)
        });
        self.publish(crate::cluster::KIND_DOC, &update);
        let msg = Message::Sync(SyncMessage::Update(update)).encode_v1();
        let _ = self.tx.send((0, Arc::new(msg)));
        self.mark_edited(editor);
    }

    fn publish(&self, kind: i16, payload: &[u8]) {
        if let Some(r) = &self.relay {
            let _ = r.send(crate::cluster::Outgoing { page: self.page_id, kind, payload: payload.to_vec() });
        }
    }

    /// Applies a document update from another replica and fans it out to local clients.
    pub fn apply_remote_update(&self, update: &[u8]) {
        let changed = match Update::decode_v1(update) {
            Ok(u) => self.with_doc(|d| {
                let before = d.transact().state_vector();
                let ok = d.transact_mut().apply_update(u).is_ok();
                ok && d.transact().state_vector() != before
            }),
            Err(_) => false,
        };
        if changed {
            let msg = Message::Sync(SyncMessage::Update(update.to_vec())).encode_v1();
            let _ = self.tx.send((0, Arc::new(msg)));
        }
    }

    /// Applies presence from another replica and fans it out to local clients.
    pub fn apply_remote_awareness(&self, payload: &[u8]) {
        let Ok(update) = AwarenessUpdate::decode_v1(payload) else { return };
        let accepted = self.presence.lock().expect("presence lock").apply(crate::cluster::REMOTE_CONN, update);
        if !accepted.clients.is_empty() {
            let _ = self.tx.send((0, Arc::new(Message::Awareness(accepted).encode_v1())));
        }
    }

    /// Merges a stored Yjs state into the room; returns the diff for local clients, if any.
    fn merge_stored(&self, stored: &[u8]) -> Option<Vec<u8>> {
        let u = Update::decode_v1(stored).ok()?;
        self.with_doc(|d| {
            let before = d.transact().state_vector();
            d.transact_mut().apply_update(u).ok()?;
            let changed = d.transact().state_vector() != before;
            changed.then(|| d.transact().encode_state_as_update_v1(&before))
        })
    }

    fn mark_edited(&self, editor: Option<Uuid>) {
        if self.closed.load(Ordering::SeqCst) {
            return;
        }
        self.dirty.store(true, Ordering::SeqCst);
        self.edited_since_version.store(true, Ordering::SeqCst);
        if editor.is_some() {
            *self.last_editor.lock().expect("lock") = editor;
        }
    }

    pub fn connections(&self) -> usize {
        self.conns.load(Ordering::SeqCst)
    }
}

impl Rooms {
    pub fn get(&self, page_id: Uuid) -> Option<Arc<Room>> {
        self.map.get(&page_id).map(|r| r.clone())
    }

    /// Returns the live room for a page, loading it from the database if needed.
    pub async fn get_or_load(&self, state: &AppState, page_id: Uuid) -> ApiResult<Arc<Room>> {
        if let Some(r) = self.get(page_id) {
            return Ok(r);
        }
        let row: Option<(Option<Vec<u8>>, serde_json::Value)> =
            sqlx::query_as("SELECT ydoc, content FROM pages WHERE id = $1 AND deleted_at IS NULL").bind(page_id).fetch_optional(&state.db).await?;
        let Some((ystate, content)) = row else { return Err(ApiError::NotFound("page")) };
        let mut doc = ydoc::new_doc();
        match ystate {
            Some(bytes) if !bytes.is_empty() => {
                ydoc::apply_update(&doc, &bytes).map_err(|e| ApiError::Internal(anyhow::anyhow!("corrupt ydoc for {page_id}: {e}")))?
            }
            _ => {
                // first collaborative open: seed Yjs from the stored ProseMirror JSON on the server,
                // and claim it in the database — if another replica (or task) seeded first, use
                // theirs, otherwise both seeds would merge into duplicated content
                let mut pm = Node::from_json(&content).unwrap_or_else(|_| Node::empty_doc());
                blocks::ensure_ids(&mut pm);
                ydoc::write_pm(&doc, &pm);
                let claimed = sqlx::query("UPDATE pages SET ydoc = $1, content = $3 WHERE id = $2 AND ydoc IS NULL")
                    .bind(ydoc::encode_state(&doc))
                    .bind(page_id)
                    .bind(pm.to_json())
                    .execute(&state.db)
                    .await?;
                if claimed.rows_affected() == 0 {
                    let theirs: Option<Vec<u8>> = sqlx::query_scalar("SELECT ydoc FROM pages WHERE id = $1").bind(page_id).fetch_one(&state.db).await?;
                    let fresh = ydoc::new_doc();
                    if let Some(b) = theirs {
                        ydoc::apply_update(&fresh, &b).map_err(|e| ApiError::Internal(anyhow::anyhow!("corrupt ydoc for {page_id}: {e}")))?;
                    }
                    doc = fresh;
                }
            }
        }
        let ids = assign_block_ids(state, page_id, &doc).await?;
        let room = Arc::new(Room::new(page_id, doc, state.relay.as_ref().map(|r| r.tx.clone())));
        let room = self.map.entry(page_id).or_insert(room).clone();
        if let Some(update) = ids {
            // live rooms on other replicas (and their clients) get the ids too
            room.publish(crate::cluster::KIND_DOC, &update);
        }
        Ok(room)
    }

    /// Closes the live session of a page (trashed pages, revoked access): connected clients are
    /// disconnected by the broadcast channel closing; nothing more is persisted.
    pub fn evict(&self, page_id: Uuid) {
        if let Some((_, room)) = self.map.remove(&page_id) {
            room.closed.store(true, Ordering::SeqCst);
            room.dirty.store(false, Ordering::SeqCst);
            room.closing.notify_waiters();
        }
    }

    fn remove_if_idle(&self, page_id: Uuid) {
        self.map.remove_if(&page_id, |_, r| r.connections() == 0 && !r.dirty.load(Ordering::SeqCst));
    }

    pub fn all(&self) -> Vec<Arc<Room>> {
        self.map.iter().map(|r| r.clone()).collect()
    }
}

/// Gives the blocks of a page that predates block ids their ids, as soon as the page is opened
/// (before the first client syncs, so editors don't assign them, which would count as an edit).
/// Stores the result without touching `updated_at` / `updated_by` or creating a version. Returns
/// the Yjs update to relay, if anything changed.
async fn assign_block_ids(state: &AppState, page_id: Uuid, doc: &yrs::Doc) -> ApiResult<Option<Vec<u8>>> {
    if !blocks::ydoc_needs_ids(doc) {
        return Ok(None);
    }
    let mut tx = state.db.begin().await?;
    // merge what was stored meanwhile (another replica's save) under the row lock, so this write
    // can't drop it
    let stored: Option<Option<Vec<u8>>> = sqlx::query_scalar("SELECT ydoc FROM pages WHERE id = $1 FOR UPDATE").bind(page_id).fetch_optional(&mut *tx).await?;
    if let Some(Some(bytes)) = stored {
        ydoc::apply_update(doc, &bytes).map_err(|e| ApiError::Internal(anyhow::anyhow!("corrupt ydoc for {page_id}: {e}")))?;
    }
    let before = doc.transact().state_vector();
    if !blocks::ensure_ydoc_ids(doc) {
        return Ok(None);
    }
    let update = doc.transact().encode_state_as_update_v1(&before);
    sqlx::query("UPDATE pages SET ydoc = $2, content = $3 WHERE id = $1 AND deleted_at IS NULL")
        .bind(page_id)
        .bind(ydoc::encode_state(doc))
        .bind(ydoc::doc_to_pm(doc).to_json())
        .execute(&mut *tx)
        .await?;
    tx.commit().await?;
    Ok(Some(update))
}

/// Persists a room's state to the database; derives ProseMirror JSON and plain text.
/// Creates an automatic version snapshot when the last editor left (or every 10 minutes).
pub async fn persist(state: &AppState, room: &Room) -> anyhow::Result<()> {
    persist_with(state, room, true).await
}

/// Like [`persist`], optionally without the automatic version (callers that create an explicit,
/// labelled version right afterwards).
pub async fn persist_with(state: &AppState, room: &Room, auto_version: bool) -> anyhow::Result<()> {
    let dirty = room.dirty.swap(false, Ordering::SeqCst);
    if room.closed.load(Ordering::SeqCst) {
        return Ok(());
    }
    let editor = *room.last_editor.lock().expect("lock");
    if dirty {
        save(state, room, editor).await?;
    }
    // the version is due even when the periodic flush already stored the content: otherwise a
    // flush between the last edit and the last disconnect would skip the automatic version
    let due = auto_version && (room.connections() == 0 || room.last_version_at.lock().expect("lock").elapsed() > VERSION_INTERVAL);
    if due && room.edited_since_version.swap(false, Ordering::SeqCst) {
        crate::routes::pages::create_version(state, room.page_id, editor, None).await?;
        *room.last_version_at.lock().expect("lock") = Instant::now();
    }
    Ok(())
}

/// Writes the room's document to the page row (Yjs state, ProseMirror JSON, plain text) and
/// updates search, tasks and mentions. On failure the room stays dirty for the next tick.
async fn save(state: &AppState, room: &Room, editor: Option<Uuid>) -> anyhow::Result<()> {
    let res: anyhow::Result<Node> = async {
        let mut tx = state.db.begin().await?;
        if state.relay.is_some() {
            // other replicas persist the same page: merge their stored state first (row lock
            // serializes writers), so nobody overwrites anybody and missed relays heal here
            let stored: Option<Option<Vec<u8>>> = sqlx::query_scalar("SELECT ydoc FROM pages WHERE id = $1 FOR UPDATE").bind(room.page_id).fetch_optional(&mut *tx).await?;
            if let Some(Some(stored)) = stored
                && let Some(diff) = room.merge_stored(&stored)
            {
                let _ = room.tx.send((0, Arc::new(Message::Sync(SyncMessage::Update(diff)).encode_v1())));
            }
        }
        let (bytes, pm) = room.with_doc(|d| (ydoc::encode_state(d), ydoc::doc_to_pm(d)));
        sqlx::query("UPDATE pages SET ydoc = $1, content = $2, text_content = $3, updated_at = now(), updated_by = COALESCE($4, updated_by) WHERE id = $5 AND deleted_at IS NULL")
            .bind(bytes)
            .bind(pm.to_json())
            .bind(text::plain_text(&pm))
            .bind(editor)
            .bind(room.page_id)
            .execute(&mut *tx)
            .await?;
        tx.commit().await?;
        Ok(pm)
    }
    .await;
    let pm = match res {
        Ok(pm) => pm,
        Err(e) => {
            room.dirty.store(true, Ordering::SeqCst); // retry next tick
            return Err(e);
        }
    };
    state.search.enqueue(room.page_id);
    if let Ok(mut conn) = state.db.acquire().await
        && let Err(e) = crate::routes::tasks::index_page(&mut conn, room.page_id, &pm).await
    {
        tracing::warn!("task indexing failed: {e}");
    }
    if let Err(e) = crate::routes::notifications::notify_page_mentions(state, room.page_id, &pm, editor).await {
        tracing::warn!("mention notifications failed: {e:#}");
    }
    Ok(())
}

/// Background loop flushing dirty rooms and evicting idle ones.
pub async fn persist_loop(state: AppState) {
    let mut tick = tokio::time::interval(Duration::from_secs(2));
    loop {
        tick.tick().await;
        flush_all(&state).await;
    }
}

pub async fn flush_all(state: &AppState) {
    for room in state.rooms.all() {
        if let Err(e) = persist(state, &room).await {
            tracing::error!(page = %room.page_id, "persisting collab state failed: {e:#}");
        }
        if room.connections() == 0 {
            state.rooms.remove_if_idle(room.page_id);
        }
    }
}

/// Rejects cross-site WebSocket hijacking: the Origin must be the configured public URL or match
/// the Host header.
fn same_origin(headers: &HeaderMap, base_url: &str) -> bool {
    let Some(origin) = headers.get(header::ORIGIN).and_then(|v| v.to_str().ok()) else {
        return true; // non-browser clients (CLI, tests) don't send Origin
    };
    if origin.trim_end_matches('/').eq_ignore_ascii_case(base_url.trim_end_matches('/')) {
        return true;
    }
    let host = headers.get("x-forwarded-host").or_else(|| headers.get(header::HOST)).and_then(|v| v.to_str().ok()).unwrap_or("");
    let origin_host = origin.split("://").nth(1).unwrap_or("").trim_end_matches('/');
    !host.is_empty() && origin_host.eq_ignore_ascii_case(host)
}

/// `GET /api/pages/{id}/collab` — WebSocket endpoint.
pub async fn ws_handler(
    State(state): State<AppState>,
    Path(page_id): Path<Uuid>,
    MaybeUser(user): MaybeUser,
    headers: HeaderMap,
    ws: WebSocketUpgrade,
) -> ApiResult<Response> {
    if !same_origin(&headers, &state.config.base_url) {
        return Err(ApiError::Forbidden);
    }
    let principal = Principal::load(&state, user.clone()).await?;
    let (access, _) = page_access(&state, &principal, page_id).await?;
    if !access.view {
        return Err(if user.is_none() { ApiError::Unauthorized } else { ApiError::NotFound("page") });
    }
    let room = state.rooms.get_or_load(&state, page_id).await?;
    let can_edit = access.edit;
    // read-only clients only send small sync/awareness messages
    let max = if can_edit { 32 * 1024 * 1024 } else { 256 * 1024 };
    Ok(ws.max_message_size(max).max_frame_size(max).on_upgrade(move |socket| async move {
        handle_socket(state, socket, room, user, can_edit).await;
    }))
}

#[allow(clippy::while_let_loop)] // select! with several exit paths reads clearer as a loop
async fn handle_socket(state: AppState, socket: WebSocket, room: Arc<Room>, user: Option<crate::auth::User>, mut can_edit: bool) {
    let user_id = user.as_ref().map(|u| u.id);
    let conn_id = room.next_conn.fetch_add(1, Ordering::SeqCst);
    room.conns.fetch_add(1, Ordering::SeqCst);
    state.metrics.ws_connections.fetch_add(1, Ordering::Relaxed);
    let (mut sink, mut stream) = socket.split();
    let (out_tx, mut out_rx) = mpsc::channel::<Vec<u8>>(256);

    // initial messages: our state vector (client answers with its missing updates) + presence
    let sv = room.with_doc(|d| d.transact().state_vector());
    let aw = room.presence.lock().expect("presence lock").full();
    let _ = out_tx.send(Message::Sync(SyncMessage::SyncStep1(sv)).encode_v1()).await;
    if !aw.clients.is_empty() {
        let _ = out_tx.send(Message::Awareness(aw).encode_v1()).await;
    }

    // writer: everything goes through one sink
    let writer = tokio::spawn(async move {
        while let Some(msg) = out_rx.recv().await {
            if sink.send(WsMessage::Binary(msg.into())).await.is_err() {
                break;
            }
        }
        let _ = sink.close().await;
    });
    // broadcast fan-in
    let mut brx = room.tx.subscribe();
    let fwd_tx = out_tx.clone();
    let forwarder = tokio::spawn(async move {
        loop {
            match brx.recv().await {
                Ok((from, msg)) => {
                    if from != conn_id && fwd_tx.send((*msg).clone()).await.is_err() {
                        break;
                    }
                }
                // too slow: drop the connection, the client reconnects and resyncs
                Err(broadcast::error::RecvError::Lagged(_)) | Err(broadcast::error::RecvError::Closed) => break,
            }
        }
    });

    let mut recheck = tokio::time::interval(Duration::from_secs(30));
    recheck.tick().await;
    loop {
        let msg = tokio::select! {
            m = stream.next() => m,
            _ = room.closing.notified() => break,
            _ = recheck.tick() => {
                // permissions may have changed (restrictions, roles, deactivation, trash)
                if room.closed.load(Ordering::SeqCst) {
                    break;
                }
                let still = async {
                    let p = Principal::load(&state, user.clone()).await.ok()?;
                    page_access(&state, &p, room.page_id).await.ok()
                }
                .await;
                match still {
                    Some((a, _)) if a.view => {
                        can_edit = a.edit;
                        continue;
                    }
                    _ => break,
                }
            }
        };
        let Some(Ok(msg)) = msg else { break };
        let data = match msg {
            WsMessage::Binary(b) => b,
            WsMessage::Close(_) => break,
            _ => continue,
        };
        state.metrics.ws_messages.fetch_add(1, Ordering::Relaxed);
        let Ok(message) = Message::decode_v1(&data) else {
            tracing::debug!("invalid collab message");
            continue;
        };
        match message {
            Message::Sync(SyncMessage::SyncStep1(sv)) => {
                let update = room.with_doc(|d| d.transact().encode_state_as_update_v1(&sv));
                let _ = out_tx.send(Message::Sync(SyncMessage::SyncStep2(update)).encode_v1()).await;
            }
            Message::Sync(SyncMessage::SyncStep2(update)) | Message::Sync(SyncMessage::Update(update)) => {
                if !can_edit {
                    continue; // read-only connection
                }
                // only real changes count: sync step 2 of a fresh client or re-sent updates are no-ops
                let changed = match Update::decode_v1(&update) {
                    Ok(u) => room.with_doc(|d| {
                        let has_deletes = !u.delete_set().is_empty();
                        let before = d.transact().state_vector();
                        let ok = d.transact_mut().apply_update(u).is_ok();
                        ok && (has_deletes || d.transact().state_vector() != before)
                    }),
                    Err(_) => false,
                };
                if changed {
                    room.mark_edited(user_id);
                    room.publish(crate::cluster::KIND_DOC, &update);
                    let msg = Message::Sync(SyncMessage::Update(update)).encode_v1();
                    let _ = room.tx.send((conn_id, Arc::new(msg)));
                }
            }
            Message::Awareness(update) => {
                let accepted = room.presence.lock().expect("presence lock").apply(conn_id, update);
                if !accepted.clients.is_empty() {
                    room.publish(crate::cluster::KIND_AWARENESS, &accepted.encode_v1());
                    let raw = Message::Awareness(accepted).encode_v1();
                    let _ = room.tx.send((conn_id, Arc::new(raw)));
                }
            }
            Message::AwarenessQuery => {
                let aw = room.presence.lock().expect("presence lock").full();
                let _ = out_tx.send(Message::Awareness(aw).encode_v1()).await;
            }
            Message::Auth(_) | Message::Custom(_, _) => {}
        }
    }

    // cleanup: remove this connection's presence and tell the others
    let removal = room.presence.lock().expect("presence lock").remove(conn_id);
    if !removal.clients.is_empty() {
        room.publish(crate::cluster::KIND_AWARENESS, &removal.encode_v1());
        let _ = room.tx.send((conn_id, Arc::new(Message::Awareness(removal).encode_v1())));
    }
    forwarder.abort();
    state.metrics.ws_connections.fetch_sub(1, Ordering::Relaxed);
    drop(out_tx);
    let _ = writer.await;
    if room.conns.fetch_sub(1, Ordering::SeqCst) == 1 {
        // last one out: persist right away (also creates a version)
        if let Err(e) = persist(&state, &room).await {
            tracing::error!(page = %room.page_id, "persist on close failed: {e:#}");
        }
        state.rooms.remove_if_idle(room.page_id);
    }
}
