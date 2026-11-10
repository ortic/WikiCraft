//! Page templates: built-in templates plus workspace/space templates created from pages.

use axum::Json;
use axum::extract::{Path, Query, State};
use axum::http::StatusCode;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use uuid::Uuid;
use wikicraft_doc_model::{Node, markdown, template};

use crate::app::{AppState, new_id};
use crate::auth::CurrentUser;
use crate::authz::{Need, Principal, Role, require_page};
use crate::error::{ApiError, ApiResult};
use crate::routes::spaces::load_space;

struct Builtin {
    id: &'static str,
    name: &'static str,
    icon: &'static str,
    description: &'static str,
    title: &'static str,
    markdown: &'static str,
}

const BUILTINS: &[Builtin] = &[
    Builtin {
        id: "builtin:meeting-notes",
        name: "Meeting notes",
        icon: "calendar",
        description: "Agenda, notes, decisions and action items",
        title: "Meeting notes {date}",
        markdown: "**Date:** {date}\n\n**Participants:** @\n\n## Goals\n\n- \n\n## Agenda & notes\n\n| Time | Item | Who | Notes |\n| --- | --- | --- | --- |\n|  |  |  |  |\n\n## Decisions\n\n- \n\n## Action items\n\n- [ ] \n",
    },
    Builtin {
        id: "builtin:decision",
        name: "Decision record",
        icon: "decision",
        description: "Document an important decision (ADR style)",
        title: "Decision: {{Decision}}",
        markdown: "> [!NOTE]\n> **Status:** proposed · **Owner:** @ · **Due date:** {date}\n\n## Context\n\n{?What is the issue that we're seeing that is motivating this decision?}\n\n## Options considered\n\n| Option | Pros | Cons |\n| --- | --- | --- |\n| A |  |  |\n| B |  |  |\n\n## Decision\n\n{?What did we decide, and why?}\n\n## Consequences\n\n{?What becomes easier or harder because of this change?}\n",
    },
    Builtin {
        id: "builtin:how-to",
        name: "How-to article",
        icon: "compass",
        description: "Step-by-step instructions",
        title: "How to {{Task}}",
        markdown: "{?Short introduction: what this guide helps you do.}\n\n> [!TIP]\n> Prerequisites: list anything the reader needs first.\n\n## Steps\n\n1. First step\n2. Second step\n3. Third step\n\n## Related articles\n\n- \n",
    },
    Builtin {
        id: "builtin:retrospective",
        name: "Retrospective",
        icon: "loop",
        description: "What went well, what to improve, actions",
        title: "Retrospective {date}",
        markdown: "**Team:** · **Sprint / period:** \n\n## What went well\n\n- \n\n## What could be improved\n\n- \n\n## Action items\n\n- [ ] \n",
    },
    Builtin {
        id: "builtin:runbook",
        name: "Runbook",
        icon: "siren",
        description: "Operational procedure for an alert or incident",
        title: "Runbook: {{Service}}",
        markdown: "> [!WARNING]\n> Use this runbook when: describe the alert or symptom.\n\n## Impact\n\nWho is affected and how badly?\n\n## Diagnosis\n\n```bash\n# commands to check the system\n```\n\n## Mitigation\n\n1. \n\n## Escalation\n\n- On-call: @\n",
    },
    Builtin {
        id: "builtin:requirements",
        name: "Product requirements",
        icon: "clipboard",
        description: "Problem, goals, requirements and open questions",
        title: "PRD: {{Product or feature}}",
        markdown: "| | |\n| --- | --- |\n| **Owner** | @ |\n| **Status** | Draft |\n| **Target release** | |\n\n## Problem\n\n## Goals\n\n- \n\n## Requirements\n\n| # | Requirement | User story | Importance |\n| --- | --- | --- | --- |\n| 1 |  |  | Must have |\n\n## Out of scope\n\n## Open questions\n\n- [ ] \n",
    },
    Builtin {
        id: "builtin:project-plan",
        name: "Project plan",
        icon: "flag",
        description: "Goals, scope, milestones, risks and the team",
        title: "Project plan: {{Project}}",
        markdown: "| Owner | Status | Target date | Budget |\n| --- | --- | --- | --- |\n| @ | Planning | {date} |  |\n\n## Background\n\n{?Why are we doing this? Link the request, the problem or the opportunity.}\n\n## Goals and non-goals\n\n- Goal: \n- Not a goal: \n\n## Scope\n\n| In scope | Out of scope |\n| --- | --- |\n|  |  |\n\n## Milestones\n\n| Milestone | Date | Owner | Done |\n| --- | --- | --- | --- |\n| Kick-off | {date} | @ |  |\n\n## Risks\n\n| Risk | Likelihood | Impact | Mitigation |\n| --- | --- | --- | --- |\n|  |  |  |  |\n\n## Team\n\n- @\n",
    },
    Builtin {
        id: "builtin:okrs",
        name: "Objectives & key results",
        icon: "chart",
        description: "Quarterly OKRs with owners and progress",
        title: "OKRs {{Quarter}}",
        markdown: "**Period:** · **Team:** \n\n## Objective 1: \n\n| Key result | Owner | Target | Current | Confidence |\n| --- | --- | --- | --- | --- |\n|  | @ |  |  |  |\n\n## Objective 2: \n\n| Key result | Owner | Target | Current | Confidence |\n| --- | --- | --- | --- | --- |\n|  | @ |  |  |  |\n\n## Notes\n\n- \n",
    },
    Builtin {
        id: "builtin:onboarding",
        name: "Onboarding guide",
        icon: "people",
        description: "First days and weeks for a new team member",
        title: "Welcome to the team, {{Name}}",
        markdown: "Welcome! This page helps you find your way in your first weeks.\n\n## Your first day\n\n- [ ] Get your laptop and accounts\n- [ ] Meet your buddy: @\n- [ ] Read the team's working agreements\n\n## Your first week\n\n- [ ] Set up the development environment\n- [ ] Ship a small change\n\n## Who's who\n\n| Person | Role | Ask them about |\n| --- | --- | --- |\n| @ |  |  |\n\n## Useful links\n\n- \n",
    },
    Builtin {
        id: "builtin:postmortem",
        name: "Incident postmortem",
        icon: "shield",
        description: "Blameless review: timeline, impact, root cause, actions",
        title: "Postmortem: {{Incident}}",
        markdown: "> [!WARNING]\n> **Severity:** · **Duration:** · **Incident commander:** @\n\n## Summary\n\nWhat happened, in two sentences.\n\n## Impact\n\nWho was affected and how?\n\n## Timeline\n\n| Time (UTC) | Event |\n| --- | --- |\n|  | Detected |\n|  | Mitigated |\n|  | Resolved |\n\n## Root cause\n\n## What went well\n\n- \n\n## What we'll change\n\n- [ ] \n",
    },
    Builtin {
        id: "builtin:architecture",
        name: "Architecture overview",
        icon: "server",
        description: "Context, building blocks, decisions and quality goals",
        title: "Architecture: {{System}}",
        markdown: "## Context and scope\n\nWhat does the system do, and who and what does it talk to?\n\n```mermaid\nflowchart LR\n  User --> App[Our system]\n  App --> DB[(Database)]\n  App --> Ext[External service]\n```\n\n## Quality goals\n\n| Goal | Scenario |\n| --- | --- |\n| Performance |  |\n| Security |  |\n\n## Building blocks\n\n| Component | Responsibility | Owner |\n| --- | --- | --- |\n|  |  | @ |\n\n## Key decisions\n\n- \n\n## Risks and technical debt\n\n- \n",
    },
    Builtin {
        id: "builtin:status-report",
        name: "Weekly status report",
        icon: "megaphone",
        description: "Highlights, progress, blockers and next steps",
        title: "Status {date}",
        markdown: "**Overall:** On track\n\n## Highlights\n\n- \n\n## Progress\n\n| Workstream | Status | Notes |\n| --- | --- | --- |\n|  | On track |  |\n\n## Blockers and risks\n\n- \n\n## Next week\n\n- [ ] \n",
    },
];

/// What makes a built-in template a blueprint: pages made from it (from the Create button, not
/// below a page someone picked) are filed under an index page that lists them, newest first,
/// and get labels, so meeting notes don't pile up at the top of the space.
pub struct Blueprint {
    pub index_title: &'static str,
    pub intro: &'static str,
    pub labels: &'static [&'static str],
}

pub fn blueprint(template_id: &str) -> Option<Blueprint> {
    let (index_title, intro, labels): (&'static str, &'static str, &'static [&'static str]) = match template_id {
        "builtin:meeting-notes" => ("Meeting notes", "Notes from our meetings, newest first.", &["meeting-notes"]),
        "builtin:decision" => ("Decisions", "The decisions we made and why, newest first.", &["decision"]),
        "builtin:retrospective" => ("Retrospectives", "What went well and what we'll change, newest first.", &["retrospective"]),
        "builtin:postmortem" => ("Postmortems", "Incident reviews, newest first.", &["postmortem", "incident"]),
        "builtin:status-report" => ("Status reports", "Status reports, newest first.", &["status-report"]),
        "builtin:how-to" => ("How-to articles", "Step-by-step guides.", &["how-to"]),
        "builtin:runbook" => ("Runbooks", "What to do when something needs fixing.", &["runbook"]),
        "builtin:requirements" => ("Product requirements", "Requirements documents.", &["requirements"]),
        _ => return None,
    };
    Some(Blueprint { index_title, intro, labels })
}

fn builtin_doc(b: &Builtin) -> Node {
    let mut doc = markdown::markdown_to_pm(&fill(b.markdown)).doc;
    wikicraft_doc_model::node::expand_placeholders(&mut doc);
    doc
}

fn fill(s: &str) -> String {
    s.replace("{date}", &chrono::Utc::now().format("%Y-%m-%d").to_string())
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct TemplateItem {
    id: String,
    name: String,
    description: Option<String>,
    icon: Option<String>,
    builtin: bool,
    space_key: Option<String>,
    /// `{{variables}}` the template asks for when a page is created from it
    variables: Vec<String>,
    /// blueprints: the index page new pages are filed under
    filed_under: Option<&'static str>,
    /// the caller may rename or delete it (its creator, workspace admins, and space admins for
    /// a space's templates)
    can_manage: bool,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ListQuery {
    space_key: Option<String>,
}

pub async fn list(State(state): State<AppState>, CurrentUser(user): CurrentUser, Query(q): Query<ListQuery>) -> ApiResult<Json<Vec<TemplateItem>>> {
    // a space's own templates only for people who can see the space
    let p = Principal::load(&state, Some(user.clone())).await?;
    let (space_key, space_role) = match &q.space_key {
        Some(k) => match load_space(&state, &p, k).await {
            Ok((_, role)) => (k.clone(), role),
            Err(_) => (String::new(), Role::None),
        },
        _ => (String::new(), Role::None),
    };
    let mut out: Vec<TemplateItem> = BUILTINS
        .iter()
        .map(|b| TemplateItem {
            id: b.id.into(),
            name: b.name.into(),
            description: Some(b.description.into()),
            icon: Some(b.icon.into()),
            builtin: true,
            space_key: None,
            variables: template::variables(Some(b.title), &builtin_doc(b)),
            filed_under: blueprint(b.id).map(|bp| bp.index_title),
            can_manage: false,
        })
        .collect();
    type Row = (Uuid, String, Option<String>, Option<String>, Option<String>, Option<String>, Value, Option<Uuid>);
    let rows: Vec<Row> = sqlx::query_as(
        "SELECT t.id, t.name, t.description, t.icon, s.key, t.title, t.content, t.created_by FROM templates t LEFT JOIN spaces s ON s.id = t.space_id
         WHERE t.workspace_id = $1 AND (t.space_id IS NULL OR lower(s.key) = lower($2)) ORDER BY lower(t.name)",
    )
    .bind(user.workspace_id)
    .bind(space_key)
    .fetch_all(&state.db)
    .await?;
    out.extend(rows.into_iter().map(|(id, name, description, icon, space_key, title, content, creator)| TemplateItem {
        id: id.to_string(),
        name,
        description,
        icon,
        builtin: false,
        can_manage: user.is_admin() || creator == Some(user.id) || (space_key.is_some() && space_role >= Role::Admin),
        space_key,
        variables: Node::from_json(&content).map(|doc| template::variables(title.as_deref(), &doc)).unwrap_or_default(),
        filed_under: None,
    }));
    Ok(Json(out))
}

/// Resolves a template id to (title, content).
/// [`resolve`] for a person: a space template only if they can see its space.
pub async fn resolve_for(state: &AppState, p: &Principal, workspace: Uuid, id: &str) -> ApiResult<(Option<String>, Node)> {
    if let Ok(tid) = id.parse::<Uuid>() {
        let space: Option<Option<Uuid>> = sqlx::query_scalar("SELECT space_id FROM templates WHERE id = $1 AND workspace_id = $2")
            .bind(tid)
            .bind(workspace)
            .fetch_optional(&state.db)
            .await?;
        if let Some(Some(space_id)) = space {
            let sa = crate::authz::load_space_access(state, space_id).await?;
            if crate::authz::space_role(state, p, &sa).await? == Role::None {
                return Err(ApiError::NotFound("template"));
            }
        }
    }
    resolve(state, workspace, id).await
}

/// `GET /templates/{id}/preview` — what a page from the template would look like.
pub async fn preview(State(state): State<AppState>, CurrentUser(user): CurrentUser, Path(id): Path<String>) -> ApiResult<Json<Value>> {
    let p = Principal::load(&state, Some(user.clone())).await?;
    let (title, mut doc) = resolve_for(&state, &p, user.workspace_id, &id).await?;
    let variables = template::variables(title.as_deref(), &doc);
    // variables show as the hints they become when nobody answers them
    template::fill(&mut doc, &Default::default());
    let html = wikicraft_doc_model::html::render(&doc, &wikicraft_doc_model::html::HtmlOptions::default());
    Ok(Json(json!({ "title": title, "html": html, "variables": variables })))
}

pub async fn resolve(state: &AppState, workspace: Uuid, id: &str) -> ApiResult<(Option<String>, Node)> {
    if let Some(b) = BUILTINS.iter().find(|b| b.id == id) {
        return Ok((Some(fill(b.title)), builtin_doc(b)));
    }
    let id: Uuid = id.parse().map_err(|_| ApiError::NotFound("template"))?;
    let row: Option<(Option<String>, Value)> =
        sqlx::query_as("SELECT title, content FROM templates WHERE id = $1 AND workspace_id = $2").bind(id).bind(workspace).fetch_optional(&state.db).await?;
    let (title, content) = row.ok_or(ApiError::NotFound("template"))?;
    Ok((title.map(|t| fill(&t)), Node::from_json(&content)?))
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CreateTemplate {
    name: String,
    description: Option<String>,
    icon: Option<String>,
    /// Template from an existing page's content.
    page_id: Uuid,
    /// Scope the template to a space (otherwise workspace-wide; requires workspace admin).
    space_key: Option<String>,
}

pub async fn create(State(state): State<AppState>, CurrentUser(user): CurrentUser, Json(req): Json<CreateTemplate>) -> ApiResult<(StatusCode, Json<Value>)> {
    let p = Principal::load(&state, Some(user.clone())).await?;
    require_page(&state, &p, req.page_id, Need::View).await?;
    let space_id = match &req.space_key {
        Some(k) => {
            let (s, role) = load_space(&state, &p, k).await?;
            if role < Role::Editor {
                return Err(ApiError::Forbidden);
            }
            Some(s.id)
        }
        None => {
            if !user.is_admin() {
                return Err(ApiError::bad("only workspace admins can create workspace-wide templates; pass spaceKey"));
            }
            None
        }
    };
    let name = req.name.trim();
    if name.is_empty() {
        return Err(ApiError::bad("name is required"));
    }
    let content: Value = match state.rooms.get(req.page_id) {
        Some(room) => room.snapshot().to_json(),
        None => sqlx::query_scalar("SELECT content FROM pages WHERE id = $1").bind(req.page_id).fetch_one(&state.db).await?,
    };
    let id = new_id();
    sqlx::query("INSERT INTO templates (id, workspace_id, space_id, name, description, icon, content, created_by) VALUES ($1, $2, $3, $4, $5, $6, $7, $8)")
        .bind(id)
        .bind(user.workspace_id)
        .bind(space_id)
        .bind(name)
        .bind(req.description)
        .bind(req.icon)
        .bind(content)
        .bind(user.id)
        .execute(&state.db)
        .await?;
    Ok((StatusCode::CREATED, Json(json!({ "id": id }))))
}

/// Who may rename or delete a template: its creator, workspace admins, and the admins of the
/// space it belongs to.
async fn require_manage(state: &AppState, user: &crate::auth::User, id: Uuid) -> ApiResult<()> {
    let row: Option<(Option<Uuid>, Option<Uuid>)> = sqlx::query_as("SELECT created_by, space_id FROM templates WHERE id = $1 AND workspace_id = $2")
        .bind(id)
        .bind(user.workspace_id)
        .fetch_optional(&state.db)
        .await?;
    let (creator, space) = row.ok_or(ApiError::NotFound("template"))?;
    if creator == Some(user.id) || user.is_admin() {
        return Ok(());
    }
    if let Some(space_id) = space {
        let p = Principal::load(state, Some(user.clone())).await?;
        let sa = crate::authz::load_space_access(state, space_id).await?;
        if crate::authz::space_role(state, &p, &sa).await? >= Role::Admin {
            return Ok(());
        }
    }
    Err(ApiError::Forbidden)
}

pub async fn delete(State(state): State<AppState>, CurrentUser(user): CurrentUser, Path(id): Path<Uuid>) -> ApiResult<StatusCode> {
    require_manage(&state, &user, id).await?;
    sqlx::query("DELETE FROM templates WHERE id = $1").bind(id).execute(&state.db).await?;
    // spaces and pages that started new pages from it fall back to a blank page
    sqlx::query("UPDATE spaces SET default_template = NULL WHERE default_template = $1").bind(id.to_string()).execute(&state.db).await?;
    sqlx::query("UPDATE pages SET child_template = NULL WHERE child_template = $1").bind(id.to_string()).execute(&state.db).await?;
    Ok(StatusCode::NO_CONTENT)
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct UpdateTemplate {
    name: Option<String>,
    /// "" removes it
    description: Option<String>,
    /// replaces the content with the page's current content
    page_id: Option<Uuid>,
}

/// `PATCH /templates/{id}` `{name?, description?, pageId?}` — rename, describe, or take the
/// content of a page again (same people as delete).
pub async fn update(
    State(state): State<AppState>,
    CurrentUser(user): CurrentUser,
    Path(id): Path<Uuid>,
    Json(req): Json<UpdateTemplate>,
) -> ApiResult<StatusCode> {
    require_manage(&state, &user, id).await?;
    if let Some(name) = &req.name {
        let name = name.split_whitespace().collect::<Vec<_>>().join(" ");
        if name.is_empty() || name.chars().count() > 120 {
            return Err(ApiError::bad("a template needs a name of up to 120 characters"));
        }
        sqlx::query("UPDATE templates SET name = $2 WHERE id = $1").bind(id).bind(name).execute(&state.db).await?;
    }
    if let Some(d) = &req.description {
        let d = d.trim();
        if d.chars().count() > 500 {
            return Err(ApiError::bad("descriptions are limited to 500 characters"));
        }
        sqlx::query("UPDATE templates SET description = $2 WHERE id = $1").bind(id).bind(Some(d).filter(|d| !d.is_empty())).execute(&state.db).await?;
    }
    if let Some(page) = req.page_id {
        let p = Principal::load(&state, Some(user.clone())).await?;
        require_page(&state, &p, page, Need::View).await?;
        let content: Value = match state.rooms.get(page) {
            Some(room) => room.snapshot().to_json(),
            None => sqlx::query_scalar("SELECT content FROM pages WHERE id = $1").bind(page).fetch_one(&state.db).await?,
        };
        sqlx::query("UPDATE templates SET content = $2 WHERE id = $1").bind(id).bind(content).execute(&state.db).await?;
    }
    Ok(StatusCode::NO_CONTENT)
}
