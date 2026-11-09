//! Outgoing webhooks. Events are written to `webhook_deliveries` (one row per matching hook)
//! and POSTed by a worker on every replica with retries and exponential backoff.
//!
//! Each request carries `X-WikiCraft-Event`, `X-WikiCraft-Delivery` and
//! `X-WikiCraft-Signature: sha256=<hex HMAC-SHA256 of the body with the hook's secret>`.
//! Targets on private, loopback and link-local addresses are refused unless
//! `WIKICRAFT_WEBHOOK_ALLOW_PRIVATE=true`; the resolved address is pinned for the request so
//! DNS can't be switched between the check and the connection.

use std::net::{IpAddr, SocketAddr};
use std::time::{Duration, Instant};

use hmac::{Hmac, KeyInit, Mac};
use serde_json::{Value, json};
use sha2::Sha256;
use uuid::Uuid;

use crate::app::AppState;

pub const EVENTS: &[&str] =
    &["page.created", "page.updated", "page.moved", "page.trashed", "page.restored", "comment.created", "attachment.created", "space.created", "ping"];
const MAX_ATTEMPTS: i32 = 8;

/// Queues `event` for every active hook of the workspace that wants it (space-scoped hooks only
/// for events in their space). Never fails the caller.
pub async fn emit(state: &AppState, workspace: Uuid, event: &str, space: Option<Uuid>, data: Value) {
    let payload = json!({
        "event": event,
        "timestamp": chrono::Utc::now(),
        "data": data,
    });
    let res = sqlx::query(
        "INSERT INTO webhook_deliveries (id, webhook_id, event, payload)
         SELECT gen_random_uuid(), w.id, $2, $3 || jsonb_build_object('webhookId', w.id) FROM webhooks w
         WHERE w.workspace_id = $1 AND w.active AND (cardinality(w.events) = 0 OR $2 = ANY(w.events) OR $2 = 'ping')
           AND (w.space_id IS NULL OR w.space_id = $4)",
    )
    .bind(workspace)
    .bind(event)
    .bind(payload)
    .bind(space)
    .execute(&state.db)
    .await;
    if let Err(e) = res {
        tracing::warn!(event, "queueing webhook failed: {e}");
    }
}

/// Page summary used in event payloads (no content: hooks get metadata only).
pub async fn page_data(state: &AppState, page: Uuid, actor: Option<Uuid>) -> Option<(Uuid, Uuid, Value)> {
    let row: Option<(Uuid, Uuid, String, String, String, i32, Option<Uuid>)> = sqlx::query_as(
        "SELECT p.workspace_id, p.space_id, p.title, s.key, s.name, p.version, p.parent_id FROM pages p JOIN spaces s ON s.id = p.space_id WHERE p.id = $1",
    )
    .bind(page)
    .fetch_optional(&state.db)
    .await
    .ok()
    .flatten();
    let (ws, space, title, key, space_name, version, parent) = row?;
    let actor = match actor {
        Some(a) => sqlx::query_as::<_, (Uuid, String, Option<String>)>("SELECT id, name, email FROM users WHERE id = $1")
            .bind(a)
            .fetch_optional(&state.db)
            .await
            .ok()
            .flatten(),
        None => None,
    };
    let base = state.config.base_url.trim_end_matches('/');
    Some((
        ws,
        space,
        json!({
            "page": {"id": page, "title": title, "version": version, "parentId": parent, "url": format!("{base}/pages/{page}")},
            "space": {"id": space, "key": key, "name": space_name},
            "actor": actor.map(|(id, name, email)| json!({"id": id, "name": name, "email": email})),
        }),
    ))
}

/// Convenience: emit a page event with the standard payload, plus `extra` fields.
pub async fn emit_page(state: &AppState, event: &str, page: Uuid, actor: Option<Uuid>, extra: Value) {
    if let Some((ws, space, mut data)) = page_data(state, page, actor).await {
        if let (Some(d), Some(e)) = (data.as_object_mut(), extra.as_object()) {
            d.extend(e.clone());
        }
        emit(state, ws, event, Some(space), data).await;
    }
}

pub fn sign(secret: &str, body: &[u8]) -> String {
    let mut mac = <Hmac<Sha256> as KeyInit>::new_from_slice(secret.as_bytes()).expect("any key length");
    mac.update(body);
    format!("sha256={}", hex::encode(mac.finalize().into_bytes()))
}

fn is_private(ip: IpAddr) -> bool {
    match ip {
        IpAddr::V4(v4) => {
            let o = v4.octets();
            v4.is_loopback()
                || v4.is_private()
                || v4.is_link_local()
                || v4.is_unspecified()
                || v4.is_broadcast()
                || v4.is_multicast()
                || (o[0] == 100 && (64..128).contains(&o[1]))
                || o[0] == 0
        }
        IpAddr::V6(v6) => {
            let s = v6.segments();
            let embedded = |a: u16, b: u16| is_private(IpAddr::V4(std::net::Ipv4Addr::new((a >> 8) as u8, a as u8, (b >> 8) as u8, b as u8)));
            v6.is_loopback()
                || v6.is_unspecified()
                || v6.is_multicast()
                || (s[0] & 0xfe00) == 0xfc00
                || (s[0] & 0xffc0) == 0xfe80
                || v6.to_ipv4_mapped().is_some_and(|v4| is_private(IpAddr::V4(v4)))
                // NAT64 (64:ff9b::/96), 6to4 (2002::/16) and IPv4-compatible (::a.b.c.d) forms
                || (s[0] == 0x64 && s[1] == 0xff9b && s[2..6].iter().all(|x| *x == 0) && embedded(s[6], s[7]))
                || (s[0] == 0x2002 && embedded(s[1], s[2]))
                || (s[0..6].iter().all(|x| *x == 0) && embedded(s[6], s[7]))
        }
    }
}

/// Validates a hook URL (scheme, host) — the address check happens at delivery time.
pub fn validate_url(url: &str) -> anyhow::Result<openidconnect::url::Url> {
    let u = openidconnect::url::Url::parse(url.trim())?;
    anyhow::ensure!(matches!(u.scheme(), "http" | "https"), "webhook URLs must use http or https");
    anyhow::ensure!(u.host_str().is_some_and(|h| !h.is_empty()), "webhook URL needs a host");
    anyhow::ensure!(u.username().is_empty() && u.password().is_none(), "put credentials in the secret, not the URL");
    Ok(u)
}

/// Resolves the target and refuses private addresses (unless allowed); returns the address to pin.
async fn resolve(u: &openidconnect::url::Url, allow_private: bool) -> anyhow::Result<SocketAddr> {
    let host = u.host_str().unwrap_or_default().trim_start_matches('[').trim_end_matches(']').to_string();
    let port = u.port_or_known_default().unwrap_or(443);
    let addrs: Vec<SocketAddr> = tokio::time::timeout(Duration::from_secs(5), tokio::net::lookup_host((host.as_str(), port)))
        .await
        .map_err(|_| anyhow::anyhow!("resolving {host} timed out"))??
        .collect();
    anyhow::ensure!(!addrs.is_empty(), "{host} doesn't resolve");
    if !allow_private && let Some(bad) = addrs.iter().find(|a| is_private(a.ip())) {
        anyhow::bail!("{host} resolves to a private address ({}); set WIKICRAFT_WEBHOOK_ALLOW_PRIVATE=true to allow internal targets", bad.ip());
    }
    Ok(addrs[0])
}

#[derive(sqlx::FromRow)]
struct Due {
    id: Uuid,
    webhook_id: Uuid,
    event: String,
    payload: Value,
    attempts: i32,
    url: String,
    secret: String,
}

/// Sends what's due. Returns the number of deliveries attempted.
pub async fn deliver_due(state: &AppState) -> anyhow::Result<usize> {
    // lease the batch for two minutes (other replicas skip it) instead of holding a
    // transaction open across slow HTTP calls; a crashed worker's lease simply expires
    let due = sqlx::query_as::<_, Due>(
        "WITH batch AS (
            SELECT d.id FROM webhook_deliveries d
            WHERE d.delivered_at IS NULL AND d.failed_at IS NULL AND d.next_attempt_at <= now()
            ORDER BY d.created_at LIMIT 20 FOR UPDATE SKIP LOCKED)
         UPDATE webhook_deliveries d SET next_attempt_at = now() + interval '2 minutes' FROM batch, webhooks w
         WHERE d.id = batch.id AND w.id = d.webhook_id
         RETURNING d.id, d.webhook_id, d.event, d.payload, d.attempts, w.url, w.secret",
    )
    .fetch_all(&state.db)
    .await?;
    let n = due.len();
    for d in due {
        let started = Instant::now();
        let mut payload = d.payload.clone();
        if let Some(o) = payload.as_object_mut() {
            o.insert("deliveryId".into(), json!(d.id));
        }
        let body = serde_json::to_vec(&payload)?;
        let result: anyhow::Result<u16> = async {
            let u = validate_url(&d.url)?;
            let addr = resolve(&u, state.config.webhook_allow_private).await?;
            let client = openidconnect::reqwest::Client::builder()
                .redirect(openidconnect::reqwest::redirect::Policy::none())
                // a proxy would connect on our behalf and skip the address check
                .no_proxy()
                .timeout(Duration::from_secs(10))
                .user_agent("WikiCraft-Webhooks/1")
                .resolve(u.host_str().unwrap_or_default(), addr)
                .build()?;
            let resp = client
                .post(u.as_str())
                .header("content-type", "application/json")
                .header("x-wikicraft-event", &d.event)
                .header("x-wikicraft-delivery", d.id.to_string())
                .header("x-wikicraft-signature", sign(&d.secret, &body))
                .body(body.clone())
                .send()
                .await?;
            Ok(resp.status().as_u16())
        }
        .await;
        let ms = started.elapsed().as_millis().min(i32::MAX as u128) as i32;
        match result {
            Ok(status) if (200..300).contains(&status) => {
                sqlx::query("UPDATE webhook_deliveries SET delivered_at = now(), attempts = attempts + 1, response_status = $2, last_error = NULL, duration_ms = $3 WHERE id = $1")
                    .bind(d.id)
                    .bind(status as i32)
                    .bind(ms)
                    .execute(&state.db)
                    .await?;
                sqlx::query("UPDATE webhooks SET failure_count = 0 WHERE id = $1").bind(d.webhook_id).execute(&state.db).await?;
            }
            other => {
                let (status, err) = match other {
                    Ok(s) => (Some(s as i32), format!("HTTP {s}")),
                    Err(e) => (None, format!("{e:#}")),
                };
                let last = d.attempts + 1 >= MAX_ATTEMPTS;
                sqlx::query(
                    "UPDATE webhook_deliveries SET attempts = attempts + 1, response_status = $2, last_error = $3, duration_ms = $4,
                        next_attempt_at = now() + make_interval(secs => $5), failed_at = CASE WHEN $6 THEN now() END WHERE id = $1",
                )
                .bind(d.id)
                .bind(status)
                .bind(&err)
                .bind(ms)
                // 10 s, 20 s, 40 s, … ~21 min
                .bind((10i64 << d.attempts.clamp(0, 7)) as f64)
                .bind(last)
                .execute(&state.db)
                .await?;
                sqlx::query("UPDATE webhooks SET failure_count = failure_count + 1 WHERE id = $1").bind(d.webhook_id).execute(&state.db).await?;
                tracing::info!(delivery = %d.id, attempt = d.attempts + 1, "webhook delivery failed: {err}");
            }
        }
    }
    Ok(n)
}

/// Starts this replica's delivery loop.
pub fn spawn_worker(state: &AppState) {
    let state = state.clone();
    tokio::spawn(async move {
        let mut tick = tokio::time::interval(Duration::from_secs(2));
        loop {
            tick.tick().await;
            if state.shutting_down.load(std::sync::atomic::Ordering::SeqCst) {
                return;
            }
            loop {
                match deliver_due(&state).await {
                    Ok(20) => continue,
                    Ok(_) => break,
                    Err(e) => {
                        tracing::warn!("webhook worker: {e:#}");
                        break;
                    }
                }
            }
        }
    });
}

/// A new random signing secret.
pub fn new_secret() -> String {
    format!("whsec_{}", crate::auth::random_token())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn private_addresses() {
        for ip in [
            "127.0.0.1",
            "10.1.2.3",
            "192.168.0.1",
            "172.16.5.4",
            "169.254.169.254",
            "100.64.0.1",
            "0.0.0.0",
            "::1",
            "fd00::1",
            "fe80::1",
            "::ffff:10.0.0.1",
            "64:ff9b::a9fe:a9fe",
            "2002:7f00:1::",
            "::7f00:1",
        ] {
            assert!(is_private(ip.parse().unwrap()), "{ip}");
        }
        for ip in ["8.8.8.8", "1.1.1.1", "2606:4700::1111"] {
            assert!(!is_private(ip.parse().unwrap()), "{ip}");
        }
    }

    #[test]
    fn signatures_and_urls() {
        // RFC 4231 test case 2
        let mut mac = <Hmac<Sha256> as KeyInit>::new_from_slice(b"Jefe").unwrap();
        mac.update(b"what do ya want for nothing?");
        assert_eq!(hex::encode(mac.finalize().into_bytes()), "5bdcc146bf60754e6a042426089575c75a003f089d2739839dec58b964ec3843");
        assert_eq!(sign("Jefe", b"what do ya want for nothing?"), "sha256=5bdcc146bf60754e6a042426089575c75a003f089d2739839dec58b964ec3843");
        assert!(validate_url("ftp://x").is_err());
        assert!(validate_url("https://user:pw@example.com/h").is_err());
        assert!(validate_url("https://hooks.example.com/wikicraft").is_ok());
    }
}
