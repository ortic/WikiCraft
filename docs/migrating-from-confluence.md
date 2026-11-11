# Leaving Confluence Data Center: migrating to WikiCraft

Confluence Data Center reaches end of life on **2029-03-28**, and Atlassian stopped selling new
Data Center licences on 2026-03-30. If you want to stay self-hosted, this guide takes you from a
Confluence export to a running WikiCraft with the same spaces, page trees, history, attachments,
comments, permissions and links.

It works the same for Confluence Server, Data Center and Cloud exports.

## 1. What comes across

| Confluence | WikiCraft | Notes |
|---|---|---|
| Spaces (key, name, description, home page) | Spaces | the key can be changed on import (`--space-key`); keys WikiCraft can't use are cleaned up, and a key that's taken gets a number |
| Personal spaces (`~username`, `~accountid` on Cloud) | Personal spaces | owned by the same person, with WikiCraft's `~NAME` keys (from their e-mail address, else the space name); someone who already has a personal space here gets the imported one as an ordinary space. Old `/display/~user/…` links keep working |
| Page tree and sibling order | Page tree | orphans go to the space root and are reported; pages without a position sort alphabetically, as in Confluence |
| Blog posts | Blog | |
| Page history | Versions | with the original authors and dates; skip with `--skip-history` |
| Current, archived pages | Pages, archive | archived pages land in Space settings → Archived |
| Unpublished pages (drafts) | Private drafts | only their author sees them, under *Home → Drafts*, until they publish them; skip with `--skip-drafts`. Unpublished edits to published pages are not imported (Confluence Cloud keeps one for every page edited in the new editor; the dry run counts them) |
| Deleted pages (trash) | Trash | with their attachments and comments, so admins can restore them; the trash retention period starts at the import; skip with `--skip-trash`. Pages that were below a deleted page move up to its parent, as in Confluence |
| Attachments (all versions) | Attachments | identical files are stored once |
| Page and inline comments, threads, resolved state | Comments | inline comments stay anchored to their text |
| Labels | Labels | personal (`my:`) labels are not imported, except favourites |
| Watches (pages, blog posts, spaces) | Watches | people keep getting notified about what they watched; daily digests and network follows are not watches |
| Favourites / saved for later | Starred pages and spaces | from the favourite relation and from older `my:favourite` labels |
| Tasks (with assignees and due dates) | Task lists | they show up under *My tasks* |
| Users referenced anywhere | Accounts or *imported authors* | see [step 5](#5-people-imported-authors) |
| Groups | Groups | |
| Space permissions | Space roles and permissions | `SETSPACEPERMISSIONS` → admin, `EDITSPACE` → editor, `COMMENT` → commenter, `VIEWSPACE` → viewer; what a user or group didn't have of the rest (delete pages, blog, files, comments, restrictions, export, archive, delete own) is left out of their grant. All signed-in users and the `confluence-users` group become the space's default role |
| Page restrictions (view / edit) | Page restrictions | view restrictions are inherited, as in Confluence; a restriction is never dropped silently (unknown subjects get an account) |
| Anonymous access | **off** | reported as `anonymousAccessInConfluence`; turn it on per space if you really want it |
| Links between pages, to attachments, to users | Rewritten | links to pages that are not in the export stay as "missing page" links and are resolved when the page arrives in a later import |
| Full URLs of the Confluence site in page text (`…/display/KEY/Title`, `…/viewpage.action?pageId=…`, `…/spaces/KEY/pages/…`, `…/x/…`, attachment downloads) | Internal links | for pages and files that are part of the import; a Cloud site's address can't be pointed at WikiCraft, so these would otherwise break after the move. Other URLs stay as they are |
| Smart links shown as a card or embed | Embeds | videos play in the page; inline smart links stay links |
| Old Confluence URLs | Redirects | `/display/KEY/Title`, `/pages/viewpage.action?pageId=…`, Cloud `/wiki/spaces/KEY/pages/ID/…` and tiny links `/x/AbCd` keep working, so bookmarks and links in tickets and chats survive |

### Macros

| Confluence macro | Becomes |
|---|---|
| info, tip, note, warning, panel | Callout |
| code, noformat | Code block with syntax highlighting |
| expand | Expand block |
| toc, toc-zone | Table of contents (levels, include/exclude, numbering, flat style) |
| children, pagetree, pagetreesearch | Child pages list (live; sort, reverse, first N, heading style; a search box for pagetreesearch) |
| livesearch | Search box (limited to the space / label it names) |
| create-from-template | A button that creates a page below the current one (Confluence's blueprints map to the built-in templates) |
| include, excerpt, excerpt-include | Included page / excerpt |
| status | Status lozenge |
| anchor | Anchor (links to `#anchor` keep working) |
| view-file, viewpdf, viewdoc, … | File block with preview |
| layouts and sections / columns | Columns |
| decision lists, `decisionreport` | Decisions and a live decision log |
| contentbylabel, recently-updated, detailssummary (page properties report), tasks-report-macro, attachments | Live report blocks, evaluated for each reader's permissions; a CQL query in contentbylabel runs as CQL (Search → *Query* takes the same language), and one using fields WikiCraft doesn't know falls back to its labels and spaces |
| details (page properties) | Kept, and read by the properties report |
| mermaid, plantuml | Code block: Mermaid is drawn as a diagram in the browser, PlantUML stays as source |
| html | the HTML as a code block (never executed) |
| math / LaTeX macros | Math (KaTeX) |
| widget, iframe | Embed (YouTube, Loom, Figma, Miro, Google Docs, …) or a link card |
| jira | the issue key as text (no live Jira embed yet) |
| unmigrated-wiki-markup | converted to normal content |
| emoticons | WikiCraft's own icon set |
| anything else | a *macro* block that keeps the macro's name, parameters and body, so nothing is lost and you can find it later |

The dry run (step 3) lists every macro of your export with how it will be handled.

## 2. Export from Confluence

You need a site admin (for a site export) or a space admin (for space exports).

**Data Center / Server (XML):**

1. *Space settings → Content tools → Export* (one space) or *Administration → Backup & Restore*
   (whole site).
2. Choose **XML**, *Full export*, and include comments and attachments.
3. Confluence writes a `.zip` with `entities.xml` and an `attachments/` folder.

**Cloud (CSV):** *Space settings → Export space → CSV* (or the site backup). WikiCraft reads both
the classic XML and the newer CSV format.

Big exports are fine: a 2,400-page space with history imports in about 20 seconds and 120 MB of
memory. Keep the zip on the WikiCraft server for very large exports and use the CLI (step 4).

## 3. Dry run

Before writing anything, let WikiCraft read the export and tell you what it found:

- **In the browser:** *Administration → Import → Import from Confluence*, pick the zip,
  **Analyze**. The zip goes up once, in 8 MB parts: a dropped connection is retried, *Stop* and
  *Continue the upload* pause it, and choosing the same file again after a reload carries on where
  it stopped. The dry run and the import then use that upload (multi-GB exports aren't sent twice;
  `WIKICRAFT_MAX_UPLOAD_MB` still caps the size). A reverse proxy in front needs to let 8 MB requests
  through (nginx: `client_max_body_size 16m;`).
- **On the server:** `wikicraft analyze-confluence export.zip > report.json`

The report counts spaces, pages, versions, attachments, comments and users, and lists links that
can't be resolved (usually pages that aren't part of the export), missing attachments, unknown
macros and pages with warnings. Nothing is written to the database.

## 4. Import

- **In the browser:** same dialog, **Import**. Options: a different space key, skip history,
  comments or attachments. The import runs in the background; its progress and final report are
  shown under *Administration → Import* (and `GET /api/jobs/{id}`).
- **On the server:**

  ```bash
  wikicraft import-confluence export.zip --as-user admin@example.com [--space-key ENG] [--skip-history] [--skip-drafts] [--skip-trash] [--update [--overwrite-local-changes]]
  ```

  `--as-user` is the WikiCraft admin the import runs as. The command prints the report when done.

You can import several spaces one after another. Links between them are resolved as each space
arrives.

### Trial run first, changes later

Teams usually import once to try WikiCraft while Confluence stays in use, then bring the changes
across at the switch-over. Export the space again and import it with **Update an earlier import**
(the dry run ticks it when it recognises the space; `--update` on the command line). Instead of a
second space, the import then:

- adds pages created in Confluence since, under the right parent;
- brings pages changed in Confluence up to date: their new Confluence versions are added to the
  history, links to blocks that didn't change keep working;
- leaves pages that were **also changed in WikiCraft** as they are and lists them, unless you
  choose *take Confluence's version* (`--overwrite-local-changes`; the WikiCraft edits stay in the
  history). Pages open in an editor at that moment are listed too;
- moves pages that were moved in Confluence, and moves pages deleted there to the trash;
- adds new comments, attachments and attachment versions, watches and favourites.

Space settings, permissions and the home page stay as your admins left them. Pages missing from
the new export (deleted for good in Confluence, or not part of a partial export) are counted in
the report, not deleted. Running the same update twice changes nothing.

## 5. People: imported authors

Confluence exports contain user ids and display names, usually without e-mail addresses. For
every author, commenter, assignee or permission holder WikiCraft:

1. uses an existing account that is already linked to that Confluence user, or has the same
   e-mail address, otherwise
2. creates an **imported author**: a placeholder without a login, so pages still show who wrote
   them.

Placeholders turn into real accounts by themselves when the person signs in with SSO, accepts an
invitation or is added with the same e-mail address. For the others (no e-mail in the export, or a
different address), go to *Administration → Users*, tick *Show imported authors without account*
and use **Merge into…** next to the placeholder. Everything the placeholder wrote, owned, liked,
watched or was granted moves to that person, mentions of them point to the person, and later
imports map the same Confluence account to them. The API call is
`POST /api/v1/users/{placeholderId}/merge {"intoUserId": "…"}`.

## 6. Sign-in

Connect the identity provider you used for Confluence (Entra ID, Okta, Keycloak, Google,
LDAP / Active Directory, SAML, …) **before** people start signing in, so their accounts claim the
imported authors automatically. See *Sign-in* in the [admin guide](admin-guide.md#sign-in).
Group sync (`syncGroups`) mirrors the provider's groups, which the imported space permissions can
then use.

## 7. Check and switch over

- [ ] Open a few large pages, pages with many macros and pages with restrictions; compare them
      with Confluence.
- [ ] Check the import report for unresolved links and unknown macros; search for `macro`
      blocks if you want to replace them with native blocks.
- [ ] Space permissions: *Space settings → Permissions*; page restrictions: the lock icon on a
      page (*Inspect permissions* shows why someone can or can't see a page).
- [ ] Point the old Confluence host name at WikiCraft (or add a redirect from it), so the
      legacy URL redirects take over.
- [ ] Export the spaces once more and run an update import, so the last changes made in
      Confluence come across.
- [ ] Make Confluence read-only for a transition period, then archive the export zips.

## Limits today

- No live migration over the Confluence REST API yet; you work from exports.
- Jira issue macros become plain issue keys.
- Third-party macros (marketplace apps) are kept as macro blocks, not rendered.

See `PLAN.md` §4.18 and §6 for what is planned and the data behind these numbers.
