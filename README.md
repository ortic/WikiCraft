# WikiCraft

A self-hosted team wiki with a **Rust backend**, a modern real-time editor, and first-class
**Confluence migration**. Built for teams that need a home after Confluence Data Center goes
end-of-life (2029-03-28).

- **Spaces & page trees**: unlimited nesting, drag-and-drop reordering, trash with restore, archive, copy, labels, favorites; space templates (team, project, knowledge base, engineering), space categories, personal spaces, space key rename (old links keep working), external links as page-tree items
- **Real-time collaboration**: Google-Docs-style co-editing with live cursors (Yjs CRDT; the server speaks the y-websocket protocol via `yrs`)
- **Rich editor**: slash commands, drag handles, tables (merge, colours, sort by column, normal / wide / full width), task lists, callouts with custom icons, expand blocks, code blocks with syntax highlighting and line numbers, math, mentions, status lozenges, dates, decisions with a live decision log, task reports, instructional text in templates, embeds and link previews, file previews, table of contents, child pages, includes, heading links, links to any block (every block has a stable id), find & replace
- **Drawings & images**: Excalidraw sketches stored in the page, image annotation (arrows, boxes, text), text wrap around images, lightbox
- **Paste anything**: Markdown becomes formatted content, spreadsheet cells become tables, images upload on paste or drop
- **Version history** with per-block diff and restore; automatic versions when editors leave; "changed since your last visit" banner
- **Page lifecycle**: owners, verification ("this page is accurate" with expiry), archive, request access
- **Comments**: page and inline comments, threads, resolve/reopen, mention notifications
- **Permissions**: workspace roles, space roles (viewer/commenter/editor/admin) for users and groups, page view/edit restrictions (view restrictions are inherited, like Confluence), anonymous access, guests
- **Search**: full-text (tantivy or Postgres) with snippets, permission-aware, filters (type, label, contributor, date), comments indexed, saved searches, browser search (OpenSearch)
- **Confluence import**: classic XML exports *and* the new Cloud CSV exports, with history, attachments, comments (incl. inline), labels, restrictions, authors, macro mapping, link rewriting and a dry-run report; imported authors without an account can later be merged into the right person
- **Markdown / Obsidian / Notion / HTML import**: zip of Markdown or HTML files (including Confluence's HTML space export) with images, attachments and links between pages
- **Word**: import .docx (headings, lists, tables, images, footnotes) and export pages or whole page trees to .docx
- **Export & backup**: single pages as Word/Markdown/HTML/PDF; whole spaces as a browsable static HTML site, a Markdown folder tree (re-importable) or a full backup that restores into a new space
- **Sign-in**: local accounts, OpenID Connect (Microsoft Entra ID, Google, GitLab, Okta, Keycloak, Authentik, …), GitHub, LDAP / Active Directory and SAML 2.0 — several at once, with just-in-time provisioning, domain rules, group sync and account linking
- **E-mail**: invitations, password reset, open sign-up for your e-mail domains (with confirmation) and notification digests over SMTP (outbox with retries)
- **AI-ready**: built-in [MCP](https://modelcontextprotocol.io) server — Claude & co. can search, read and write pages with the user's permissions; *Ask the wiki* answers questions from the pages a person can see, with sources (optional, needs an Anthropic API key)
- **Templates**: built-in (meeting notes, decision record, how-to, retrospective, runbook, PRD), custom space/workspace templates and a default template per space
- **Notifications**: mentions, comments, watched-page and watched-space updates (in-app and as e-mail digest), per-page muting; a "My tasks" view of task items assigned to you; webhooks with Slack, Microsoft Teams and Google Chat formats
- **Administration**: workspace name and logo, announcement banner, feature switches (public links, exports, Ask the wiki, personal spaces), audit log, usage overview
- **Migration-friendly URLs**: old Confluence links (`/display/KEY/Title`, `viewpage.action?pageId=`, Cloud `/wiki/spaces/…/pages/ID`, `/x/` tiny links) redirect to the imported pages
- **API**: REST + personal API tokens; audit log

Documentation: [user guide](docs/user-guide.md) · [admin guide](docs/admin-guide.md) ·
[leaving Confluence Data Center](docs/migrating-from-confluence.md)

## Quick start (Docker)

```bash
docker compose up -d
open http://localhost:8787     # first visit runs the setup wizard
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
  when the database is unreachable).
- **Metrics**: Prometheus text format at `GET /api/metrics` (HTTP responses, latency, WebSocket
  connections, live rooms, DB pool, jobs); protect with `WIKICRAFT_METRICS_TOKEN`.
- **Shutdown**: SIGTERM marks the pod not-ready, drains and flushes live documents.
- **Migrations** run automatically on startup and are serialized by a Postgres advisory lock, so
  rolling updates with several pods are safe.

## Development

Requirements: Rust ≥ 1.85, Node ≥ 20, PostgreSQL ≥ 15 (or Docker).

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

### Tests

```bash
cargo test                                # unit + integration tests (needs the dev Postgres)
./research/confluence-import/fetch_fixtures.sh   # optional: real Confluence exports for the import tests
cargo test -p wikicraft-confluence --release --test real_exports -- --nocapture
```

Integration tests create a throwaway database per test, boot the full app on an ephemeral port
and exercise it over HTTP and WebSockets (auth/CSRF, permissions, collaboration between two Yjs
peers, both Confluence export formats end-to-end, security regressions).

### Browser tests (end to end)

`e2e/` drives Chromium through one journey on a fresh database: setup, a space and a page with
rich content, two people editing live, comments and search, a public link, space export and
two-factor sign-in.

```bash
cd web && npm run build && cd ../e2e
npm ci && npx playwright install chromium
npx playwright test      # starts the server on :8799 with a fresh database (scripts/e2e-server.sh)
```

### CLI

```bash
wikicraft serve                                    # web server (default)
wikicraft migrate                                  # apply migrations
wikicraft analyze-confluence export.zip            # dry run: JSON report, nothing written
wikicraft import-confluence export.zip --as-user admin@example.com [--space-key KEY] [--skip-history]
wikicraft reindex                                  # rebuild the search index
```

## Configuration

All settings are flags or environment variables:

| Variable | Default | Meaning |
|---|---|---|
| `WIKICRAFT_DATABASE_URL` | `postgres://wikicraft:wikicraft@127.0.0.1:55432/wikicraft` | PostgreSQL |
| `WIKICRAFT_BIND` | `127.0.0.1:8787` | listen address |
| `WIKICRAFT_BASE_URL` | `http://127.0.0.1:8787` | public URL |
| `WIKICRAFT_DATA_DIR` | `./data` | search index, uploads, local attachment storage |
| `WIKICRAFT_STATIC_DIR` | `./web/dist` | built frontend; when it holds no build, the copy embedded in the binary is served |
| `WIKICRAFT_S3_BUCKET` / `WIKICRAFT_S3_ENDPOINT` | — | store attachments and uploads in S3/MinIO/R2/Ceph |
| `WIKICRAFT_S3_REGION` / `WIKICRAFT_S3_ACCESS_KEY_ID` / `WIKICRAFT_S3_SECRET_ACCESS_KEY` | `AWS_*` env | S3 credentials |
| `WIKICRAFT_SEARCH` | `tantivy` | `tantivy` (embedded) or `postgres` (shared, for several replicas) |
| `WIKICRAFT_CLUSTER` | `none` | `postgres` to run several replicas (live-edit relay) |
| `WIKICRAFT_LOG_FORMAT` | `text` | `json` for structured logs |
| `WIKICRAFT_METRICS_TOKEN` | — | bearer token for `/api/metrics` |
| `WIKICRAFT_MAX_UPLOAD_MB` | `2048` | max upload (attachments, imports) |
| `WIKICRAFT_SECURE_COOKIES` | `false` | set `true` behind HTTPS (an `https://` base URL also turns on HSTS) |
| `WIKICRAFT_SESSION_DAYS` | `30` | session lifetime |
| `WIKICRAFT_SESSION_IDLE_MINUTES` | `0` (off) | sign out sessions inactive this long |
| `WIKICRAFT_AUTH_PROVIDERS` / `_FILE` | — | sign-in providers (JSON list), see below |
| `WIKICRAFT_GOOGLE_CLIENT_ID` / `_SECRET` / `_ALLOWED_DOMAINS` | — | "Continue with Google" shortcut |
| `WIKICRAFT_MICROSOFT_CLIENT_ID` / `_SECRET` / `_TENANT` / `_ALLOWED_DOMAINS` | — | "Continue with Microsoft" (Entra ID) shortcut |
| `WIKICRAFT_OIDC_ISSUER` / `_CLIENT_ID` / `_CLIENT_SECRET` / `_NAME` / `_AUTO_CREATE` | — | single generic OIDC provider (redirect URI `{BASE_URL}/api/auth/oidc/callback`) |
| `WIKICRAFT_DISABLE_PASSWORD_LOGIN` | `false` | turn off local passwords (SSO / LDAP only) |
| `WIKICRAFT_SMTP_URL` | — | outgoing mail: `smtps://user:pass@host` (TLS), `smtp://user:pass@host:587?tls=required` (STARTTLS) or `smtp://host:25` (plain, local relays) |
| `WIKICRAFT_MAIL_FROM` | `WikiCraft <wikicraft@localhost>` | sender address |
| `WIKICRAFT_UNFURL_ALLOW_PRIVATE` | `false` | let link previews fetch private / loopback addresses (closed intranets only) |
| `WIKICRAFT_AI_API_KEY` | — | Anthropic API key: turns on *Ask the wiki* (off when unset; nothing leaves the server) |
| `WIKICRAFT_AI_MODEL` / `WIKICRAFT_AI_BASE_URL` | `claude-opus-5-5` / `https://api.anthropic.com` | model and API endpoint (e.g. a gateway) for *Ask the wiki* |

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
users can't sign in through any provider.

## Architecture

```
web/                React 19 + Tiptap 3 (ProseMirror) + Yjs, TanStack Query
crates/server       axum 0.8 HTTP API + WebSocket collaboration (yrs), sqlx/PostgreSQL,
                    tantivy search, object_store blobs, importers, CLI  → single binary
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
missing from it) — load it into Swagger UI, Redoc, Postman or a client generator.

| Area | Endpoints |
|---|---|
| Auth | `GET/POST /setup`, `POST /auth/login`, `POST /auth/logout`, `GET /me`, `GET/POST /me/tokens` |
| Spaces | `GET/POST /spaces`, `GET/PATCH/DELETE /spaces/{key}`, `GET /spaces/{key}/tree`, `GET/PUT /spaces/{key}/permissions`, `GET /spaces/{key}/trash` |
| Pages | `POST /pages`, `GET/PATCH/DELETE /pages/{id}`, `PUT /pages/{id}/content` (ProseMirror JSON or Markdown), `POST /pages/{id}/move`, `/copy`, `/restore`, `GET /pages/{id}/children`, `GET/POST /pages/{id}/versions`, `POST /pages/{id}/versions/{n}/restore`, `GET/PUT /pages/{id}/restrictions`, `GET/PUT /pages/{id}/labels`, `GET /pages/{id}/export?format=md\|html\|json`, `GET /pages/{id}/collab` (WebSocket) |
| Comments | `GET/POST /pages/{id}/comments`, `PATCH/DELETE /comments/{id}` |
| Attachments | `GET/POST /pages/{id}/attachments`, `GET /attachments/{id}/content`, `DELETE /attachments/{id}` |
| Discovery | `GET /search?q=&space=`, `GET /recent`, `GET /favorites`, `GET /labels/{name}`, `GET /resolve?space=&title=` / `?source=confluence:<id>` |
| Reports | `GET /reports/pages?labels=&spaces=&operator=all&sort=&max=`, `GET /reports/properties?labels=`, `GET /reports/tasks?spaces=&status=`, `GET /tasks` (mine) |
| Sharing | `GET/POST /pages/{id}/share-links`, `DELETE /share-links/{id}`, public: `GET /public/{token}[/pages/{id}]`, `GET /public/{token}/attachments/{id}` |
| Import / export | `POST /imports/confluence` (multipart `file`, `options`), `POST /imports/confluence/analyze`, `POST /imports/markdown`, `POST /imports/backup`, `POST /spaces/{key}/export {format: html\|markdown\|backup}`, `GET /jobs/{id}`, `GET /jobs/{id}/download` |
| Account | `POST /auth/forgot`, `POST /auth/reset`, `GET /auth/invite`, `POST /auth/accept-invite`, `GET/PUT /me/preferences`, `GET /auth/providers` |
| Admin | `GET/POST /users`, `POST /users/invite`, `PATCH /users/{id}`, `GET/POST /groups`, group members, `GET /admin/audit`, `GET /admin/stats`, `POST /admin/reindex`, `GET/PUT /admin/settings`, `GET /admin/mail`, `POST /admin/mail/test`, webhooks (below) |

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

## AI assistants (MCP)

Create a personal API token (profile menu → *Profile, API tokens & AI*) and point any MCP client
at `https://your-wiki/api/mcp`:

```bash
claude mcp add --transport http wikicraft https://your-wiki/api/mcp --header "Authorization: Bearer wct_…"
```

Tools: `search`, `get_page` (Markdown), `list_spaces`, `list_children`, `recent_changes`,
`create_page`, `update_page`, `append_to_page`, `add_comment`. Every call runs through the same
authorization checks as the UI. Give an assistant that should only read a **read-only** token
(`{"scope": "read"}` when creating it): the writing tools then refuse.

## Ask the wiki

With `WIKICRAFT_AI_API_KEY` set, the search page offers *Ask the wiki*: the question runs through
the normal, permission-filtered search, the most relevant passages of the best pages are sent to
Claude, and the answer comes back with numbered citations linking to those pages. Only pages the
asker can open are ever sent; page text is passed as data, not instructions. Questions are rate
limited per person (20 per 10 minutes).

## License

AGPL-3.0-or-later (see PLAN.md §7 for the licensing decision still to be made).
