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
    let (following, followers): (bool, i64) = sqlx::query_as(
        "SELECT EXISTS (SELECT 1 FROM user_follows WHERE follower_id = $2 AND followee_id = $1), (SELECT count(*) FROM user_follows WHERE followee_id = $1)",
    )
    .bind(id)
    .bind(user.id)
    .fetch_one(&state.db)
    .await?;
    o.insert("following".into(), json!(following));
    o.insert("followers".into(), json!(followers));
    Ok(Json(v))
}

/// `PUT /people/{id}/follow` — follow someone: their edits and comments show up in Home →
/// Activity → People I follow (only on pages the follower can see anyway).
pub async fn follow(State(state): State<AppState>, CurrentUser(user): CurrentUser, Path(id): Path<Uuid>) -> ApiResult<axum::http::StatusCode> {
    if id == user.id {
        return Err(ApiError::bad("you can't follow yourself"));
    }
    if user.is_guest() {
        return Err(ApiError::Forbidden);
    }
    let exists: bool = sqlx::query_scalar("SELECT EXISTS (SELECT 1 FROM users WHERE id = $1 AND workspace_id = $2 AND status = 'active')")
        .bind(id)
        .bind(user.workspace_id)
        .fetch_one(&state.db)
        .await?;
    if !exists {
        return Err(ApiError::NotFound("person"));
    }
    sqlx::query("INSERT INTO user_follows (follower_id, followee_id) VALUES ($1, $2) ON CONFLICT DO NOTHING").bind(user.id).bind(id).execute(&state.db).await?;
    Ok(axum::http::StatusCode::NO_CONTENT)
}

/// `DELETE /people/{id}/follow`
pub async fn unfollow(State(state): State<AppState>, CurrentUser(user): CurrentUser, Path(id): Path<Uuid>) -> ApiResult<axum::http::StatusCode> {
    sqlx::query("DELETE FROM user_follows WHERE follower_id = $1 AND followee_id = $2").bind(user.id).bind(id).execute(&state.db).await?;
    Ok(axum::http::StatusCode::NO_CONTENT)
}

/// `GET /me/following` — the people the caller follows, by name.
pub async fn following(State(state): State<AppState>, CurrentUser(user): CurrentUser) -> ApiResult<Json<Vec<Person>>> {
    Ok(Json(
        sqlx::query_as::<_, Person>(sqlx::AssertSqlSafe(format!(
            "SELECT {PERSON} FROM users WHERE id IN (SELECT followee_id FROM user_follows WHERE follower_id = $1) AND status = 'active' ORDER BY lower(name)"
        )))
        .bind(user.id)
        .fetch_all(&state.db)
        .await?,
    ))
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ProfileUpdate {
    job_title: Option<String>,
    department: Option<String>,
    location: Option<String>,
    timezone: Option<String>,
    bio: Option<String>,
    /// interface language (one of `branding::LANGUAGES`); "" = the workspace's default
    language: Option<String>,
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
        // a name the database knows (reminders are scheduled in it)
        let ok: bool = sqlx::query_scalar("SELECT EXISTS (SELECT 1 FROM pg_timezone_names WHERE name = $1)").bind(tz).fetch_one(&state.db).await?;
        if !ok {
            return Err(ApiError::bad("timezone must be an IANA time zone name such as Europe/Zurich"));
        }
    }
    let language = match &req.language {
        Some(l) => Some(crate::routes::branding::clean_language(l)?),
        None => None,
    };
    for (column, value) in
        [("job_title", job_title), ("department", department), ("location", location), ("timezone", timezone), ("bio", bio), ("language", language)]
    {
        if let Some(v) = value {
            sqlx::query(sqlx::AssertSqlSafe(format!("UPDATE users SET {column} = $2 WHERE id = $1"))).bind(user.id).bind(v).execute(&state.db).await?;
        }
    }
    profile(State(state), CurrentUser(user.clone()), Path(user.id)).await
}

/// Uploads up to this size are accepted (they're scaled down anyway).
pub const MAX_AVATAR_UPLOAD: usize = 8 << 20;
const AVATAR_SIZE: u32 = 256;

/// A profile picture from an upload: decoded within limits, cropped to a centred square, scaled
/// to 256 px and stored as PNG. Re-encoding drops whatever else the file carried (EXIF, GPS
/// position, scripts in a disguised file).
fn avatar_png(data: &[u8]) -> Option<Vec<u8>> {
    let mut reader = image::ImageReader::new(std::io::Cursor::new(data)).with_guessed_format().ok()?;
    if !matches!(reader.format(), Some(image::ImageFormat::Png | image::ImageFormat::Jpeg | image::ImageFormat::WebP | image::ImageFormat::Gif)) {
        return None;
    }
    let mut limits = image::Limits::default();
    limits.max_image_width = Some(10_000);
    limits.max_image_height = Some(10_000);
    limits.max_alloc = Some(256 << 20);
    reader.limits(limits);
    let img = reader.decode().ok()?;
    let side = img.width().min(img.height());
    let square = img.crop_imm((img.width() - side) / 2, (img.height() - side) / 2, side, side);
    let small = if side > AVATAR_SIZE { square.resize_exact(AVATAR_SIZE, AVATAR_SIZE, image::imageops::FilterType::Lanczos3) } else { square };
    let mut out = std::io::Cursor::new(Vec::new());
    small.write_to(&mut out, image::ImageFormat::Png).ok()?;
    Some(out.into_inner())
}

/// `POST /me/avatar` — a new profile picture (PNG, JPEG, WebP or GIF).
pub async fn upload_avatar(State(state): State<AppState>, CurrentUser(user): CurrentUser, mut mp: axum::extract::Multipart) -> ApiResult<Json<Value>> {
    let mut data = None;
    while let Some(field) = mp.next_field().await.map_err(|e| ApiError::bad(format!("multipart: {e}")))? {
        if field.file_name().is_some() {
            data = Some(field.bytes().await.map_err(|_| ApiError::bad("pictures are limited to 8 MB"))?);
            break;
        }
    }
    let data = data.ok_or_else(|| ApiError::bad("no file in request"))?;
    if data.len() > MAX_AVATAR_UPLOAD {
        return Err(ApiError::bad("pictures are limited to 8 MB"));
    }
    let png = tokio::task::spawn_blocking(move || avatar_png(&data))
        .await
        .map_err(|e| ApiError::Internal(e.into()))?
        .ok_or_else(|| ApiError::bad("use a PNG, JPEG, WebP or GIF picture"))?;
    let key = state.blobs.put(&state.db, bytes::Bytes::from(png)).await.map_err(ApiError::Internal)?;
    // the URL changes with the picture, so browsers may keep it for good
    let url = format!("/api/users/{}/avatar?v={}", user.id, &key[..12]);
    sqlx::query("UPDATE users SET avatar_key = $2, avatar_url = $3 WHERE id = $1").bind(user.id).bind(&key).bind(&url).execute(&state.db).await?;
    Ok(Json(json!({ "avatarUrl": url })))
}

/// `DELETE /me/avatar` — back to initials.
pub async fn delete_avatar(State(state): State<AppState>, CurrentUser(user): CurrentUser) -> ApiResult<axum::http::StatusCode> {
    sqlx::query("UPDATE users SET avatar_key = NULL, avatar_url = NULL WHERE id = $1").bind(user.id).execute(&state.db).await?;
    Ok(axum::http::StatusCode::NO_CONTENT)
}

/// `GET /users/{id}/avatar` — a profile picture (people of the same workspace).
pub async fn avatar(State(state): State<AppState>, CurrentUser(user): CurrentUser, Path(id): Path<Uuid>) -> ApiResult<axum::response::Response> {
    use axum::http::header;
    use axum::response::IntoResponse;
    let key: Option<Option<String>> = sqlx::query_scalar("SELECT avatar_key FROM users WHERE id = $1 AND workspace_id = $2")
        .bind(id)
        .bind(user.workspace_id)
        .fetch_optional(&state.db)
        .await?;
    let Some(Some(key)) = key else { return Err(ApiError::NotFound("picture")) };
    let data = state.blobs.get(&key).await.map_err(ApiError::Internal)?;
    Ok((
        [(header::CONTENT_TYPE, "image/png"), (header::CACHE_CONTROL, "private, max-age=31536000, immutable"), (header::X_CONTENT_TYPE_OPTIONS, "nosniff")],
        data,
    )
        .into_response())
}

/// `GET /people/avatars` — id → picture URL of everybody who has one (for showing pictures
/// wherever a person appears). Guests get none, like the directory.
pub async fn avatars(State(state): State<AppState>, CurrentUser(user): CurrentUser) -> ApiResult<Json<Value>> {
    if user.is_guest() {
        return Ok(Json(json!({})));
    }
    let rows: Vec<(Uuid, String)> =
        sqlx::query_as("SELECT id, avatar_url FROM users WHERE workspace_id = $1 AND avatar_url LIKE '/api/users/%' AND status <> 'placeholder'")
            .bind(user.workspace_id)
            .fetch_all(&state.db)
            .await?;
    Ok(Json(Value::Object(rows.into_iter().map(|(id, url)| (id.to_string(), Value::String(url))).collect())))
}
