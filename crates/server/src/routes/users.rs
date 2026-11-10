use axum::Json;
use axum::extract::{Path, Query, State};
use axum::http::StatusCode;
use serde::{Deserialize, Serialize};
use serde_json::json;
use uuid::Uuid;

use crate::app::{AppState, new_id};
use crate::audit;
use crate::auth::{CurrentUser, hash_password, require_admin};
use crate::error::{ApiError, ApiResult};
use crate::routes::auth::{validate_email, validate_password};

#[derive(Serialize, sqlx::FromRow)]
#[serde(rename_all = "camelCase")]
pub struct UserListItem {
    id: Uuid,
    name: String,
    email: Option<String>,
    role: String,
    status: String,
    avatar_url: Option<String>,
    last_login_at: Option<chrono::DateTime<chrono::Utc>>,
}

#[derive(Deserialize)]
pub struct ListQuery {
    q: Option<String>,
    #[serde(default)]
    include_placeholders: bool,
}

/// Lists workspace members (e-mails only visible to admins). `q` filters by name/e-mail.
pub async fn list(State(state): State<AppState>, CurrentUser(user): CurrentUser, Query(q): Query<ListQuery>) -> ApiResult<Json<Vec<UserListItem>>> {
    if user.is_guest() {
        // guests don't get the member directory
        return Ok(Json(vec![]));
    }
    let pattern = q.q.map(|s| format!("%{}%", s.replace(['%', '_'], "")));
    let mut rows = sqlx::query_as::<_, UserListItem>(
        "SELECT id, name, email, role, status, avatar_url, last_login_at FROM users WHERE workspace_id = $1
         AND ($2::text IS NULL OR name ILIKE $2 OR email ILIKE $2) AND ($3 OR status <> 'placeholder') ORDER BY lower(name) LIMIT 1000",
    )
    .bind(user.workspace_id)
    .bind(pattern)
    .bind(q.include_placeholders && user.is_admin())
    .fetch_all(&state.db)
    .await?;
    if !user.is_admin() {
        for r in &mut rows {
            r.email = None;
            r.last_login_at = None;
        }
    }
    Ok(Json(rows))
}

#[derive(Deserialize)]
pub struct CreateUser {
    email: String,
    name: String,
    password: String,
    role: Option<String>,
}

fn validate_role(r: &str) -> ApiResult<()> {
    if matches!(r, "admin" | "member" | "guest") { Ok(()) } else { Err(ApiError::bad("role must be admin, member or guest")) }
}

pub async fn create(
    State(state): State<AppState>,
    CurrentUser(admin): CurrentUser,
    Json(req): Json<CreateUser>,
) -> ApiResult<(StatusCode, Json<serde_json::Value>)> {
    require_admin(&admin)?;
    let email = validate_email(&req.email)?;
    validate_password(&req.password)?;
    let role = req.role.unwrap_or_else(|| "member".into());
    validate_role(&role)?;
    let hash = hash_password(req.password).await?;
    // claim an imported placeholder with the same e-mail, if any
    let claimed: Option<Uuid> = sqlx::query_scalar("UPDATE users SET name = $3, password_hash = $4, role = $5, status = 'active' WHERE workspace_id = $1 AND lower(email) = $2 AND status = 'placeholder' RETURNING id")
        .bind(admin.workspace_id)
        .bind(&email)
        .bind(req.name.trim())
        .bind(&hash)
        .bind(&role)
        .fetch_optional(&state.db)
        .await?;
    let id = match claimed {
        Some(id) => id,
        None => {
            let id = new_id();
            sqlx::query("INSERT INTO users (id, workspace_id, email, name, password_hash, role) VALUES ($1, $2, $3, $4, $5, $6)")
                .bind(id)
                .bind(admin.workspace_id)
                .bind(&email)
                .bind(req.name.trim())
                .bind(&hash)
                .bind(&role)
                .execute(&state.db)
                .await
                .map_err(|e| match ApiError::from(e) {
                    ApiError::Conflict(_) => ApiError::Conflict("a user with this e-mail already exists".into()),
                    o => o,
                })?;
            id
        }
    };
    audit::log(&state, admin.workspace_id, Some(admin.id), "user.create", Some("user"), Some(id), json!({"email": email, "role": role})).await;
    Ok((StatusCode::CREATED, Json(json!({ "id": id }))))
}

#[derive(Deserialize)]
pub struct UpdateUser {
    name: Option<String>,
    role: Option<String>,
    status: Option<String>,
    password: Option<String>,
}

pub async fn update(State(state): State<AppState>, CurrentUser(me): CurrentUser, Path(id): Path<Uuid>, Json(req): Json<UpdateUser>) -> ApiResult<StatusCode> {
    let self_edit = me.id == id;
    if !self_edit {
        require_admin(&me)?;
    }
    let target = crate::routes::auth::load_user(&state, me.workspace_id, id).await?;
    if req.password.is_some() && !self_edit && (target.role == "owner" || target.role == "admin") {
        // otherwise any admin could take over the owner's (or another admin's) account
        return Err(ApiError::Forbidden);
    }
    if (req.role.is_some() || req.status.is_some()) && (!me.is_admin() || self_edit) {
        return Err(ApiError::Forbidden);
    }
    if target.role == "owner" && (req.role.is_some() || req.status.is_some()) {
        return Err(ApiError::bad("the owner's role and status can't be changed"));
    }
    if let Some(r) = &req.role {
        validate_role(r)?;
    }
    if let Some(s) = &req.status
        && !matches!(s.as_str(), "active" | "deactivated")
    {
        return Err(ApiError::bad("status must be active or deactivated"));
    }
    let password_changed = req.password.is_some();
    let hash = match req.password {
        Some(pw) => {
            validate_password(&pw)?;
            Some(hash_password(pw).await?)
        }
        None => None,
    };
    sqlx::query("UPDATE users SET name = COALESCE($2, name), role = COALESCE($3, role), status = COALESCE($4, status), password_hash = COALESCE($5, password_hash) WHERE id = $1")
        .bind(id)
        .bind(req.name.as_deref().map(str::trim).filter(|n| !n.is_empty()))
        .bind(&req.role)
        .bind(&req.status)
        .bind(hash)
        .execute(&state.db)
        .await?;
    if req.status.as_deref() == Some("deactivated") || (password_changed && !self_edit) {
        sqlx::query("DELETE FROM sessions WHERE user_id = $1").bind(id).execute(&state.db).await?;
    }
    audit::log(&state, me.workspace_id, Some(me.id), "user.update", Some("user"), Some(id), json!({"role": req.role, "status": req.status})).await;
    Ok(StatusCode::NO_CONTENT)
}

#[derive(Serialize, sqlx::FromRow)]
#[serde(rename_all = "camelCase")]
pub struct GroupRow {
    id: Uuid,
    name: String,
    description: Option<String>,
    member_count: i64,
    members: Option<serde_json::Value>,
}

pub async fn list_groups(State(state): State<AppState>, CurrentUser(user): CurrentUser) -> ApiResult<Json<Vec<GroupRow>>> {
    if user.is_guest() {
        return Ok(Json(vec![]));
    }
    Ok(Json(
        sqlx::query_as::<_, GroupRow>(
            "SELECT g.id, g.name, g.description, (SELECT count(*) FROM group_members m WHERE m.group_id = g.id) AS member_count,
                (SELECT json_agg(json_build_object('id', u.id, 'name', u.name) ORDER BY u.name) FROM group_members m JOIN users u ON u.id = m.user_id WHERE m.group_id = g.id) AS members
             FROM groups g WHERE g.workspace_id = $1 ORDER BY lower(g.name)",
        )
        .bind(user.workspace_id)
        .fetch_all(&state.db)
        .await?,
    ))
}

#[derive(Deserialize)]
pub struct CreateGroup {
    name: String,
    description: Option<String>,
}

pub async fn create_group(
    State(state): State<AppState>,
    CurrentUser(admin): CurrentUser,
    Json(req): Json<CreateGroup>,
) -> ApiResult<(StatusCode, Json<serde_json::Value>)> {
    require_admin(&admin)?;
    let name = req.name.trim();
    if name.is_empty() {
        return Err(ApiError::bad("name is required"));
    }
    let id = new_id();
    sqlx::query("INSERT INTO groups (id, workspace_id, name, description) VALUES ($1, $2, $3, $4)")
        .bind(id)
        .bind(admin.workspace_id)
        .bind(name)
        .bind(req.description)
        .execute(&state.db)
        .await?;
    audit::log(&state, admin.workspace_id, Some(admin.id), "group.create", Some("group"), Some(id), json!({"name": name})).await;
    Ok((StatusCode::CREATED, Json(json!({ "id": id }))))
}

pub async fn delete_group(State(state): State<AppState>, CurrentUser(admin): CurrentUser, Path(id): Path<Uuid>) -> ApiResult<StatusCode> {
    require_admin(&admin)?;
    let deleted = sqlx::query("DELETE FROM groups WHERE id = $1 AND workspace_id = $2").bind(id).bind(admin.workspace_id).execute(&state.db).await?;
    if deleted.rows_affected() == 0 {
        return Err(ApiError::NotFound("group"));
    }
    sqlx::query("DELETE FROM space_permissions WHERE subject_type = 'group' AND subject_id = $1").bind(id).execute(&state.db).await?;
    sqlx::query("DELETE FROM page_restrictions WHERE subject_type = 'group' AND subject_id = $1").bind(id).execute(&state.db).await?;
    audit::log(&state, admin.workspace_id, Some(admin.id), "group.delete", Some("group"), Some(id), json!({})).await;
    Ok(StatusCode::NO_CONTENT)
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AddMember {
    user_id: Uuid,
}

pub async fn add_member(
    State(state): State<AppState>,
    CurrentUser(admin): CurrentUser,
    Path(id): Path<Uuid>,
    Json(req): Json<AddMember>,
) -> ApiResult<StatusCode> {
    require_admin(&admin)?;
    let ok: bool = sqlx::query_scalar(
        "SELECT EXISTS (SELECT 1 FROM groups WHERE id = $1 AND workspace_id = $3) AND EXISTS (SELECT 1 FROM users WHERE id = $2 AND workspace_id = $3)",
    )
    .bind(id)
    .bind(req.user_id)
    .bind(admin.workspace_id)
    .fetch_one(&state.db)
    .await?;
    if !ok {
        return Err(ApiError::NotFound("group or user"));
    }
    sqlx::query("INSERT INTO group_members (group_id, user_id) VALUES ($1, $2) ON CONFLICT DO NOTHING").bind(id).bind(req.user_id).execute(&state.db).await?;
    audit::log(&state, admin.workspace_id, Some(admin.id), "group.add_member", Some("group"), Some(id), json!({"userId": req.user_id})).await;
    Ok(StatusCode::NO_CONTENT)
}

pub async fn remove_member(State(state): State<AppState>, CurrentUser(admin): CurrentUser, Path((id, user_id)): Path<(Uuid, Uuid)>) -> ApiResult<StatusCode> {
    require_admin(&admin)?;
    sqlx::query("DELETE FROM group_members m USING groups g WHERE m.group_id = g.id AND g.workspace_id = $3 AND m.group_id = $1 AND m.user_id = $2")
        .bind(id)
        .bind(user_id)
        .bind(admin.workspace_id)
        .execute(&state.db)
        .await?;
    audit::log(&state, admin.workspace_id, Some(admin.id), "group.remove_member", Some("group"), Some(id), json!({"userId": user_id})).await;
    Ok(StatusCode::NO_CONTENT)
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct MergeUser {
    into_user_id: Uuid,
}

/// Rows of `table` whose `column` is `from` now point at `to`. Where that would duplicate a row
/// the target already has (likes, watches, permissions …), the placeholder's row is dropped.
async fn repoint(tx: &mut sqlx::PgConnection, table: &str, column: &str, extra: &str, from: Uuid, to: Uuid) -> ApiResult<u64> {
    let ident = |s: &str| format!("\"{}\"", s.replace('"', ""));
    let (t, c) = (ident(table), ident(column));
    sqlx::query("SAVEPOINT repoint").execute(&mut *tx).await?;
    let bulk = sqlx::query(sqlx::AssertSqlSafe(format!("UPDATE {t} SET {c} = $2 WHERE {c} = $1 {extra}"))).bind(from).bind(to).execute(&mut *tx).await;
    match bulk {
        Ok(r) => {
            sqlx::query("RELEASE SAVEPOINT repoint").execute(&mut *tx).await?;
            return Ok(r.rows_affected());
        }
        Err(sqlx::Error::Database(e)) if e.code().as_deref() == Some("23505") => {
            sqlx::query("ROLLBACK TO SAVEPOINT repoint").execute(&mut *tx).await?;
        }
        Err(e) => return Err(e.into()),
    }
    // row by row: move what doesn't collide, drop the duplicates
    let rows: Vec<String> =
        sqlx::query_scalar(sqlx::AssertSqlSafe(format!("SELECT ctid::text FROM {t} WHERE {c} = $1 {extra}"))).bind(from).fetch_all(&mut *tx).await?;
    let mut moved = 0;
    for ctid in rows {
        sqlx::query("SAVEPOINT repoint_row").execute(&mut *tx).await?;
        match sqlx::query(sqlx::AssertSqlSafe(format!("UPDATE {t} SET {c} = $2 WHERE ctid = $1::tid"))).bind(&ctid).bind(to).execute(&mut *tx).await {
            Ok(_) => {
                moved += 1;
                sqlx::query("RELEASE SAVEPOINT repoint_row").execute(&mut *tx).await?;
            }
            Err(sqlx::Error::Database(e)) if e.code().as_deref() == Some("23505") => {
                sqlx::query("ROLLBACK TO SAVEPOINT repoint_row").execute(&mut *tx).await?;
                sqlx::query(sqlx::AssertSqlSafe(format!("DELETE FROM {t} WHERE ctid = $1::tid"))).bind(&ctid).execute(&mut *tx).await?;
            }
            Err(e) => return Err(e.into()),
        }
    }
    Ok(moved)
}

/// `POST /users/{id}/merge` — an imported author (placeholder) turns out to be an existing
/// person: everything the placeholder authored, owned, liked, watched or was granted moves to
/// that account, mentions of it in pages, versions, comments and templates point at the person,
/// and later imports map the Confluence account to them. The placeholder is removed.
pub async fn merge(
    State(state): State<AppState>,
    CurrentUser(me): CurrentUser,
    Path(id): Path<Uuid>,
    Json(req): Json<MergeUser>,
) -> ApiResult<Json<serde_json::Value>> {
    require_admin(&me)?;
    let source = crate::routes::auth::load_user(&state, me.workspace_id, id).await?;
    let target = crate::routes::auth::load_user(&state, me.workspace_id, req.into_user_id).await?;
    if source.status != "placeholder" {
        return Err(ApiError::bad("only imported placeholder users can be merged into another account"));
    }
    if target.status == "placeholder" || target.id == source.id {
        return Err(ApiError::bad("merge into a real account"));
    }
    let (from, to) = (source.id, target.id);
    let mut tx = state.db.begin().await?;
    // every foreign key to users (new tables are covered automatically)
    let fks: Vec<(String, String)> = sqlx::query_as(
        "SELECT kcu.table_name::text, kcu.column_name::text
           FROM information_schema.referential_constraints rc
           JOIN information_schema.key_column_usage kcu ON kcu.constraint_name = rc.constraint_name AND kcu.constraint_schema = rc.constraint_schema
           JOIN information_schema.constraint_column_usage ccu ON ccu.constraint_name = rc.constraint_name AND ccu.constraint_schema = rc.constraint_schema
          WHERE ccu.table_name = 'users' AND ccu.column_name = 'id' AND kcu.table_schema = current_schema() AND kcu.table_name <> 'users'
          ORDER BY 1, 2",
    )
    .fetch_all(&mut *tx)
    .await?;
    let mut moved = serde_json::Map::new();
    for (table, column) in &fks {
        let n = repoint(&mut tx, table, column, "", from, to).await?;
        if n > 0 {
            moved.insert(format!("{table}.{column}"), json!(n));
        }
    }
    // references without a foreign key
    for table in ["space_permissions", "page_restrictions"] {
        let n = repoint(&mut tx, table, "subject_id", "AND subject_type = 'user'", from, to).await?;
        if n > 0 {
            moved.insert(format!("{table}.subject_id"), json!(n));
        }
    }
    repoint(&mut tx, "audit_events", "actor_id", "", from, to).await?;
    repoint(&mut tx, "import_mappings", "target_id", "AND source_type = 'user'", from, to).await?;
    // the Confluence account now maps to the person (unless they already carry another one)
    let ext: Option<String> = sqlx::query_scalar("SELECT external_ref FROM users WHERE id = $1").bind(from).fetch_one(&mut *tx).await?;
    sqlx::query("UPDATE users SET external_ref = NULL WHERE id = $1").bind(from).execute(&mut *tx).await?;
    sqlx::query("UPDATE users SET external_ref = COALESCE(external_ref, $2) WHERE id = $1").bind(to).bind(ext).execute(&mut *tx).await?;

    // mentions in stored documents (JSON and the pages' Yjs state)
    let (from_s, to_s) = (from.to_string(), to.to_string());
    let like = format!("%{from_s}%");
    let mut pages_changed: Vec<Uuid> = Vec::new();
    let pages: Vec<(Uuid, serde_json::Value, Option<Vec<u8>>)> =
        sqlx::query_as("SELECT id, content, ydoc FROM pages WHERE workspace_id = $1 AND content::text LIKE $2 FOR UPDATE")
            .bind(me.workspace_id)
            .bind(&like)
            .fetch_all(&mut *tx)
            .await?;
    for (pid, content, ystate) in pages {
        let mut doc = wikicraft_doc_model::Node::from_json(&content)?;
        if wikicraft_doc_model::node::replace_mentions(&mut doc, &from_s, &to_s, &target.name) == 0 {
            continue;
        }
        let ystate = match ystate.filter(|b| !b.is_empty()) {
            Some(bytes) => {
                let y = wikicraft_doc_model::ydoc::new_doc();
                wikicraft_doc_model::ydoc::apply_update(&y, &bytes).map_err(|e| ApiError::Internal(anyhow::anyhow!("{e}")))?;
                wikicraft_doc_model::ydoc::replace_mentions(&y, &from_s, &to_s, &target.name);
                doc = wikicraft_doc_model::ydoc::doc_to_pm(&y);
                Some(wikicraft_doc_model::ydoc::encode_state(&y))
            }
            None => None,
        };
        sqlx::query("UPDATE pages SET content = $2, ydoc = COALESCE($3, ydoc) WHERE id = $1")
            .bind(pid)
            .bind(doc.to_json())
            .bind(ystate)
            .execute(&mut *tx)
            .await?;
        pages_changed.push(pid);
    }
    for (table, filter) in [
        ("page_versions", "page_id IN (SELECT id FROM pages WHERE workspace_id = $1)"),
        ("comments", "page_id IN (SELECT id FROM pages WHERE workspace_id = $1)"),
        ("templates", "workspace_id = $1"),
    ] {
        let rows: Vec<(Uuid, serde_json::Value)> =
            sqlx::query_as(sqlx::AssertSqlSafe(format!("SELECT id, content FROM {table} WHERE {filter} AND content::text LIKE $2")))
                .bind(me.workspace_id)
                .bind(&like)
                .fetch_all(&mut *tx)
                .await?;
        let mut n = 0;
        for (rid, content) in rows {
            let mut doc = wikicraft_doc_model::Node::from_json(&content)?;
            if wikicraft_doc_model::node::replace_mentions(&mut doc, &from_s, &to_s, &target.name) > 0 {
                sqlx::query(sqlx::AssertSqlSafe(format!("UPDATE {table} SET content = $2 WHERE id = $1")))
                    .bind(rid)
                    .bind(doc.to_json())
                    .execute(&mut *tx)
                    .await?;
                n += 1;
            }
        }
        if n > 0 {
            moved.insert(format!("{table}.mentions"), json!(n));
        }
    }
    if !pages_changed.is_empty() {
        moved.insert("pages.mentions".into(), json!(pages_changed.len()));
    }
    sqlx::query("DELETE FROM users WHERE id = $1").bind(from).execute(&mut *tx).await?;
    tx.commit().await?;

    // pages open right now: their live document gets the same change (it would otherwise be
    // saved over the rewritten one); a no-op where the room was loaded after the commit
    for pid in &pages_changed {
        if let Some(room) = state.rooms.get(*pid) {
            room.system_change(|d| wikicraft_doc_model::ydoc::replace_mentions(d, &from_s, &to_s, &target.name) > 0);
        }
        state.search.enqueue(*pid);
    }
    audit::log(
        &state,
        me.workspace_id,
        Some(me.id),
        "user.merge",
        Some("user"),
        Some(to),
        json!({"placeholder": source.name, "into": target.name, "moved": moved}),
    )
    .await;
    Ok(Json(json!({"mergedInto": to, "moved": moved})))
}
