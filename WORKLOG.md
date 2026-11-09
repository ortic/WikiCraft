# WikiCraft — Work log

Autonomous build session started 2026-11-09 (user asked for ~15 h of work following `PLAN.md`).
No git commits are made (user didn't ask). Dev Postgres: docker container `wikicraft-pg` on `127.0.0.1:55432`
(user/pass/db = `wikicraft`). Port 5432 and 5173 are taken by other tools on this machine.

## Status

| Area | State |
|---|---|
| `crates/doc-model` (PM JSON, Yjs ⇄ PM, Markdown, HTML, text) | OK 11 tests |
| `crates/confluence` (XML + CSV reader, storage → PM, import plan) | OK 10 unit + 2 real-export tests |
| `crates/server` (axum API, authz, yrs collab, tantivy, importer) | OK 3 unit + 8 integration tests |
| `web/` frontend | not started |

## Log
- Research + PLAN.md + Confluence dry-run analysis (research/confluence-import/)
- Cargo workspace (doc-model, confluence, server). yrs 0.28, quick-xml 0.42, comrak 0.59
- doc-model: y-prosemirror-compatible Yjs⇄PM conversion verified by roundtrip + concurrent-merge tests
- confluence crate: streaming entities.xml + Cloud CSV readers, storage→PM converter (50+ macros), wiki markup, planner
- Real-export verification (crates/confluence/tests/real_exports.rs): ToIP CSV 2,378 bodies, ModelOp XML 331 bodies:
  0 lenient parses, 0 structural errors, 0 Yjs round-trip mismatches; links 95.4% / 100%, attachments 99.6% / 100%
- Bugs found by real data: yrs default OffsetKind=Bytes (JS uses UTF-16) → `ydoc::new_doc()` uses Utf16;
  XML reference props need trimming; history versions lack `space`; links in history use former titles
- Cloud CSV specifics: content properties are CUSTOM rows (pageid=owner, title=name, body=JSON);
  inline comments carry `inline-serialized-highlights` ([[text,path,offset,len]]); mirror has no history bodies
- server: axum 0.8 + sqlx 0.9 (needs `sqlx::AssertSqlSafe` for format!-built SQL) + tantivy 0.27 + object_store 0.14
- collab: y-websocket protocol on yrs; own presence map (yrs Awareness is !Send); no-op updates ignored
  (a fresh client's sync-step-2 used to mark pages edited); persist every 2s; auto-version on last disconnect
- integration tests (crates/server/tests): fresh DB per test; api.rs (5), collab.rs (two Yjs peers over WS), confluence_import.rs (both real exports via HTTP)
- import perf: per-row autocommits made ToIP import take 3:44 → batched tx (25 pages) → 18.6 s, 121 MB RSS
- CLI: `wikicraft serve|migrate|analyze-confluence <zip>|import-confluence <zip> --as-user <email>|reindex`

## Security review findings (independent review agent, 2026-11-09) — status
1. [x] HIGH copy w/ children leaks restricted descendants (pages.rs copy) → filter via visible_page_ids, copy restrictions
2. [x] HIGH cross-space move exposes restricted descendants → require view on all descendants + source space admin
3. [x] HIGH stack overflow via deep nesting (markdown blockquotes `>`*20000, Yjs nested elements) → depth caps
4. [x] MED 2 GiB body limit on all routes incl. login/setup; argon2 unthrottled → 1 MiB JSON limit, rate limit login
5. [x] MED WS: 64 MiB messages, unbounded presence, spoofable client ids, no re-check after revoke/trash, persist into trashed page
6. [x] MED markdown zip import: bombs/alloc from declared size, file count; parentId not validated (also copy parentId) → breadcrumbs leak
7. [x] MED admin can reset owner/admin passwords; password change doesn't revoke sessions
8. [x] MED move doesn't reindex → stale space_id in search; filter_visible trusts index space
9. [x] MED importer drops unmappable restriction subjects (fail-open); allUsersSubject→anonymous_access needs checking
10. [x] LOW-MED trash list shows restricted titles; restore drops inherited restriction; trash affects invisible descendants
11. [◐ O(n²) fixed; entry caps/zip reopen remain (admin-only)] LOW confluence archive hardening (64 GiB entry cap, gz unbounded, O(n²) normalize, reopen zip per entry)
12. [x (URL schemes sanitized server-side too)] LOW guests list users/groups; group ops not ws-scoped; no server-side URL scheme check; space admin can enable anonymous
- Security fixes landed with regression tests (crates/server/tests/security.rs, 5 tests). Depth cap: MAX_DEPTH=40 because
  each doc level = 2 JSON levels and serde_json's recursion limit (128) made deep docs unreadable from Postgres.
- Imports never enable anonymous access automatically (reported as anonymousAccessInConfluence instead).
- Frontend (web/): React 19 + Tiptap 3.33 + y-websocket 3; verified in Chromium via Playwright: setup, space/page
  creation, slash menu, mentions, two-user live editing with carets (needed hex colors), UI Confluence import.
- Docker: multi-stage image builds (115 MB) and runs; docker-compose.yml with Postgres
- Features added after the core: legacy Confluence URL redirects (/display, viewpage.action, Cloud /wiki/spaces/…/pages/ID,
  /x/ tiny links), templates (6 built-in + custom), notifications (comment/mention/page_mention/page_updated) + watch,
  print/PDF stylesheet, MCP server (rmcp 3.9, /api/mcp, 9 tools, bearer token, tested over raw JSON-RPC),
  OIDC SSO (openidconnect 4.1; PKCE+nonce; JIT + link by verified email; tested against
  ghcr.io/navikt/mock-oauth2-server on 127.0.0.1:8099 — container `wc-mock-oidc`), settings page (profile, tokens, MCP config)
