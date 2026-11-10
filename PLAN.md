# WikiCraft — Product & Implementation Plan

> A self-hostable, fast, open knowledge base / team wiki with a **Rust backend**: aiming for Confluence-level
> functionality, first-class Confluence migration, and a modern real-time editor.
>
> Status: in development (see WORKLOG.md) · Last updated: 2026-11-10

---

## 0. How to read this plan

**Checkboxes:** `[ ]` = not started, `[~]` = in progress, `[x]` = done.

**Priorities:**

| Tag | Meaning |
|-----|---------|
| **P0** | MVP: a team can use it as its daily wiki |
| **P1** | v1.0, a credible Confluence replacement (parity for the ~80% of features people actually use) |
| **P2** | Enterprise / scale / compliance |
| **P3** | Differentiators & later |

**Who has it** (after each item): `C` Confluence Cloud · `DC` Confluence Data Center · `D` Docmost · `X` XWiki ·
`O` Outline · `B` BookStack · `N` Notion. No tag means a new idea or table stakes.

---

## 1. Why now / positioning

- **Confluence Data Center goes end-of-life on 2029-03-28.** New customers have been unable to buy DC since 2026-03-30,
  and existing customers can buy new DC subscriptions only until 2028-03-30. Thousands of self-hosting orgs (regulated,
  air-gapped, public sector, EU data sovereignty) must pick a new home: Cloud, or an alternative.
- Open-source alternatives are either **modern but shallow** (Docmost, Outline: great editor, thin on
  macros, structured content and enterprise admin) or **powerful but dated** (XWiki: very deep, Java, complex
  admin/UX). Nothing combines a modern editor, Confluence-grade depth and a lean, cheap-to-run backend.
- **WikiCraft's angle:**
  1. **Lossless-as-possible Confluence migration**: XML, the new Cloud CSV export, HTML, *with history, comments,
     permissions, attachments*, plus a dry-run report. This is the #1 adoption blocker.
  2. **Single Rust binary + Postgres + S3.** Low memory, fast, easy to operate, air-gap friendly.
  3. **Modern real-time block editor** (Yjs-compatible CRDT) with a Confluence macro parity layer.
  4. **AI-native but self-hostable**: bring your own LLM, built-in MCP server, permission-aware RAG.
  5. **Open core** with a clear line: everything a team needs is open; SSO-at-scale/compliance is enterprise.

---

## 2. Competitive landscape

| Product | License | Stack | Strengths | Weaknesses / gaps |
|---|---|---|---|---|
| **Confluence Cloud** | Proprietary SaaS | Java/Kotlin, ADF editor | Deepest feature set: macros, Jira integration, whiteboards, databases, live docs, Rovo AI, marketplace, analytics, Guard admin | Expensive, slow, cloud-only from 2029, vendor lock-in, data residency concerns |
| **Confluence DC** | Proprietary, EOL 2029-03-28 | Java, Synchrony collab | Self-hosted, clustering, mature permissions | End-of-life; no new customers since 2026-03-30 |
| **Docmost** | AGPL-3.0 core + EE | TypeScript/NestJS, PostgreSQL, Redis, Tiptap/ProseMirror + Yjs | Modern UX, real-time, spaces, inline comments, history, Draw.io/Excalidraw/Mermaid, Confluence/Notion import, Bases/Kanban, AI chat & search, MCP, page verification (EE), audit logs (EE), SCIM (EE) | Few macros, limited structured content, thinner admin; SSO/permissions/importer are paid (Business) |
| **XWiki** | LGPL-2.1 | Java, Hibernate, Solr, CKEditor, realtime extension | Extremely deep: structured data (classes/objects), AppWithinMinutes, scripting (Velocity/Groovy/Python), 900+ extensions, subwikis (farm), fine-grained rights, page translations, Confluence migrator, office import, LiveData | Heavy (JVM), complex admin, dated UX, scripting = security risk, steep learning curve |
| **Outline** | BSL 1.1 | Node/TS, PostgreSQL, Redis, ProseMirror + Yjs | Beautiful & fast, collections, Slack integration, API, public sharing, AI answers | Flat-ish hierarchy, limited macros/permissions, BSL license |
| **BookStack** | MIT | PHP/Laravel, MySQL | Simple Shelves → Books → Chapters → Pages, roles, LDAP/SAML/OIDC, audit, webhooks, API | No real-time collab, rigid hierarchy, basic editor |
| **Wiki.js** | AGPL-3.0 | Node, PostgreSQL | Markdown, Git sync, many auth modules | v3 slow to arrive, limited collab |
| **Notion** | Proprietary SaaS | — | Blocks, databases, AI, templates | Not self-hostable, weak permissions/compliance |
| **AppFlowy / AFFiNE** | OSS | **Rust** (AppFlowy Cloud: actix + `yrs`; AFFiNE: `y-octo`) + Flutter/TS | Proof that Rust + Yjs-style CRDT backends work at scale | Notion-like, not wiki/enterprise oriented |

**Takeaways:**
- Copy **Docmost's UX simplicity** and its editor/collab stack choices (ProseMirror/Tiptap + Yjs) so we can reuse that ecosystem.
- Copy **XWiki's depth**: structured data, page translations, rights model, extension system, Confluence-migration rigor.
  Its Confluence importer test suite is the best edge-case reference (see §6).
- Match **Confluence's** information architecture (spaces → page tree, folders, live docs, whiteboards, databases)
  and macro vocabulary, because migrating users expect them.

### 2.1 Key feature matrix

| Capability | C | DC | D | X | O | B | WikiCraft target |
|---|:-:|:-:|:-:|:-:|:-:|:-:|:-:|
| Spaces + nested page tree | Yes | Yes | Yes | Yes (nested pages, subwikis) | Yes (collections) | Yes (books) | P0 |
| Real-time co-editing | Yes | Yes | Yes | Yes | Yes | No | P0 |
| Inline + page comments | Yes | Yes | Yes | Yes (annotations) | Yes | Yes (page only) | P0 |
| Version history + diff + restore | Yes | Yes | Yes | Yes | Yes | Yes | P0 |
| Page-level restrictions | Yes | Yes | Yes (Business) | Yes | Partial | Yes | P0/P1 |
| Templates / blueprints | Yes | Yes | Yes | Yes | Yes | Yes | P1 |
| Macro ecosystem | Yes+ | Yes+ | Partial | Yes+ | Partial | No | P1 (parity layer) |
| Structured data / page properties | Yes (page properties, databases) | Yes (page properties) | Yes (Bases) | Yes+ (classes, AWM) | No | No | P1/P2 |
| Whiteboards | Yes | No | Yes (Excalidraw) | Partial (draw.io) | No | No | P2 |
| Databases | Yes | No | Yes | Yes (AWM/LiveData) | No | No | P2 |
| Full-text search incl. attachments | Yes | Yes | Yes (Business) | Yes (Solr) | Yes | Yes | P0/P1 |
| AI Q&A / semantic search | Yes (Rovo) | Partial | Yes | Partial (LLM extension) | Yes | No | P2 |
| SSO (OIDC/SAML/LDAP) | Yes (Guard) | Yes | Yes (Business) | Yes | Yes | Yes | P1 |
| SCIM | Yes | Partial | Yes (EE) | Partial | Partial | No | P2 |
| Audit log | Yes | Yes | Yes (EE) | Yes | Yes | Yes | P1 |
| Public / anonymous access | Yes | Yes | Yes | Yes | Yes | Yes | P1 |
| Confluence import | — | — | Yes (Business) | Yes+ | Yes | Partial | **P0/P1 (flagship)** |
| PDF/Word export | Yes | Yes | Partial | Yes | Partial | Yes | P1 |
| REST API + webhooks | Yes | Yes | Yes | Yes | Yes | Yes | P0/P1 |
| Plugin / app framework | Yes (Forge) | Yes (P2) | No | Yes+ | Partial | Partial (themes) | P2 (WASM) |
| MCP server | Yes (Rovo MCP) | No | Yes | Partial | Yes | No | P1 |
| Multilingual page translations | Partial (apps) | Partial (apps) | No | Yes | No | No | P3 |
| Self-hosted / air-gapped | No | Yes (until 2029) | Yes | Yes | Yes | Yes | P0 |

---

## 3. Architecture & Rust stack

### 3.1 High-level shape
- [ ] **P0** Modular monolith: one `wikicraft` binary (HTTP API, WebSocket collab, background workers, CLI) with
      role flags (`--role api,collab,worker`) so it can scale out later without a microservice rewrite — partial: one binary runs the HTTP API, WebSocket collab, background workers (jobs, mail, webhooks) and CLI subcommands (`migrate`, `import-confluence`, `reindex`, …) / no `--role` flags: every replica runs every role
- [x] **P0** PostgreSQL as the system of record (≥ 15), object storage (S3-compatible, or local FS) for blobs
- [x] **P0** Optional Redis/Valkey, only for multi-node pub/sub and rate limits. Single-node mode needs Postgres only
      (use `LISTEN/NOTIFY` and a Postgres job queue) — no Redis at all: multi-node uses Postgres LISTEN/NOTIFY (`WIKICRAFT_CLUSTER=postgres`), jobs a `SKIP LOCKED` queue, login throttling a Postgres table
- [x] **P0** Frontend: TypeScript SPA (React + Tiptap/ProseMirror + Yjs). The Rust ecosystem has no
      ProseMirror-class editor, so we don't fight it. Served by the binary (embedded static assets) — assets are served from `WIKICRAFT_STATIC_DIR` (`web/dist`), not embedded in the binary
- [ ] **P2** Horizontal scaling: stateless API nodes; collab sessions pinned by document id (consistent hashing),
      cross-node awareness via Redis/NATS
- [ ] **P3** Desktop/offline client sharing the Yjs document model

### 3.2 Crate layout (Cargo workspace)
- [ ] `crates/server`: axum app, routing, middleware, OpenAPI (`utoipa`), static asset serving
- [ ] `crates/domain`: entities, invariants, use cases (no I/O)
- [ ] `crates/db`: `sqlx` repositories, migrations, multi-tenant scoping
- [ ] `crates/authz`: permission engine (see §4.12), effective-permission cache
- [ ] `crates/collab`: `yrs` sync/awareness server, persistence, snapshotting
- [ ] `crates/doc-model`: canonical schema; ProseMirror JSON ⇄ Yjs ⇄ HTML ⇄ Markdown ⇄ plain text; schema migrations
- [ ] `crates/search`: indexing pipeline, `tantivy` backend + adapters (Meilisearch/Typesense/OpenSearch)
- [ ] `crates/storage`: `object_store` (S3/GCS/Azure/local), content-addressed blobs, thumbnails
- [ ] `crates/jobs`: Postgres `SKIP LOCKED` queue (or `apalis`), cron scheduler, retries, dead-letter
- [ ] `crates/importers`: confluence (xml/csv/html), notion, markdown/obsidian, html, docx, mediawiki, xwiki, docmost, outline, bookstack
- [ ] `crates/exporters`: pdf, docx, html, markdown, native backup format
- [ ] `crates/ai`: embeddings, RAG, LLM provider abstraction, MCP server (`rmcp`)
- [ ] `crates/plugins`: WASM host (`wasmtime` / `extism`), plugin manifest, capability-based sandbox
- [ ] `crates/cli`: `wikicraft migrate | import | export | reindex | backup | restore | admin`
- [ ] `web/`: frontend (pnpm workspace), shared TS types generated from the OpenAPI spec

### 3.3 Library choices (to validate with spikes)

| Concern | Choice | Alternatives / notes |
|---|---|---|
| HTTP | `axum` + `tokio` + `tower` / `tower-http` | `actix-web` (AppFlowy uses it) |
| DB | `sqlx` (compile-time checked SQL) | `sea-orm`; `refinery` for migrations |
| Real-time CRDT | **`yrs`** (Yjs port, wire-compatible with `y-prosemirror` / Tiptap) | `loro` (has a movable-tree CRDT, attractive for the page tree, but no Tiptap-ecosystem compatibility) |
| Search | `tantivy` embedded (single node) | Meilisearch / Typesense / OpenSearch for clusters; Postgres FTS as fallback |
| Vectors | `pgvector` | Qdrant adapter |
| Storage | `object_store` | — |
| XML / HTML | `quick-xml` (streaming, Confluence storage format), `html5ever` / `scraper`, `lol_html` (rewriting) | — |
| Markdown | `comrak` (GFM, round-trip) | `pulldown-cmark` |
| HTML sanitization | `ammonia` | — |
| PDF export | **Typst** (Rust-native, fast, offline) via `typst` crate | headless Chromium (`chromiumoxide`) or a Gotenberg sidecar for pixel-exact HTML |
| DOCX | `docx-rs` (write), `quick-xml` + zip (read) | LibreOffice sidecar for legacy `.doc`/`.ppt` |
| Images | `image`, `fast_image_resize`, `resvg` (SVG raster), EXIF stripping | libvips bindings for very large images |
| Text extraction (attachments) | `pdf-extract` / `lopdf`, DOCX/XLSX/PPTX via zip+XML, `calamine` (spreadsheets) | Apache Tika sidecar (optional) |
| Auth | `openidconnect`, `samael` (SAML), `ldap3`, `argon2`, `webauthn-rs` (passkeys), `totp-rs` | — |
| Authorization | Own ACL engine | `cedar-policy` (Rust-native policies) for advanced rules |
| Email | `lettre` + templates (`minijinja`) | — |
| Jobs / cron | Postgres queue + `tokio-cron-scheduler` | `apalis` |
| Observability | `tracing`, `tracing-opentelemetry`, `metrics` + Prometheus exporter | — |
| Config | `figment` (file + env) | — |
| Rate limiting | `tower_governor` | — |
| AI | OpenAI-compatible HTTP client (`async-openai`), Ollama | `rmcp` for MCP |
| Plugins | `wasmtime` component model / `extism` | — |
| Testing | `cargo-nextest`, `insta` (snapshots), `proptest`, `testcontainers`, Playwright (e2e), `k6`/`goose` (load) | — |

### 3.4 Core data model (first cut)
- [ ] **P0** `workspace` (tenant) → `space` → `node` (page | folder | blog_post | whiteboard | database | live_doc | smart_link) — partial: workspaces → spaces → pages with kind page | folder | blog | link / no whiteboard, database or live_doc node kinds (drawings are blocks inside pages)
- [x] **P0** Page tree: `parent_id` + fractional-index `position` (stable concurrent reordering); closure table or `ltree` for fast subtree queries — float `position` with midpoint inserts (siblings renumbered when the gap runs out); subtree queries use recursive CTEs, no closure table / `ltree`
- [ ] **P0** `page_version` (immutable snapshots: ProseMirror JSON + author + message + timestamp), `page_draft`, `yjs_update` log + periodic `yjs_snapshot` — partial: immutable `page_versions` (PM JSON, author, message, timestamp); live Yjs state stored merged in `pages.ydoc` / no `page_draft`, no `yjs_update` log or separate `yjs_snapshot` table
- [x] **P0** `attachment` (+ `attachment_version`) → content-addressed `blob` (sha256, dedup)
- [x] **P0** `comment` (page / inline with anchor + thread + resolved state), `reaction` — reactions are likes (`page_likes`, `comment_likes`); the emoji `reactions` table is unused
- [x] **P0** `user`, `group`, `group_member`, `identity` (external IdP links), `session`, `api_token`
- [ ] **P0** `acl_entry` (subject: user/group/anonymous/guest/role × resource: workspace/space/node × permission × allow/deny × inherit) — partial: role-based grants instead of generic ACL rows: workspace roles, `space_permissions` (user/group → role), space default role and anonymous access, page view (inherited) / edit restrictions / no deny entries, no per-permission matrix
- [ ] **P1** `label`, `node_label`, `page_property` (typed key/values), `link` (outgoing link graph for backlinks and broken-link reports)
- [ ] **P1** `watch`, `notification`, `notification_preference`, `mention`
- [ ] **P1** `template`, `blueprint`
- [ ] **P1** `audit_event` (append-only, hash-chained for tamper evidence in P2)
- [ ] **P1** `job`, `import_run`, `import_mapping` (source id → new id, for re-runs and link fixups)
- [ ] **P2** `database`, `database_field`, `database_row` (JSONB values + typed indexes), `database_view`
- [ ] **P2** `embedding` (chunk, vector, node, version, acl fingerprint)
- [ ] **P0** Every row carries `workspace_id`; optional Postgres Row-Level Security as defense-in-depth (P2) — partial: top-level tables (users, groups, spaces, pages, attachments, jobs, audit_events, import_mappings, webhooks, templates) carry `workspace_id` / child tables (page_versions, comments, attachment_versions, labels, sessions, notifications, …) only via their parent; no RLS
- [ ] **P0** IDs: UUIDv7 (sortable); human-friendly short slugs for URLs; permanent URLs survive renames and moves — partial: UUIDv7 ids (`app::new_id`), permanent `/pages/<id>` URLs survive renames and moves / no human-friendly slugs

### 3.5 Document model
- [x] **P0** Canonical format = versioned ProseMirror JSON schema (like Confluence's ADF), with a `schema_version` and forward migrations — `pages.schema_version` (current: 2 = block ids), `doc_model::schema::{upgrade, upgrade_ydoc}` (JSON and Yjs, as CRDT operations); old pages are upgraded by a background backfill (`schema_upgrade.rs`, batches under the row lock, not counted as edits) and when opened. Version snapshots keep the format they were written in
- [x] **P0** Live state in Yjs (`yrs`); a published version is a frozen ProseMirror JSON snapshot — live mode only: versions are snapshots taken when the last editor leaves, every 10 min, or explicitly; no separate publish step
- [ ] **P0** Derived renditions cached per version: sanitized HTML, plain text (search), Markdown (export / AI) — partial: HTML, Markdown and plain-text renderers exist (`doc-model`); plain text is stored for the current page / HTML and Markdown are rendered on demand, nothing is cached per version
- [x] **P0** Stable block IDs on every block (needed for deep links, inline comments, synced blocks, AI citations) — `blockId` attribute on 24 block types (list shared by `doc-model/assets/block_types.json` and the editor's UniqueID config); the server assigns ids on create / import / restore and to older pages when they are first opened (Yjs attribute changes, not counted as an edit); content replaced through the API keeps the ids of unchanged blocks; "copy link to block" (drag handle, hover button for readers), `#block-<id>` deep links with highlight; `id="block-…"` anchors in shared pages and HTML exports. Not yet used by inline comments or AI citations
- [ ] **P1** "Extension node" envelope for macros: `{type: "macro", name, params, body?, fallback_html}`, so unknown or imported
      macros survive round-trips (lesson from the Confluence `ac:adf-extension` + `ac:adf-fallback` design)
- [ ] **P1** Server-side rendering of documents to HTML in Rust (for public pages, exports, email, SEO), with no JS required

---

## 4. Feature checklist

Status as of 2026-11-09: `[x]` done · `[~]` partial · `[ ]` not started.

### 4.1 Workspace, tenancy & organization
- [~] **P0** Single-workspace self-hosted install with first-run setup wizard (admin account, base URL, email, storage) `D X B` (workspace + admin only; other settings via env)
- [ ] **P1** Multi-workspace (multi-tenant) mode for SaaS / large orgs, each with its own branding, users and settings `C`
- [ ] **P2** XWiki-style "subwikis" / wiki farm: isolated wikis sharing users `X`
- [ ] **P1** Custom domain per workspace; base-path deployments (`/wiki`) `C X`
- [ ] **P1** Workspace-level settings: name, logo, favicon, colors, default language, timezone, date format `C D X` — partial: name and logo (Admin → Overview; header, sign-in and shared pages)
- [ ] **P2** Sandbox / staging workspace cloned from production `C`
- [ ] **P2** Release tracks (choose when new features roll out) `C`

### 4.2 Spaces
- [~] **P0** Create / edit / delete spaces (name, key, description, icon/emoji, cover) `C D X O B` (no cover image)
- [x] **P0** Space home / overview page `C D X`
- [x] **P0** Personal spaces per user `C D`
- [~] **P1** Space directory with search, categories, filters (team, personal, archived, starred) `C X` (name/key filter, categories, starred filter and starred first; archived spaces in Admin → Spaces; no personal-space filter)
- [x] **P1** Space categories / tags `C` — one category per space (Space settings → Details); the space directory groups and filters by it
- [x] **P1** Space archive (read-only, hidden from search by default) and restore `C`
- [~] **P1** Space sidebar: page tree, shortcuts/links, blog, whiteboards, databases, custom sections `C` (page tree + blog only)
- [ ] **P1** Space-level look & feel: theme, colors, header image, sidebar config `C X`
- [x] **P1** Space templates (create a space prefilled with a page structure) `C X`
- [x] **P1** Space permissions screen (users × groups × anonymous × guests matrix) `C DC X B`
- [~] **P1** Space admins / owners, ownership transfer `C` (admin role only; no owner/transfer)
- [~] **P1** Copy space; move pages between spaces `C X` (cross-space move via API only; no copy)
- [x] **P1** Space key rename with redirects `X`
- [ ] **P2** Space-level announcement banner `C`
- [ ] **P2** Space analytics (views, contributors, popular pages) `C`
- [x] **P2** Space export / import (native format) `C DC X`
- [ ] **P2** Watch space / follow space activity `C X`

### 4.3 Content types & information architecture
- [x] **P0** Pages with unlimited nesting (page tree) `C D X O B`
- [x] **P0** Drag & drop reorder / move in the tree (with permission checks and link preservation) `C D X O`
- [x] **P1** Folders (containers without content) `C` — new folder from the tree header, a folder or the command palette; a folder shows its contents (pages, folders, excerpts) with New page / New folder instead of an editor; Confluence Cloud folders import as folders
- [~] **P1** Blog posts / news per space (date-based, separate from tree, RSS/Atom) `C X` (no RSS/Atom feed)
- [~] **P1** Live docs (always-live document, no publish step) vs. classic pages (draft → publish) as a per-page mode `C` (live mode only; no draft/publish)
- [x] **P1** Smart link / embed nodes in the tree (link to an external resource as a tree item) `C` — Create → "Link to a website": a tree item that opens the URL in a new tab (its page shows a link preview)
- [ ] **P2** Whiteboards as first-class tree items (see §4.10) `C D`
- [ ] **P2** Databases as first-class tree items (see §4.9) `C D`
- [x] **P1** Page emoji / icon and cover image `C D O` (SVG symbol icons by design)
- [x] **P1** Page width setting (fixed / full width) `C D`
- [x] **P1** Breadcrumbs `C D X O B`
- [~] **P1** Permanent links (`/p/<id>`) + human URLs (`/s/<space>/<slug>`); redirects after rename/move `C X` (id permalinks + Confluence redirects; no slug URLs)
- [~] **P1** Copy page (with or without children, attachments, labels, permissions) `C X D` (copy dialog: title, with child pages; labels and restrictions copied; attachments not copied yet)
- [x] **P1** Page owner (distinct from creator) and ownership transfer `C`
- [x] **P1** Page status lozenge (Rough draft / In progress / Ready for review / Verified / custom) `C` — picker on the lozenge and in the page menu (Rough draft, In progress, Ready for review, custom text); "Verified" is the separate page verification
- [x] **P1** Archive page / subtree `C`
- [x] **P1** Trash with restore and purge (per space; retention period) `C D X` — Delete forever per page and Empty trash (space admins), workspace retention period (`trashRetentionDays`, 0 = forever) applied every 10 minutes; purged pages take history, comments and attachments along, unused files are removed from storage
- [x] **P1** Duplicate titles allowed (Cloud allows them; the importer must handle both) `C`
- [ ] **P2** Page tree lazy loading & virtualization (≥ 100k pages per space) `C X`
- [ ] **P2** Hide pages from tree / navigation `X`
- [ ] **P3** Knowledge-base / help-center mode: public portal, article feedback ("was this helpful?") `C (JSM KB)`

### 4.4 Editor
#### 4.4.1 Fundamentals
- [x] **P0** WYSIWYG block editor (Tiptap/ProseMirror) with slash `/` command menu `C D O N`
- [x] **P0** Markdown shortcuts (`#`, `-`, `1.`, `>`, ```` ``` ````, `**`, `[]`, `---`) `C D O`
- [x] **P0** Undo/redo (collaboration-aware, per user) `C D`
- [x] **P0** Copy/paste from Word, Google Docs, web pages, Excel/Sheets (to tables), Markdown text, images from clipboard `C D O`
- [x] **P0** Drag & drop blocks (block handles), drag & drop files/images `C D O N`
- [x] **P0** Keyboard shortcuts + shortcut help dialog `C D O`
- [~] **P1** Floating formatting toolbar + fixed toolbar option `C D` (bubble menu only)
- [x] **P1** Find & replace in page `C`
- [x] **P1** Word count / reading time `C`
- [x] **P1** Autosave + offline-tolerant editing (queue updates while disconnected) `C D`
- [~] **P1** Spellcheck (browser) + optional server-side grammar/AI assist `C` (browser spellcheck only)
- [ ] **P2** Raw source view (Markdown / storage) for power users `X`
- [ ] **P2** Wiki-syntax editor mode (XWiki/Confluence wiki markup) for power users `X`
- [ ] **P2** Accessibility: full keyboard operation, ARIA, screen-reader announcements in collab `C`

#### 4.4.2 Text & inline
- [x] **P0** Headings H1–H6, paragraph, bold, italic, underline, strikethrough, inline code, sub/superscript `C D X O`
- [x] **P0** Links (URL, page link with search picker, anchor link, attachment link, email) `C D X O`
- [x] **P0** @mentions of users (with notification) and pages `C D O` — `@` suggests people (mention + notification) and, from two letters on, pages (inserts a page link); `[[` links pages, headings and attachments
- [~] **P0** Emoji (picker, `:shortcode:` autocomplete, custom emoji P2) `C D O` (`:name` autocomplete for our SVG emoticon set; no emoji by design)
- [~] **P1** Text color & highlight color (limited, theme-aware palette) `C D` (highlight toggle only; no color picker)
- [x] **P1** Date / time inline node (localized rendering, timezone aware) `C`
- [x] **P1** Status lozenges (colored labels inline) `C`
- [x] **P1** Inline smart links (unfurl title/icon via oEmbed/OpenGraph) `C D` — link cards from OpenGraph / <title> (server-side fetch with SSRF guards, 1-day cache); no site icons yet
- [x] **P1** Hard line break, horizontal rule `C D`
- [~] **P1** Text alignment, indentation `C D` (alignment via shortcuts only; no indent)
- [x] **P1** Inline math (KaTeX) `D O`
- [ ] **P2** Glossary terms / definitions (hover to see the definition; "AI definitions") `C`
- [ ] **P2** Footnotes `O`
- [ ] **P3** Inline comments on images

#### 4.4.3 Blocks
- [x] **P0** Bullet, numbered, nested lists `C D X O B`
- [x] **P0** Task / checklist items (assignee via @mention, due date via date node) `C D O`
- [x] **P0** Code blocks with syntax highlighting, language picker, line numbers, wrap, copy button `C D X O B`
- [x] **P0** Blockquote `C D O`
- [x] **P0** Callouts / panels: info, note, tip/success, warning, error, custom (emoji + color) `C D X O` (custom icon from the SVG set, by design no emoji)
- [x] **P0** Tables (see §4.4.4) `C D X O B`
- [x] **P0** Images (see §4.4.5) `C D X O B`
- [x] **P1** Expand / collapse (toggle) blocks `C D O`
- [~] **P1** Layouts / columns (2–3 columns, sidebars) `C D` (two-column insert only)
- [~] **P1** Decisions block (decision list) `C` (imported only; no insert UI)
- [x] **P1** Block math (KaTeX / LaTeX) `D O X`
- [~] **P1** Mermaid, PlantUML (server-side render, sandboxed) `D X` (Mermaid client-side only; no PlantUML render)
- [ ] **P1** Draw.io diagrams (embedded editor, stored as attachment + SVG preview) `D X C(app)`
- [x] **P1** Excalidraw sketches `D`
- [~] **P1** File / attachment block with inline preview (PDF, Office, video, audio) `C D` (PDF/image preview only)
- [~] **P1** Embeds: YouTube, Vimeo, Loom, Figma, Miro, Google Docs/Sheets/Slides, Airtable, Typeform, GitHub gist, iframe allowlist `C D O` (no Airtable/Typeform/gist)
- [x] **P1** Anchors / table of contents block `C X`
- [x] **P1** Placeholder text (in templates) `C` — "Instructions" (/ slash command, `{?…}` in Markdown templates, Confluence `ac:placeholder`): shown to authors, replaced by typing, hidden from readers, exports and search
- [ ] **P2** Synced blocks / excerpts: author once, reuse in many pages; edits propagate `C D`
- [~] **P2** Transclusion (include another page or section) `C X` (plain-text preview; from import only)
- [ ] **P2** Video / audio upload with streaming (HTTP range) and poster frame `C`
- [ ] **P2** Charts from table data `C(macro)`
- [ ] **P3** Interactive / form blocks (polls, surveys, voting) `C(apps)`
- [ ] **P3** Slides / presentation mode for a page `C`

#### 4.4.4 Tables
- [x] **P0** Insert / delete rows & columns, header row / column, resize columns `C D X O`
- [~] **P1** Merge / split cells, cell background color, cell alignment `C D` (no cell alignment)
- [ ] **P1** Numbered rows, sort by column (view-time), sticky header `C` — partial: the table toolbar sorts rows by the current column (A–Z / Z–A, numbers and percentages numerically, header rows stay)
- [x] **P1** Table width modes (fixed / wide / full) `C` — toolbar toggle Normal / Wide / Full width; Confluence `data-layout` imports keep it
- [x] **P1** Paste from spreadsheets preserving cells `C D` — HTML tables from Excel / Google Sheets / LibreOffice, and plain tab-separated rows (first row becomes the header)
- [ ] **P2** Table → chart conversion `C`
- [ ] **P2** Large table performance (1,000+ rows) `C`

#### 4.4.5 Images & media
- [x] **P0** Upload, paste, drag & drop, URL images `C D X O B`
- [x] **P0** Resize, align (left/center/right/wrap), caption, alt text `C D`
- [~] **P1** Lightbox / zoom, image border `C D` (lightbox; no borders)
- [x] **P1** Image annotation (arrows, boxes, text) `C`
- [ ] **P1** Responsive thumbnails & WebP/AVIF renditions generated server-side `C`
- [ ] **P2** Gallery block `C X`
- [ ] **P2** Watermarks on images `C`

#### 4.4.6 Macro parity layer (Confluence vocabulary)
> Purpose: imported Confluence content keeps working, and users find familiar building blocks.
> Order comes from **observed real-world frequency** in the two analyzed exports (§6.2).

- [x] **P0** `info` / `note` / `tip` / `warning` / `panel` → callout
- [x] **P0** `code` / `noformat` → code block
- [x] **P0** `expand` → toggle block
- [x] **P0** `toc` → table-of-contents block (min/max level, style, exclude regex, printable) — min/max level, include/exclude patterns, outline numbering, nested or one-line style; same rules in the editor, shared pages, HTML and Word exports (`doc-model/src/toc.rs`); settings on the block; Confluence `type=flat`, `outline`, `include`, `exclude` imported
- [x] **P0** `anchor` → anchor node
- [x] **P0** `view-file`, `viewpdf`, `viewdoc`, `viewppt`, `viewxls` → file preview block
- [x] **P0** `status` → status lozenge
- [~] **P1** `children` (child pages list: depth, sort, reverse, style, first N) → dynamic children block (sort/style params ignored)
- [~] **P1** `pagetree` / `pagetreesearch` → dynamic tree block (rendered as child list; no search)
- [~] **P1** `attachments` (list + upload) → attachments block (live list; no upload in the block)
- [~] **P1** `include` (include page) → transclusion block (plain-text excerpt preview)
- [~] **P1** `excerpt` / `excerpt-include` → synced block / named excerpt (excerpt inlined; include shows page start)
- [x] **P1** `details` + `detailssummary` (page properties + report) → page properties block + "properties report" query table
- [x] **P1** `contentbylabel` (incl. CQL parameter) → label query block
- [x] **P1** `recently-updated` → recent changes block
- [x] **P1** `widget` / `iframe` / `multimedia` → embed block (with allowlist)
- [ ] **P1** `livesearch` → search box block
- [~] **P1** `tasks-report-macro` → task report block (by space, assignee, label, status, due) (no assignee filter)
- [x] **P1** `decisionreport` → decisions report block — "Decision log" block (live, permission-filtered, Word export evaluates it); `/decision` and `/decision log` slash commands
- [ ] **P1** `create-from-template` → "create from template" button block
- [x] **P1** `content-report-table` / blueprint index pages → query table block
- [~] **P1** `jira` / `jiraissues` / Jira chart → issue tracker embed (Jira, GitHub, GitLab, Linear) (issue key kept as text; no live embed)
- [~] **P1** `unmigrated-wiki-markup` → convert wiki markup; keep the original as a fallback (converted; original not kept)
- [x] **P1** `section` / `column` (legacy layouts) → layout block
- [ ] **P2** `blog-posts`, `contributors`, `contributors-summary`, `profile`, `userlister`, `popular-labels`, `related-labels`, `listlabels`, `navmap`, `index`, `spaces`, `space-details`, `favpages`, `gallery`, `chart`, `roadmap`, `team-calendar`, `html` (admin opt-in, sanitized)
- [~] **P2** Third-party app macros: `drawio`, `gliffy` (→ draw.io conversion), `mathblock`/`mathinline`, `plantuml`, `mermaid-cloud` (math/mermaid native; drawio PNG preview)
- [x] **P1** **Unknown macro fallback:** preserve the raw macro (name, params, body) as an inert "legacy macro" block, render a
      placeholder, list it in the import report, and let admins map it to a native block later
- [ ] **P2** Macro mapping admin UI: map unknown macro name → native block or plugin

### 4.5 Real-time collaboration
- [x] **P0** Multi-user co-editing via Yjs protocol over WebSocket (`yrs` server); conflict-free merges `C D X O`
- [x] **P0** Presence: avatars of current viewers/editors, live cursors and selections with names/colors `C D O`
- [x] **P0** Persistence: append Yjs updates, compact to snapshots, bounded memory per document `C D`
- [x] **P0** Auth on connect + per-message permission re-check (revocation kicks the user out of the doc) `C D`
- [x] **P0** Reconnect / resume with state vectors; offline edits merged on reconnect `C D`
- [ ] **P1** "Who changed what" attribution during a session (colored authorship view) `C`
- [ ] **P1** Collaborative drafts: unpublished changes visible to co-editors only; publish creates a version `C`
- [x] **P1** Live docs: changes are live without publish; versions created by time/edit heuristics `C`
- [x] **P1** Follow a collaborator (jump to their cursor) `C`
- [ ] **P2** Collaboration on comments, titles and page properties (not only body)
- [x] **P2** Multi-node collab: document-affinity routing, failover, Redis/NATS awareness relay
- [ ] **P2** Large-document performance (≥ 1 MB docs, 50 concurrent editors) with load tests

### 4.6 Drafts, versions & history
- [x] **P0** Version on publish (or periodic for live docs) with author, timestamp, optional message `C D X O B`
- [x] **P0** Version list, view any version, restore version (creates a new version) `C D X O B`
- [~] **P1** Visual diff between any two versions (inline + side-by-side, block-aware) `C X B` (paragraph diff vs previous version only)
- [~] **P1** "What changed since my last visit" highlighting `C` (banner + diff since the last seen version; no inline highlight)
- [x] **P1** Minor edit flag (no notifications) `C X` — "Minor change" switch while editing (per person and editing session, `PUT /pages/{id}/minor-edit`): the session's version is minor when everybody who edited it said so, watchers aren't notified, history shows *minor*; `minor` on named versions; webhooks carry it
- [x] **P1** Personal unpublished drafts (new pages not yet published) and a drafts list `C D` — `pages.draft`: only the creator sees a draft and the pages below it (space admins included; enforced in the central access checks), no mentions / watcher notifications / webhooks until Publish (which also creates a "Published" version); new pages from the Create dialog start as drafts (remembered choice); draft banner + Publish, draft badge in the tree, Drafts on Home, `GET /me/drafts`
- [ ] **P1** Scheduled publishing `C(apps)`
- [x] **P1** Attachment versioning (upload a new version of the same file name) `C X` — version history in the attachments panel: download any version, make an earlier one current again (as a new version); `GET /attachments/{id}/versions`
- [ ] **P2** Version retention rules (keep N versions / X days; per space) `C DC`
- [ ] **P2** Version history for page properties, labels, permissions (audit-backed)
- [ ] **P2** Delete specific versions (admin, for sensitive data) `C X`

### 4.7 Comments, reactions, mentions, tasks
- [x] **P0** Page (footer) comments with threads and replies `C D X O B`
- [x] **P0** Inline comments anchored to a text range (Yjs relative positions; survive edits) `C D O`
- [x] **P0** Resolve / reopen inline comments; show resolved comments `C D O`
- [x] **P0** Edit / delete own comments; admins can moderate `C D X`
- [~] **P1** Rich comments (formatting, mentions, emoji, images, code) `C D O` (Markdown + @mentions; no images)
- [~] **P1** Reactions (emoji) on pages and comments `C O` (likes on pages and comments; no emoji by design)
- [x] **P1** Page likes / "helpful" signal `C X`
- [x] **P1** Comment sorting and filtering (open / resolved / mine) `C` — Open / Resolved / Mine (threads you started or replied to) / All, with counts; a clicked resolved comment switches the filter
- [~] **P1** Tasks: assign via mention, due dates, "my tasks" view across spaces, reminders `C` (My tasks page done; no reminders)
- [~] **P1** Comment notifications + reply via email `C DC` (e-mail digest; no reply by e-mail)
- [x] **P2** Comment permissions separate from edit (commenter role) `C X`
- [ ] **P2** Suggestions / suggested edits (track-changes mode) `N(GDocs-like)`
- [ ] **P3** Annotations on attachments (PDF comments)

### 4.8 Templates & blueprints
- [~] **P1** Page templates: global and per space; create from template; template picker with preview `C D X O B` (template picker with preview; space templates only for people who can see the space; workspace templates API only)
- [~] **P1** Built-in template gallery (meeting notes, decision log, retrospective, how-to, troubleshooting, product
      requirements, project plan, runbook, incident postmortem, onboarding, architecture/arc42, ADR, OKRs) `C` (12 built-ins so far)
- [x] **P1** Template variables / placeholders (prompt for values on create) `C X` — `{{Name}}` in a template's title or text: the Create dialog asks for the values (`variables` on `POST /pages`, listed per template); answers keep the text's formatting, unanswered ones become instructional hints (dropped from titles), code is never scanned; built-ins ask for their title; `{date}` as before
- [ ] **P1** Blueprints: template + index page + labels + naming rule (e.g. meeting notes auto-dated under a parent) `C`
- [~] **P1** Default template per space / per parent (child pages inherit a template) `C X` (per space)
- [ ] **P2** Space templates (full page structure) `C X`
- [ ] **P2** Template sharing / import/export as files
- [ ] **P2** AI-generated templates from a prompt `C`

### 4.9 Structured content: labels, properties, databases
- [~] **P1** Labels/tags on pages, blog posts and attachments; label autocomplete; label pages (all content with label X) `C X O B` (pages and blog posts; autocomplete by usage over the pages you can see, `GET /labels?q=`; not on attachments)
- [ ] **P1** Page properties (typed key/value: text, number, date, user, select, URL, page link) shown in a properties panel `C X N`
- [~] **P1** Properties report: query table across pages (filter by space, label, property) `C` (space + label filters; no property filters)
- [ ] **P2** **Databases** `C D N X`
  - [ ] Field types: text, rich text, number, currency, date, checkbox, select, multi-select, user, page link, URL, email, attachment, formula, rollup, relation, created/updated by/at
  - [ ] Views: table, board/kanban (incl. swimlanes), list/cards, calendar, timeline/gantt, gallery
  - [ ] Filter, sort, group, hide fields, saved views per user
  - [ ] Row = page (open a row as a full page with body)
  - [ ] Embed database / view inline in pages and whiteboards
  - [ ] CSV import/export
  - [ ] Linked records to issue trackers (Jira/GitHub) with live status
  - [ ] Real-time collaborative editing of cells
  - [ ] Permissions per database (inherit from tree)
- [ ] **P3** XWiki-style structured page classes: define a "class" (schema) and attach typed objects to pages; forms; AppWithinMinutes-like no-code app builder `X`
- [ ] **P3** Calculated content via a safe query language (CQL-like) in blocks

### 4.10 Whiteboards & diagrams
- [ ] **P1** Draw.io integration (self-hosted draw.io editor in an iframe; store `.drawio` + SVG) `D X C(app)`
- [x] **P1** Excalidraw block `D`
- [~] **P1** Mermaid / PlantUML text diagrams `D X` (Mermaid client-side only)
- [ ] **P2** Native infinite-canvas whiteboards: sticky notes, shapes, connectors, text, frames, images, pen `C`
  - [ ] Real-time multi-user (Yjs)
  - [ ] Templates (retro, brainstorming, journey map, SWOT, kanban)
  - [ ] Voting, timer, emoji reactions on objects `C`
  - [ ] Convert stickies → tasks / issues / database rows `C`
  - [ ] Embed pages, databases, smart links on the canvas `C`
  - [ ] Export PNG/SVG/PDF; dark mode `C`
- [ ] **P3** Gliffy → draw.io converter (import)

### 4.11 Search & discovery
- [x] **P0** Full-text search across pages, titles, comments, labels with permission filtering `C D X O B`
- [x] **P0** Quick search (as-you-type, title-weighted, recent items first) `C D O`
- [x] **P0** Filters: space, type, author/contributor, label, date modified `C X`
- [x] **P1** Attachment content search (PDF, DOCX, XLSX, PPTX, TXT, CSV, ODT) `C D X`
- [~] **P1** Highlighted snippets, typo tolerance, stemming per language, CJK tokenization `C X` (snippets only)
- [x] **P1** Search within a space (scoped) and within a page (Ctrl+F style) `C`
- [ ] **P1** Query language (CQL-like: `space = ENG AND label = runbook AND lastModified > now("-30d")`) for macros/API `C`
- [x] **P1** Saved searches `X`
- [x] **P1** Incremental indexing via the job queue; full reindex CLI `X`
- [ ] **P2** Semantic / hybrid search (BM25 + vectors, reciprocal rank fusion) `C D`
- [ ] **P2** Search analytics (top queries, zero-result queries) `C`
- [ ] **P2** Federated search across connected tools (GitHub, Drive, Slack) `C(Rovo)`
- [ ] **P2** OpenSearch/Elasticsearch backend for very large installs `DC X`
- [x] **P1** OpenSearch description (browser search integration)

### 4.12 Permissions, sharing & access control
> Must be at least as expressive as Confluence (global → space → page restrictions) and safe by default.

- [x] **P0** Global/workspace roles: owner, admin, member, guest, (anonymous) `C D X B`
- [x] **P0** Space roles: admin, editor, commenter, viewer (+ custom roles in P2) `C D`
- [x] **P0** Groups (local) and group-based grants `C D X B`
- [x] **P0** Page-level restrictions: restrict view and/or edit to users/groups; **inherited by descendants** `C D X`
- [x] **P0** Permission checks enforced in API, collab WebSocket, search, exports, AI, notifications, webhooks (single authz crate, no bypass paths)
- [ ] **P1** Granular space permissions (Confluence-compatible set): view, add/delete pages, add/delete blog, add/delete comments, add/delete attachments, set page restrictions, export, admin, archive, delete own content `C DC`
- [x] **P1** Anonymous / public access per space (read-only) `C X B O`
- [~] **P1** Public share links for a single page (with optional expiry, password, subtree inclusion, disable globally) `C D O` (no password)
- [x] **P1** Guest users limited to specific spaces `C`
- [~] **P1** Request access flow (request → owner approves) `C` (request notifies page admins; they grant via restrictions/permissions)
- [x] **P1** "Inspect permissions": why can user X see/edit page Y? `C`
- [ ] **P1** Effective permission cache with invalidation (fast tree rendering for 100k pages)
- [ ] **P2** Custom space roles / permission schemes `C`
- [ ] **P2** Deny rules (XWiki-style explicit deny) `X`
- [ ] **P2** External sharing controls and domain allowlists `C`
- [ ] **P2** Data classification levels (Public / Internal / Confidential / Restricted) with policies (block public links, block export, block AI) `C(Guard)`
- [ ] **P2** Admin key / "break-glass" access with audit trail `C`
- [ ] **P3** ABAC policies via `cedar-policy` (attributes like department, location)

### 4.13 Authentication & identity
- [x] **P0** Local accounts: email + password (argon2id), email verification, password reset `D X B O` (addresses are verified by invitation/sign-up links)
- [x] **P0** Sessions: secure cookies, CSRF protection, session list + revoke, idle/absolute timeouts
- [x] **P0** Invitations (email link), invite-only or open sign-up with domain restriction `C D O`
- [x] **P1** OIDC (Google, Microsoft Entra ID, Okta, Keycloak, Authentik, GitLab) `C D X B O`
- [x] **P1** SAML 2.0 SSO `C D X B`
- [x] **P1** LDAP / Active Directory (bind auth + group sync) `DC D X B`
- [~] **P1** MFA: TOTP, WebAuthn/passkeys, recovery codes; enforce per workspace/group `C D` (TOTP + recovery codes; required for admins or everyone as a workspace setting, enforced on every session incl. SSO; no passkeys)
- [~] **P1** Personal API tokens (scoped, expiring) and OAuth2 apps (authorization code + PKCE) `C D X O B` (read-only or read/write scope and expiry; read-only tokens also limit the MCP tools; no OAuth2 apps)
- [~] **P1** User profiles (avatar, title, department, timezone, language, bio) and people directory `C X` (name/password only; no directory)
- [ ] **P2** SCIM 2.0 provisioning (users + groups) `C D`
- [x] **P2** JIT provisioning + group mapping from IdP claims `C D`
- [ ] **P2** Service accounts / bot users `C`
- [~] **P2** User deactivation keeps content attributed; anonymization (GDPR right to be forgotten) `C` (deactivation only; no anonymization)
- [ ] **P2** Login policies: IP allowlists, session duration, device trust `C(Guard)`

### 4.14 Navigation, home & personalization
- [x] **P0** Home dashboard: recent pages, starred, drafts, tasks assigned to me `C D O` — recently viewed / updated, activity, popular, starred, Drafts (unpublished), open tasks
- [x] **P0** Recently viewed / recently edited `C D O`
- [x] **P1** Starred/favorite pages and spaces `C D O` — pages (sidebar, Home) and spaces (`space_favorites`; star in the directory, starred filter, starred first in directory and sidebar)
- [ ] **P1** Activity feed (following people/spaces; popular in my org) `C X` — partial: Home → Activity (everything visible, or watched pages and spaces) and Popular; following people not yet
- [~] **P1** "For you" / popular pages `C` (popular pages; no personalisation)
- [x] **P1** Command palette (Ctrl/Cmd+K): jump to page/space/action `C O` — pages (search, recently viewed) plus actions for the context (create page / folder / space, go to Home, Spaces, My tasks, blog, trash, space settings, profile, administration, dark mode, shortcuts); `>` lists actions only
- [~] **P1** Dark mode, theme switching, high-contrast `C D O` (no high-contrast theme)
- [ ] **P1** UI localization (i18n; ≥ EN, DE, FR, ES, PT, IT, NL, PL, JA, ZH, KO); RTL support (AR, HE) `C D X`
- [ ] **P2** Customizable home per workspace (announcements, featured spaces) `C X`
- [ ] **P2** Sidebar customization per user (pinned items) `C`

### 4.15 Notifications & watching
- [x] **P0** In-app notification center (mentions, comments, replies, shares, task assignments) `C D O`
- [x] **P1** Email notifications with batching / digest (immediate, hourly, daily) `C DC X O` — per person: a few minutes after (default), at most hourly, or daily; only what's still unread goes out
- [~] **P1** Watch page / page subtree / space / blog; auto-watch on create/edit `C X` (pages, page subtrees (changes and new pages below; unwatching one page below opts out of it) and spaces; auto-watch on create, on edit and when mentioned, unless you stopped watching on purpose; no blog-only watch)
- [x] **P1** Notification preferences per channel and event type `C`
- [~] **P1** Slack / Microsoft Teams / Mattermost notifications (per user and per space channel) `C O` (per workspace/space channel via webhook formats; not per user)
- [ ] **P2** Web push notifications (PWA) `C`
- [ ] **P2** Read confirmations / acknowledgement campaigns ("please confirm you read the policy") `D(EE, planned) C(apps)`
- [ ] **P2** Content review reminders (page owner must re-verify every N days) `D(EE)`

### 4.16 Content lifecycle & governance
- [x] **P1** Trash, restore, permanent delete `C D X` — see the trash item in §4.3
- [~] **P1** Archive pages/spaces `C` (space archive via API only)
- [x] **P1** Page verification: request review, approve, "verified" badge with expiry `D(EE) C`
- [ ] **P2** Approval workflows (draft → review → approved → published; multiple approvers; per space) `C(apps e.g. Comala) X(change requests)`
- [ ] **P2** XWiki-style change requests (propose changes, review, merge) `X`
- [ ] **P2** Stale-content detection and bulk archive ("content manager") `C`
- [ ] **P2** Retention policies (delete or archive after N days, version pruning) `D(EE) C`
- [ ] **P2** Legal hold (prevent deletion) `C`
- [ ] **P2** Bulk operations: move, archive, delete, relabel, change owner, change restrictions `C`
- [ ] **P2** Broken-link report and orphaned-page report per space `X`

### 4.17 Analytics & insights
- [x] **P1** Page views count and unique viewers ("who viewed", privacy-configurable) `C` — counts for everyone; who viewed and when for the page's editors once an admin turns on the viewer list (off by default)
- [ ] **P2** Page insights: views over time, readers, engagement, time on page `C`
- [ ] **P2** Space and workspace analytics (most viewed, most active users, top contributors) `C`
- [ ] **P2** Search analytics `C`
- [ ] **P2** Export analytics (CSV) and privacy controls (opt-out, aggregation only) `C`

### 4.18 Import (migration)
> The Confluence importer is the flagship feature. See §6 for the verification run on real exports and the requirements derived from it.

- [x] **P0** **Confluence space export, classic XML** (`entities.xml` + `attachments/<containerId>/<attachmentId>/<version>`; Server/DC and older Cloud) `D X`
- [x] **P0** **Confluence Cloud CSV space export** (`exportFormat=csv`: `spaces.csv`, `content.csv`, `bodycontent.csv`, users …; used by current Cloud exports) **new, verify all files**
- [x] **P1** Confluence HTML export (fallback when XML/CSV isn't available) `D` — hierarchy from breadcrumbs, attachment names; macros arrive flattened, no authors/comments/labels
- [~] **P1** Confluence site / full backup import (multiple spaces, users, groups, global permissions) `X` (multi-space + users; no global permissions)
- [ ] **P1** Confluence REST API live migration (pull spaces directly with an API token: no export size limits, incremental re-sync) `X(pro)`
- [x] **P0** Markdown / Markdown-zip (Obsidian vault with wikilinks, front matter) `D O`
- [x] **P1** HTML files / zip `D` — Admin → Import (same importer as Markdown): pages, folders, attachments, cross-links
- [~] **P1** Notion export (Markdown + CSV databases, HTML) `D O` (Markdown + CSV; no HTML export format)
- [~] **P1** DOCX (Word) import as page(s); split by heading `D X` (one page per document; no split)
- [ ] **P2** PDF import (text + images) `D`
- [ ] **P2** Google Docs (via export or Drive API)
- [ ] **P2** MediaWiki XML dump `X`
- [ ] **P2** XWiki XAR export
- [ ] **P2** Docmost, Outline (JSON export), BookStack (API), Wiki.js (DB / git), DokuWiki, SharePoint pages, GitHub wiki, Quip `X(MediaWiki,Dokuwiki)`
- [~] **P1** Import job UI: upload (resumable, multi-GB), dry-run analysis report, mapping (users, spaces, macros), progress, cancel, retry, final report with warnings per page (no resumable upload, mapping, cancel or retry)
- [~] **P1** Re-runnable & incremental imports (stable source-id mapping; update instead of duplicate) (Confluence: update imports, see §6.3; Notion / Word / HTML / Markdown imports still create new pages)

### 4.19 Export, print & publishing
- [~] **P1** Export page / subtree to PDF (headers/footers, TOC, page numbers, cover page, custom CSS/template) `C X B` (browser print-to-PDF only)
- [x] **P1** Export to Word (DOCX) `C X B` (page or page tree; report macros evaluated)
- [x] **P1** Export to Markdown / HTML (zip with attachments) `D O B`
- [x] **P1** Print-friendly view `C X B`
- [x] **P1** Native space export/backup format (JSON + blobs, versioned) for move/restore between instances `C X`
- [ ] **P2** Export to Confluence-compatible format (reverse migration, an anti-lock-in promise)
- [ ] **P2** Scheduled exports to S3/Git (docs-as-code mirror)
- [~] **P2** Static site publishing of a space (public docs portal, custom domain, SEO, sitemap, search) `C(apps) X` (static HTML export only)
- [ ] **P3** Git sync (two-way Markdown sync with a repository) `Wiki.js`

### 4.20 Integrations
- [ ] **P1** Smart links / unfurling via oEmbed + OpenGraph, with provider plugins `C D O` — partial: OpenGraph / `<title>` link cards (no oEmbed providers yet)
- [ ] **P1** Issue trackers: Jira (issue macro, create issue from selection, issue status lozenge), GitHub, GitLab, Linear `C`
- [ ] **P1** Slack / Microsoft Teams / Mattermost: link unfurl, notifications, search from chat `C O`
- [~] **P1** Diagrams: draw.io, Excalidraw, Mermaid, PlantUML (server-side, sandboxed) `D X` (Mermaid client-side only)
- [~] **P1** Embeds: Figma, Miro, Loom, Google Drive/Docs/Sheets, Airtable, Typeform, YouTube, Vimeo `D C` (no Airtable/Typeform)
- [ ] **P2** Calendar (team calendars; ICS feed; Google/Outlook sync) `C X`
- [ ] **P2** Email-to-page (mail in a space; create page/blog/comment) `DC X`
- [ ] **P2** Office documents: view/edit via Collabora/OnlyOffice (WOPI) `X`
- [ ] **P2** Zapier / n8n / Make connectors
- [ ] **P2** Browser extension (clip web page, quick search)

### 4.21 Extensibility & developer platform
- [x] **P0** REST API (OpenAPI 3.1 spec, generated; versioned `/api/v1`) covering all entities `C D X O B`
- [~] **P1** API keys, OAuth2 apps, scopes, rate limits, pagination (cursor), ETags / conditional requests `C` (API tokens only)
- [x] **P1** Webhooks (page created/updated/deleted/moved, comment, space, user; HMAC-signed, retries, delivery log) `C B O`
- [x] **P1** Built-in MCP server (search, read page, create/update page, comment) honoring permissions `D C`
- [~] **P1** CLI (`wikicraft`) and SDKs (TypeScript, Python, Rust) generated from OpenAPI (server admin CLI only; no SDKs)
- [ ] **P2** Plugin system based on WASM (sandboxed, capability-scoped):
  - [ ] Custom blocks/macros (server render + client UI bundle)
  - [ ] Event hooks (before/after save, on publish)
  - [ ] Custom smart-link providers, importers, exporters, auth providers
  - [ ] UI extension points (page byline, sidebar panels, toolbar buttons, admin pages)
  - [ ] Plugin marketplace / registry; signed plugins; per-workspace enablement
- [ ] **P2** Automation rules: trigger (page published, label added, comment, schedule) → condition → action (move, label, notify, webhook, create page, assign task) `C`
- [ ] **P2** GraphQL read API (optional) `C`
- [ ] **P3** Safe scripting (XWiki-like, but sandboxed: WASM, Rhai or Lua) for power users `X`
- [ ] **P3** Custom themes/skins API `X`

### 4.22 AI (self-hostable, permission-aware)
- [ ] **P2** Provider abstraction: OpenAI-compatible APIs, Azure OpenAI, Anthropic, Gemini, Ollama, vLLM, local embeddings `D`
- [~] **P2** Admin controls: enable per workspace/space, data retention, no-training guarantee, classification-aware exclusion `C` (workspace feature switch; nothing per space)
- [~] **P2** Ask the wiki (RAG chat) with citations to blocks, follow-ups, permission-filtered retrieval `C D O` (search page: permission-filtered retrieval, Claude Messages API, numbered citations to pages, rate limited; no follow-ups, citations to pages not blocks; only tested against a stub API so far)
- [ ] **P2** AI semantic search / answers in the search bar `C D O`
- [ ] **P2** Writing assist in editor: improve, shorten, expand, fix grammar, change tone, translate, summarize, continue `C D`
- [ ] **P2** Page / thread summaries; "catch me up" on changes since last visit `C`
- [ ] **P2** Generate page/template/whiteboard/database from a prompt `C`
- [ ] **P2** Auto-labels, title suggestions, duplicate-page detection
- [ ] **P2** Definitions of company jargon (glossary) `C`
- [ ] **P3** Agents: scheduled/triggered AI tasks (e.g. "update the status page weekly from Jira"), with an audit trail `C(Rovo agents)`
- [ ] **P3** Meeting notes from transcripts / Loom-like video summaries `C`
- [ ] **P2** Embedding pipeline: chunk by block, re-embed on publish, ACL fingerprint per chunk, background backfill

### 4.23 Multilingual content
- [ ] **P1** UI i18n (Crowdin/Weblate workflow) `C D X`
- [ ] **P3** Page translations: one page with language variants, fallback language, translation status (outdated marker) `X`
- [ ] **P3** AI-assisted translation that keeps variants in sync

### 4.24 Mobile, offline & accessibility
- [x] **P1** Responsive web UI usable on phones (read, comment, light edit) `C D O`
- [ ] **P2** PWA: installable, push notifications, offline reading of starred pages
- [ ] **P3** Native mobile apps (iOS/Android) `C`
- [ ] **P1** WCAG 2.2 AA compliance target; automated axe checks in CI `C` — partial: axe (critical/serious, WCAG 2.x A/AA) on the main screens in the e2e suite, all fixed; admin/space dialogs not covered yet

### 4.25 Administration
- [x] **P0** Admin console: users (invite, deactivate, reset MFA, change role), groups, spaces overview `C D X B`
- [x] **P0** Email (SMTP) settings with a test send `D X B`
- [~] **P0** Storage settings (local / S3) and usage `D` (env config only; usage in admin stats)
- [ ] **P1** Branding: logo, colors, login page, custom CSS (scoped), custom HTML header/footer (admin opt-in) `C X B` — partial: logo (also on the sign-in page)
- [x] **P1** Global announcement banner `C` — Admin → Overview; info / warning / good-news styles, clickable links, dismissed per person until it changes
- [x] **P1** Feature toggles (disable public links, disable exports, disable AI, disable personal spaces) `D` — Admin → Overview; enforced server-side (exports stay available to admins), `GET /api/features` for clients
- [~] **P1** Global templates & default space permissions `C` (global templates via API only)
- [~] **P1** Attachment limits (max size, allowed/blocked types), quota per space/workspace `C X` (max upload size via env only)
- [~] **P1** System info / health page (version, DB, queue depth, index status) `X DC` (health endpoint + admin stats only)
- [~] **P1** Background job dashboard (imports, reindex, exports) with retry/cancel (job list only; no retry/cancel)
- [~] **P2** Audit log viewer with filters + export + SIEM streaming (syslog, webhook, Splunk HEC) `C D(EE) X` (basic viewer; no filter UI, export, SIEM)
- [ ] **P2** License/edition management (open-core enterprise key)
- [ ] **P2** Maintenance / read-only mode `DC`
- [~] **P2** Usage reporting (active users, storage, growth) (current counts only; no growth)

### 4.26 Security & compliance
- [~] **P0** OWASP ASVS L2 baseline; threat model per feature (import, uploads, embeds, plugins, AI) (internal security review; no formal ASVS)
- [~] **P0** Strict HTML sanitization (`ammonia`) for all rendered/imported content; CSP with nonces; no inline user JS (escaping renderer + app-wide CSP; no nonces needed: no inline scripts)
- [~] **P0** Upload safety: content sniffing, `Content-Disposition: attachment` for risky types, serve user files from a separate origin/subdomain, SVG sanitization (no separate user-content origin)
- [x] **P0** Rate limiting and brute-force protection on auth; account lockout policies
- [~] **P0** Secure secrets handling (env/files, never in DB in plaintext); TLS everywhere; HSTS (HSTS sent when the base URL is https; TLS at the proxy; secrets via env or `_FILE`)
- [ ] **P1** Antivirus scanning hook (ClamAV) for uploads `X`
- [~] **P1** Iframe/embed allowlist; SSRF protection for link unfurling (block private IP ranges, DNS rebinding) (embed allowlist; unfurling not built)
- [~] **P1** Import hardening: zip-slip, zip bombs, XML entity expansion disabled, per-file and total size limits, timeouts (Confluence zip entry caps incomplete)
- [~] **P1** Audit log of security-relevant events (logins, permission changes, exports, admin actions) `C D(EE) X B` (exports not audited)
- [ ] **P2** Encryption at rest for blobs (SSE-S3/KMS) and DB (deployment guidance); BYOK for SaaS `C`
- [ ] **P2** Data residency (region pinning) for SaaS `C`
- [ ] **P2** GDPR tooling: data export per user, deletion/anonymization, DPA-ready docs `C`
- [ ] **P2** SOC 2 / ISO 27001 readiness docs; dependency scanning (`cargo-audit`, `cargo-deny`), SBOM (CycloneDX), signed releases (cosign) `C`
- [ ] **P2** Penetration test before 1.0; bug bounty after 1.0
- [ ] **P3** FedRAMP / BSI C5 / air-gapped hardened profile `D`

### 4.27 Deployment & operations
- [~] **P0** Single static binary (musl) + embedded frontend + embedded migrations (frontend and migrations embedded: release builds carry `web/dist`, served when no `WIKICRAFT_STATIC_DIR` build exists, hashed assets cached as immutable; glibc build, not musl)
- [x] **P0** Docker image (distroless, non-root), `docker-compose.yml` (app + Postgres + optional MinIO) `D X B O`
- [x] **P0** Zero-downtime-friendly migrations (expand/contract), automatic on start (opt-out)
- [x] **P0** Health/readiness endpoints, structured JSON logs
- [x] **P1** Helm chart / Kubernetes manifests; horizontal scaling docs
- [ ] **P1** Backup & restore CLI (DB dump + blob sync, consistent point-in-time) + documented RPO/RTO `DC X`
- [~] **P1** Prometheus metrics, OpenTelemetry traces, Grafana dashboards (Prometheus only; no OTel/Grafana)
- [~] **P1** Air-gapped install (offline bundle, no phone-home, telemetry opt-in only) `D DC` (no phone-home; no offline bundle)
- [x] **P2** HA reference architecture (multi-node API/collab, Postgres HA, S3) `DC`
- [ ] **P2** CDN for static assets & public pages `DC`
- [ ] **P2** Upgrade assistant / pre-upgrade checks
- [ ] **P1** Performance budgets: page open p95 < 300 ms server time; search p95 < 200 ms; idle RSS < 150 MB; 1k concurrent editors per collab node (to validate)

### 4.28 Quality & documentation
- [~] **P0** CI: fmt, clippy (pedantic for core crates), nextest, sqlx offline check, frontend lint/typecheck/test (fmt/clippy/tests/typecheck; no nextest or frontend lint)
- [~] **P0** Integration tests with `testcontainers` (Postgres, MinIO) (local Postgres/MinIO, not testcontainers)
- [x] **P0** Snapshot tests (`insta`) for doc-model conversions and importers (golden fixtures, §6.4) — `crates/confluence/tests/golden/` (our own storage-format and wiki-markup fixtures: macros, links, users, attachments, tables, tasks, layouts, images, emoticons, placeholders, inline comments, ADF decisions, legacy syntax) and `crates/doc-model/tests/golden/` (Markdown: GFM, details, hostile input); each snapshot holds stats, ProseMirror JSON, Markdown and HTML, plus Yjs round-trip and Markdown-stability assertions. They found and fixed: one-line wiki `{info}…{info}` / `{code}…{code}` (content lost or document swallowed), `*` and `#` lists merged, block math exported as broken `$…$`, our own `<details>` export not re-importable, lists in table cells run together in Markdown, a trailing empty line in imported code blocks. Real kitchen-sink / XWiki corpora (below) still open
- [ ] **P1** Property tests (`proptest`) for the tree ops, permission inheritance and CRDT convergence — partial: doc model (Yjs round trip, CRDT convergence, Markdown, HTML safety); tree ops and permissions not yet
- [ ] **P1** E2E with Playwright (multi-browser co-editing scenarios)
- [ ] **P1** Load tests (collab WebSocket fan-out, search, page tree)
- [ ] **P1** Fuzzing (`cargo-fuzz`) for importers, Markdown/HTML parsers, upload handling
- [x] **P0** User docs, admin docs, API docs (generated), migration guide "Leaving Confluence DC" — `docs/user-guide.md`, `docs/admin-guide.md`, `docs/migrating-from-confluence.md`; OpenAPI 3.1 at `/api/openapi.yaml`
- [ ] **P1** Public demo instance; seed data generator

---

## 5. Roadmap (milestones)

| Milestone | Scope | Exit criteria |
|---|---|---|
| **M0: Spikes** (2–4 wks) | `yrs` ⇄ Tiptap sync over axum WS; Postgres schema for tree + ACL; tantivy indexing; Typst PDF of a ProseMirror doc; Confluence storage → ProseMirror converter on the two fixtures | All spikes demoable; ADRs written |
| **M1: Core wiki (P0)** | Auth (local), workspace, spaces, page tree, editor basics, real-time collab, versions, comments, search, attachments, basic permissions + page restrictions, REST API, Docker | Team of 10 uses it daily for 2 weeks |
| **M2: Confluence importer v1** | XML + CSV import: pages, hierarchy, history, attachments, comments (incl. inline), labels, users mapping, macro parity P0, dry-run report | Both §6 fixtures import with 0 crashes; ≥ 99% links/attachments resolved where the target exists; report lists every fallback |
| **M3: v1.0 parity (P1)** | SSO (OIDC/SAML/LDAP), MFA, granular space permissions, templates/blueprints, macro parity P1, notifications/email, watch, tasks, labels/properties, PDF/Word export, webhooks, MCP, audit log, i18n, public links, trash/archive | Pilot migration of a real ≥ 1k-page Confluence space signed off by users |
| **M4: Enterprise (P2)** | SCIM, audit/SIEM, retention, verification/approvals, analytics, databases, whiteboards, AI, plugins (WASM), HA, automation | First paying enterprise customer |
| **M5: Differentiators (P3)** | Structured classes/no-code apps, page translations, scripting, git sync, native apps | — |

---

## 6. Confluence import verification (run 2026-11-09)

> Request: *"find a free Confluence export and try to import it to verify if we support Confluence well."*
> No importer exists yet, so instead I ran a **dry-run analysis** with a throwaway Python analyzer over two real,
> publicly available exports: rebuild the page tree, resolve links/attachments/users, inventory every macro and
> storage-format element, and test whether bodies parse as strict XML (the decision between `quick-xml` and an
> HTML5 parser for the Rust importer). Tooling and full reports: [`research/confluence-import/`](research/confluence-import/).

### 6.1 Fixtures used

| Fixture | Source | Format | Size |
|---|---|---|---|
| **A: Trust over IP Foundation wiki, space `HOME`** | `lf-toip.atlassian.net`, mirrored at `github.com/LFDT-way/mcp-cache` | **Confluence Cloud CSV export** (`exportFormat=csv`, Oct 2026) | 28,307 content rows · 2,171 current pages/folders · 14,551 historical page versions · 8,047 attachment records · 229 comments · 656 users. **No attachment binaries in the mirror** |
| **B: ModelOp docs, space `DV21`** | `github.com/modelop/docs-export` | Confluence Cloud **classic XML** export (`entities.xml`, Sep 2020) | 5,241 entities · 44 current pages · 341 historical versions · 218 attachments with binaries (23 MB zip) |

Both repos carry **no license**. Use them for local testing only, and never commit or redistribute them
(`research/confluence-import/fetch_fixtures.sh` downloads them into a git-ignored folder).
Checksums of fixture A matched its `content-sha256.tsv`.

### 6.2 Results

| Check | A (Cloud CSV, 2026) | B (classic XML, 2020) |
|---|---|---|
| Page tree rebuilt | OK depth 0–7, 0 cycles, **2 orphans** (parent missing), 1 duplicate title | OK depth 0–4, 0 orphans, 0 cycles |
| Sibling order | Partial: `child_position` set on only 1,316 / 2,171 pages, so we need a fallback (Confluence default: alphabetical) | OK `position` on all |
| Storage bodies parse as strict XML (after normalization, §6.3 #6) | OK 2,366 / 2,366 | OK 44 / 44 (**94 bodies, incl. history, failed before un-escaping `]] >`**) |
| Page links (`ri:page`) resolved by (space, title) | Partial: 718 resolved · **194 point to titles not in the export** (21%) · 3 cross-space | OK 12 / 12 |
| Attachment refs (`ri:attachment`) resolved by filename | Partial: 1,912 / 1,932 (20 unresolved) | OK 102 / 102 |
| Attachment binaries at `attachments/<container>/<id>/<version>` | n/a (not mirrored) | OK 218 / 218 present |
| User refs (`ri:user`) resolved against the user list | Partial: 29,455 / 29,609 (154 unresolved); **86% legacy `userkey`, 14% `account-id`** | n/a (14 users, `atlassianAccountId`) |
| Body formats | `bodytypeid` 2 = storage XHTML (pages, comments, blogs), 0 = **wiki markup** (space descriptions), 1 = JSON content properties (3,184 rows) | `bodyType` 2 = storage, 0 = wiki markup (space description), 1 = content properties |
| Content statuses | current, **draft** (2,281 rows, 46 `drafttype=live`), deleted, historical | current, draft, deleted, **archived**, historical |

**Macros seen** (current bodies, both fixtures): `view-file` 451 · `info` 175 · `toc` 124 · `code` 103 · `children` 84 ·
`anchor` 49 · `widget` 33 · `attachments` 16 · `status` 16 · `include` 11 · `expand` 7 · `viewpdf` 4 ·
`unmigrated-wiki-markup` 3 · `contentbylabel` 2 · `create-from-template` 2 · `content-report-table` 2 · `details` 1 ·
`pagetree` 1 · `tasks-report-macro` 1 · `decisionreport` 1 · `livesearch` 1 · `recently-updated` 1 · `note` 1 ·
`appsplus-google-calendar-macro` 1 (third-party app). Also 4 legacy `<ac:macro>` elements (pre-`structured-macro` syntax).

**Non-macro storage elements that must be supported:** `ac:link` + `ri:user` (30k mentions), `ac:task-list`/`ac:task`
(5,680 tasks), `ac:image` (1,604), `ac:inline-comment-marker` (114), `time` (711 date nodes), `ac:layout`/`-section`/`-cell`
(23 layouts), `ac:adf-extension` with `ac:adf-fallback` (decision lists), `ac:emoticon`, `ac:placeholder`, `ac:caption`,
`ri:url`, `ri:space`, `ri:content-entity`, `ac:plain-text-link-body`, `ac:link-body`.
**Attributes:** `data-highlight-colour` (6.5k), `data-card-appearance` (inline/block/embed smart links, 1.2k),
`data-layout`, `data-width`, `data-table-width`, `data-table-display-mode`, `local-id` / `ac:local-id` (41k, which become stable block IDs),
`ac:macro-id`, `ac:schema-version`, inline `style=` in 1,917 bodies (text colors). **38 distinct HTML named entities** (`&nbsp;` ×49k, `&rsquo;`, `&mdash;` …).

**XML-export entity classes seen:** `Page`, `BlogPost`, `Comment`, `Attachment`, `BodyContent`, `ContentProperty`
(attachment `MEDIA_TYPE`/`FILESIZE`, inline-comment anchors, page status), `CustomContentEntityObject`, `Space`,
`SpaceDescription`, `SpacePermission` (16 types: VIEWSPACE, EDITSPACE, COMMENT, CREATEATTACHMENT, REMOVEPAGE, EDITBLOG,
REMOVEBLOG, REMOVECOMMENT, REMOVEATTACHMENT, SETPAGEPERMISSIONS, SETSPACEPERMISSIONS, REMOVEMAIL, REMOVEOWNCONTENT,
ARCHIVEPAGE, EXPORTSPACE, EXPORTPAGE), `Label`/`Labelling`, `OutgoingLink`, `Notification` (watches),
`User2ContentRelationEntity` (favourites/touched), `ConfluenceUserImpl`.

**Verdict:** the formats are fully tractable. With three normalization steps, 100% of bodies parse, so a streaming
`quick-xml` importer is viable, with no lenient HTML parsing needed for storage format. The real work is in
**link/user/attachment resolution, history, inline-comment anchoring, and macro mapping**, not parsing.
We **cannot yet claim** full Cloud-CSV support: the public mirror only contains 4 of the export's files (no
attachments, labels, permissions or inline-task file, though the descriptor says `inlineTasksFileIncluded=true`).
*Update 2026-11-10:* the XWiki corpus has two complete Cloud CSV exports (`csv-RJTest`, `csv-site-backup`): the
label, watch, favourite, permission and restriction tables have the column names our reader assumed, and attachment
files can also be stored as `attachments/<FILESTORE_ID>` (now handled).

### 6.3 Importer requirements derived from the run
- [x] **P0** Auto-detect format from `exportDescriptor.properties` (`exportType`, `exportFormat=csv`, `source=cloud|server`, `buildNumber`) and support **both** XML (`entities.xml`) and Cloud CSV (`*.csv.gz`) — detection uses `exportFormat` plus the file names; `source` / `buildNumber` go into the report
- [ ] **P0** Streaming parsers only: `entities.xml` and `bodycontent.csv` can be multiple GB; CSV fields hold multi-MB bodies (raise field limits); constant memory — partial: `quick-xml` / `csv` readers stream entries out of the zip / every body is collected into the in-memory `Export`, so memory grows with the export (not constant)
- [x] **P0** Import **full version history** (`prevver` / `originalVersion`): fixture A averages ~6.7 versions per page
- [x] **P0** Map content statuses: current → page, draft → user draft (or skip, configurable), deleted → trash, archived → archived, historical → version — never-published drafts become their author's private drafts (skipped without author or body; `--skip-drafts`); Cloud's shared drafts of published pages (unpublished edits, `prevver` set: 2,169 in fixture A, 48 in B) are counted and skipped, no longer mistaken for old versions; deleted pages go to the trash with their attachments and comments (`--skip-trash`), live children of a trashed page move up to its parent; archived → `archived_at`
- [x] **P0** Storage-format normalization before parsing:
      1. un-escape the CDATA terminator `]] >` → `]]>` (Confluence's export escape; caused **all** XML parse failures in fixture B);
      2. map HTML named entities → characters (full HTML5 entity table, not just the 38 seen); — HTML 4 table plus common HTML5 names (`entities.rs`), not the full HTML5 set; unknown names are kept as text
      3. wrap the body in a root that declares the `ac:`, `ri:`, `at:` namespaces
- [ ] **P0** Users: resolve `ri:user` by `ri:userkey` (legacy, dominant), `ri:account-id` (Cloud) and `ri:username` (old Server); build an import user-mapping step (export users have only id + display name, no email, so match by email/name or invite); unresolved users become an "unknown user (id)" placeholder that admins can remap later — partial: all three ref kinds resolve; authors match existing users by `external_ref` or e-mail, the rest become placeholder users that are claimed on invite / SSO with the same e-mail; Admin → Users → "Merge into…" maps a placeholder to an existing account (authorship, likes, watches, permissions, restrictions, mentions in pages / versions / comments / templates incl. open editing sessions; the Confluence account then maps to that person on re-import) / no bulk mapping step before the import
- [x] **P0** Links: resolve `ri:page` by **(space key, title)**, falling back to `ri:content-id`; links to missing titles (21% in A!) become "missing page" links (red link + "create page"); cross-space links stay pending and get **re-resolved after every subsequent import** (global source-id → target-id mapping table) — `ri:content-id` is tried first, then (space, title) incl. former titles; pending links resolve when clicked (`/missing` → `/api/resolve` via `import_mappings` or space + title) instead of being rewritten after later imports; absolute URLs into the export's own site (`/display/KEY/Title`, `viewpage.action?pageId=`, `/spaces/KEY/pages/ID`, tiny `/x/` links, `/download/attachments/…`) become internal links when they resolve (2,334 of 2,543 in fixture A: Cloud sites can't be redirected after the move); the site is inferred from page-id links that resolve
- [x] **P0** Attachments: resolve `ri:attachment` by filename within the containing page (or `ri:page` / `ri:blog-post` child for attachments on other pages); import all attachment versions; read media type and size from `ContentProperty` (`MEDIA_TYPE`, `FILESIZE`) in XML exports; dedupe blobs by sha256
- [x] **P0** Tree: preserve `parent` + `position`/`child_position`; null positions sort alphabetically; orphans attach to the space root with a warning; cycles are broken and reported; duplicate titles are allowed
- [x] **P0** Tasks: `ac:task-list`/`ac:task` (`task-id`, `task-uuid`, `task-status`, `task-body`) → task blocks, with assignees from `ri:user` and due dates from `<time datetime>`
- [x] **P0** Comments: footer comments with threading (`parentcommentid` / `parent`); **inline comments** = `ac:inline-comment-marker ac:ref` + content properties (`inline-marker-ref`, `inline-original-selection`, `inline-comment`), anchored to our inline-comment model; resolved state
- [ ] **P1** Layouts (`ac:layout*`, legacy `section`/`column`) → columns; ADF extensions (`ac:adf-extension`) → native blocks where known (decision list), else render `ac:adf-fallback`
- [~] **P1** Formatting: `data-highlight-colour`, inline `style` colors (map to palette), `data-layout`/`data-width` for images and tables, `data-card-appearance` → smart link inline/card/embed — highlight and text colours, table width modes, image width/alignment; smart links shown as card or embed become embed blocks (videos play), inline ones stay links
- [ ] **P1** Keep Confluence `local-id`s as block IDs and store `source_content_id` per page, so deep links and anchors can be redirected (`/display/SPACE/Title`, `/pages/viewpage.action?pageId=`, `/wiki/spaces/KEY/pages/ID/...`)
- [x] **P1** **Redirect table for old Confluence URLs** (all URL shapes above + tiny links `/x/AbCd`) so bookmarks keep working — `/display/KEY/Title`, `/pages/viewpage.action?pageId=`, `/wiki/spaces/KEY/pages/ID/…` and `/x/…` redirect via `import_mappings` (also after space key renames); covered by the XML import test
- [x] **P1** Wiki-markup converter (`bodytype 0`, `unmigrated-wiki-markup`) for old content and space descriptions — `wiki.rs` (golden snapshot test)
- [~] **P1** Space permissions: map the 16 Confluence permission types and user/group/anonymous (`allUsersSubject`) grants to our model; page restrictions (`ContentPermissionSet` view/edit) → page restrictions — roles from VIEWSPACE / COMMENT / EDITSPACE / SETSPACEPERMISSIONS for users, groups and all signed-in users (`allUsersSubject` both as boolean and as `all-licensed-users` / `anonymous-users` / `all-product-admins`), inactive grants ignored, anonymous access reported (never enabled); page restrictions mapped (fail closed); the finer permission types have no equivalent yet (§4.13 granular permissions)
- [x] **P1** Labels (global/personal `my:` namespaces), watches (`Notification`), favourites (`User2ContentRelationEntity`), blog posts, space description/home page — global labels; page, blog and space watches (digests, network follows and label watches left out; 124 from the 149 rows of fixture B, the rest are duplicates via old versions); favourites from the `favourite` relation and `my:favourite` labels (on the space description = space favourite); personal labels otherwise skipped; CSV `notifications` / `usercontent_relation` read by DB column names (unverified)
- [ ] **P1** Custom content: content properties (consume), page status, team calendars, whiteboards and databases (placeholder + report until supported)
- [x] **P1** **Dry-run mode** that produces exactly this kind of report (counts, unresolved refs, unknown macros, warnings per page) before writing anything — `analyze-confluence` CLI, `POST /imports/confluence/analyze`, Analyze button in Admin → Import
- [~] **P1** Idempotent, resumable import job with checkpoints and source-id mapping (re-import updates instead of duplicating) — update imports (`update`, `--update`): the earlier import's space continues; new pages / versions / attachment versions / comments are added, pages changed in Confluence get its new versions, pages changed on both sides are reported (or `overwriteLocalChanges`), moves and deletions follow Confluence, running it twice changes nothing (`import_mappings.source_version` / `target_version`); no checkpoints: a failed import isn't resumed
- [~] **P1** Security: zip-slip, zip bombs, XML entity expansion off, size/time limits, sanitize any `html` macro content and SVGs (zip paths with `..` / absolute / backslashes ignored, declared sizes never trusted, entry size cap; quick-xml never expands DTD entities; `html` macro → code block, never rendered; no overall time limit)

### 6.4 Golden fixtures to build next
- [ ] **P0** Create a free Confluence Cloud site and build a **"kitchen sink" space** exercising every macro, element, table
      feature, inline comment, task, restriction, label, blog, whiteboard, database, folder, live doc and attachment type.
      Export it as XML (if offered), CSV and HTML. We own this content, so it can be committed as a test fixture
      (and it verifies the complete CSV file set)
- [x] **P0** Use the **XWiki Confluence importer test corpus** (`xwiki-contrib/confluence`, LGPL-2.1, ~80 targeted cases:
      `brokenlink`, `buggyhistory*`, `cdatazerowidthspace`, `fixcdata`, `missingparents`, `orphans`, `inlinecomments`,
      `multilingual`, `pageswithdots`, `permissions`, `attachmenthistory`, `homepagerewrite`, `confluence8.5.6`, …)
      as edge-case regression tests. Check LGPL compatibility before vendoring; downloading in CI is fine. —
      downloaded at a pinned commit (`fetch_xwiki_corpus.sh`, required in CI), not vendored; `tests/xwiki_corpus.rs`
      converts all 86 exports (strict parse, structure, Yjs round trip; `confluence80tags` has one genuinely broken
      body) plus targeted checks. Found and fixed: tables stored uncompressed under `.csv.gz` names, Cloud backups
      that store attachment files by `FILESTORE_ID`, bodies without a `bodyType` (wiki markup) imported empty,
      wiki-markup links / images never resolved (`[^file]`, `[Page^file]`, `[SPACE:]`, blog paths; every `!image!`
      was "missing"), duplicate history versions
- [ ] **P1** A Confluence DC 8.x/9.x export (DC trial license) to cover Server-only macros and `username`-based refs
- [~] **P1** Re-run the analyzer on every fixture in CI against the Rust importer and assert: 0 crashes, 100% body
      parse, resolution rates ≥ the analyzer's, every unknown macro reported (XWiki corpus in CI; the real exports
      when available; unknown macros are reported but not asserted)
- [ ] **P2** Large-scale fixture (≥ 50k pages, ≥ 10 GB attachments) for memory/time benchmarks

**Prior art to study** (read only, don't copy unlicensed code): XWiki `confluence-xml` module (LGPL-2.1), Docmost's
Confluence importer (AGPL-3.0 / EE), Nosdesk's Rust Confluence importer (`backend/src/services/docs_markdown/confluence.rs`;
the repo has no license), Hallo Welt's `migrate-confluence` (PHP), and AppFlowy Cloud's Rust + `yrs` collab server.

---

## 7. Open decisions (ADRs to write)
- [ ] Canonical doc storage: ProseMirror JSON + Yjs updates (recommended) vs. Yjs-only vs. own block model
- [ ] CRDT: `yrs` (Tiptap ecosystem compatibility, recommended) vs. `loro` (movable tree, better perf, no Tiptap bindings)
- [ ] Page tree concurrency: fractional indexing in Postgres (recommended) vs. CRDT tree
- [ ] Search: tantivy embedded (default) + pluggable external engine; one index per workspace or a shared index with tenant filter
- [ ] PDF engine: Typst (fast, Rust-native, layout differs from web) vs. headless Chromium (pixel-faithful, heavy)
- [ ] License: AGPL-3.0 core + commercial EE (Docmost model) vs. Apache-2.0 + EE vs. BSL. Note: Docmost puts SSO,
      page permissions and the Confluence importer in paid tiers, so offering them **free** is a differentiator
- [ ] Frontend framework (React vs. Svelte vs. Vue) and component library
- [ ] Plugin ABI: WASM component model vs. extism vs. out-of-process HTTP apps (Forge-like)
- [ ] Hosted SaaS from day one, or self-hosted first (recommended: self-hosted first, to target Confluence DC EOL)

---

## 8. Sources
- Confluence: [What's new](https://www.atlassian.com/software/confluence/whats-new) ·
  [Databases spring 2026 drop](https://community.atlassian.com/forums/Confluence-Databases-articles/What-s-New-in-Confluence-Databases-Spring-2026-Feature-Drop/ba-p/3237974) ·
  [Cloud roadmap](https://www.atlassian.com/roadmap/cloud) ·
  [Pricing](https://www.atlassian.com/software/confluence/pricing) ·
  [Data Center end of life](https://www.atlassian.com/licensing/data-center-end-of-life) ·
  [DC EOL timeline (Adaptavist)](https://www.adaptavist.com/blog/atlassian-data-center-end-of-life-march-2029) ·
  [Cloud vs. legacy editor](https://support.atlassian.com/confluence-cloud/docs/differences-using-confluence-legacy-editor-and-cloud-editor/) ·
  [DC vs. Cloud differences](https://support.atlassian.com/migration/docs/differences-using-confluence-data-center-and-cloud/)
- Docmost: [Website](https://docmost.com/) · [Pricing / editions](https://docmost.com/pricing) · [GitHub](https://github.com/docmost/docmost)
- XWiki: [Features (user guide)](https://playgroundtemplate.xwiki.org/xwiki/bin/view/Documentation/UserGuide/Features/) ·
  [Realtime WYSIWYG editor](https://extensions.xwiki.org/xwiki/bin/view/Extension/Realtime%20WYSIWYG%20Editor) ·
  [Confluence XML format](https://www.xwiki.org/xwiki/bin/view/documentation/extensions/admin/confluence/xml-format/) ·
  [Exploring Confluence exports](https://www.xwiki.org/xwiki/bin/view/documentation/extensions/admin/confluence/explore-exports/) ·
  [Importer source + test corpus](https://github.com/xwiki-contrib/confluence)
- Fixtures: [LFDT-way/mcp-cache](https://github.com/LFDT-way/mcp-cache) · [modelop/docs-export](https://github.com/modelop/docs-export)
- Prior art: [Nosdesk](https://github.com/Nosdesk/Nosdesk) · [hallowelt/migrate-confluence](https://github.com/hallowelt/migrate-confluence)
