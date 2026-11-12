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
- Delegated to subagents (user request at ~04:27 UTC, each in its own git worktree, merged after review):
  PLAN.md audit of the 31 open P0 lines (16 ticked with evidence, 12 partial; found that archived Confluence pages
  were imported as live pages → fixed in `53c565b`); HTML / Confluence HTML export import (`80eab4a`); accessibility
  (axe checks in the e2e suite, contrast/labels/avatar colours fixed, `d01692f`, `7895b6a`); property tests for the
  doc model (`fa33950`: 9 properties, 7 Markdown export bugs fixed, 2 left documented under an ignored property).
- GitHub Actions stopped starting jobs at ~04:49 UTC: "recent account payments have failed or your spending limit
  needs to be increased" (billing setting on the GitHub account). Later commits were verified locally only.
- Morning (2026-11-10, after the PC restart): verified the merged state `fee1481` (166 Rust tests, 11 e2e tests
  green). Integration tests never dropped their databases (728 left, 7.5 GB): names now carry a timestamp and the
  first test of each binary drops those older than two hours. Stable block ids (P0): `blockId` on every block,
  assigned by the editor (Tiptap UniqueID) and by the server (create, import, restore, and old pages when first
  opened, without counting as an edit), kept for unchanged blocks when content is replaced via the API; copy link to
  a block, `#block-<id>` deep links, anchors in shared pages and exports. Single binary (web app embedded in
  release builds, immutable caching of hashed assets) and HSTS on https. `@` suggests pages. Table of contents
  options (levels, include/exclude, numbering, one line) shared by editor, shared pages, HTML and Word export.
  Merge an imported placeholder user into a real account (generic over all foreign keys, mentions rewritten in
  JSON and Yjs, live rooms updated via a new "system change" that is saved without counting as an edit).
  Docs: user guide, admin guide, "Leaving Confluence Data Center" (docs/). Golden snapshot tests (insta) for the
  Confluence converter and Markdown import, which found six conversion bugs (all fixed, see PLAN §4.28).
  Document format version (pages.schema_version) with a background upgrade of old pages. Page status picker, copy
  dialog, comment filters, label suggestions. Trash: delete forever, empty trash, retention period (unused files
  are removed from storage; a test caught that content-addressed files need their hashed path). Folders (view +
  create), starred spaces, command palette actions. The axe check caught 22 px buttons side by side (WCAG 2.2
  target size). Attachment version history; notification e-mails a few minutes after / hourly / daily. "Who viewed"
  (opt-in). Unpublished drafts: hidden from everyone but the creator in the central access checks (page_access,
  visible_page_ids, filter_visible), notifications and webhooks held back until Publish. Read-only API tokens
  (REST mutating requests refused, MCP write tools refuse; read-like POSTs such as Markdown conversion allowed).
  Template previews in the Create dialog; found and fixed: templates of a space you can't see were listed and could
  be used to create pages (their content leaked).
- 2026-11-10 (after the second PC restart): committed the work the restart interrupted (drafts, read-only tokens,
  required 2FA, who viewed, auto-watch on edit; verified first). Confluence content statuses: drafts → private drafts,
  trash → trash (with attachments and comments), and a bug found on the real exports: Cloud's shared draft of every
  published page (48 in the ModelOp XML) was imported as an extra old version carrying the current content.
- XWiki's Confluence importer test corpus (LGPL, downloaded at a pinned commit, required in CI): all 86 exports
  convert cleanly; it found 5 bugs (plain tables named `.csv.gz`, Cloud backup attachment files named by
  `FILESTORE_ID`, untyped wiki-markup bodies imported empty, unresolved wiki-markup links and images, duplicate
  history versions) and verified the CSV tables we had only guessed. Confluence watches and favourites imported.
- Template variables (`{{Name}}`, asked for in the Create dialog), minor changes (no watcher notifications), watching
  a page and the pages below. The axe check caught 3:1 contrast on the tree's draft badge.
- Confluence update imports (trial import now, changes across at cutover; conflicts reported, idempotent);
  space permission subjects as current exports write them (`all-licensed-users` was read as anonymous); full URLs
  of the old site become internal links (2,334 in the ToIP export: a Cloud address can't be redirected); smart
  link embeds. People directory and profiles. A PPTX overview deck was made on request (scratchpad, not in the repo).
- Confluence page bodies on disk instead of in memory (synthetic 317 MB export: 361 MB → 71 MB peak); scheduled
  publishing; blueprints (index pages for meeting notes etc.); profile pictures; draw.io diagrams (embed mode,
  Confluence draw.io macros import editable; e2e test with a stand-in for diagrams.net). The overview deck is in
  docs/presentation/.
- Editor: text colour / highlight / alignment menu, three-column and sidebar layouts (imported Confluence section
  types now render), image borders. Task reminders on the due date (assignee's time zone). Password-protected
  public links. Atom feeds for blogs. A full gentle run of the Rust suite (`cargo test -j 4 --workspace --
  --test-threads 2`) passed; it also caught nothing new, but a filtered run had missed a test the draw.io commit broke.
- Note: running the whole test suite saturates this WSL machine (load ~100); a parallel demo server then hits DB
  pool timeouts, and the mail test's 4-second wait can flake. Both pass when the machine is idle.
- 2026-11-10 (day): live Confluence macros (children options, search box, create-page button); history compares any
  two versions word by word; high-contrast theme (axe AAA check); copy/move pages across spaces with attachments.
- Page properties (typed: text, select, number, date, person, web address, page link) in a panel above the body;
  properties report with `where` conditions, `/properties` slash command, Confluence properties tables still
  count; searchable, copied, exported (HTML table, Markdown front matter), backed up; front matter and Notion
  database columns import as properties (types inferred per column; Notion's `Key: Value` lines removed).
- Indentation of paragraphs and headings (Tab at the start / Ctrl+], Shift+Tab / Ctrl+[; Confluence margin-left and
  Word w:ind import; HTML/Word export). Pitfall: right after Home, ProseMirror's selection lags the DOM caret, so
  the Tab handler reads the DOM selection.
- Jobs: cancel (queued at once, running at the next progress update via `jobs::checkpoint`) and retry (uploads kept
  7 days; a retried Confluence import runs as an update import = resume).
- Attachment rules (largest file ≤ server cap, blocked/allowed extensions, storage per space and per wiki counting
  all versions; also applied to files in Markdown/HTML/Notion imports, refused ones reported). Space owners with
  handover (owner becomes space admin, notified; Admin → Spaces flags owners whose account is gone). Whole-space
  copy (settings, templates, people, visible published pages; links/attachments/page-link properties point at the
  copies; page copy now shares `copy_tree`). Template for pages below a page (nearest ancestor wins, then the space).
- Space sidebar shortcuts; request access answered from the notification (Grant: view entries on the page and the
  restricted pages above it, space viewer if needed); changed blocks since the last visit tinted in the page (block-id
  diff; pitfall: decorations must be recomputed, not mapped, because the Yjs sync replaces the document); task report
  filters (assignees incl. me and imported Confluence user keys, due range); page exports audited; Admin → System;
  ClamAV scanning (clamd INSTREAM; uploads, imports, restores; fail-closed unless WIKICRAFT_CLAMAV_FAIL_OPEN; tested
  with a stand-in clamd only); Word import split by heading; numbered table rows.
- 2026-11-11 (evening): link graph (`page_links`, `crate::links`): links, included pages, page-link properties and
  child-page listings per page, refreshed with the search index (like the RAG passages) and backfilled at startup
  (`retrieval_state.linker` version). "Linked from" below pages (incl. `/missing?…` red links that now resolve),
  a warning before trashing a linked page, and Space settings → Links (broken links: trashed / deleted / missing /
  not imported; orphaned pages). Verified in Chromium against a seeded workspace.
- Readable addresses `/wiki/<KEY>/<slug>` (migration 0055: `wc_slug()`, `wc_resolve()`, `wc_space_by_key()` in SQL,
  `page_paths` kept by a trigger on title/space changes, backfilled from version titles). The address bar switches
  to it (before the editor mounts, so the collaboration socket opens once); `/resolve`, Confluence `/display` links
  and the link graph share the resolution. Found on the way: the drag handle and table menu only rendered after an
  unrelated re-render once the editor turned editable (the e2e journey caught it): editability is now React state.
- Search quality (tantivy): CJK bigram tokenizer, ASCII folding, stemmed title/body fields in the workspace language,
  typo tolerance for unknown words, prefix on the last word; `wikicraft-index.json` marks a complete index (format +
  language), so an index built differently (old installs, language change) is rebuilt in the background. Before, a
  recreated index stayed empty until someone pressed "Rebuild index".
- Whole-installation backup / restore CLI (`crate::backup`): one zip with a snapshot of every table (binary COPY in
  a repeatable-read transaction) and the stored files; restore into an empty database runs the backup's migrations,
  loads without foreign keys (spaces ⇄ pages cycle), re-adds them, resets sequences, upgrades. Tested end to end
  (restored copy served by a second app: login, pages, attachment bytes, comments, former addresses, search).
- Image renditions (`crate::renditions`): `?w=` copies cached in storage; the client picks the width from the measured
  column × devicePixelRatio and uses a plain `src` (a `srcset` with width descriptors would blow small images up).
- CI flake (`semantic_retrieval_with_embeddings`, 2 of 3 runs): the tantivy reader only reloads when its 500 ms
  file watch notices a commit; on CI the whole test ran in 1.2 s, so keyword search still saw an empty index. The
  indexer now reloads the reader right after each commit (searches see a saved page at once).
- Uploads in parts (`routes/uploads.rs`, migration 0056): 8 MB parts in file storage, in order, idempotent resend,
  resumable (localStorage remembers file → upload); Confluence dry run and import read the same upload. Tested with
  a synthetic two-part export and in Chromium (stop mid-part, continue, reload + same file = no parts resent).
- Passkeys (`crate::webauthn` + `routes/passkeys.rs`, migration 0057): pure-Rust verification (ring + ciborium, both
  already in the tree), software authenticator for tests; second step, passwordless sign-in, required-2FA, admin
  reset. Verified with Chrome's virtual authenticator (base URL must be `localhost`, not an IP). Also found that
  `openapi.yaml` is generated by `scripts/gen-openapi.py`: this session's earlier hand edits moved into the script.
- Embeds: Airtable, Typeform, Google Drive files, CodePen; GitHub gists through `/api/unfurl/gist` (GitHub API via the
  preview fetcher, shown as code). File downloads answer single byte ranges; attachment blocks play video and audio.
- Morning (2026-11-12, after a PC restart): finished and committed security review 5 (`e886144`), then: included
  pages render the other page read-only with the page's own extensions (`web/src/editor/extensions.ts`, shared by the
  editor), excerpt blocks (Confluence `excerpt` → `excerpt` node with name / hidden), loop guard via an include chain
  option; issue chips (`routes/issues.rs`, `GET /api/unfurl/issue`: GitHub (+ Enterprise), GitLab, Jira Cloud / DC;
  admin-configured trackers may be private hosts and get credentials, never across a redirect); a fixed formatting
  toolbar; exports fill includes per reader (`crate::includes`, backups keep references); readers sort tables (a
  sorted copy beside the table: changing the editor's own cells counts as an edit and loops) and sticky header rows
  for tables that fit.
- Bug found by a new Chromium + Firefox e2e test: concurrent typing in one paragraph scrambled the letters of whoever
  typed further along ("beta" → "ateb"), in all browsers, at any speed. `@tiptap/y-tiptap` 3.2 / 3.3 overrides the
  Yjs cursor position with the old offset whenever the paragraph's text changed. Fixed with a Vite transform
  (`web/scripts/fix-y-tiptap.ts`, fails the build if the code changes). Remaining y-tiptap quirk: a remote block type
  change (paragraph → heading) moves the typist's next words into a new paragraph.

## Security review 5 (2026-11-11/12: readable addresses, link graph, uploads in parts, passkeys) — status
No authorization bypass on page content. Fixed (with regression tests in `security.rs`, `mfa.rs`, `confluence_import.rs`):
1. [x] MED old Confluence addresses (`/display`, `viewpage.action`, `/x/` tiny links, Cloud `/wiki/spaces/…/pages/ID`)
   and `GET /api/resolve` told restricted pages apart from missing ones, and a restricted page with the same title
   hid the page the reader may open → `wc_resolve_candidates()` + `paths::resolve_for`: the first page the caller
   may open; visitors go to the sign-in page (then back), signed-in people to "doesn't exist" either way
2. [x] MED link report: a link by name to a restricted page wasn't listed as broken (an existence oracle for any
   title) → judged per reader: a page they can't open counts as missing
3. [x] MED API tokens could add or remove passkeys and start TOTP enrolment → `SessionUser` (browser sessions only)
4. [x] MED passkey removal and TOTP setup next to a passkey needed only a session → recent sign-in (15 min) or the
   password (`auth::confirm_recent`, wrong passwords rate-limited per person); the recent-session check now also
   requires the session to be the person's own
5. [x] MED with `WIKICRAFT_DISABLE_PASSWORD_LOGIN` a passkey alone still signed in (people removed in the IdP kept
   access) → passkeys only as the second step then; `/auth/providers` says `passkeyLogin`
6. [x] LOW drafts' former addresses resolved (`page_paths`) → drafts have no address at all
7. [x] LOW two requests for the same upload both started an import → `uploads.importing` claimed atomically (409)
8. [x] LOW unbounded passkeys per person (20 max) and RSA keys of any size (2048–8192 bits); `auth_states` pruned
   by an index on `expires_at`

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
