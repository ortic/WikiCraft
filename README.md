# WikiCraft

A self-hosted team wiki with a **Rust backend**, a modern real-time editor, and first-class
**Confluence migration**. Built for teams that need a home after Confluence Data Center goes
end-of-life (2029-03-28).

- **Spaces & page trees**: unlimited nesting, drag-and-drop reordering, trash with restore, copy, labels, favorites
- **Real-time collaboration**: Google-Docs-style co-editing with live cursors (Yjs CRDT; the server speaks the y-websocket protocol via `yrs`)
- **Rich editor**: slash commands, tables, task lists, info/warning panels, expand blocks, code blocks with syntax highlighting, math, mentions, status lozenges, dates, embeds, file previews, table of contents, child pages, includes
- **Version history** with diff and restore; automatic versions when editors leave
- **Comments**: page and inline comments, threads, resolve/reopen, mention notifications
- **Permissions**: workspace roles, space roles (viewer/commenter/editor/admin) for users and groups, page view/edit restrictions (view restrictions are inherited, like Confluence), anonymous access, guests
- **Search**: full-text (tantivy) with snippets, permission-aware
- **Confluence import**: classic XML exports *and* the new Cloud CSV exports, with history, attachments, comments (incl. inline), labels, restrictions, authors, macro mapping, link rewriting and a dry-run report
- **Markdown / Obsidian import**: zip of Markdown files with images and `[[wikilinks]]`
- **Export**: Markdown, HTML, JSON
- **Single sign-on**: OpenID Connect (Keycloak, Entra ID, Okta, Google, Authentik, …) with just-in-time provisioning and account linking
- **AI-ready**: built-in [MCP](https://modelcontextprotocol.io) server — Claude & co. can search, read and write pages with the user's permissions
- **Templates**: built-in (meeting notes, decision record, how-to, retrospective, runbook, PRD) and custom space/workspace templates
- **Notifications**: mentions, comments, watched-page updates; watch/unwatch pages
- **Migration-friendly URLs**: old Confluence links (`/display/KEY/Title`, `viewpage.action?pageId=`, Cloud `/wiki/spaces/…/pages/ID`, `/x/` tiny links) redirect to the imported pages
- **API**: REST + personal API tokens; audit log

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
| `WIKICRAFT_STATIC_DIR` | `./web/dist` | built frontend |
| `WIKICRAFT_S3_BUCKET` / `WIKICRAFT_S3_ENDPOINT` | — | store attachments and uploads in S3/MinIO/R2/Ceph |
| `WIKICRAFT_S3_REGION` / `WIKICRAFT_S3_ACCESS_KEY_ID` / `WIKICRAFT_S3_SECRET_ACCESS_KEY` | `AWS_*` env | S3 credentials |
| `WIKICRAFT_SEARCH` | `tantivy` | `tantivy` (embedded) or `postgres` (shared, for several replicas) |
| `WIKICRAFT_CLUSTER` | `none` | `postgres` to run several replicas (live-edit relay) |
| `WIKICRAFT_LOG_FORMAT` | `text` | `json` for structured logs |
| `WIKICRAFT_METRICS_TOKEN` | — | bearer token for `/api/metrics` |
| `WIKICRAFT_MAX_UPLOAD_MB` | `2048` | max upload (attachments, imports) |
| `WIKICRAFT_SECURE_COOKIES` | `false` | set `true` behind HTTPS |
| `WIKICRAFT_SESSION_DAYS` | `30` | session lifetime |
| `WIKICRAFT_OIDC_ISSUER` | — | OpenID Connect issuer URL (enables SSO) |
| `WIKICRAFT_OIDC_CLIENT_ID` / `WIKICRAFT_OIDC_CLIENT_SECRET` | — | OIDC client; redirect URI is `{BASE_URL}/api/auth/oidc/callback` |
| `WIKICRAFT_OIDC_NAME` | `Single sign-on` | label of the SSO button |
| `WIKICRAFT_OIDC_AUTO_CREATE` | `true` | create accounts for unknown SSO users |
| `WIKICRAFT_DISABLE_PASSWORD_LOGIN` | `false` | SSO-only login |

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

All endpoints live under `/api`. Authenticate with the session cookie (browser; mutating requests
must send `x-wikicraft: 1`) or with a personal token: `Authorization: Bearer wct_…`
(create one via `POST /api/me/tokens`).

| Area | Endpoints |
|---|---|
| Auth | `GET/POST /setup`, `POST /auth/login`, `POST /auth/logout`, `GET /me`, `GET/POST /me/tokens` |
| Spaces | `GET/POST /spaces`, `GET/PATCH/DELETE /spaces/{key}`, `GET /spaces/{key}/tree`, `GET/PUT /spaces/{key}/permissions`, `GET /spaces/{key}/trash` |
| Pages | `POST /pages`, `GET/PATCH/DELETE /pages/{id}`, `PUT /pages/{id}/content` (ProseMirror JSON or Markdown), `POST /pages/{id}/move`, `/copy`, `/restore`, `GET /pages/{id}/children`, `GET/POST /pages/{id}/versions`, `POST /pages/{id}/versions/{n}/restore`, `GET/PUT /pages/{id}/restrictions`, `GET/PUT /pages/{id}/labels`, `GET /pages/{id}/export?format=md\|html\|json`, `GET /pages/{id}/collab` (WebSocket) |
| Comments | `GET/POST /pages/{id}/comments`, `PATCH/DELETE /comments/{id}` |
| Attachments | `GET/POST /pages/{id}/attachments`, `GET /attachments/{id}/content`, `DELETE /attachments/{id}` |
| Discovery | `GET /search?q=&space=`, `GET /recent`, `GET /favorites`, `GET /labels/{name}`, `GET /resolve?space=&title=` / `?source=confluence:<id>` |
| Import | `POST /imports/confluence` (multipart `file`, `options`), `POST /imports/confluence/analyze`, `POST /imports/markdown`, `GET /jobs/{id}` |
| Admin | `GET/POST /users`, `PATCH /users/{id}`, `GET/POST /groups`, group members, `GET /admin/audit`, `GET /admin/stats`, `POST /admin/reindex` |

## AI assistants (MCP)

Create a personal API token (profile menu → *Profile, API tokens & AI*) and point any MCP client
at `https://your-wiki/api/mcp`:

```bash
claude mcp add --transport http wikicraft https://your-wiki/api/mcp --header "Authorization: Bearer wct_…"
```

Tools: `search`, `get_page` (Markdown), `list_spaces`, `list_children`, `recent_changes`,
`create_page`, `update_page`, `append_to_page`, `add_comment`. Every call runs through the same
authorization checks as the UI.

## License

AGPL-3.0-or-later (see PLAN.md §7 for the licensing decision still to be made).
