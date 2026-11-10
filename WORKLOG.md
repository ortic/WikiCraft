# WikiCraft — Work log

Autonomous build session started 2026-11-09 (user asked for ~15 h of work following `PLAN.md`).
Since the user's request every change is committed and pushed to `origin/master` (github.com/ortic/WikiCraft). Dev Postgres: docker container `wikicraft-pg` on `127.0.0.1:55432`
(user/pass/db = `wikicraft`). Port 5432 and 5173 are taken by other tools on this machine.

## Status (2026-11-10, ~04:15 UTC)

| Area | State |
|---|---|
| `crates/doc-model` (PM JSON, Yjs ⇄ PM, Markdown, HTML, text, SVG icon set) | OK, 17 unit tests |
| `crates/confluence` (XML + CSV reader, storage → PM, import plan) | OK, 12 unit + 2 real-export tests |
| `crates/server` (API, authz, collab, search, SSO, mail, webhooks, export, MFA, Word, AI, …) | OK, ~30 unit + 28 integration test files |
| `web/` frontend (React 19 + Tiptap 3) | OK, verified in Chromium via Playwright |
| Docker image (SAML on, healthcheck), compose (single + 2-replica), Kubernetes manifests | OK |

Local test services: Postgres `wikicraft-pg` :55432, mock OIDC `wc-mock-oidc` :8099, lldap `wc-lldap` :3890
(user `dana`/`dana-secret-1`), SAML IdP `wc-saml-idp` :8180 (signature.algorithm switched to rsa-sha256),
mailpit `wc-mailpit` SMTP :11025 / API :18025, SAML build container `wc-samlbuild` (libxmlsec1).

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

- Evening (2026-11-09): scaled deployment (k8s, 2-replica compose verified with live co-editing), `wikicraft
  healthcheck`; sign-in providers (multi OIDC: Microsoft/Google/GitLab/any issuer, GitHub, LDAP/AD, SAML via
  samael+xmlsec, verified against lldap and SimpleSAMLphp), login-CSRF binding; My tasks; space export (HTML site,
  Markdown, backup) + restore; e-mail (outbox, invitations, password reset, notification digests); own SVG icon set
  replacing all emoji (user request); table toolbar, image resize/align/caption/alt; live Confluence report macros
  (contentbylabel, page properties report, tasks report, recently updated, attachments); public share links;
  webhooks; TOTP 2FA + session management; inspect permissions; `[[` page links, `:emoticon`, shortcuts dialog;
  Notion import + import UI; OpenAPI 3.1 spec with coverage test; idle session timeout; PLAN.md audited
  (subagent) and kept in sync.
- Night (2026-11-09/10, user asleep): security review 2 (14 findings, all fixed); e2e Playwright suite + CI job
  (8 tests); likes; space lifecycle (archive / delete); @mentions in comments; 12 built-in templates; page
  icons + covers; CI disk fix; Word export (docx-rs; report macros evaluated per reader; icons rasterised with
  resvg, SVG file access disabled) checked by rendering through LibreOffice; Word import (styles/outline levels,
  numbering incl. LibreOffice's split numIds, merged cells, images, fields, footnotes, callout round trip; pandoc
  + LibreOffice fixtures); find & replace (Ctrl+F/H); image lightbox; page verification + review requests +
  report; request access; OpenSearch; Slack/Teams/Google Chat webhook formats; "changed since your last visit";
  Ask the wiki (Claude Messages API over raw HTTP, permission-filtered retrieval, mock-API tests).
- Night, second half (2026-11-10 00:40–04:15 UTC): space key rename with aliases; Word export of a page tree;
  per-block history diff; phone layout fixes (+ e2e phone test); archive pages; popular pages; space templates
  (team / project / knowledge base / engineering); search filters + comment search + comment editing; code block
  line numbers / wrap; Markdown paste; open sign-up for e-mail domains (confirmation e-mail, no account rows
  until confirmed); personal spaces; drag handles; `/api/v1`; open tasks on Home; image text wrap; callout
  type / icon menu; `[[#heading` and attachment links; notification muting; saved searches; security review 3
  (all findings fixed); watch a space; default template per space; page owners; jump to a collaborator;
  Excalidraw drawings (fonts self-hosted, PNG preview attachment for readers / exports); image annotation;
  link previews (server-side OpenGraph fetch with the webhook SSRF guards, cached); announcement banner;
  feature switches (public links, exports, Ask the wiki, personal spaces); space categories; spreadsheet
  (tab-separated) paste; table sort by column; instructional text (template hints, Confluence `ac:placeholder`);
  decision log (`decisionreport`) + `/decision`, `/decision log`, `/task report`; workspace name and logo; activity feed;
  security review 4 (5 findings, all fixed); table width modes (normal / wide / full, Confluence `data-layout`); external links in the page tree.
- Bug found by CI (04:20): the automatic version on the last disconnect was skipped whenever the 2-second background
  save had already stored the content, so history could miss whole editing sessions. Fixed in `4aaac2f`; the collab test
  now runs the background save before everyone leaves and fails without the fix.
- Note: running the whole test suite saturates this WSL machine (load ~100); a parallel demo server then hits DB
  pool timeouts, and the mail test's 4-second wait can flake. Both pass when the machine is idle.

## Security review 4 (independent review agent, 2026-11-10 ~03:40 UTC: link previews, banner, switches, decision log, branding) — status
No critical or high findings. All five fixed in `9f0b9a9`:
1. Medium (only with `WIKICRAFT_WEBHOOK_ALLOW_PRIVATE=true`): link previews shared the webhook private-network flag → own
   `WIKICRAFT_UNFURL_ALLOW_PRIVATE` (default off).
2. Low: preview errors disclosed resolved internal addresses / acted as a port oracle → generic "preview unavailable".
3. Low–Medium (DoS): decision log scanned and loaded up to 2000 full documents → space required, 300 pages max,
   only decision items selected in SQL.
4. Low: export downloads didn't re-check the exports switch → checked; admin hint says the switch doesn't stop reading.
5. Low: link preview cache never pruned → batch prune of expired rows (index on `fetched_at`).
Note (not a vulnerability): instructional text is hidden from readers by the UI and exports, but is part of the page JSON.

## Security review 3 (independent review agent, 2026-11-10 night: Word import/export, AI, sign-up, archive, …) — status
No authorization bypass found; all new read paths check visibility. Fixed:
1. [x] HIGH docx import: repeated references to a huge footnote multiplied its text (OOM with one ~1 MB upload)
   → footnotes listed once, capped at 20k chars; 16 MB text budget per document; XML parts capped at 24 MB
2. [x] MED Word export amplification (same image via `?1`, `?2`; unbounded occurrences; macros × page tree)
   → images keyed by attachment id and loaded once per export (≤100, ≤150 MB); ≤400 embeddings / 300 MB;
   ≤3,000 icons; 40 report macros per export; two exports at a time; page-tree export needs sign-in
3. [x] MED prompt injection could exfiltrate restricted text via images in AI answers → images dropped, only
   links into the wiki kept; closing tags in page text neutralised in any case
4. [x] MED docx import: media across a zip of Word files unbounded; O(n³) name collisions → 512 MB per
   import, 500 images per document, HashSet naming
5. [x] LOW open sign-up: junk user rows, mail spam, could cancel an admin's invitation → requests live in
   `signup_requests` (no user until confirmed, invitations untouched), 200 per hour overall
6. [x] LOW Teams cards: Markdown in names/titles (phishing links) → escaped
7. [x] LOW review requests revealed who may edit and allowed spam → same answer, 20 per hour

## Security review 2 (independent review agent, 2026-11-09 evening) — status
1. [x] HIGH stored XSS: status colour unescaped in server-rendered HTML (public share pages, exports) → allow-list + CSP
2. [x] HIGH TOTP brute force via fresh challenges / racy attempt counter → per-user limit, atomic counter
3. [x] MED SSO logins skipped TOTP → challenge cookie + code step for OIDC/GitHub/SAML
4. [x] MED password reset signed in without TOTP → hands over to the code step
5. [x] MED SSO e-mail linking trusted unverified addresses (fail-open) → per-provider `trustEmail`, admins need verified e-mail
6. [x] MED group sync took over existing groups by name; LIKE wildcard in prefix → own groups only, exact prefix
7. [x] MED backup restore exposed restricted pages while running / after failure → closed space, cleanup on failure
8. [x] LOW open redirect via tab/backslash in `next` → strict filter (server + client)
9. [x] LOW TOTP re-enrolment with only a session → refused while active
10. [x] LOW memory DoS through imports → semaphore, lower caps
11. [x] LOW reports queried all pages before filtering → visible spaces in SQL, content only when needed
12. [x] LOW e-mail leaks (inspect, HTML exports) → names only
13. [x] LOW webhooks: system proxy, NAT64/6to4, DNS timeout, tx across HTTP → fixed (payloads still carry titles of
    restricted pages; hooks are admin-configured — documented)
14. [x] LOW API tokens survived password/MFA reset → revoked

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
