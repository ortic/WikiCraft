//! `GET/POST /api/retrieve`: passages for AI applications (RAG), see [`crate::retrieval`].

use axum::Json;
use axum::extract::{Query, State};
use serde::Deserialize;

use crate::app::AppState;
use crate::auth::MaybeUser;
use crate::authz::Principal;
use crate::error::ApiResult;
use crate::retrieval::{Mode, Options, Retrieval, retrieve};

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RetrieveQuery {
    q: String,
    /// space keys, comma-separated
    space: Option<String>,
    /// labels, comma-separated (pages with any of them)
    label: Option<String>,
    limit: Option<usize>,
    per_page: Option<usize>,
    mode: Option<String>,
    max_chars: Option<usize>,
}

fn list(s: Option<&str>) -> Vec<String> {
    s.unwrap_or("").split(',').map(str::trim).filter(|s| !s.is_empty()).map(str::to_string).collect()
}

/// `GET /api/retrieve?q=&space=&label=&limit=&perPage=&mode=&maxChars=`
pub async fn get(State(state): State<AppState>, MaybeUser(user): MaybeUser, Query(q): Query<RetrieveQuery>) -> ApiResult<Json<Retrieval>> {
    let p = Principal::load(&state, user).await?;
    let opts = Options {
        query: q.q,
        spaces: list(q.space.as_deref()),
        labels: list(q.label.as_deref()),
        limit: q.limit.unwrap_or(8),
        per_page: q.per_page.unwrap_or(3),
        mode: Mode::parse(q.mode.as_deref())?,
        max_chars: q.max_chars,
    };
    Ok(Json(retrieve(&state, &p, &opts).await?))
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RetrieveBody {
    query: String,
    #[serde(default)]
    spaces: Vec<String>,
    #[serde(default)]
    labels: Vec<String>,
    limit: Option<usize>,
    per_page: Option<usize>,
    mode: Option<String>,
    max_chars: Option<usize>,
}

/// `POST /api/retrieve {query, spaces?, labels?, limit?, perPage?, mode?, maxChars?}` (open to
/// read-only tokens: it only reads)
pub async fn post(State(state): State<AppState>, MaybeUser(user): MaybeUser, Json(b): Json<RetrieveBody>) -> ApiResult<Json<Retrieval>> {
    let p = Principal::load(&state, user).await?;
    let opts = Options {
        query: b.query,
        spaces: b.spaces,
        labels: b.labels,
        limit: b.limit.unwrap_or(8),
        per_page: b.per_page.unwrap_or(3),
        mode: Mode::parse(b.mode.as_deref())?,
        max_chars: b.max_chars,
    };
    Ok(Json(retrieve(&state, &p, &opts).await?))
}
