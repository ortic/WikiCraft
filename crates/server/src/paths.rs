//! Human-readable page addresses, `/wiki/<SPACE KEY>/<slug>`, next to the permanent
//! `/pages/<id>` ones.
//!
//! The slug comes from the title (`wc_slug()` in SQL, migration 0055: lower case, runs of spaces
//! and punctuation become one `-`). An address leads to the oldest published page with that slug
//! in the space; a page whose slug an older page already has keeps only its permanent address.
//! Addresses a page had before a rename or a move stay in `page_paths` (kept by a trigger), so
//! links to them keep working until another page takes the address over. Old space keys work
//! through `space_key_aliases`.
//!
//! Links to pages by name (`/wiki/…`, `/display/…`, and the `/missing?space=…&title=…` links of
//! Confluence imports) are resolved here: exact title first, then slug, then former addresses.

use sqlx::PgPool;
use uuid::Uuid;

use crate::app::AppState;
use crate::authz::Principal;
use crate::error::ApiResult;

/// The space with this key, or that had it before a rename (`wc_space_by_key()`).
pub async fn space_by_key(db: &PgPool, workspace: Uuid, key: &str) -> sqlx::Result<Option<Uuid>> {
    sqlx::query_scalar("SELECT wc_space_by_key($1, $2)").bind(workspace).bind(key).fetch_one(db).await
}

/// The page a name leads to in a space: a published page with exactly that title (the oldest),
/// else the oldest with that slug, else the page that last had that address (`wc_resolve()`).
pub async fn resolve(db: &PgPool, space: Uuid, name: &str) -> sqlx::Result<Option<Uuid>> {
    sqlx::query_scalar("SELECT wc_resolve($1, $2)").bind(space).bind(name).fetch_one(db).await
}

/// The first page a name leads to in a space that `p` may open (`wc_resolve_candidates()`): a
/// page they can't see neither shows up nor hides one they can.
pub async fn resolve_for(state: &AppState, p: &Principal, space: Uuid, name: &str) -> ApiResult<Option<Uuid>> {
    let ids: Vec<Uuid> = sqlx::query_scalar("SELECT wc_resolve_candidates($1, $2)").bind(space).bind(name).fetch_all(&state.db).await?;
    for id in ids {
        if crate::authz::page_access(state, p, id).await.is_ok_and(|(a, _)| a.view) {
            return Ok(Some(id));
        }
    }
    Ok(None)
}

/// The slug of the page's readable address, if it has one (a title with letters or digits, the
/// oldest published page with that slug in its space).
pub async fn slug_of(db: &PgPool, page: Uuid) -> sqlx::Result<Option<String>> {
    sqlx::query_scalar(
        "SELECT wc_slug(p.title) FROM pages p
         WHERE p.id = $1 AND p.deleted_at IS NULL AND NOT p.draft AND wc_slug(p.title) <> ''
           AND NOT EXISTS (SELECT 1 FROM pages o WHERE o.space_id = p.space_id AND o.deleted_at IS NULL AND NOT o.draft
                           AND wc_slug(o.title) = wc_slug(p.title) AND (o.created_at, o.id) < (p.created_at, p.id))",
    )
    .bind(page)
    .fetch_optional(db)
    .await
}
