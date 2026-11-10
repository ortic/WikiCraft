//! Page properties: typed key/value fields (text, number, date, person, select, URL, page
//! link) shown in a panel above the page body. They are searchable, copied with the page,
//! exported as Markdown front matter, and properties reports can show and filter by them.

use std::collections::{HashMap, HashSet};

use axum::Json;
use axum::extract::{Path, State};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use uuid::Uuid;

use crate::app::AppState;
use crate::auth::{CurrentUser, MaybeUser};
use crate::authz::{Need, Principal, require_page};
use crate::error::{ApiError, ApiResult};
use crate::routes::search::{filter_visible, visible_spaces};
use crate::routes::spaces::load_space;

pub const KINDS: &[&str] = &["text", "number", "date", "user", "select", "url", "page"];
const MAX_PROPERTIES: usize = 50;

#[derive(Debug, Clone, Serialize, sqlx::FromRow)]
#[serde(rename_all = "camelCase")]
pub struct Property {
    #[serde(skip)]
    #[sqlx(default)]
    pub page_id: Uuid,
    pub key: String,
    #[serde(rename = "type")]
    pub kind: String,
    pub value: Option<Value>,
    #[serde(skip)]
    pub text_value: String,
    /// the person's name or the linked page's title (null when gone or not visible)
    #[sqlx(default)]
    pub label: Option<String>,
}

impl Property {
    /// The value as report text: names and titles for people and pages.
    pub fn display(&self) -> String {
        match self.kind.as_str() {
            "user" | "page" => self.label.clone().unwrap_or_default(),
            _ => self.text_value.clone(),
        }
    }
}

/// The properties of several pages, with names and titles resolved for `p` (pages `p`
/// can't see keep a null label).
pub async fn load_many(state: &AppState, p: &Principal, pages: &[Uuid]) -> ApiResult<HashMap<Uuid, Vec<Property>>> {
    let mut rows =
        sqlx::query_as::<_, Property>("SELECT page_id, key, kind, value, text_value FROM page_properties WHERE page_id = ANY($1) ORDER BY page_id, position")
            .bind(pages)
            .fetch_all(&state.db)
            .await?;
    let id_of = |pr: &Property| pr.value.as_ref().and_then(Value::as_str).and_then(|s| Uuid::parse_str(s).ok());
    let user_ids: Vec<Uuid> = rows.iter().filter(|r| r.kind == "user").filter_map(id_of).collect();
    let page_ids: Vec<Uuid> = rows.iter().filter(|r| r.kind == "page").filter_map(id_of).collect();
    let mut names: HashMap<Uuid, String> = HashMap::new();
    if !user_ids.is_empty() {
        let found: Vec<(Uuid, String)> = sqlx::query_as("SELECT id, name FROM users WHERE id = ANY($1)").bind(&user_ids).fetch_all(&state.db).await?;
        names.extend(found);
    }
    if !page_ids.is_empty() {
        let found: Vec<(Uuid, Uuid, String)> = sqlx::query_as("SELECT id, space_id, title FROM pages WHERE id = ANY($1) AND deleted_at IS NULL AND NOT draft")
            .bind(&page_ids)
            .fetch_all(&state.db)
            .await?;
        let spaces = visible_spaces(state, p).await?;
        let pairs: Vec<(Uuid, Uuid)> = found.iter().map(|(id, space, _)| (*id, *space)).collect();
        let visible = filter_visible(state, p, &spaces, &pairs).await?;
        names.extend(found.into_iter().filter(|(id, _, _)| visible.contains(id)).map(|(id, _, title)| (id, title)));
    }
    let mut out: HashMap<Uuid, Vec<Property>> = HashMap::new();
    for r in &mut rows {
        if r.kind == "user" || r.kind == "page" {
            r.label = id_of(r).and_then(|id| names.get(&id).cloned());
        }
    }
    for r in rows {
        out.entry(r.page_id).or_default().push(r);
    }
    Ok(out)
}

pub async fn load(state: &AppState, p: &Principal, page: Uuid) -> ApiResult<Vec<Property>> {
    Ok(load_many(state, p, &[page]).await?.remove(&page).unwrap_or_default())
}

#[derive(Deserialize)]
pub struct PropertyIn {
    pub key: String,
    #[serde(rename = "type")]
    pub kind: String,
    #[serde(default)]
    pub value: Value,
}

#[derive(Deserialize)]
pub struct PutProperties {
    properties: Vec<PropertyIn>,
}

/// Collapses whitespace in a key.
pub fn clean_key(k: &str) -> String {
    k.split_whitespace().collect::<Vec<_>>().join(" ")
}

/// Numbers as people write them: `3`, not `3.0`.
fn number_text(n: f64) -> String {
    if n.fract() == 0.0 && n.abs() < 1e15 { format!("{}", n as i64) } else { n.to_string() }
}

fn bad(msg: impl Into<String>) -> ApiError {
    ApiError::BadRequest(msg.into())
}

/// Checks a value against its type: `(stored value, searchable text)`; empty values are null.
async fn normalize(state: &AppState, p: &Principal, ws: Uuid, key: &str, kind: &str, value: Value) -> ApiResult<(Option<Value>, String)> {
    if !KINDS.contains(&kind) {
        return Err(bad(format!("unknown property type \"{kind}\"")));
    }
    let s = match &value {
        Value::Null => return Ok((None, String::new())),
        Value::String(s) if s.trim().is_empty() => return Ok((None, String::new())),
        Value::String(s) => s.trim().to_string(),
        Value::Number(n) if kind == "number" || kind == "text" || kind == "select" => n.to_string(),
        _ => return Err(bad(format!("\"{key}\": not a valid {kind} value"))),
    };
    Ok(match kind {
        "text" => {
            if s.chars().count() > 5000 {
                return Err(bad(format!("\"{key}\" is too long")));
            }
            (Some(Value::String(s.clone())), s)
        }
        "select" => {
            if s.chars().count() > 200 {
                return Err(bad(format!("\"{key}\" is too long")));
            }
            (Some(Value::String(s.clone())), s)
        }
        "number" => {
            let n: f64 = s.replace(['\'', '_'], "").parse().ok().filter(|n: &f64| n.is_finite()).ok_or_else(|| bad(format!("\"{key}\" must be a number")))?;
            let v = if n.fract() == 0.0 && n.abs() < 1e15 { json!(n as i64) } else { json!(n) };
            (Some(v), number_text(n))
        }
        "date" => {
            let d = chrono::NaiveDate::parse_from_str(&s, "%Y-%m-%d").map_err(|_| bad(format!("\"{key}\" must be a date (YYYY-MM-DD)")))?;
            let t = d.format("%Y-%m-%d").to_string();
            (Some(Value::String(t.clone())), t)
        }
        "url" => {
            let lower = s.to_lowercase();
            if !(lower.starts_with("https://") || lower.starts_with("http://") || lower.starts_with("mailto:"))
                || s.chars().any(char::is_whitespace)
                || s.len() > 2000
            {
                return Err(bad(format!("\"{key}\" must be a web address (https://…)")));
            }
            (Some(Value::String(s.clone())), s)
        }
        "user" => {
            let id = Uuid::parse_str(&s).map_err(|_| bad(format!("\"{key}\" must be a person")))?;
            let name: Option<String> =
                sqlx::query_scalar("SELECT name FROM users WHERE id = $1 AND workspace_id = $2").bind(id).bind(ws).fetch_optional(&state.db).await?;
            let name = name.ok_or_else(|| bad(format!("\"{key}\": unknown person")))?;
            (Some(Value::String(id.to_string())), name)
        }
        "page" => {
            let id = Uuid::parse_str(&s).map_err(|_| bad(format!("\"{key}\" must be a page")))?;
            if require_page(state, p, id, Need::View).await.is_err() {
                return Err(bad(format!("\"{key}\": unknown page")));
            }
            // no title in the search text: readers of this page may not see the other one
            (Some(Value::String(id.to_string())), String::new())
        }
        _ => unreachable!(),
    })
}

/// Validates and stores a page's properties (replacing all of them). Shared by the API, page
/// copies and imports.
pub async fn save(state: &AppState, p: &Principal, ws: Uuid, page: Uuid, props: Vec<PropertyIn>) -> ApiResult<()> {
    if props.len() > MAX_PROPERTIES {
        return Err(bad(format!("a page can have at most {MAX_PROPERTIES} properties")));
    }
    let mut seen = HashSet::new();
    let mut rows = Vec::new();
    for prop in props {
        let key = clean_key(&prop.key);
        if key.is_empty() || key.chars().count() > 100 {
            return Err(bad("property names need 1 to 100 characters"));
        }
        if !seen.insert(key.to_lowercase()) {
            return Err(bad(format!("\"{key}\" appears twice")));
        }
        let (value, text) = normalize(state, p, ws, &key, &prop.kind, prop.value).await?;
        rows.push((key, prop.kind, value, text));
    }
    let mut tx = state.db.begin().await?;
    sqlx::query("DELETE FROM page_properties WHERE page_id = $1").bind(page).execute(&mut *tx).await?;
    for (i, (key, kind, value, text)) in rows.into_iter().enumerate() {
        sqlx::query("INSERT INTO page_properties (page_id, position, key, kind, value, text_value) VALUES ($1, $2, $3, $4, $5, $6)")
            .bind(page)
            .bind(i as i32)
            .bind(key)
            .bind(kind)
            .bind(value)
            .bind(text)
            .execute(&mut *tx)
            .await?;
    }
    tx.commit().await?;
    state.search.enqueue(page);
    Ok(())
}

/// `GET /api/pages/{id}/properties`
pub async fn get(State(state): State<AppState>, MaybeUser(user): MaybeUser, Path(id): Path<Uuid>) -> ApiResult<Json<Value>> {
    let p = Principal::load(&state, user).await?;
    require_page(&state, &p, id, Need::View).await?;
    Ok(Json(json!({ "properties": load(&state, &p, id).await? })))
}

/// `PUT /api/pages/{id}/properties` `{properties: [{key, type, value}]}` (replaces all)
pub async fn put(
    State(state): State<AppState>,
    CurrentUser(user): CurrentUser,
    Path(id): Path<Uuid>,
    Json(req): Json<PutProperties>,
) -> ApiResult<Json<Value>> {
    let ws = user.workspace_id;
    let p = Principal::load(&state, Some(user)).await?;
    require_page(&state, &p, id, Need::Edit).await?;
    save(&state, &p, ws, id, req.properties).await?;
    Ok(Json(json!({ "properties": load(&state, &p, id).await? })))
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct KeyUse {
    key: String,
    #[serde(rename = "type")]
    kind: String,
    pages: usize,
    /// the most used text and select values
    values: Vec<String>,
}

/// `GET /api/spaces/{key}/properties`: property names used on pages of a space (that the
/// caller can see), most used first, with their common values, for suggestions.
pub async fn space_keys(State(state): State<AppState>, MaybeUser(user): MaybeUser, Path(key): Path<String>) -> ApiResult<Json<Vec<KeyUse>>> {
    let p = Principal::load(&state, user).await?;
    let (space, _) = load_space(&state, &p, &key).await?;
    let rows: Vec<(Uuid, String, String, String)> = sqlx::query_as(
        "SELECT pp.page_id, pp.key, pp.kind, pp.text_value FROM page_properties pp JOIN pages pg ON pg.id = pp.page_id
         WHERE pg.space_id = $1 AND pg.deleted_at IS NULL ORDER BY pg.updated_at DESC LIMIT 5000",
    )
    .bind(space.id)
    .fetch_all(&state.db)
    .await?;
    let spaces = visible_spaces(&state, &p).await?;
    let pairs: Vec<(Uuid, Uuid)> = rows.iter().map(|r| (r.0, space.id)).collect();
    let visible = filter_visible(&state, &p, &spaces, &pairs).await?;
    struct Acc {
        key: String,
        kinds: HashMap<String, usize>,
        pages: usize,
        values: HashMap<String, usize>,
    }
    let mut by_key: HashMap<String, Acc> = HashMap::new();
    for (_, key, kind, text) in rows.into_iter().filter(|r| visible.contains(&r.0)) {
        let a = by_key.entry(key.to_lowercase()).or_insert_with(|| Acc { key: key.clone(), kinds: HashMap::new(), pages: 0, values: HashMap::new() });
        a.pages += 1;
        if (kind == "select" || kind == "text") && !text.is_empty() && text.chars().count() <= 100 {
            *a.values.entry(text).or_default() += 1;
        }
        *a.kinds.entry(kind).or_default() += 1;
    }
    let mut out: Vec<KeyUse> = by_key
        .into_values()
        .map(|a| {
            let kind = a.kinds.into_iter().max_by(|x, y| x.1.cmp(&y.1).then_with(|| y.0.cmp(&x.0))).map(|(k, _)| k).unwrap_or_else(|| "text".into());
            let mut values: Vec<(String, usize)> = a.values.into_iter().collect();
            values.sort_by(|x, y| y.1.cmp(&x.1).then_with(|| x.0.to_lowercase().cmp(&y.0.to_lowercase())));
            KeyUse { key: a.key, kind, pages: a.pages, values: values.into_iter().take(30).map(|(v, _)| v).collect() }
        })
        .collect();
    out.sort_by(|x, y| y.pages.cmp(&x.pages).then_with(|| x.key.to_lowercase().cmp(&y.key.to_lowercase())));
    Ok(Json(out))
}

/// One condition of a properties report filter (`Status = Done; Effort >= 3`).
#[derive(Debug, PartialEq)]
pub struct Condition {
    key: String,
    op: &'static str,
    value: String,
}

/// Parses `key op value` conditions separated by `;` (ops: `=`, `!=`, `~` contains, `<`,
/// `<=`, `>`, `>=`). Unparseable parts are ignored.
pub fn parse_conditions(s: &str) -> Vec<Condition> {
    const OPS: [&str; 7] = ["!=", "<=", ">=", "=", "~", "<", ">"];
    s.split(';')
        .filter_map(|part| {
            // the first operator in the text (keys rarely contain these characters)
            let (at, op) = OPS.iter().filter_map(|op| part.find(op).map(|i| (i, *op))).min_by(|a, b| a.0.cmp(&b.0).then(b.1.len().cmp(&a.1.len())))?;
            let key = clean_key(&part[..at]);
            let value = part[at + op.len()..].trim().trim_matches('"').trim().to_string();
            (!key.is_empty()).then_some(Condition { key, op, value })
        })
        .collect()
}

/// Whether a page's properties (key → display text) meet every condition. Numbers compare
/// as numbers, everything else case-insensitively (ISO dates sort as text).
pub fn matches(props: &[(String, String)], conds: &[Condition]) -> bool {
    conds.iter().all(|c| {
        let v = props.iter().find(|(k, _)| k.eq_ignore_ascii_case(&c.key)).map(|(_, v)| v.as_str());
        let Some(v) = v.filter(|v| !v.is_empty()) else {
            // a missing value is only "not equal"
            return (c.op == "!=" && !c.value.is_empty()) || (c.op == "=" && c.value.is_empty());
        };
        let (a, b) = (v.to_lowercase(), c.value.to_lowercase());
        let ord = match (v.trim().parse::<f64>(), c.value.parse::<f64>()) {
            (Ok(x), Ok(y)) => x.partial_cmp(&y),
            _ => Some(a.cmp(&b)),
        };
        match c.op {
            "=" => a == b || ord == Some(std::cmp::Ordering::Equal),
            "!=" => !(a == b || ord == Some(std::cmp::Ordering::Equal)),
            "~" => a.contains(&b),
            "<" => ord == Some(std::cmp::Ordering::Less),
            "<=" => matches!(ord, Some(std::cmp::Ordering::Less | std::cmp::Ordering::Equal)),
            ">" => ord == Some(std::cmp::Ordering::Greater),
            ">=" => matches!(ord, Some(std::cmp::Ordering::Greater | std::cmp::Ordering::Equal)),
            _ => false,
        }
    })
}

/// Properties from Markdown front matter: our own export's `properties: {…}` flow mapping
/// and any other plain `key: value` line (Obsidian style). Types are inferred: numbers,
/// ISO dates and web addresses; everything else is text.
pub fn from_front_matter(fm: &str) -> Vec<PropertyIn> {
    const RESERVED: [&str; 7] = ["title", "labels", "tags", "created", "updated", "aliases", "properties"];
    let mut out: Vec<PropertyIn> = Vec::new();
    let mut push = |key: &str, v: Value| {
        let key = clean_key(key);
        if key.is_empty() || key.chars().count() > 100 || out.iter().any(|p| p.key.eq_ignore_ascii_case(&key)) || out.len() >= MAX_PROPERTIES {
            return;
        }
        let (kind, value) = match v {
            Value::Number(n) => ("number", Value::Number(n)),
            Value::Bool(b) => ("text", Value::String(if b { "yes" } else { "no" }.into())),
            Value::String(s) => {
                let s = s.trim().to_string();
                let lower = s.to_lowercase();
                if chrono::NaiveDate::parse_from_str(&s, "%Y-%m-%d").is_ok() {
                    ("date", Value::String(s))
                } else if (lower.starts_with("https://") || lower.starts_with("http://")) && !s.contains(char::is_whitespace) {
                    ("url", Value::String(s))
                } else if s.parse::<f64>().is_ok_and(f64::is_finite) {
                    ("number", Value::String(s))
                } else {
                    ("text", Value::String(s))
                }
            }
            _ => return,
        };
        out.push(PropertyIn { key, kind: kind.into(), value });
    };
    for line in fm.lines() {
        // only top-level keys (nested YAML is skipped)
        if line.starts_with([' ', '\t', '-', '#']) {
            continue;
        }
        let Some((k, v)) = line.split_once(':') else { continue };
        let (k, v) = (k.trim(), v.trim());
        if k.eq_ignore_ascii_case("properties") {
            if let Ok(Ordered(fields)) = serde_json::from_str::<Ordered>(v) {
                for (k, v) in fields {
                    push(&k, v);
                }
            }
            continue;
        }
        if v.is_empty() || RESERVED.contains(&k.to_lowercase().as_str()) || v.starts_with(['[', '{', '|', '>', '&', '*']) {
            continue;
        }
        let v = serde_json::from_str::<Value>(v)
            .ok()
            .filter(|v| !v.is_object() && !v.is_array())
            .unwrap_or_else(|| Value::String(v.trim_matches(['\'', '"']).to_string()));
        push(k.trim_matches(['\'', '"']), v);
    }
    out
}

/// A JSON object's fields in their written order.
struct Ordered(Vec<(String, Value)>);

impl<'de> Deserialize<'de> for Ordered {
    fn deserialize<D: serde::Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        struct Fields;
        impl<'de> serde::de::Visitor<'de> for Fields {
            type Value = Ordered;
            fn expecting(&self, f: &mut std::fmt::Formatter) -> std::fmt::Result {
                f.write_str("an object")
            }
            fn visit_map<A: serde::de::MapAccess<'de>>(self, mut m: A) -> Result<Ordered, A::Error> {
                let mut out = Vec::new();
                while let Some(entry) = m.next_entry::<String, Value>()? {
                    out.push(entry);
                }
                Ok(Ordered(out))
            }
        }
        d.deserialize_map(Fields)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn props(p: &[(&str, &str)]) -> Vec<(String, String)> {
        p.iter().map(|(k, v)| (k.to_string(), v.to_string())).collect()
    }

    #[test]
    fn conditions() {
        let c = parse_conditions("Status = Done; Effort >= 3 ; Owner ~ ann; Due < 2026-12-01; Phase != \"Beta\"; junk");
        assert_eq!(c.len(), 5);
        assert_eq!(c[1], Condition { key: "Effort".into(), op: ">=", value: "3".into() });
        assert_eq!(c[4].value, "Beta");
        let page = props(&[("status", "done"), ("Effort", "12"), ("Owner", "Ann Lee"), ("Due", "2026-11-30")]);
        assert!(matches(&page, &c), "numbers compare as numbers (12 >= 3), dates as text");
        assert!(!matches(&page, &parse_conditions("Effort < 4")));
        assert!(matches(&page, &parse_conditions("Effort = 12.0")));
        assert!(matches(&page, &parse_conditions("Phase != Beta")), "a missing value is not equal");
        assert!(!matches(&page, &parse_conditions("Phase = Beta")));
        assert!(matches(&page, &parse_conditions("Phase =")), "empty means unset");
        assert!(!matches(&page, &parse_conditions("Status =")));
    }

    #[test]
    fn front_matter_properties() {
        let p = from_front_matter(
            "title: \"Plan\"\ntags: [a]\nproperties: {\"Status\": \"Done\", \"Effort\": 3}\nstatus: ignored-dup\ndue: 2026-12-01\nsource: https://example.com/x\nrating: 4.5\nnested:\n  child: 1\ndraft: true",
        );
        let got: Vec<(String, String, Value)> = p.into_iter().map(|p| (p.key, p.kind, p.value)).collect();
        assert_eq!(
            got,
            vec![
                ("Status".into(), "text".into(), json!("Done")),
                ("Effort".into(), "number".into(), json!(3)),
                ("due".into(), "date".into(), json!("2026-12-01")),
                ("source".into(), "url".into(), json!("https://example.com/x")),
                ("rating".into(), "number".into(), json!(4.5)),
                ("draft".into(), "text".into(), json!("yes")),
            ]
        );
    }
}
