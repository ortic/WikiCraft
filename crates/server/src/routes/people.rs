//! People directory and profiles: who works here, what they do, what they wrote lately.

use axum::Json;
use axum::extract::{Path, Query, State};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use uuid::Uuid;

use crate::app::AppState;
use crate::auth::CurrentUser;
use crate::authz::Principal;
use crate::error::{ApiError, ApiResult};

#[derive(Serialize, sqlx::FromRow)]
#[serde(rename_all = "camelCase")]
pub struct Person {
    id: Uuid,
    name: String,
    job_title: Option<String>,
    department: Option<String>,
    location: Option<String>,
    timezone: Option<String>,
    avatar_url: Option<String>,
}

const PERSON: &str = "id, name, job_title, department, location, timezone, avatar_url";

#[derive(Deserialize)]
pub struct DirectoryQuery {
    q: Option<String>,
    department: Option<String>,
}

/// `GET /people` — everybody with an account (not imported authors, not deactivated), by name;
/// `q` searches name, title, department and location. Guests don't get the directory.
pub async fn directory(State(state): State<AppState>, CurrentUser(user): CurrentUser, Query(q): Query<DirectoryQuery>) -> ApiResult<Json<Value>> {
    if user.is_guest() {
        return Ok(Json(json!({ "people": [], "departments": [] })));
    }
    let pattern = q.q.map(|s| s.trim().replace(['%', '_', '\\'], "")).filter(|s| !s.is_empty()).map(|s| format!("%{s}%"));
    let people = sqlx::query_as::<_, Person>(sqlx::AssertSqlSafe(format!(
        "SELECT {PERSON} FROM users WHERE workspace_id = $1 AND status = 'active'
            AND ($2::text IS NULL OR name ILIKE $2 OR job_title ILIKE $2 OR department ILIKE $2 OR location ILIKE $2)
            AND ($3::text IS NULL OR lower(department) = lower($3))
          ORDER BY lower(name) LIMIT 2000"
    )))
    .bind(user.workspace_id)
    .bind(pattern)
    .bind(q.department.filter(|d| !d.trim().is_empty()))
    .fetch_all(&state.db)
    .await?;
    let departments: Vec<(String, i64)> = sqlx::query_as(
        // one entry per department however it's capitalised, named in its most common spelling
        // (ties: the capitalised one)
        "SELECT mode() WITHIN GROUP (ORDER BY department COLLATE \"C\"), count(*) FROM users
          WHERE workspace_id = $1 AND status = 'active' AND department IS NOT NULL
          GROUP BY lower(department) ORDER BY lower(department)",
    )
    .bind(user.workspace_id)
    .fetch_all(&state.db)
    .await?;
    let departments: Vec<Value> = departments.into_iter().map(|(name, count)| json!({"name": name, "count": count})).collect();
    Ok(Json(json!({ "people": people, "departments": departments })))
}

#[derive(Serialize, sqlx::FromRow)]
#[serde(rename_all = "camelCase")]
struct RecentPage {
    id: Uuid,
    title: String,
    space_id: Uuid,
    space_key: String,
    space_name: String,
    edited_at: chrono::DateTime<chrono::Utc>,
}

/// `GET /people/{id}` — a profile: what the person does, their local time zone, their personal
/// space and the pages they edited lately (only those the viewer can see). E-mail addresses
/// only for admins and the person themself, like the member list.
pub async fn profile(State(state): State<AppState>, CurrentUser(user): CurrentUser, Path(id): Path<Uuid>) -> ApiResult<Json<Value>> {
    if user.is_guest() && user.id != id {
        return Err(ApiError::NotFound("person"));
    }
    let person = sqlx::query_as::<_, Person>(sqlx::AssertSqlSafe(format!(
        "SELECT {PERSON} FROM users WHERE id = $1 AND workspace_id = $2 AND status IN ('active', 'deactivated')"
    )))
    .bind(id)
    .bind(user.workspace_id)
    .fetch_optional(&state.db)
    .await?
    .ok_or(ApiError::NotFound("person"))?;
    let (bio, email, status): (Option<String>, Option<String>, String) =
        sqlx::query_as("SELECT bio, email, status FROM users WHERE id = $1").bind(id).fetch_one(&state.db).await?;
    let email = (user.is_admin() || user.id == id).then_some(email).flatten();

    let p = Principal::load(&state, Some(user.clone())).await?;
    let spaces = crate::routes::search::visible_spaces(&state, &p).await?;
    let recent = sqlx::query_as::<_, RecentPage>(
        "SELECT p.id, p.title, p.space_id, s.key AS space_key, s.name AS space_name, max(v.created_at) AS edited_at
           FROM page_versions v JOIN pages p ON p.id = v.page_id JOIN spaces s ON s.id = p.space_id
          WHERE v.created_by = $1 AND p.deleted_at IS NULL AND p.archived_at IS NULL AND s.archived_at IS NULL
          GROUP BY p.id, s.key, s.name ORDER BY max(v.created_at) DESC LIMIT 60",
    )
    .bind(id)
    .fetch_all(&state.db)
    .await?;
    let pairs: Vec<(Uuid, Uuid)> = recent.iter().map(|r| (r.id, r.space_id)).collect();
    let visible = crate::routes::search::filter_visible(&state, &p, &spaces, &pairs).await?;
    let recent: Vec<RecentPage> = recent.into_iter().filter(|r| visible.contains(&r.id)).take(15).collect();
    let personal: Option<String> =
        sqlx::query_scalar("SELECT key FROM spaces WHERE personal_owner_id = $1 AND archived_at IS NULL").bind(id).fetch_optional(&state.db).await?;
    let personal = personal.filter(|k| spaces.iter().any(|(s, _)| &s.key == k));

    let mut v = serde_json::to_value(&person)?;
    let o = v.as_object_mut().expect("object");
    o.insert("bio".into(), json!(bio));
    o.insert("email".into(), json!(email));
    o.insert("deactivated".into(), json!(status == "deactivated"));
    o.insert("personalSpaceKey".into(), json!(personal));
    o.insert("recentPages".into(), serde_json::to_value(recent)?);
    Ok(Json(v))
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ProfileUpdate {
    job_title: Option<String>,
    department: Option<String>,
    location: Option<String>,
    timezone: Option<String>,
    bio: Option<String>,
}

/// A field of the profile: trimmed, empty = cleared, at most `max` characters.
fn field(v: Option<String>, name: &str, max: usize) -> ApiResult<Option<Option<String>>> {
    let Some(v) = v else { return Ok(None) };
    let v = v.trim().to_string();
    if v.chars().count() > max {
        return Err(ApiError::bad(format!("{name} is longer than {max} characters")));
    }
    Ok(Some((!v.is_empty()).then_some(v)))
}

/// `PUT /me/profile` — the caller's job title, department, location, time zone (IANA name such
/// as `Europe/Zurich`) and a short bio. Fields left out stay as they are; empty clears them.
pub async fn update_mine(State(state): State<AppState>, CurrentUser(user): CurrentUser, Json(req): Json<ProfileUpdate>) -> ApiResult<Json<Value>> {
    let job_title = field(req.job_title.map(|v| v.replace('\n', " ")), "jobTitle", 100)?;
    let department = field(req.department.map(|v| v.replace('\n', " ")), "department", 100)?;
    let location = field(req.location.map(|v| v.replace('\n', " ")), "location", 100)?;
    let bio = field(req.bio, "bio", 2000)?;
    let timezone = field(req.timezone, "timezone", 64)?;
    if let Some(Some(tz)) = &timezone {
        let ok = tz == "UTC" || (tz.contains('/') && tz.chars().all(|c| c.is_ascii_alphanumeric() || matches!(c, '/' | '_' | '-' | '+')));
        if !ok {
            return Err(ApiError::bad("timezone must be an IANA time zone name such as Europe/Zurich"));
        }
    }
    for (column, value) in [("job_title", job_title), ("department", department), ("location", location), ("timezone", timezone), ("bio", bio)] {
        if let Some(v) = value {
            sqlx::query(sqlx::AssertSqlSafe(format!("UPDATE users SET {column} = $2 WHERE id = $1"))).bind(user.id).bind(v).execute(&state.db).await?;
        }
    }
    profile(State(state), CurrentUser(user.clone()), Path(user.id)).await
}
