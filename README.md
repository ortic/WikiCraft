# WikiCraft

A self-hosted team wiki with a **Rust backend**, a real-time editor and first-class **Confluence
migration**. Built for teams that need a new home when Confluence Data Center reaches its end of
life (2029-03-28).

**Website:** <https://ortic.github.io/WikiCraft/>: features, screenshots and how to get started.

![A runbook page in WikiCraft: page tree, status, page properties, callout, tasks and a table](docs/screenshots/page.png)

## Features

**Writing and collaboration**

- **Real-time co-editing** with live cursors (Yjs CRDT; the server speaks the y-websocket protocol via `yrs`)
- **Rich editor**: slash commands, drag handles, tables (merge, colours, sort, numbered rows, normal / wide / full width), task lists with assignees, due dates and reminders, callouts with custom icons, expand blocks, columns, text colours and highlights, code blocks with syntax highlighting, Mermaid diagrams, math, mentions, status lozenges, dates, decisions, table of contents, child pages, includes, embeds (YouTube, Vimeo, Loom, Figma, Miro, Google Docs) and link previews, file previews, links to any block, find & replace
- **Drawings**: Excalidraw sketches and draw.io diagrams stored in the page; image annotation, text wrap around images
- **Paste anything**: Markdown becomes formatted content, spreadsheet cells become tables, images upload on paste or drop
- **Drafts and history**: private drafts until published, scheduled publishing, minor changes, version history with word-by-word comparison and restore, "changed since your last visit" highlights
- **Templates and blueprints**: built-in and custom templates with variables, a default template per space or for the pages below a page, blueprints that file pages under an index page
- **Comments**: page and inline comments, threads, images, resolve/reopen, mentions, likes

**Organising and finding**

- **Spaces**: page trees with folders and links, trash, archive, copy and move, blog with Atom feeds, space templates, categories and a space directory, personal spaces, shortcuts, an accent colour per space, key renames that keep old links working
- **Labels** on pages, blog posts and files; **page properties** (typed fields) with properties reports; live reports for content by label, recently updated pages, tasks and decisions
- **Search**: full text with snippets, inside attachments (PDF, Office, text), filters, saved searches, a **query language** (CQL: `space = ENG and label = runbook and lastModified > now("-30d")`) and browser search integration
- **People**: profiles with photos, a people directory, following people's activity, who viewed a page
- **Notifications**: watch pages, page trees or spaces; in-app and e-mail digests; "My tasks"

**Permissions and security**

- Workspace roles, **space roles with Confluence's permission set** (add/delete pages, blog posts, comments and files, restrictions, export, archive, delete own content) per user, group and the space default
- Page view/edit restrictions (view restrictions are inherited, as in Confluence), *Inspect permissions*, access requests, anonymous (public) spaces, guests, public share links with expiry, password and subtree
- **Sign-in**: local accounts, OpenID Connect (Microsoft Entra ID, Google, GitLab, Okta, Keycloak, Authentik, …), GitHub, LDAP / Active Directory and SAML 2.0, several at once, with group sync; **two-factor authentication** (TOTP and recovery codes, can be required)
- Audit log, personal API tokens (read-only or read/write, expiring), virus scanning with ClamAV, strict CSP and HSTS

**Moving in and out**

- **Confluence import**: classic XML exports and Cloud CSV exports with history, attachments, comments (incl. inline), labels, restrictions and space permissions, authors, statuses, drafts and trash, watches and favourites, 50+ macros mapped, links rewritten, a dry-run report, and update imports that bring later changes across. Old Confluence URLs (`/display/KEY/Title`, `viewpage.action?pageId=`, Cloud `/wiki/spaces/…/pages/ID`, `/x/` tiny links) redirect to the imported pages
- **Other imports**: Markdown / Obsidian, Notion (databases become page properties), HTML (incl. Confluence's HTML export) and Word (.docx)
- **Export**: pages and page trees as **PDF** (typeset on the server: cover, contents, headers, page numbers), Word, Markdown or HTML; whole spaces as a browsable static site, a Markdown folder tree or a full backup that restores into a new space

**Integrations and AI**

- REST API with an OpenAPI 3.1 description, webhooks (JSON, Slack / Mattermost, Microsoft Teams, Google Chat)
- Built-in [MCP](https://modelcontextprotocol.io) server: Claude & co. search, query, read and write pages with the user's permissions
- Retrieval API for RAG: the passages that best answer a question (sections of pages and attached files, cut along headings), hybrid keyword + semantic ranking, permission-filtered, each with a link to the exact block; semantic part optional with any OpenAI-compatible embeddings API (OpenAI, Ollama, vLLM, Voyage, …) and pgvector
- *Ask the wiki*: answers from the pages a person can see, with sources (optional: Claude, or any model behind an OpenAI-compatible API, also one running in your own network such as Ollama or vLLM)

**Interface and administration**

- English, German and French ([translating](docs/translating.md)); light, dark and high-contrast themes; mobile layout; checked against WCAG 2.2 AA
- Workspace name, logo, accent colour, date format, time zone and default language; announcement banner; feature switches; attachment limits and blocked file types; usage overview; space administration

Documentation: [user guide](docs/user-guide.md) · [admin guide](docs/admin-guide.md) ·
[leaving Confluence Data Center](docs/migrating-from-confluence.md) ·
[translating](docs/translating.md) · [overview deck](docs/presentation/WikiCraft-overview.pptx) (PowerPoint)

## Quick start (Docker)

```bash
export WIKICRAFT_DB_PASSWORD=$(openssl rand -hex 16)   # else the database password is "change-me"
docker compose up -d                                    # builds the image on first start (takes a while)
open http://localhost:8787                              # the first visit runs the setup wizard
```

## Deployment & scaling

WikiCraft runs as a single stateless binary. Pick the mode that fits:

| | Single instance (default) | Scaled (several replicas) |
|---|---|---|
| Start with | `docker compose up -d` | [`deploy/kubernetes`](deploy/kubernetes) (`kubectl apply -k`) or [`deploy/compose/docker-compose.scale.yml`](deploy/compose/docker-compose.scale.yml) |
| Database | PostgreSQL | PostgreSQL (managed/HA recommended) |
| Attachments & import uploads | local volume (`/data`) | S3-compatible: `WIKICRAFT_S3_BUCKET`, `_ENDPOINT`, `_REGION`, `_ACCESS_KEY_ID`, `_SECRET_ACCESS_KEY` |
| Search | `WIKICRAFT_SEARCH=tantivy` (embedded) | `WIKICRAFT_SEARCH=postgres` (shared full-text index) |
| Live collaboration | in-process | `WIKICRAFT_CLUSTER=postgres` (replicas relay edits via LISTEN/NOTIFY; **no sticky sessions needed**) |
| Background jobs | Postgres queue | same queue; any replica claims jobs, heartbeats, crashed jobs are detected |
| Sessions, SSO state, login throttling | PostgreSQL | PostgreSQL |

Nothing else is kept on local disk in scaled mode: `/data` only holds import scratch space
(an `emptyDir` on Kubernetes), and the container runs as non-root with a read-only root filesystem.

**Operations**

- **Logs**: stdout only; `WIKICRAFT_LOG_FORMAT=json` for log shippers (Loki, ELK, Datadog). Every
  request gets an `x-request-id` (accepted from your proxy or generated), included in the logs.
  Levels via `RUST_LOG` (e.g. `info,wikicraft=debug`).
- **Probes**: liveness `GET /api/health`, readiness `GET /api/ready` (503 while shutting down or
  when the database is unreachable); `wikicraft healthcheck` for container `HEALTHCHECK`s.
- **Metrics**: Prometheus text format at `GET /api/metrics` (HTTP responses, latency, WebSocket
  connections, live rooms, DB pool, jobs); protect with `WIKICRAFT_METRICS_TOKEN`.
- **Shutdown**: SIGTERM marks the pod not-ready, drains and flushes live documents.
- **Migrations** run automatically on startup and are serialized by a Postgres advisory lock, so
  rolling updates with several pods are safe.
- **Fonts for PDF export**: Noto Sans is built in; for other scripts (CJK, Arabic, …) install fonts
  in the image or put `.ttf`/`.otf` files in `<data dir>/fonts`.

## Development

Requirements: Rust ≥ 1.94, Node 20.19+ or 22.12+, PostgreSQL ≥ 15 (or Docker).

```bash
# Postgres for development (port 55432)
docker run -d --name wikicraft-pg -p 127.0.0.1:55432:5432 \
  -e POSTGRES_USER=wikicraft -e POSTGRES_PASSWORD=wikicraft -e POSTGRES_DB=wikicraft pgvector/pgvector:pg17

# frontend
cd web && npm install && npm run build && cd ..

# backend (serves web/dist, applies migrations automatically)
cargo run -p wikicraft -- serve          # http://127.0.0.1:8787

# a single binary: release builds embed web/dist as it is at build time
(cd web && npm run build) && cargo build --release -p wikicraft
./target/release/wikicraft serve         # no web/ directory needed next to it

# frontend dev server with hot reload (proxies /api to the backend)
cd web && npm run dev                     # http://127.0.0.1:5199
```

Interface texts go through `t()` with the English text as the key; `node web/scripts/i18n.mjs
check de fr` lists translations that are missing (see [translating](docs/translating.md)).

### Tests

```bash
cargo test                                # unit + integration tests (needs the dev Postgres)
./research/confluence-import/fetch_xwiki_corpus.sh    # XWiki's Confluence test corpus (pinned), for the importer regression tests
./research/confluence-import/fetch_fixtures.sh        # optional: real Confluence exports
cargo test -p wikicraft-confluence --release --test real_exports -- --nocapture
```

Integration tests create a throwaway database per test, boot the full app on an ephemeral port
and exercise it over HTTP and WebSockets (auth/CSRF, permissions, collaboration between two Yjs
peers, both Confluence export formats end-to-end, exports, security regressions).

### Browser tests (end to end)

`e2e/` drives Chromium through one journey on a fresh database (24 steps): setup, rich content,
two people editing live, comments and search, public links, exports, two-factor sign-in, templates,
draw.io, history, page properties and reports, the workspace look, and accessibility checks
(axe, WCAG 2.2 AA; the high-contrast theme at 7:1).

```bash
cd web && npm run build && cd ../e2e
npm ci && npx playwright install chromium
npx playwright test      # starts the server on :8799 with a fresh database (scripts/e2e-server.sh)
E2E_BASE_URL=http://127.0.0.1:8799 npx playwright test   # against a server you started yourself
```

### CLI

```bash
wikicraft serve                                    # web server (default)
wikicraft migrate                                  # apply migrations
wikicraft analyze-confluence export.zip            # dry run: JSON report, nothing written
wikicraft import-confluence export.zip --as-user admin@example.com [--space-key KEY] [--skip-history] [--skip-drafts] [--skip-trash] [--update [--overwrite-local-changes]]
wikicraft reindex                                  # rebuild the search index
wikicraft backup wikicraft.zip                     # whole installation: database snapshot + files
wikicraft restore wikicraft.zip                    # into an empty database (also to move servers / to S3)
wikicraft healthcheck                              # probe a running server's /api/health
```

## Configuration

All settings are flags or environment variables:

| Variable | Default | Meaning |
|---|---|---|
| `WIKICRAFT_DATABASE_URL` | `postgres://wikicraft:wikicraft@127.0.0.1:55432/wikicraft` | PostgreSQL |
| `WIKICRAFT_BIND` | `127.0.0.1:8787` | listen address |
| `WIKICRAFT_BASE_URL` | `http://127.0.0.1:8787` | public URL |
| `WIKICRAFT_DATA_DIR` | `./data` | search index, uploads, local attachment storage, extra fonts (`fonts/`) |
| `WIKICRAFT_STATIC_DIR` | `./web/dist` | built frontend; when it holds no build, the copy embedded in the binary is served |
| `WIKICRAFT_S3_BUCKET` / `WIKICRAFT_S3_ENDPOINT` | — | store attachments and uploads in S3/MinIO/R2/Ceph |
| `WIKICRAFT_S3_REGION` / `WIKICRAFT_S3_ACCESS_KEY_ID` / `WIKICRAFT_S3_SECRET_ACCESS_KEY` | `AWS_*` env | S3 credentials |
| `WIKICRAFT_SEARCH` | `tantivy` | `tantivy` (embedded) or `postgres` (shared, for several replicas) |
| `WIKICRAFT_SEARCH_LANGUAGE` | `auto` | language whose word forms search matches (`auto`: the workspace's default language; `en`, `de`, `fr`, `es`, `it`, `nl`, `pt`, … or `none`); tantivy only |
| `WIKICRAFT_CLUSTER` | `none` | `postgres` to run several replicas (live-edit relay) |
| `WIKICRAFT_LOG_FORMAT` | `text` | `json` for structured logs |
| `WIKICRAFT_METRICS_TOKEN` | — | bearer token for `/api/metrics` |
| `WIKICRAFT_MAX_UPLOAD_MB` | `2048` | max upload (attachments, imports) |
| `WIKICRAFT_CLAMAV` | — | virus scanning with ClamAV: `tcp://clamd:3310` or `unix:///run/clamav/clamd.ctl`; infected uploads are refused, imports leave infected files out |
| `WIKICRAFT_CLAMAV_FAIL_OPEN` | `false` | accept files when clamd can't be reached (default: refuse them) |
| `WIKICRAFT_SECURE_COOKIES` | `false` | set `true` behind HTTPS (an `https://` base URL also turns on HSTS) |
| `WIKICRAFT_SESSION_DAYS` | `30` | session lifetime |
| `WIKICRAFT_SESSION_IDLE_MINUTES` | `0` (off) | sign out sessions inactive this long |
| `WIKICRAFT_AUTH_PROVIDERS` / `_FILE` | — | sign-in providers (JSON list), see below |
| `WIKICRAFT_GOOGLE_CLIENT_ID` / `_SECRET` / `_ALLOWED_DOMAINS` | — | "Continue with Google" shortcut |
| `WIKICRAFT_MICROSOFT_CLIENT_ID` / `_SECRET` / `_TENANT` / `_ALLOWED_DOMAINS` | — | "Continue with Microsoft" (Entra ID) shortcut |
| `WIKICRAFT_OIDC_ISSUER` / `_CLIENT_ID` / `_CLIENT_SECRET` / `_NAME` / `_AUTO_CREATE` | — | single generic OIDC provider (redirect URI `{BASE_URL}/api/auth/oidc/callback`) |
| `WIKICRAFT_DISABLE_PASSWORD_LOGIN` | `false` | turn off local passwords (SSO / LDAP only) |
| `WIKICRAFT_SMTP_URL` | — | outgoing mail: `smtps://user:pass@host` (TLS), `smtp://user:pass@host:587?tls=required` (STARTTLS) or `smtp://host:25` (plain, local relays); also turns on *Forgot password?* |
| `WIKICRAFT_MAIL_FROM` | `WikiCraft <wikicraft@localhost>` | sender address |
| `WIKICRAFT_WEBHOOK_ALLOW_PRIVATE` | `false` | let webhooks call private / loopback addresses |
| `WIKICRAFT_UNFURL_ALLOW_PRIVATE` | `false` | let link previews fetch private / loopback addresses (closed intranets only) |
| `WIKICRAFT_AI_PROVIDER` | — | the model behind *Ask the wiki*: `anthropic` (Claude) or `openai-compatible` (any server with an OpenAI-compatible chat API: Ollama, vLLM, llama.cpp's server, LM Studio, LocalAI, hosted services). Unset: `anthropic` when a key is set, `openai-compatible` when only a base URL is set, otherwise off (nothing leaves the server) |
| `WIKICRAFT_AI_API_KEY` | — | API key: required for Anthropic; sent as a bearer token to an OpenAI-compatible server when set (a local model usually needs none) |
| `WIKICRAFT_AI_MODEL` | `claude-opus-5-5` (Anthropic) | model; required for an OpenAI-compatible server, e.g. `llama3.1` |
| `WIKICRAFT_AI_BASE_URL` | `https://api.anthropic.com` (Anthropic) | API address: a gateway in front of Anthropic, or the OpenAI-compatible server's `/v1` address, e.g. `http://ollama:11434/v1` |
| `WIKICRAFT_AI_TIMEOUT_SECS` | `180` | how long an answer may take |
| `WIKICRAFT_EMBEDDINGS_URL` / `WIKICRAFT_EMBEDDINGS_MODEL` | — | OpenAI-compatible embeddings API and model: turns on semantic retrieval (e.g. `https://api.openai.com/v1` + `text-embedding-3-small`, or Ollama's `http://localhost:11434/v1` + `nomic-embed-text`); needs pgvector ≥ 0.7. Off when unset: keyword retrieval only, nothing leaves the server |
| `WIKICRAFT_EMBEDDINGS_API_KEY` | — | bearer token for the embeddings API |
| `WIKICRAFT_EMBEDDINGS_DIMENSIONS` | — | ask for shorter vectors (models that support it, like `text-embedding-3-*`); at most 4000 |
| `WIKICRAFT_EMBEDDINGS_QUERY_PREFIX` / `…_DOCUMENT_PREFIX` | — | text put before questions / passages for models trained with prefixes (`nomic-embed-text`: `search_query: ` / `search_document: `) |

### Sign-in: SSO, LDAP / Active Directory, SAML

Local accounts (e-mail + password) work out of the box. Any number of external providers can be
added; the login page shows a button per provider, and LDAP accounts use the normal password form
("E-mail or username"). `WIKICRAFT_AUTH_PROVIDERS` (or a mounted file via
`WIKICRAFT_AUTH_PROVIDERS_FILE`) takes a JSON list:

```json
[
  {"type": "oidc", "id": "microsoft", "preset": "microsoft", "tenant": "<tenant-id>",
   "clientId": "…", "clientSecret": "…", "groupsClaim": "groups", "syncGroups": true},
  {"type": "oidc", "id": "google", "preset": "google", "clientId": "…", "clientSecret": "…",
   "allowedDomains": ["acme.com"]},
  {"type": "oidc", "id": "keycloak", "name": "Acme SSO", "issuer": "https://sso.acme.com/realms/acme",
   "clientId": "wikicraft", "clientSecret": "…"},
  {"type": "github", "id": "github", "clientId": "…", "clientSecret": "…", "allowedOrgs": ["acme"]},
  {"type": "ldap", "id": "ad", "name": "ACME directory", "url": "ldaps://dc1.acme.local",
   "bindDn": "CN=wikicraft,OU=Service,DC=acme,DC=local", "bindPassword": "…",
   "baseDn": "DC=acme,DC=local", "syncGroups": true},
  {"type": "saml", "id": "okta", "name": "Okta", "idpMetadataUrl": "https://acme.okta.com/app/…/sso/saml/metadata"}
]
```

| Type | Covers | Notes |
|---|---|---|
| `oidc` | Microsoft Entra ID (`preset: microsoft` + `tenant`), Google (`preset: google`), GitLab (`preset: gitlab`), Okta, Keycloak, Authentik, Auth0, Zitadel, … (`issuer`) | authorization code + PKCE + nonce; redirect URI `{BASE_URL}/api/auth/sso/{id}/callback` |
| `github` | GitHub and GitHub Enterprise (`apiUrl`, `webUrl`) | verified primary e-mail; `allowedOrgs` restricts sign-in to org members |
| `ldap` | OpenLDAP, Active Directory, FreeIPA, lldap, … | search with a service account (`bindDn`), then bind as the user; `startTls`; default filter matches `uid`, `mail`, `sAMAccountName`, `userPrincipalName` (`userFilter` to override, `{username}` is escaped) |
| `saml` | Okta, Entra ID, ADFS, Keycloak, Shibboleth, OneLogin, … | build feature `saml` (on in the Docker image); SP metadata at `{BASE_URL}/api/auth/sso/{id}/metadata`, ACS `{BASE_URL}/api/auth/sso/{id}/acs`; signed responses only (no SHA-1), SP-initiated only |

Common options: `name` (button label), `allowedDomains` (only these e-mail domains),
`trustEmail` (use e-mail addresses the provider doesn't mark as verified — default on for LDAP,
SAML and single-tenant Microsoft, off otherwise; `true` also allows linking existing admin
accounts),
`autoCreate` (create accounts on first login; default on for private providers and for public
ones restricted by `allowedDomains` / `allowedOrgs`, off for unrestricted Google/GitHub), `defaultRole` (`member` /
`guest`), `syncGroups` (mirror the provider's groups into WikiCraft groups; a name already used by a
local group gets a "(provider)" suffix instead of taking that group over).

Accounts are linked by the provider's stable subject id; on first login an existing account with
the same verified e-mail (including authors imported from Confluence) is claimed. Deactivated
users can't sign in through any provider. Workspaces can require two-factor authentication for
admins or for everyone; it then applies after SSO sign-ins too.

## Architecture

```
web/                React 19 + Tiptap 3 (ProseMirror) + Yjs, TanStack Query; translations in src/locales
crates/server       axum 0.8 HTTP API + WebSocket collaboration (yrs), sqlx/PostgreSQL,
                    tantivy search, object_store blobs, CQL, importers, Word and PDF (Typst)
                    exports, MCP server, CLI  → single binary
crates/doc-model    ProseMirror-compatible document model: Yjs ⇄ JSON (y-prosemirror format),
                    Markdown ⇄ JSON, safe HTML rendering, plain text
crates/confluence   Confluence export readers (entities.xml streaming, Cloud CSV),
                    storage-format → ProseMirror converter (50+ macros), import planner
```

Documents are stored as Yjs state (for collaboration) plus derived ProseMirror JSON and plain text
(for API, search, exports). Every block/inline type the importer produces exists in the editor
schema, so imported content stays fully editable.

See [`PLAN.md`](PLAN.md) for the feature roadmap and competitive analysis, and
[`research/confluence-import/`](research/confluence-import/) for the Confluence export analysis.

## API

All endpoints live under `/api/v1` (integrations should use this versioned path; `/api` is the
same API without the version, used by the web app). Authenticate with the session cookie (browser; mutating requests
must send `x-wikicraft: 1`) or with a personal token: `Authorization: Bearer wct_…`
(create one via `POST /api/me/tokens`). The full **OpenAPI 3.1** description is served at
`/api/openapi.yaml` (generated by `scripts/gen-openapi.py`; a unit test fails when a route is
missing from it) — load it into Swagger UI, Redoc, Postman or a client generator. The main
endpoints:

| Area | Endpoints |
|---|---|
| Auth | `GET/POST /setup`, `POST /auth/login`, `POST /auth/logout`, `GET /me`, `GET/POST /me/tokens` |
| Spaces | `GET/POST /spaces`, `GET/PATCH/DELETE /spaces/{key}`, `GET /spaces/{key}/tree`, `GET/PUT /spaces/{key}/permissions` (roles and the permissions each grant leaves out), `GET /spaces/{key}/trash` |
| Pages | `POST /pages`, `GET/PATCH/DELETE /pages/{id}`, `PUT /pages/{id}/content` (ProseMirror JSON or Markdown), `POST /pages/{id}/move`, `/copy`, `/restore`, `GET /pages/{id}/children`, `GET/POST /pages/{id}/versions`, `POST /pages/{id}/versions/{n}/restore`, `GET/PUT /pages/{id}/restrictions`, `GET/PUT /pages/{id}/labels`, `GET/PUT /pages/{id}/properties`, `GET /pages/{id}/export?format=md\|html\|docx\|pdf\|json` (`tree`, `toc`, `cover`), `GET /pages/{id}/collab` (WebSocket) |
| Comments | `GET/POST /pages/{id}/comments`, `PATCH/DELETE /comments/{id}` |
| Attachments | `GET/POST /pages/{id}/attachments`, `GET /attachments/{id}/content`, `PUT /attachments/{id}/labels`, `DELETE /attachments/{id}` |
| Discovery | `GET /search?q=&space=`, `GET /cql?cql=&space=` (query language), `GET/POST /retrieve` (passages for RAG, below), `GET /recent`, `GET /favorites`, `GET /labels/{name}`, `GET /labels/{name}/attachments`, `GET /resolve?space=&title=` / `?source=confluence:<id>` |
| Reports | `GET /reports/pages?labels=&spaces=&cql=&sort=&max=`, `GET /reports/properties?labels=&where=`, `GET /reports/tasks?spaces=&status=`, `GET /reports/decisions`, `GET /tasks` (mine) |
| Sharing | `GET/POST /pages/{id}/share-links`, `DELETE /share-links/{id}`, public: `GET /public/{token}[/pages/{id}]`, `GET /public/{token}/attachments/{id}` |
| Import / export | `POST /imports/confluence` (multipart `file`, `options`), `POST /imports/confluence/analyze`, `POST /imports/markdown`, `POST /imports/backup`, `POST /spaces/{key}/export {format: html\|markdown\|backup}`, `GET /jobs/{id}`, `GET /jobs/{id}/download` |
| Account | `POST /auth/forgot`, `POST /auth/reset`, `GET /auth/invite`, `POST /auth/accept-invite`, `GET/PUT /me/preferences`, `PUT /me/profile` (incl. `language`), `GET /auth/providers` |
| Admin | `GET/POST /users`, `POST /users/invite`, `PATCH /users/{id}`, `GET/POST /groups`, group members, `PUT /admin/workspace` (name, look, date format, time zone, default language), `GET /admin/audit`, `GET /admin/stats`, `POST /admin/reindex`, `GET/PUT /admin/settings`, `GET /admin/mail`, `POST /admin/mail/test`, webhooks (below) |

### Webhooks

Admins register endpoints under Admin → Webhooks (or `POST /api/admin/webhooks {url, events, spaceKey}`).
Events: `page.created`, `page.updated` (new version), `page.moved`, `page.trashed`, `page.restored`,
`comment.created`, `attachment.created`, `space.created` (plus `ping` from "Send test"). Each
delivery is a JSON `POST`:

```json
{"event": "page.created", "timestamp": "…", "webhookId": "…", "deliveryId": "…",
 "data": {"page": {"id": "…", "title": "…", "version": 1, "url": "https://wiki/pages/…"},
          "space": {"key": "ENG", "name": "Engineering"}, "actor": {"name": "…", "email": "…"}}}
```

Verify `X-WikiCraft-Signature: sha256=<hex HMAC-SHA256(body, secret)>`. Non-2xx answers are
retried with exponential backoff (8 attempts). Payloads carry metadata only, never page content.
Targets on private/loopback addresses are refused unless `WIKICRAFT_WEBHOOK_ALLOW_PRIVATE=true`.

Chat channels: pick *Slack or Mattermost*, *Microsoft Teams (Workflows)* or *Google Chat* as
the format (`"format": "slack" | "teams" | "googlechat"`) and paste the channel's incoming-webhook
URL; events then arrive as readable messages ("Ann updated *Q3 plan* in Finance") with a link.

## Retrieval API (RAG)

Other AI applications can use the wiki as their knowledge source: `GET /api/retrieve?q=…` (or
`POST /api/retrieve {"query": …}`) returns the passages that best answer a question, taken only
from pages the caller may open (API token or session; read-only tokens work for both).

```bash
curl -H "Authorization: Bearer wct_…" "https://your-wiki/api/retrieve?q=How+do+I+rotate+the+VPN+certificates&space=IT&limit=5"
```

```json
{"mode": "hybrid", "passages": [{
  "pageId": "…", "title": "VPN guide", "spaceKey": "IT", "spaceName": "IT", "path": ["Infrastructure"],
  "headings": ["Operations", "Certificates"], "blockId": "x1y2z3", "url": "https://your-wiki/pages/…#block-x1y2z3",
  "text": "## Certificates\n\nCertificates expire after 90 days. Rotate them with …", "labels": ["vpn"],
  "updatedAt": "…", "version": 12, "attachment": null, "score": 0.0325, "matchedBy": ["keyword", "semantic"]}]}
```

- **Passages** follow the document's structure: a new one starts at every heading, long sections
  are cut between blocks (~1200 characters), big tables and code blocks between lines (repeating
  the table header / code fence). Each passage is Markdown, knows its heading path and links to
  the block it starts at. The extracted text of attached files (PDF, Word, text, …) adds
  passages of its own (`attachment` says which file). Passages are cut again whenever a page,
  its labels or its attachments change.
- **Ranking**: keyword retrieval (the full-text search picks candidate pages, BM25 picks their
  passages) and, when configured, semantic retrieval (nearest passage vectors) are fused by
  reciprocal rank fusion. `mode`: `auto` (default: hybrid when semantic retrieval is available),
  `hybrid`, `keyword` or `semantic`; `note` explains when semantic retrieval was asked for but
  couldn't run.
- **Parameters**: `space` (keys, comma-separated), `label` (pages with any of them), `limit`
  (default 8, at most 50), `perPage` (passages per page, default 3), `maxChars` (character budget
  for the passage texts).
- **Semantic retrieval** needs `WIKICRAFT_EMBEDDINGS_URL` + `WIKICRAFT_EMBEDDINGS_MODEL` (any
  OpenAI-compatible `/embeddings` endpoint) and the pgvector extension (≥ 0.7; the Docker Compose
  file uses the `pgvector/pgvector` image; elsewhere run `CREATE EXTENSION vector;` once as a
  superuser). A background worker embeds every passage, then only passages whose text changed;
  vectors are stored as `halfvec` with an HNSW index, and changing the model embeds everything
  again. Page text goes to the embeddings API, so pick a local model (Ollama) if it must stay
  in-house. Admin → Overview shows progress and errors.
- Keeping an external vector store in sync instead? Export spaces as Markdown
  (`POST /spaces/{key}/export`) and follow the webhooks. Content copied out of the wiki no longer
  follows its permissions, though; the retrieval API does.

## AI assistants (MCP)

Create a personal API token (profile menu → *Profile, API tokens & AI*) and point any MCP client
at `https://your-wiki/api/mcp`:

```bash
claude mcp add --transport http wikicraft https://your-wiki/api/mcp --header "Authorization: Bearer wct_…"
```

Tools: `retrieve` (the passages that best answer a question, with URLs to cite), `search`, `query_pages` (CQL), `get_page` (Markdown), `list_spaces`, `list_children`,
`recent_changes`, `create_page`, `update_page`, `append_to_page`, `add_comment`. Every call runs
through the same authorization checks as the UI. Give an assistant that should only read a
**read-only** token (`{"scope": "read"}` when creating it): the writing tools then refuse.

## Ask the wiki

With a model set up, the search page offers *Ask the wiki*: the question goes through the
retrieval above (hybrid when embeddings are configured), the best passages are sent to the
model, and the answer comes back with numbered citations linking to the exact sections. Only
pages the asker can open are ever sent; page text is passed as data, not instructions. Questions
are rate limited per person (20 per 10 minutes).

The model can be Claude or any model behind an OpenAI-compatible chat API, also one running in
your own network, so that no page text leaves it:

```sh
# Claude
WIKICRAFT_AI_API_KEY=sk-ant-…

# a model in your own network (Ollama, vLLM, llama.cpp's server, LM Studio, LocalAI)
WIKICRAFT_AI_PROVIDER=openai-compatible
WIKICRAFT_AI_BASE_URL=http://ollama:11434/v1
WIKICRAFT_AI_MODEL=llama3.1

# another hosted service with an OpenAI-compatible API
WIKICRAFT_AI_PROVIDER=openai-compatible
WIKICRAFT_AI_BASE_URL=https://…/v1
WIKICRAFT_AI_MODEL=…
WIKICRAFT_AI_API_KEY=…
```

Up to 24,000 characters of excerpts go with each question, so give a local model room for them:
Ollama's default context can be shorter and cuts longer prompts without a word (e.g.
`OLLAMA_CONTEXT_LENGTH=16384`). Reasoning that some models write before their answer
(`<think>…</think>`) is left out. *Administration → Overview → System* shows which model answers
and whether it runs in your own network.

## License

Licensed under either of

- Apache License, Version 2.0 ([LICENSE-APACHE](LICENSE-APACHE))
- MIT license ([LICENSE-MIT](LICENSE-MIT))

at your option. Third-party components and their licenses are listed in
[THIRD-PARTY-LICENSES.md](THIRD-PARTY-LICENSES.md) (`scripts/third-party-licenses.py`; the Docker
image carries them in `/usr/share/doc/wikicraft`).

Unless you explicitly state otherwise, any contribution intentionally submitted for inclusion in
the work by you, as defined in the Apache-2.0 license, shall be dual licensed as above, without any
additional terms or conditions.
