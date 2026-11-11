//! A query language for pages, close to Confluence's CQL:
//!
//! ```text
//! space = ENG AND label in (runbook, "on-call") AND lastModified > now("-30d")
//! type = blogpost and creator = currentUser() order by created desc
//! text ~ "release notes" and not label = archived and ancestor = 1234567
//! ```
//!
//! Fields: `space`, `space.title`, `title`, `text`, `label`, `type` (page, blogpost, folder, link),
//! `creator`, `contributor`, `mention`, `favourite`, `watcher`, `created`, `lastModified`,
//! `id`, `parent`, `ancestor` (page ids, or the ids of imported Confluence pages), `status` and
//! `property[Name]`. Operators: `=` `!=` `~` (contains, `*`/`?` wildcards) `!~` `<` `<=` `>`
//! `>=`, `in (…)`, `not in (…)`, combined with `and`, `or`, `not` and parentheses. Functions:
//! `currentUser()`, `currentSpace()`, `now("-2w")`, `startOfDay|Week|Month|Year("-1")` and
//! `endOf…`. Dates are UTC.
//!
//! A query becomes one SQL condition in which every value is a bound parameter; the results
//! are then filtered by what the caller may see, like search.

use chrono::{DateTime, Datelike, Duration, NaiveDate, TimeZone, Utc};
use serde::Serialize;
use sqlx::{Postgres, QueryBuilder};
use uuid::Uuid;

use crate::app::AppState;
use crate::authz::Principal;
use crate::error::{ApiError, ApiResult};

const MAX_LEN: usize = 4_000;
const MAX_CLAUSES: usize = 60;
const MAX_DEPTH: usize = 30;
const MAX_LIST: usize = 100;

#[derive(Debug, Clone, PartialEq)]
pub struct CqlError {
    pub message: String,
    /// character offset in the query
    pub at: usize,
}

impl std::fmt::Display for CqlError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{} (at character {})", self.message, self.at + 1)
    }
}

fn err<T>(message: impl Into<String>, at: usize) -> Result<T, CqlError> {
    Err(CqlError { message: message.into(), at })
}

// ---------------------------------------------------------------------------------------------
// Tokens

#[derive(Debug, Clone, PartialEq)]
enum Tok {
    Word(String),
    Str(String),
    LParen,
    RParen,
    LBracket,
    RBracket,
    Comma,
    Op(&'static str),
}

fn lex(src: &str) -> Result<Vec<(Tok, usize)>, CqlError> {
    let chars: Vec<char> = src.chars().collect();
    let mut out = Vec::new();
    let mut i = 0;
    while i < chars.len() {
        let c = chars[i];
        let at = i;
        match c {
            c if c.is_whitespace() => i += 1,
            '(' => {
                out.push((Tok::LParen, at));
                i += 1
            }
            ')' => {
                out.push((Tok::RParen, at));
                i += 1
            }
            '[' => {
                out.push((Tok::LBracket, at));
                i += 1
            }
            ']' => {
                out.push((Tok::RBracket, at));
                i += 1
            }
            ',' => {
                out.push((Tok::Comma, at));
                i += 1
            }
            '"' | '\'' => {
                let quote = c;
                let mut s = String::new();
                i += 1;
                loop {
                    match chars.get(i) {
                        None => return err("unclosed quote", at),
                        Some('\\') if chars.get(i + 1).is_some() => {
                            s.push(chars[i + 1]);
                            i += 2;
                        }
                        Some(&ch) if ch == quote => {
                            i += 1;
                            break;
                        }
                        Some(&ch) => {
                            s.push(ch);
                            i += 1;
                        }
                    }
                }
                out.push((Tok::Str(s), at));
            }
            '!' | '=' | '~' | '<' | '>' => {
                let next = chars.get(i + 1).copied();
                let op = match (c, next) {
                    ('!', Some('=')) => "!=",
                    ('!', Some('~')) => "!~",
                    ('<', Some('=')) => "<=",
                    ('>', Some('=')) => ">=",
                    ('=', _) => "=",
                    ('~', _) => "~",
                    ('<', _) => "<",
                    ('>', _) => ">",
                    _ => return err("unexpected “!”", at),
                };
                i += op.len();
                out.push((Tok::Op(op), at));
            }
            _ => {
                let mut s = String::new();
                while let Some(&ch) = chars.get(i) {
                    if ch.is_whitespace() || matches!(ch, '(' | ')' | '[' | ']' | ',' | '"' | '\'' | '!' | '=' | '~' | '<' | '>') {
                        break;
                    }
                    s.push(ch);
                    i += 1;
                }
                out.push((Tok::Word(s), at));
            }
        }
    }
    Ok(out)
}

// ---------------------------------------------------------------------------------------------
// Syntax tree

#[derive(Debug, Clone, PartialEq)]
pub enum Field {
    Space,
    SpaceTitle,
    Title,
    Text,
    Label,
    Type,
    Creator,
    Contributor,
    Mention,
    Favourite,
    Watcher,
    Created,
    Updated,
    Id,
    Parent,
    Ancestor,
    Status,
    Property(String),
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Op {
    Eq,
    Ne,
    Contains,
    NotContains,
    Lt,
    Le,
    Gt,
    Ge,
    In,
    NotIn,
}

#[derive(Debug, Clone, PartialEq)]
pub enum Value {
    Text(String),
    /// lower-case function name and its arguments
    Func(String, Vec<String>),
}

#[derive(Debug, Clone, PartialEq)]
pub enum Expr {
    And(Box<Expr>, Box<Expr>),
    Or(Box<Expr>, Box<Expr>),
    Not(Box<Expr>),
    Clause { field: Field, op: Op, values: Vec<Value>, at: usize },
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub enum OrderField {
    Title,
    Created,
    Updated,
    Space,
}

#[derive(Debug, Clone, PartialEq)]
pub struct Query {
    pub expr: Option<Expr>,
    /// sort keys, `true` = descending
    pub order: Vec<(OrderField, bool)>,
}

fn field(name: &str, at: usize) -> Result<Field, CqlError> {
    Ok(match name.to_ascii_lowercase().as_str() {
        "space" | "space.key" => Field::Space,
        "space.title" | "space.name" => Field::SpaceTitle,
        "title" => Field::Title,
        "text" | "sitesearch" => Field::Text,
        "label" | "labels" => Field::Label,
        "type" => Field::Type,
        "creator" | "created_by" | "author" => Field::Creator,
        "contributor" => Field::Contributor,
        "mention" => Field::Mention,
        "favourite" | "favorite" => Field::Favourite,
        "watcher" => Field::Watcher,
        "created" => Field::Created,
        "lastmodified" | "updated" | "modified" => Field::Updated,
        "id" | "content" => Field::Id,
        "parent" => Field::Parent,
        "ancestor" => Field::Ancestor,
        "status" => Field::Status,
        other => return err(format!("unknown field “{other}”"), at),
    })
}

struct Parser {
    toks: Vec<(Tok, usize)>,
    pos: usize,
    end: usize,
    clauses: usize,
}

impl Parser {
    fn peek(&self) -> Option<&Tok> {
        self.toks.get(self.pos).map(|t| &t.0)
    }

    fn at(&self) -> usize {
        self.toks.get(self.pos).map(|t| t.1).unwrap_or(self.end)
    }

    fn keyword(&self, kw: &str) -> bool {
        matches!(self.peek(), Some(Tok::Word(w)) if w.eq_ignore_ascii_case(kw))
    }

    fn keyword_at(&self, offset: usize, kw: &str) -> bool {
        matches!(self.toks.get(self.pos + offset), Some((Tok::Word(w), _)) if w.eq_ignore_ascii_case(kw))
    }

    fn expect(&mut self, t: Tok, what: &str) -> Result<(), CqlError> {
        if self.peek() == Some(&t) {
            self.pos += 1;
            Ok(())
        } else {
            err(format!("expected {what}"), self.at())
        }
    }

    fn query(&mut self) -> Result<Query, CqlError> {
        let expr = if self.peek().is_none() || (self.keyword("order") && self.keyword_at(1, "by")) { None } else { Some(self.or(0)?) };
        let mut order = Vec::new();
        if self.keyword("order") {
            self.pos += 1;
            if !self.keyword("by") {
                return err("expected “by” after “order”", self.at());
            }
            self.pos += 1;
            loop {
                let at = self.at();
                let Some(Tok::Word(w)) = self.peek().cloned() else { return err("expected a field to sort by", at) };
                self.pos += 1;
                let f = match w.to_ascii_lowercase().as_str() {
                    "title" => OrderField::Title,
                    "created" => OrderField::Created,
                    "lastmodified" | "updated" | "modified" => OrderField::Updated,
                    "space" | "space.title" | "space.key" => OrderField::Space,
                    other => return err(format!("can't sort by “{other}” (title, created, lastModified or space)"), at),
                };
                let desc = if self.keyword("desc") {
                    self.pos += 1;
                    true
                } else {
                    if self.keyword("asc") {
                        self.pos += 1;
                    }
                    false
                };
                order.push((f, desc));
                if self.peek() == Some(&Tok::Comma) {
                    self.pos += 1;
                } else {
                    break;
                }
            }
        }
        if self.peek().is_some() {
            return err("unexpected text; join conditions with “and” or “or”", self.at());
        }
        Ok(Query { expr, order })
    }

    fn or(&mut self, depth: usize) -> Result<Expr, CqlError> {
        let mut left = self.and(depth)?;
        while self.keyword("or") {
            self.pos += 1;
            let right = self.and(depth)?;
            left = Expr::Or(Box::new(left), Box::new(right));
        }
        Ok(left)
    }

    fn and(&mut self, depth: usize) -> Result<Expr, CqlError> {
        let mut left = self.not(depth)?;
        while self.keyword("and") {
            self.pos += 1;
            let right = self.not(depth)?;
            left = Expr::And(Box::new(left), Box::new(right));
        }
        Ok(left)
    }

    fn not(&mut self, depth: usize) -> Result<Expr, CqlError> {
        if depth > MAX_DEPTH {
            return err("the query is nested too deeply", self.at());
        }
        if self.keyword("not") {
            self.pos += 1;
            return Ok(Expr::Not(Box::new(self.not(depth + 1)?)));
        }
        if self.peek() == Some(&Tok::LParen) {
            self.pos += 1;
            let e = self.or(depth + 1)?;
            self.expect(Tok::RParen, "“)”")?;
            return Ok(e);
        }
        self.clause()
    }

    fn clause(&mut self) -> Result<Expr, CqlError> {
        let at = self.at();
        self.clauses += 1;
        if self.clauses > MAX_CLAUSES {
            return err(format!("at most {MAX_CLAUSES} conditions"), at);
        }
        let Some(Tok::Word(name)) = self.peek().cloned() else { return err("expected a field such as space, label, title or text", at) };
        self.pos += 1;
        let f = if name.eq_ignore_ascii_case("property") && self.peek() == Some(&Tok::LBracket) {
            self.pos += 1;
            // property["Due date"] or property[Due date]
            let mut words = Vec::new();
            while let Some(Tok::Word(w) | Tok::Str(w)) = self.peek().cloned() {
                words.push(w);
                self.pos += 1;
            }
            let key = words.join(" ").trim().to_string();
            if key.is_empty() || key.chars().count() > 100 {
                return err("expected a property name: property[Status]", self.at());
            }
            self.expect(Tok::RBracket, "“]”")?;
            Field::Property(key)
        } else {
            field(&name, at)?
        };
        let op_at = self.at();
        let op = match self.peek().cloned() {
            Some(Tok::Op(o)) => {
                self.pos += 1;
                match o {
                    "=" => Op::Eq,
                    "!=" => Op::Ne,
                    "~" => Op::Contains,
                    "!~" => Op::NotContains,
                    "<" => Op::Lt,
                    "<=" => Op::Le,
                    ">" => Op::Gt,
                    _ => Op::Ge,
                }
            }
            Some(Tok::Word(w)) if w.eq_ignore_ascii_case("in") => {
                self.pos += 1;
                Op::In
            }
            Some(Tok::Word(w)) if w.eq_ignore_ascii_case("not") && self.keyword_at(1, "in") => {
                self.pos += 2;
                Op::NotIn
            }
            _ => return err(format!("expected an operator after “{name}” (=, !=, ~, !~, <, >, in, not in)"), op_at),
        };
        let values = if matches!(op, Op::In | Op::NotIn) {
            self.expect(Tok::LParen, "“(” after in")?;
            let mut vs = vec![self.value()?];
            while self.peek() == Some(&Tok::Comma) {
                self.pos += 1;
                vs.push(self.value()?);
                if vs.len() > MAX_LIST {
                    return err(format!("at most {MAX_LIST} values in a list"), self.at());
                }
            }
            self.expect(Tok::RParen, "“)”")?;
            vs
        } else {
            vec![self.value()?]
        };
        check_op(&f, op, op_at)?;
        Ok(Expr::Clause { field: f, op, values, at })
    }

    fn value(&mut self) -> Result<Value, CqlError> {
        let at = self.at();
        match self.peek().cloned() {
            Some(Tok::Str(s)) => {
                self.pos += 1;
                Ok(Value::Text(s))
            }
            Some(Tok::Word(w)) => {
                self.pos += 1;
                if self.peek() == Some(&Tok::LParen) {
                    self.pos += 1;
                    let mut args = Vec::new();
                    while self.peek() != Some(&Tok::RParen) {
                        match self.peek().cloned() {
                            Some(Tok::Str(s)) | Some(Tok::Word(s)) => args.push(s),
                            _ => return err("expected a function argument", self.at()),
                        }
                        self.pos += 1;
                        if self.peek() == Some(&Tok::Comma) {
                            self.pos += 1;
                        }
                        if args.len() > 4 {
                            return err("too many function arguments", self.at());
                        }
                    }
                    self.pos += 1;
                    let name = w.to_ascii_lowercase();
                    if !matches!(
                        name.as_str(),
                        "currentuser"
                            | "currentspace"
                            | "now"
                            | "startofday"
                            | "startofweek"
                            | "startofmonth"
                            | "startofyear"
                            | "endofday"
                            | "endofweek"
                            | "endofmonth"
                            | "endofyear"
                    ) {
                        return err(format!("unknown function “{w}()”"), at);
                    }
                    Ok(Value::Func(name, args))
                } else {
                    Ok(Value::Text(w))
                }
            }
            _ => err("expected a value", at),
        }
    }
}

fn check_op(f: &Field, op: Op, at: usize) -> Result<(), CqlError> {
    use Op::*;
    let ok = match f {
        Field::Space | Field::Label | Field::Type | Field::Creator | Field::Contributor | Field::Mention | Field::Favourite | Field::Watcher => {
            matches!(op, Eq | Ne | In | NotIn)
        }
        Field::Id | Field::Parent | Field::Ancestor => matches!(op, Eq | Ne | In | NotIn),
        Field::SpaceTitle | Field::Title | Field::Status => matches!(op, Eq | Ne | Contains | NotContains | In | NotIn),
        Field::Text => matches!(op, Contains | NotContains | Eq | Ne),
        Field::Created | Field::Updated => matches!(op, Eq | Ne | Lt | Le | Gt | Ge),
        Field::Property(_) => matches!(op, Eq | Ne | Contains | NotContains | Lt | Le | Gt | Ge | In | NotIn),
    };
    if ok { Ok(()) } else { err("this operator doesn't work with that field", at) }
}

/// Parses a query.
pub fn parse(src: &str) -> Result<Query, CqlError> {
    if src.chars().count() > MAX_LEN {
        return err(format!("queries are limited to {MAX_LEN} characters"), MAX_LEN);
    }
    let toks = lex(src)?;
    let end = src.chars().count();
    Parser { toks, pos: 0, end, clauses: 0 }.query()
}

// ---------------------------------------------------------------------------------------------
// Dates

/// A point in time, or a whole day/period when the value names one (`2026-11-01`).
#[derive(Debug, Clone, Copy, PartialEq)]
struct Span {
    start: DateTime<Utc>,
    end: Option<DateTime<Utc>>,
}

/// `-30d`, `+1w`, `-2M`, `1y`, `-4h`, `-15m`, several separated by spaces; a bare number counts
/// in `unit`.
fn offset(s: &str, unit: char, now: DateTime<Utc>) -> Option<DateTime<Utc>> {
    let mut t = now;
    let s = s.trim();
    if s.is_empty() {
        return Some(t);
    }
    for part in s.split_whitespace() {
        let (sign, rest) = match part.as_bytes().first()? {
            b'-' => (-1i64, &part[1..]),
            b'+' => (1, &part[1..]),
            _ => (1, part),
        };
        let digits: String = rest.chars().take_while(|c| c.is_ascii_digit()).collect();
        let n: i64 = digits.parse().ok()?;
        let u = rest[digits.len()..].chars().next().unwrap_or(unit);
        if n > 100_000 {
            return None;
        }
        t = match u {
            'y' => add_months(t, sign * n * 12)?,
            'M' => add_months(t, sign * n)?,
            'w' => t + Duration::weeks(sign * n),
            'd' => t + Duration::days(sign * n),
            'h' => t + Duration::hours(sign * n),
            'm' => t + Duration::minutes(sign * n),
            _ => return None,
        };
    }
    Some(t)
}

fn add_months(t: DateTime<Utc>, months: i64) -> Option<DateTime<Utc>> {
    if months >= 0 { t.checked_add_months(chrono::Months::new(months as u32)) } else { t.checked_sub_months(chrono::Months::new((-months) as u32)) }
}

fn midnight(d: NaiveDate) -> DateTime<Utc> {
    Utc.from_utc_datetime(&d.and_hms_opt(0, 0, 0).unwrap_or_default())
}

fn date_value(v: &Value, now: DateTime<Utc>, at: usize) -> Result<Span, CqlError> {
    let bad = |what: &str| err(format!("“{what}” isn't a date (YYYY-MM-DD) or a date function such as now(\"-7d\")"), at);
    match v {
        Value::Text(s) => {
            let s = s.trim();
            for f in ["%Y-%m-%d %H:%M", "%Y/%m/%d %H:%M"] {
                if let Ok(dt) = chrono::NaiveDateTime::parse_from_str(s, f) {
                    return Ok(Span { start: Utc.from_utc_datetime(&dt), end: None });
                }
            }
            for f in ["%Y-%m-%d", "%Y/%m/%d", "%d.%m.%Y"] {
                if let Ok(d) = NaiveDate::parse_from_str(s, f) {
                    let start = midnight(d);
                    return Ok(Span { start, end: Some(start + Duration::days(1)) });
                }
            }
            bad(s)
        }
        Value::Func(name, args) => {
            let arg = args.first().map(String::as_str).unwrap_or("");
            let (unit, period) = match name.as_str() {
                "now" => {
                    let Some(t) = offset(arg, 'd', now) else { return bad(arg) };
                    return Ok(Span { start: t, end: None });
                }
                n if n.ends_with("day") => ('d', "day"),
                n if n.ends_with("week") => ('w', "week"),
                n if n.ends_with("month") => ('M', "month"),
                n if n.ends_with("year") => ('y', "year"),
                _ => return bad(&format!("{name}()")),
            };
            let Some(shifted) = offset(arg, unit, now) else { return bad(arg) };
            let d = shifted.date_naive();
            let first = match period {
                "day" => d,
                "week" => d - Duration::days(d.weekday().num_days_from_monday() as i64),
                "month" => d.with_day(1).unwrap_or(d),
                _ => NaiveDate::from_ymd_opt(d.year(), 1, 1).unwrap_or(d),
            };
            let start = midnight(first);
            let next = match period {
                "day" => start + Duration::days(1),
                "week" => start + Duration::weeks(1),
                "month" => add_months(start, 1).unwrap_or(start),
                _ => add_months(start, 12).unwrap_or(start),
            };
            Ok(if name.starts_with("end") { Span { start: next, end: None } } else { Span { start, end: None } })
        }
    }
}

// ---------------------------------------------------------------------------------------------
// SQL

/// What the functions refer to.
pub struct Context {
    pub workspace: Uuid,
    pub user: Option<Uuid>,
    /// the space of the page or report (`currentSpace()`)
    pub space_key: Option<String>,
    pub now: DateTime<Utc>,
}

fn text(v: &Value, ctx: &Context, at: usize) -> Result<String, CqlError> {
    match v {
        Value::Text(s) => Ok(s.clone()),
        Value::Func(f, _) if f == "currentspace" => match &ctx.space_key {
            Some(k) => Ok(k.clone()),
            None => err("currentSpace() only works on a page or in a space", at),
        },
        Value::Func(f, _) => err(format!("{f}() can't be used here"), at),
    }
}

/// `~` patterns: Confluence's `*` and `?` wildcards; without them, "contains".
fn like_pattern(s: &str) -> String {
    let mut p = String::new();
    let wild = s.contains(['*', '?']);
    for c in s.chars() {
        match c {
            '%' | '_' | '\\' => {
                p.push('\\');
                p.push(c);
            }
            '*' => p.push('%'),
            '?' => p.push('_'),
            c => p.push(c),
        }
    }
    if wild { p } else { format!("%{p}%") }
}

fn normalize_label(s: &str) -> String {
    crate::routes::pages::normalize_label(s).unwrap_or_default()
}

/// A page kind for `type`: pages, blog posts, folders and links (nothing else is searched).
fn kind(s: &str) -> Option<&'static str> {
    match s.to_ascii_lowercase().as_str() {
        "page" => Some("page"),
        "blogpost" | "blog" | "blog_post" | "post" => Some("blog"),
        "folder" => Some("folder"),
        "link" | "smartlink" => Some("link"),
        _ => None,
    }
}

struct Sql<'a> {
    qb: QueryBuilder<Postgres>,
    ctx: &'a Context,
}

impl<'a> Sql<'a> {
    fn push(&mut self, s: &str) {
        self.qb.push(s);
    }

    fn bind<'t, T>(&mut self, v: T)
    where
        T: sqlx::Encode<'t, Postgres> + sqlx::Type<Postgres>,
    {
        self.qb.push_bind(v);
    }

    /// `(SELECT id FROM users WHERE …)` for user values: `currentUser()`, an id, an e-mail address,
    /// a name or the user key of an imported Confluence account.
    fn users(&mut self, values: &[Value], at: usize) -> Result<(), CqlError> {
        self.push("(SELECT id FROM users WHERE workspace_id = ");
        self.bind(self.ctx.workspace);
        self.push(" AND (FALSE");
        for v in values {
            match v {
                Value::Func(f, _) if f == "currentuser" => {
                    self.push(" OR id = ");
                    self.bind(self.ctx.user);
                }
                Value::Func(f, _) => return err(format!("{f}() isn't a person"), at),
                Value::Text(s) => {
                    let s = s.trim().to_string();
                    self.push(" OR id::text = ");
                    self.bind(s.clone());
                    self.push(" OR lower(email) = lower(");
                    self.bind(s.clone());
                    self.push(") OR lower(name) = lower(");
                    self.bind(s.clone());
                    self.push(") OR external_ref = ");
                    self.bind(format!("confluence:{s}"));
                }
            }
        }
        self.push("))");
        Ok(())
    }

    /// `(SELECT …)` page ids for values: WikiCraft ids or ids of imported Confluence pages.
    fn page_ids(&mut self, values: &[Value], ctx: &Context, at: usize) -> Result<(), CqlError> {
        let ids: Vec<String> = values.iter().map(|v| text(v, ctx, at)).collect::<Result<_, _>>()?;
        self.push("(SELECT id FROM pages WHERE workspace_id = ");
        self.bind(ctx.workspace);
        self.push(" AND id::text = ANY(");
        self.bind(ids.clone());
        self.push(") UNION SELECT target_id FROM import_mappings WHERE workspace_id = ");
        self.bind(ctx.workspace);
        self.push(" AND source = 'confluence' AND source_type = 'page' AND source_id = ANY(");
        self.bind(ids);
        self.push("))");
        Ok(())
    }

    fn expr(&mut self, e: &Expr) -> Result<(), CqlError> {
        match e {
            Expr::And(a, b) | Expr::Or(a, b) => {
                self.push("(");
                self.expr(a)?;
                self.push(if matches!(e, Expr::And(..)) { ") AND (" } else { ") OR (" });
                self.expr(b)?;
                self.push(")");
            }
            Expr::Not(a) => {
                self.push("NOT (");
                self.expr(a)?;
                self.push(")");
            }
            Expr::Clause { field, op, values, at } => {
                let negate = matches!(op, Op::Ne | Op::NotIn | Op::NotContains);
                self.push(if negate { "NOT COALESCE((" } else { "COALESCE((" });
                self.clause(field, *op, values, *at)?;
                self.push("), FALSE)");
            }
        }
        Ok(())
    }

    /// The positive form of a clause (negations are added around it).
    fn clause(&mut self, f: &Field, op: Op, values: &[Value], at: usize) -> Result<(), CqlError> {
        let ctx = self.ctx;
        let contains = matches!(op, Op::Contains | Op::NotContains);
        match f {
            Field::Space => {
                let keys: Vec<String> = values.iter().map(|v| text(v, ctx, at).map(|k| k.to_uppercase())).collect::<Result<_, _>>()?;
                self.push("upper(s.key) = ANY(");
                self.bind(keys);
                self.push(")");
            }
            Field::SpaceTitle | Field::Title | Field::Status => {
                let col = match f {
                    Field::SpaceTitle => "s.name",
                    Field::Title => "p.title",
                    _ => "p.page_status",
                };
                if contains {
                    let pat = like_pattern(&text(&values[0], ctx, at)?);
                    self.push(&format!("{col} ILIKE "));
                    self.bind(pat);
                } else {
                    let vs: Vec<String> = values.iter().map(|v| text(v, ctx, at).map(|s| s.to_lowercase())).collect::<Result<_, _>>()?;
                    self.push(&format!("lower({col}) = ANY("));
                    self.bind(vs);
                    self.push(")");
                }
            }
            Field::Text => {
                // every word must appear in the title or the text
                let words: Vec<String> = text(&values[0], ctx, at)?.split_whitespace().take(20).map(like_pattern).collect();
                if words.is_empty() {
                    return err("text needs something to look for", at);
                }
                self.push("TRUE");
                for w in words {
                    self.push(" AND (p.title ILIKE ");
                    self.bind(w.clone());
                    self.push(" OR p.text_content ILIKE ");
                    self.bind(w);
                    self.push(")");
                }
            }
            Field::Label => {
                let names: Vec<String> = values.iter().map(|v| text(v, ctx, at).map(|l| normalize_label(&l))).collect::<Result<_, _>>()?;
                self.push("EXISTS (SELECT 1 FROM labels l WHERE l.page_id = p.id AND l.name = ANY(");
                self.bind(names);
                self.push("))");
            }
            Field::Type => {
                let kinds: Vec<String> =
                    values.iter().map(|v| text(v, ctx, at)).collect::<Result<Vec<_>, _>>()?.iter().filter_map(|k| kind(k)).map(String::from).collect();
                self.push("p.kind = ANY(");
                self.bind(kinds);
                self.push(")");
            }
            Field::Creator => {
                self.push("p.created_by IN ");
                self.users(values, at)?;
            }
            Field::Contributor => {
                self.push("p.created_by IN ");
                self.users(values, at)?;
                self.push(" OR p.updated_by IN ");
                self.users(values, at)?;
                self.push(" OR EXISTS (SELECT 1 FROM page_versions v WHERE v.page_id = p.id AND v.created_by IN ");
                self.users(values, at)?;
                self.push(")");
            }
            Field::Mention => {
                self.push("EXISTS (SELECT 1 FROM ");
                self.users(values, at)?;
                self.push(
                    r#" mu WHERE jsonb_path_exists(p.content, '$.** ? (@.type == "mention" && @.attrs.id == $id)', jsonb_build_object('id', mu.id::text)))"#,
                );
            }
            Field::Favourite | Field::Watcher => {
                let table = if matches!(f, Field::Favourite) { "favorites" } else { "watches" };
                self.push(&format!("EXISTS (SELECT 1 FROM {table} x WHERE x.page_id = p.id AND x.user_id IN "));
                self.users(values, at)?;
                self.push(")");
            }
            Field::Created | Field::Updated => {
                let col = if matches!(f, Field::Created) { "p.created_at" } else { "p.updated_at" };
                let span = date_value(&values[0], ctx.now, at)?;
                let end = span.end.unwrap_or(span.start);
                match (op, span.end) {
                    // a day (or period) means all of it
                    (Op::Eq | Op::Ne, Some(e)) => {
                        self.push(&format!("{col} >= "));
                        self.bind(span.start);
                        self.push(&format!(" AND {col} < "));
                        self.bind(e);
                    }
                    (Op::Eq | Op::Ne, None) => {
                        self.push(&format!("{col} = "));
                        self.bind(span.start);
                    }
                    (Op::Gt, _) => {
                        self.push(&format!("{col} >= "));
                        self.bind(end);
                    }
                    (Op::Ge, _) => {
                        self.push(&format!("{col} >= "));
                        self.bind(span.start);
                    }
                    (Op::Lt, _) => {
                        self.push(&format!("{col} < "));
                        self.bind(span.start);
                    }
                    _ => {
                        self.push(&format!("{col} < "));
                        self.bind(end);
                    }
                }
            }
            Field::Id | Field::Parent => {
                self.push(if matches!(f, Field::Id) { "p.id IN " } else { "p.parent_id IN " });
                self.page_ids(values, ctx, at)?;
            }
            Field::Ancestor => {
                self.push("p.id IN (WITH RECURSIVE d(id, depth) AS (SELECT id, 1 FROM pages WHERE parent_id IN ");
                self.page_ids(values, ctx, at)?;
                self.push(" UNION ALL SELECT c.id, d.depth + 1 FROM pages c JOIN d ON c.parent_id = d.id WHERE d.depth < 100) SELECT id FROM d)");
            }
            Field::Property(key) => {
                self.push("EXISTS (SELECT 1 FROM page_properties pp WHERE pp.page_id = p.id AND lower(pp.key) = lower(");
                self.bind(key.clone());
                self.push(") AND ");
                let v = text(&values[0], ctx, at)?;
                match op {
                    Op::Contains | Op::NotContains => {
                        self.push("pp.text_value ILIKE ");
                        self.bind(like_pattern(&v));
                    }
                    Op::Lt | Op::Le | Op::Gt | Op::Ge => {
                        let sym = match op {
                            Op::Lt => "<",
                            Op::Le => "<=",
                            Op::Gt => ">",
                            _ => ">=",
                        };
                        match v.trim().parse::<f64>() {
                            // numbers compare as numbers, everything else (dates are YYYY-MM-DD) as text
                            Ok(n) if n.is_finite() => {
                                self.push(&format!("pp.kind = 'number' AND jsonb_typeof(pp.value) = 'number' AND (pp.value #>> '{{}}')::float8 {sym} "));
                                self.bind(n);
                            }
                            _ => {
                                self.push(&format!("pp.text_value {sym} "));
                                self.bind(v);
                            }
                        }
                    }
                    _ => {
                        let vs: Vec<String> = values.iter().map(|v| text(v, ctx, at).map(|s| s.to_lowercase())).collect::<Result<_, _>>()?;
                        self.push("lower(pp.text_value) = ANY(");
                        self.bind(vs);
                        self.push(")");
                    }
                }
                self.push(")");
            }
        }
        Ok(())
    }
}

/// A page found by a query.
#[derive(Serialize, sqlx::FromRow, Clone)]
#[serde(rename_all = "camelCase")]
pub struct CqlPage {
    pub id: Uuid,
    pub title: String,
    pub kind: String,
    pub space_id: Uuid,
    pub space_key: String,
    pub space_name: String,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
    pub updated_by_name: Option<String>,
    pub labels: Vec<String>,
    pub excerpt: String,
}

fn bad_request(e: CqlError) -> ApiError {
    ApiError::bad(format!("CQL: {e}"))
}

/// Runs a query for `p`: the pages it matches that `p` may see, in its order (default: last
/// modified first), at most `limit`. `space_key` is `currentSpace()`.
pub async fn search(state: &AppState, p: &Principal, query: &str, space_key: Option<&str>, limit: usize) -> ApiResult<Vec<CqlPage>> {
    let q = parse(query).map_err(bad_request)?;
    run(state, p, &q, space_key, limit).await
}

/// [`search`] with a parsed query.
pub async fn run(state: &AppState, p: &Principal, q: &Query, space_key: Option<&str>, limit: usize) -> ApiResult<Vec<CqlPage>> {
    let ws = crate::routes::spaces::workspace_of(state, p).await?;
    let visible_sp = crate::routes::search::visible_spaces(state, p).await?;
    if visible_sp.is_empty() {
        return Ok(vec![]);
    }
    let space_ids: Vec<Uuid> = visible_sp.iter().map(|(s, _)| s.id).collect();
    let ctx = Context { workspace: ws, user: p.user_id(), space_key: space_key.map(str::to_string), now: Utc::now() };
    let mut sql = Sql { qb: QueryBuilder::new(""), ctx: &ctx };
    sql.push(
        "SELECT p.id, p.title, p.kind, p.space_id, s.key AS space_key, s.name AS space_name, p.created_at, p.updated_at, u.name AS updated_by_name,
                COALESCE((SELECT array_agg(l.name ORDER BY l.name) FROM labels l WHERE l.page_id = p.id), '{}') AS labels,
                left(COALESCE(p.text_content, ''), 240) AS excerpt
         FROM pages p JOIN spaces s ON s.id = p.space_id LEFT JOIN users u ON u.id = p.updated_by
         WHERE p.workspace_id = ",
    );
    sql.bind(ws);
    sql.push(" AND p.deleted_at IS NULL AND s.archived_at IS NULL AND p.space_id = ANY(");
    sql.bind(space_ids);
    sql.push(")");
    if let Some(e) = &q.expr {
        sql.push(" AND (");
        sql.expr(e).map_err(bad_request)?;
        sql.push(")");
    }
    sql.push(" ORDER BY ");
    if q.order.is_empty() {
        sql.push("p.updated_at DESC");
    }
    for (i, (f, desc)) in q.order.iter().enumerate() {
        if i > 0 {
            sql.push(", ");
        }
        sql.push(match f {
            OrderField::Title => "lower(p.title)",
            OrderField::Created => "p.created_at",
            OrderField::Updated => "p.updated_at",
            OrderField::Space => "lower(s.name)",
        });
        sql.push(if *desc { " DESC" } else { " ASC" });
    }
    sql.push(", p.id LIMIT 2000");
    // text and wildcard conditions scan pages: bounded
    let mut tx = state.db.begin().await?;
    sqlx::query("SET LOCAL statement_timeout = '10s'").execute(&mut *tx).await?;
    let rows: Vec<CqlPage> = sql.qb.build_query_as::<CqlPage>().fetch_all(&mut *tx).await.map_err(|e| match &e {
        sqlx::Error::Database(d) if d.code().as_deref() == Some("57014") => ApiError::bad("CQL: the query took too long; narrow it down (space, label, type)"),
        _ => e.into(),
    })?;
    tx.commit().await?;
    let pairs: Vec<(Uuid, Uuid)> = rows.iter().map(|r| (r.id, r.space_id)).collect();
    let visible = crate::routes::search::filter_visible(state, p, &visible_sp, &pairs).await?;
    let archived = crate::authz::archived_among(state, &rows.iter().map(|r| r.id).collect::<Vec<_>>()).await?;
    Ok(rows.into_iter().filter(|r| visible.contains(&r.id) && !archived.contains(&r.id)).take(limit).collect())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn clause(f: Field, op: Op, v: &str, at: usize) -> Expr {
        Expr::Clause { field: f, op, values: vec![Value::Text(v.into())], at }
    }

    #[test]
    fn parses_confluence_style_queries() {
        let q = parse(r#"space = ENG AND label in (runbook, "on-call") and lastModified > now("-30d") order by title, created desc"#).unwrap();
        let Some(Expr::And(a, c)) = q.expr else { panic!() };
        let Expr::And(s, l) = *a else { panic!() };
        assert_eq!(*s, clause(Field::Space, Op::Eq, "ENG", 0));
        assert_eq!(*l, Expr::Clause { field: Field::Label, op: Op::In, values: vec![Value::Text("runbook".into()), Value::Text("on-call".into())], at: 16 });
        assert!(
            matches!(*c, Expr::Clause { field: Field::Updated, op: Op::Gt, ref values, .. } if values[0] == Value::Func("now".into(), vec!["-30d".into()]))
        );
        assert_eq!(q.order, vec![(OrderField::Title, false), (OrderField::Created, true)]);
        // or binds weaker than and; not and parentheses
        let q = parse("type = page or not (label = a and creator = currentUser())").unwrap();
        assert!(matches!(q.expr, Some(Expr::Or(_, ref r)) if matches!(**r, Expr::Not(_))));
        let q = parse("property[Due date] < 2026-12-01 and property[\"Owner\"] ~ ann").unwrap();
        assert!(matches!(q.expr, Some(Expr::And(ref a, _)) if matches!(**a, Expr::Clause { field: Field::Property(ref k), .. } if k == "Due date")));
        assert!(parse("order by lastmodified desc").unwrap().expr.is_none());
        assert!(parse("").unwrap().expr.is_none());
    }

    #[test]
    fn explains_mistakes() {
        assert_eq!(parse("colour = red").unwrap_err().message, "unknown field “colour”");
        assert_eq!(parse("space = ENG label = x").unwrap_err().at, 12);
        assert!(parse("label").unwrap_err().message.starts_with("expected an operator"));
        assert!(parse("title = \"open").unwrap_err().message.contains("unclosed"));
        assert!(parse("created ~ 2026").unwrap_err().message.contains("operator"));
        assert!(parse("space = whoami()").unwrap_err().message.contains("unknown function"));
        assert!(parse(&"(".repeat(40)).unwrap_err().message.contains("nested"));
        let many = vec!["label = x"; 61].join(" or ");
        assert!(parse(&many).unwrap_err().message.contains("at most"));
    }

    #[test]
    fn dates_and_offsets() {
        let now = Utc.with_ymd_and_hms(2026, 11, 11, 15, 30, 0).unwrap();
        let d = |s: &str| date_value(&Value::Text(s.into()), now, 0).unwrap();
        let f = |n: &str, a: &str| date_value(&Value::Func(n.into(), if a.is_empty() { vec![] } else { vec![a.into()] }), now, 0).unwrap();
        assert_eq!(d("2026-11-01").start, Utc.with_ymd_and_hms(2026, 11, 1, 0, 0, 0).unwrap());
        assert_eq!(d("2026-11-01").end, Some(Utc.with_ymd_and_hms(2026, 11, 2, 0, 0, 0).unwrap()));
        assert_eq!(d("2026/11/01 10:00").end, None);
        assert_eq!(f("now", "-30d").start, now - Duration::days(30));
        assert_eq!(f("now", "-1w 2d").start, now - Duration::days(5));
        assert_eq!(f("now", "+1M").start, Utc.with_ymd_and_hms(2026, 12, 11, 15, 30, 0).unwrap());
        assert_eq!(f("startofweek", "").start, Utc.with_ymd_and_hms(2026, 11, 9, 0, 0, 0).unwrap(), "Monday");
        assert_eq!(f("startofmonth", "-1").start, Utc.with_ymd_and_hms(2026, 10, 1, 0, 0, 0).unwrap());
        assert_eq!(f("endofyear", "").start, Utc.with_ymd_and_hms(2027, 1, 1, 0, 0, 0).unwrap());
        assert!(date_value(&Value::Text("yesterday".into()), now, 0).is_err());
        assert_eq!(like_pattern("50%_off"), "%50\\%\\_off%");
        assert_eq!(like_pattern("rel*se?"), "rel%se_");
    }
}
