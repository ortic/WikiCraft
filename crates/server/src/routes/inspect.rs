//! "Inspect permissions": explains why a person (or an anonymous visitor) can or can't view,
//! comment on or edit a page. The verdict comes from the same `page_access` the API enforces;
//! the explanation lists each rule that contributed.

use axum::Json;
use axum::extract::{Path, State};
use serde_json::{Value, json};
use uuid::Uuid;

use crate::app::AppState;
use crate::auth::{CurrentUser, USER_COLUMNS, User};
use crate::authz::{Need, Principal, Role, load_space_access, page_access, require_page, space_role};
use crate::error::{ApiError, ApiResult};

fn step(ok: Option<bool>, text: impl Into<String>) -> Value {
    json!({ "ok": ok, "text": text.into() })
}

/// `GET /api/pages/{id}/access/{user}` (`user` = a user id or `anonymous`) — page admins only.
pub async fn explain(State(state): State<AppState>, CurrentUser(me): CurrentUser, Path((page, who)): Path<(Uuid, String)>) -> ApiResult<Json<Value>> {
    let mine = Principal::load(&state, Some(me.clone())).await?;
    require_page(&state, &mine, page, Need::Admin).await?;

    let target: Option<User> = if who == "anonymous" {
        None
    } else {
        let id: Uuid = who.parse().map_err(|_| ApiError::bad("user must be a user id or 'anonymous'"))?;
        Some(
            sqlx::query_as::<_, User>(sqlx::AssertSqlSafe(format!("SELECT {USER_COLUMNS} FROM users WHERE id = $1 AND workspace_id = $2")))
                .bind(id)
                .bind(me.workspace_id)
                .fetch_optional(&state.db)
                .await?
                .ok_or(ApiError::NotFound("user"))?,
        )
    };
    let p = Principal::load(&state, target.clone()).await?;
    let (access, space_id) = page_access(&state, &p, page).await?;
    let space = load_space_access(&state, space_id).await?;
    let role = space_role(&state, &p, &space).await?;
    let space_name: String = sqlx::query_scalar("SELECT name FROM spaces WHERE id = $1").bind(space_id).fetch_one(&state.db).await?;
    let name = |kind: &str, id: Uuid| {
        let state = state.clone();
        let kind = kind.to_string();
        async move {
            let q = if kind == "group" { "SELECT name FROM groups WHERE id = $1" } else { "SELECT name FROM users WHERE id = $1" };
            sqlx::query_scalar::<_, String>(q).bind(id).fetch_optional(&state.db).await.ok().flatten().unwrap_or_else(|| "(deleted)".into())
        }
    };

    let mut steps: Vec<Value> = Vec::new();
    // 1. who
    match &target {
        None => steps.push(step(None, "Anonymous visitor (not signed in)")),
        Some(u) => {
            steps.push(step(None, format!("{} — workspace role: {}", u.name, u.role)));
            if u.status != "active" {
                steps.push(step(Some(false), format!("The account is {} and can't sign in", u.status)));
            }
        }
    }
    // 2. space role and where it comes from
    match &target {
        None => steps.push(step(
            Some(space.anonymous_access),
            if space.anonymous_access {
                format!("“{space_name}” allows anonymous access (viewer)")
            } else {
                format!("“{space_name}” doesn't allow anonymous access")
            },
        )),
        Some(u) if u.is_admin() => steps.push(step(Some(true), "Workspace admins have full access to every space")),
        Some(u) if space.personal_owner_id == Some(u.id) => steps.push(step(Some(true), "Owner of this personal space")),
        Some(u) => {
            let grants: Vec<(String, Uuid, String, Vec<String>)> =
                sqlx::query_as("SELECT subject_type, subject_id, role, denied FROM space_permissions WHERE space_id = $1")
                    .bind(space_id)
                    .fetch_all(&state.db)
                    .await?;
            let mut any = false;
            for (kind, id, r, denied) in &grants {
                if p.matches(kind, *id) {
                    any = true;
                    let label = if kind == "group" { format!("member of group “{}”", name("group", *id).await) } else { "granted directly".to_string() };
                    let without = if denied.is_empty() || r == "admin" { String::new() } else { format!(", without: {}", denied.join(", ")) };
                    steps.push(step(Some(Role::parse(r) > Role::None), format!("Space role {r} — {label}{without}")));
                }
            }
            if u.is_guest() {
                steps.push(step(None, format!("Guests don't get the space's default role ({}); only explicit grants count", space.default_role)));
            } else {
                let without = if space.default_denied.is_empty() { String::new() } else { format!(", without: {}", space.default_denied.join(", ")) };
                steps.push(step(
                    Some(Role::parse(&space.default_role) > Role::None),
                    format!("Space default role for workspace members: {}{without}", space.default_role),
                ));
            }
            if space.anonymous_access {
                steps.push(step(Some(true), "The space is public (anonymous access gives everyone viewer)"));
            }
            if !any && u.is_guest() && !space.anonymous_access {
                steps.push(step(Some(false), "No grant in this space"));
            }
        }
    }
    steps.push(step(Some(role > Role::None), format!("Effective role in “{space_name}”: {}", role.as_str())));

    // 3. page restrictions along the ancestor chain
    if role > Role::None {
        let chain: Vec<(Uuid, String, i32)> = sqlx::query_as(
            "WITH RECURSIVE chain AS (
                SELECT id, parent_id, title, 0 AS depth FROM pages WHERE id = $1
                UNION ALL SELECT p.id, p.parent_id, p.title, c.depth + 1 FROM pages p JOIN chain c ON p.id = c.parent_id WHERE c.depth < 200)
             SELECT id, title, depth FROM chain ORDER BY depth DESC",
        )
        .bind(page)
        .fetch_all(&state.db)
        .await?;
        let bypass = role == Role::Admin;
        let mut restricted = false;
        for (id, title, depth) in &chain {
            for kind in ["view", "edit"] {
                if kind == "edit" && *depth != 0 {
                    continue; // edit restrictions aren't inherited
                }
                let subjects: Vec<(String, Uuid)> = sqlx::query_as("SELECT subject_type, subject_id FROM page_restrictions WHERE page_id = $1 AND kind = $2")
                    .bind(id)
                    .bind(kind)
                    .fetch_all(&state.db)
                    .await?;
                if subjects.is_empty() {
                    continue;
                }
                restricted = true;
                let mut names = Vec::new();
                for (t, sid) in &subjects {
                    names.push(if t == "group" { format!("group “{}”", name("group", *sid).await) } else { name("user", *sid).await });
                }
                let matched = subjects.iter().any(|(t, sid)| p.matches(t, *sid));
                let where_ = if *depth == 0 { "this page".to_string() } else { format!("ancestor “{title}” (inherited)") };
                let verb = if kind == "view" { "View" } else { "Edit" };
                let text = format!("{verb} restriction on {where_}: only {}", names.join(", "));
                steps.push(step(
                    Some(matched || bypass),
                    if matched {
                        format!("{text} — on the list")
                    } else if bypass {
                        format!("{text} — not on the list, but space admins bypass restrictions")
                    } else {
                        format!("{text} — not on the list")
                    },
                ));
            }
        }
        if !restricted {
            steps.push(step(Some(true), "No page restrictions on this page or its ancestors"));
        }
    }
    Ok(Json(json!({
        "user": target.map(|u| json!({"id": u.id, "name": u.name, "role": u.role, "status": u.status})),
        "spaceRole": role.as_str(),
        "access": {"view": access.view, "comment": access.comment, "edit": access.edit, "admin": access.admin, "perms": access.perms},
        "steps": steps,
    })))
}
