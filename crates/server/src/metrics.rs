//! Minimal Prometheus metrics (`/api/metrics`, text exposition format).

use std::sync::atomic::{AtomicU64, Ordering};

use axum::extract::{Request, State};
use axum::http::{StatusCode, header};
use axum::middleware::Next;
use axum::response::{IntoResponse, Response};

use crate::app::AppState;

#[derive(Default)]
pub struct Metrics {
    pub http_2xx: AtomicU64,
    pub http_3xx: AtomicU64,
    pub http_4xx: AtomicU64,
    pub http_5xx: AtomicU64,
    pub http_latency_ms_sum: AtomicU64,
    pub ws_connections: AtomicU64,
    pub ws_messages: AtomicU64,
}

/// Counts responses by status class and accumulates latency.
pub async fn track(State(state): State<AppState>, req: Request, next: Next) -> Response {
    let start = std::time::Instant::now();
    let res = next.run(req).await;
    let m = &state.metrics;
    let c = match res.status().as_u16() {
        200..=299 => &m.http_2xx,
        300..=399 => &m.http_3xx,
        400..=499 => &m.http_4xx,
        _ => &m.http_5xx,
    };
    c.fetch_add(1, Ordering::Relaxed);
    m.http_latency_ms_sum.fetch_add(start.elapsed().as_millis() as u64, Ordering::Relaxed);
    res
}

pub async fn render(State(state): State<AppState>, headers: axum::http::HeaderMap) -> Response {
    if let Some(token) = &state.config.metrics_token {
        let ok = headers.get(header::AUTHORIZATION).and_then(|v| v.to_str().ok()).is_some_and(|v| v.strip_prefix("Bearer ") == Some(token.as_str()));
        if !ok {
            return StatusCode::UNAUTHORIZED.into_response();
        }
    }
    let m = &state.metrics;
    let jobs: Vec<(String, i64)> = sqlx::query_as("SELECT status, count(*) FROM jobs GROUP BY status").fetch_all(&state.db).await.unwrap_or_default();
    let pool = state.db.size();
    let idle = state.db.num_idle();
    let mut out = String::new();
    out.push_str("# HELP wikicraft_http_responses_total HTTP responses by status class\n# TYPE wikicraft_http_responses_total counter\n");
    for (class, v) in [("2xx", &m.http_2xx), ("3xx", &m.http_3xx), ("4xx", &m.http_4xx), ("5xx", &m.http_5xx)] {
        out.push_str(&format!("wikicraft_http_responses_total{{class=\"{class}\"}} {}\n", v.load(Ordering::Relaxed)));
    }
    out.push_str(&format!("# TYPE wikicraft_http_latency_ms_sum counter\nwikicraft_http_latency_ms_sum {}\n", m.http_latency_ms_sum.load(Ordering::Relaxed)));
    out.push_str(&format!("# TYPE wikicraft_ws_connections gauge\nwikicraft_ws_connections {}\n", m.ws_connections.load(Ordering::Relaxed)));
    out.push_str(&format!("# TYPE wikicraft_ws_messages_total counter\nwikicraft_ws_messages_total {}\n", m.ws_messages.load(Ordering::Relaxed)));
    out.push_str(&format!("# TYPE wikicraft_collab_rooms gauge\nwikicraft_collab_rooms {}\n", state.rooms.all().len()));
    out.push_str(&format!("# TYPE wikicraft_db_pool_connections gauge\nwikicraft_db_pool_connections{{state=\"open\"}} {pool}\nwikicraft_db_pool_connections{{state=\"idle\"}} {idle}\n"));
    out.push_str("# TYPE wikicraft_jobs gauge\n");
    for (status, n) in jobs {
        out.push_str(&format!("wikicraft_jobs{{status=\"{status}\"}} {n}\n"));
    }
    out.push_str(&format!("# TYPE wikicraft_build_info gauge\nwikicraft_build_info{{version=\"{}\"}} 1\n", env!("CARGO_PKG_VERSION")));
    ([(header::CONTENT_TYPE, "text/plain; version=0.0.4")], out).into_response()
}
