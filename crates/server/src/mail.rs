//! Outgoing e-mail over SMTP (`WIKICRAFT_SMTP_URL`).
//!
//! Mails are written to the `outbox` table and sent by a worker on every replica
//! (`FOR UPDATE SKIP LOCKED`, retries with exponential backoff), so a request never waits for
//! the mail server and nothing is lost when a pod restarts. Notification e-mails are a per-user
//! digest of notifications that are still unread a couple of minutes after they were created.

use std::collections::BTreeMap;
use std::time::Duration;

use lettre::message::{Mailbox, MultiPart, header::ContentType};
use lettre::{AsyncSmtpTransport, AsyncTransport, Message, Tokio1Executor};
use serde_json::Value;
use uuid::Uuid;

use crate::app::{AppState, new_id};
use crate::authz::{Principal, page_access};
use crate::config::Config;

const MAX_ATTEMPTS: i32 = 8;
/// Notifications read within this delay don't produce an e-mail.
const DIGEST_DELAY_SECS: i64 = 120;

pub struct Mailer {
    transport: Option<AsyncSmtpTransport<Tokio1Executor>>,
    from: Mailbox,
}

impl Mailer {
    pub fn new(config: &Config) -> anyhow::Result<Self> {
        let from: Mailbox = config.mail_from.parse().map_err(|e| anyhow::anyhow!("invalid WIKICRAFT_MAIL_FROM {:?}: {e}", config.mail_from))?;
        let transport = match config.smtp_url.as_deref().filter(|u| !u.trim().is_empty()) {
            Some(url) => Some(
                AsyncSmtpTransport::<Tokio1Executor>::from_url(url)
                    .map_err(|e| anyhow::anyhow!("invalid WIKICRAFT_SMTP_URL: {e}"))?
                    .timeout(Some(Duration::from_secs(20)))
                    .build(),
            ),
            None => None,
        };
        Ok(Self { transport, from })
    }

    pub fn enabled(&self) -> bool {
        self.transport.is_some()
    }

    pub fn from_address(&self) -> String {
        self.from.to_string()
    }

    /// Sends immediately (used by the worker and the admin test mail).
    pub async fn send_now(&self, to: &str, subject: &str, text: &str, html: Option<&str>) -> anyhow::Result<()> {
        let transport = self.transport.as_ref().ok_or_else(|| anyhow::anyhow!("e-mail is not configured (WIKICRAFT_SMTP_URL)"))?;
        let to: Mailbox = to.parse().map_err(|e| anyhow::anyhow!("invalid recipient {to:?}: {e}"))?;
        let builder = Message::builder().from(self.from.clone()).to(to).subject(subject);
        let msg = match html {
            Some(h) => builder.multipart(MultiPart::alternative_plain_html(text.to_string(), h.to_string()))?,
            None => builder.header(ContentType::TEXT_PLAIN).body(text.to_string())?,
        };
        transport.send(msg).await?;
        Ok(())
    }
}

/// Queues an e-mail. Returns `false` (and logs) when mail isn't configured.
pub async fn queue(state: &AppState, to: &str, subject: &str, text: &str, html: Option<&str>) -> anyhow::Result<bool> {
    if !state.mailer.enabled() {
        tracing::info!(to, subject, "e-mail not configured; not sending");
        return Ok(false);
    }
    sqlx::query("INSERT INTO outbox (id, to_address, subject, body_text, body_html) VALUES ($1, $2, $3, $4, $5)")
        .bind(new_id())
        .bind(to)
        .bind(subject)
        .bind(text)
        .bind(html)
        .execute(&state.db)
        .await?;
    Ok(true)
}

/// Starts this replica's mail worker (outbox + notification digests).
pub fn spawn_worker(state: &AppState) {
    if !state.mailer.enabled() {
        return;
    }
    let state = state.clone();
    tokio::spawn(async move {
        let mut tick = tokio::time::interval(Duration::from_secs(3));
        let mut n: u64 = 0;
        loop {
            tick.tick().await;
            if state.shutting_down.load(std::sync::atomic::Ordering::SeqCst) {
                return;
            }
            if n.is_multiple_of(10)
                && let Err(e) = digests(&state).await
            {
                tracing::warn!("notification e-mails failed: {e:#}");
            }
            n += 1;
            if let Err(e) = drain(&state).await {
                tracing::warn!("outbox failed: {e:#}");
            }
        }
    });
}

#[derive(sqlx::FromRow)]
struct OutboxRow {
    id: Uuid,
    to_address: String,
    subject: String,
    body_text: String,
    body_html: Option<String>,
    attempts: i32,
}

/// Sends what's due in the outbox. Rows are leased (not locked in an open transaction) while
/// the SMTP conversation runs, so a slow mail server doesn't pin database connections.
pub async fn drain(state: &AppState) -> anyhow::Result<usize> {
    let mut sent = 0;
    loop {
        let rows = sqlx::query_as::<_, OutboxRow>(
            "WITH batch AS (
                SELECT id FROM outbox WHERE sent_at IS NULL AND attempts < $1 AND next_attempt_at <= now()
                ORDER BY created_at LIMIT 10 FOR UPDATE SKIP LOCKED)
             UPDATE outbox o SET next_attempt_at = now() + interval '2 minutes' FROM batch WHERE o.id = batch.id
             RETURNING o.id, o.to_address, o.subject, o.body_text, o.body_html, o.attempts",
        )
        .bind(MAX_ATTEMPTS)
        .fetch_all(&state.db)
        .await?;
        if rows.is_empty() {
            return Ok(sent);
        }
        for r in rows {
            match state.mailer.send_now(&r.to_address, &r.subject, &r.body_text, r.body_html.as_deref()).await {
                Ok(()) => {
                    sqlx::query("UPDATE outbox SET sent_at = now(), attempts = attempts + 1, last_error = NULL WHERE id = $1")
                        .bind(r.id)
                        .execute(&state.db)
                        .await?;
                    sent += 1;
                }
                Err(e) => {
                    tracing::warn!(to = %r.to_address, attempt = r.attempts + 1, "sending e-mail failed: {e:#}");
                    // 1, 2, 4, … minutes
                    sqlx::query(
                        "UPDATE outbox SET attempts = attempts + 1, last_error = $2, next_attempt_at = now() + make_interval(mins => $3) WHERE id = $1",
                    )
                    .bind(r.id)
                    .bind(format!("{e:#}"))
                    .bind(1i32 << r.attempts.clamp(0, 10))
                    .execute(&state.db)
                    .await?;
                }
            }
        }
    }
}

#[derive(sqlx::FromRow)]
struct PendingNotification {
    user_id: Uuid,
    kind: String,
    page_id: Option<Uuid>,
    data: Value,
    read_at: Option<chrono::DateTime<chrono::Utc>>,
    actor_name: Option<String>,
    page_title: Option<String>,
    email: Option<String>,
    user_name: String,
    status: String,
    preference: String,
}

/// Claims notifications older than the digest delay and mails each user a summary of the
/// unread ones they want by e-mail and can still see.
pub async fn digests(state: &AppState) -> anyhow::Result<usize> {
    let rows = sqlx::query_as::<_, PendingNotification>(
        "WITH claimed AS (
            UPDATE notifications SET emailed_at = now() WHERE id IN (
                SELECT id FROM notifications WHERE emailed_at IS NULL AND created_at < now() - make_interval(secs => $1)
                ORDER BY created_at LIMIT 500 FOR UPDATE SKIP LOCKED)
            RETURNING id, user_id, kind, page_id, actor_id, data, created_at, read_at)
         SELECT c.user_id, c.kind, c.page_id, c.data, c.read_at, a.name AS actor_name, p.title AS page_title,
                u.email, u.name AS user_name, u.status, u.email_notifications AS preference
         FROM claimed c JOIN users u ON u.id = c.user_id LEFT JOIN users a ON a.id = c.actor_id LEFT JOIN pages p ON p.id = c.page_id AND p.deleted_at IS NULL
         ORDER BY c.user_id, c.created_at",
    )
    .bind(DIGEST_DELAY_SECS as f64)
    .fetch_all(&state.db)
    .await?;
    let mut per_user: BTreeMap<Uuid, Vec<PendingNotification>> = BTreeMap::new();
    for r in rows {
        let wanted = match r.preference.as_str() {
            "none" => false,
            "all" => true,
            _ => r.kind != "page_updated",
        };
        if r.read_at.is_none() && wanted && r.status == "active" && r.email.is_some() && r.page_title.is_some() {
            per_user.entry(r.user_id).or_default().push(r);
        }
    }
    let base = state.config.base_url.trim_end_matches('/').to_string();
    let mut mails = 0;
    for (user_id, items) in per_user {
        // access may have changed since the notification was created
        let user = sqlx::query_as::<_, crate::auth::User>(sqlx::AssertSqlSafe(format!("SELECT {} FROM users WHERE id = $1", crate::auth::USER_COLUMNS)))
            .bind(user_id)
            .fetch_one(&state.db)
            .await?;
        let p = Principal::load(state, Some(user)).await.map_err(|e| anyhow::anyhow!("{e}"))?;
        let mut lines: Vec<(String, String, Option<String>)> = Vec::new();
        for n in &items {
            let Some(page) = n.page_id else { continue };
            if !page_access(state, &p, page).await.map(|(a, _)| a.view).unwrap_or(false) {
                continue;
            }
            let who = n.actor_name.clone().unwrap_or_else(|| "Someone".into());
            let title = n.page_title.clone().unwrap_or_default();
            let what = match n.kind.as_str() {
                "page_mention" => format!("{who} mentioned you on “{title}”"),
                "mention" => format!("{who} mentioned you in a comment on “{title}”"),
                "comment" => format!("{who} commented on “{title}”"),
                "reply" => format!("{who} replied to your comment on “{title}”"),
                "page_updated" => format!("{who} updated “{title}”"),
                "share" => format!("{who} shared “{title}” with you"),
                other => format!("{who}: {other} on “{title}”"),
            };
            let anchor = n.data.get("commentId").and_then(Value::as_str).map(|c| format!("#comment-{c}")).unwrap_or_default();
            lines.push((what, format!("{base}/pages/{page}{anchor}"), n.data.get("excerpt").and_then(Value::as_str).map(String::from)));
        }
        if lines.is_empty() {
            continue;
        }
        let first = &items[0];
        let subject = if lines.len() == 1 { lines[0].0.clone() } else { format!("{} updates in WikiCraft", lines.len()) };
        let mut text = format!("Hi {},\n\n", first.user_name);
        let mut html = String::new();
        for (what, url, excerpt) in &lines {
            text.push_str(&format!("• {what}\n  {url}\n"));
            if let Some(e) = excerpt {
                text.push_str(&format!("  “{e}”\n"));
            }
            text.push('\n');
            html.push_str(&format!(
                "<tr><td style=\"padding:10px 0;border-bottom:1px solid #eee\"><a href=\"{}\" style=\"color:#1d4ed8;text-decoration:none;font-weight:600\">{}</a>{}</td></tr>",
                esc(url),
                esc(what),
                excerpt.as_deref().map(|e| format!("<div style=\"color:#555;margin-top:4px\">{}</div>", esc(e))).unwrap_or_default()
            ));
        }
        text.push_str(&format!("You get these e-mails for unread notifications. Change this in your settings: {base}/settings\n"));
        let html = layout(
            &subject,
            &format!("<p>Hi {},</p><table style=\"width:100%;border-collapse:collapse\">{html}</table>", esc(&first.user_name)),
            &format!("You get these e-mails for unread notifications. <a href=\"{base}/settings\">Change e-mail settings</a>."),
        );
        if queue(state, first.email.as_deref().unwrap_or_default(), &subject, &text, Some(&html)).await? {
            mails += 1;
        }
    }
    Ok(mails)
}

pub fn esc(s: &str) -> String {
    wikicraft_doc_model::html::escape(s)
}

/// Minimal, client-friendly HTML wrapper.
pub fn layout(title: &str, body: &str, footer: &str) -> String {
    format!(
        "<!doctype html><html><head><meta charset=\"utf-8\"><title>{t}</title></head><body style=\"margin:0;background:#f6f7f9;font-family:-apple-system,'Segoe UI',Roboto,sans-serif;color:#1f2328\">\
         <div style=\"max-width:560px;margin:0 auto;padding:24px\"><div style=\"font-weight:700;font-size:18px;margin-bottom:16px\"><span style=\"display:inline-block;width:22px;height:22px;border-radius:6px;background:#2563eb;color:#fff;text-align:center;line-height:22px;font-size:13px;margin-right:8px;vertical-align:-3px\">W</span>WikiCraft</div>\
         <div style=\"background:#fff;border:1px solid #e5e7eb;border-radius:12px;padding:24px;line-height:1.55\">{body}</div>\
         <p style=\"color:#6b7280;font-size:12px;margin-top:16px\">{footer}</p></div></body></html>",
        t = esc(title)
    )
}

/// A call-to-action e-mail (invitation, password reset): returns (text, html).
pub fn action_mail(greeting: &str, intro: &str, button: &str, url: &str, outro: &str) -> (String, String) {
    let text = format!("{greeting}\n\n{intro}\n\n{button}: {url}\n\n{outro}\n");
    let html = layout(
        button,
        &format!(
            "<p>{}</p><p>{}</p><p style=\"margin:24px 0\"><a href=\"{}\" style=\"background:#2563eb;color:#fff;padding:10px 18px;border-radius:8px;text-decoration:none;font-weight:600\">{}</a></p><p style=\"color:#555;font-size:13px\">Or open this link: <br><a href=\"{2}\" style=\"color:#1d4ed8;word-break:break-all\">{2}</a></p>",
            esc(greeting),
            esc(intro),
            esc(url),
            esc(button)
        ),
        &esc(outro),
    );
    (text, html)
}
