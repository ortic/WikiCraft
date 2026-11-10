//! Space templates: a new space prefilled with a page structure built from the page templates.

use axum::Json;
use serde_json::{Value, json};
use uuid::Uuid;
use wikicraft_doc_model::{Node, markdown, nodes};

use crate::app::{AppState, new_id};
use crate::error::ApiResult;
use crate::routes::pages::{NewPage, insert_page};

struct BpPage {
    title: &'static str,
    /// a page template id; otherwise `markdown` (folders get a child-pages list)
    template: Option<&'static str>,
    markdown: &'static str,
    children: &'static [BpPage],
}

struct Blueprint {
    id: &'static str,
    name: &'static str,
    icon: &'static str,
    description: &'static str,
    home: &'static str,
    pages: &'static [BpPage],
}

const fn page(title: &'static str, template: &'static str) -> BpPage {
    BpPage { title, template: Some(template), markdown: "", children: &[] }
}

const fn folder(title: &'static str, markdown: &'static str, children: &'static [BpPage]) -> BpPage {
    BpPage { title, template: None, markdown, children }
}

const BLUEPRINTS: &[Blueprint] = &[
    Blueprint {
        id: "team",
        name: "Team space",
        icon: "people",
        description: "Who the team is, how it works, meeting notes, decisions and onboarding.",
        home: "## About us\n\nWhat this team does and who to ask.\n\n| Role | Person |\n| --- | --- |\n| Team lead |  |\n| On call |  |\n\n## How we work\n\n- Planning every other Monday\n- Weekly sync on Thursdays\n- Questions go to the team channel\n",
        pages: &[
            folder("Meeting notes", "Notes from our meetings, newest first.", &[page("Weekly sync", "builtin:meeting-notes")]),
            folder("Decisions", "Decisions we made and why.", &[page("Decision: first decision", "builtin:decision")]),
            page("Onboarding", "builtin:onboarding"),
            folder("How-to guides", "Step-by-step guides for recurring tasks.", &[page("How to request access", "builtin:how-to")]),
        ],
    },
    Blueprint {
        id: "project",
        name: "Project",
        icon: "rocket",
        description: "Plan, requirements, decisions, status reports and a retrospective for one project.",
        home: "| Owner | Status | Target |\n| --- | --- | --- |\n|  | Planning |  |\n\n## Goal\n\nWhat this project delivers and why it matters.\n\n## Links\n\n- Tracker:\n- Design:\n",
        pages: &[
            page("Project plan", "builtin:project-plan"),
            page("Requirements", "builtin:requirements"),
            folder("Decisions", "Decisions made in this project.", &[page("Decision: scope of the first release", "builtin:decision")]),
            folder("Status reports", "Regular updates for stakeholders.", &[page("Status report", "builtin:status-report")]),
            page("Retrospective", "builtin:retrospective"),
        ],
    },
    Blueprint {
        id: "knowledge",
        name: "Knowledge base",
        icon: "book",
        description: "How-to articles, troubleshooting and answers to common questions.",
        home: "Find answers here before asking. Search with **Ctrl K**, or browse the sections below.\n\n> Missing something? Create a page from the *How-to* template and add it.\n",
        pages: &[
            folder("How-to articles", "Step-by-step instructions.", &[page("How to set up your laptop", "builtin:how-to")]),
            folder(
                "Troubleshooting",
                "Known problems and their fixes.",
                &[BpPage {
                    title: "Cannot connect to the VPN",
                    template: None,
                    markdown: "## Symptom\n\nThe VPN client says *handshake timed out*.\n\n## Cause\n\nThe profile expired.\n\n## Fix\n\n1. Remove the old profile\n2. Download a new one from the IT portal\n3. Reconnect\n",
                    children: &[],
                }],
            ),
            BpPage {
                title: "FAQ",
                template: None,
                markdown: "## How do I get access to a space?\n\nOpen the page and choose **Request access**, or ask a space admin.\n\n## Where do I report a problem?\n\nIn the support channel.\n",
                children: &[],
            },
        ],
    },
    Blueprint {
        id: "engineering",
        name: "Engineering docs",
        icon: "code",
        description: "Architecture, decision records, runbooks and postmortems for a service or team.",
        home: "## Services\n\n| Service | Owner | Runbook |\n| --- | --- | --- |\n|  |  |  |\n\n## On call\n\nRotation and escalation: see the runbooks below.\n",
        pages: &[
            page("Architecture overview", "builtin:architecture"),
            folder("Decision records", "Architecture decisions (ADRs).", &[page("ADR 1: record architecture decisions", "builtin:decision")]),
            folder("Runbooks", "What to do when something breaks.", &[page("Runbook: service restart", "builtin:runbook")]),
            folder("Postmortems", "What we learned from incidents.", &[page("Postmortem template", "builtin:postmortem")]),
        ],
    },
];

/// `GET /api/space-templates`
pub async fn list() -> Json<Vec<Value>> {
    Json(
        BLUEPRINTS
            .iter()
            .map(|b| {
                let pages: Vec<&str> = b.pages.iter().map(|p| p.title).collect();
                json!({ "id": b.id, "name": b.name, "icon": b.icon, "description": b.description, "pages": pages })
            })
            .collect(),
    )
}

pub fn exists(id: &str) -> bool {
    BLUEPRINTS.iter().any(|b| b.id == id)
}

/// The space icon (`symbol:colour`) a template suggests.
pub fn icon(id: &str) -> Option<String> {
    let colours = ["teal", "violet", "amber", "blue"];
    BLUEPRINTS.iter().position(|b| b.id == id).map(|i| format!("{}:{}", BLUEPRINTS[i].icon, colours[i % colours.len()]))
}

fn with_children(mut doc: Node) -> Node {
    doc.content.push(Node::new(nodes::CHILD_PAGES));
    doc
}

/// Fills a fresh space: the home page's content and the page tree under it.
pub async fn apply(state: &AppState, id: &str, ws: Uuid, space: Uuid, home: Uuid, actor: Uuid) -> ApiResult<usize> {
    let Some(bp) = BLUEPRINTS.iter().find(|b| b.id == id) else { return Ok(0) };
    let mut home_doc = with_children(markdown::markdown_to_pm(bp.home).doc);
    wikicraft_doc_model::blocks::ensure_ids(&mut home_doc);
    sqlx::query("UPDATE pages SET content = $2, text_content = $3 WHERE id = $1")
        .bind(home)
        .bind(home_doc.to_json())
        .bind(wikicraft_doc_model::text::plain_text(&home_doc))
        .execute(&state.db)
        .await?;
    sqlx::query("UPDATE page_versions SET content = $2 WHERE page_id = $1 AND version = 1").bind(home).bind(home_doc.to_json()).execute(&state.db).await?;
    let mut created = 0;
    // depth-first with an explicit stack: (parent, pages)
    let mut stack: Vec<(Uuid, &'static [BpPage])> = vec![(home, bp.pages)];
    while let Some((parent, pages)) = stack.pop() {
        for (i, p) in pages.iter().enumerate() {
            let content = match p.template {
                Some(t) => crate::routes::templates::resolve(state, ws, t).await?.1,
                None if p.children.is_empty() => markdown::markdown_to_pm(p.markdown).doc,
                None => with_children(markdown::markdown_to_pm(p.markdown).doc),
            };
            let pid = insert_page(
                &state.db,
                NewPage {
                    id: new_id(),
                    workspace_id: ws,
                    space_id: space,
                    parent_id: Some(parent),
                    kind: "page".into(),
                    title: p.title.to_string(),
                    content,
                    position: (i + 1) as f64,
                    created_by: Some(actor),
                    created_at: None,
                    source_ref: None,
                },
            )
            .await?;
            state.search.enqueue(pid);
            created += 1;
            if !p.children.is_empty() {
                stack.push((pid, p.children));
            }
        }
    }
    state.search.enqueue(home);
    Ok(created)
}
