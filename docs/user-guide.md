# WikiCraft user guide

A quick tour for everyone who reads and writes in WikiCraft. Press **`Ctrl /`** (`Cmd /` on a
Mac) in the app for the keyboard shortcuts.

## Finding your way

- **Home** shows the pages you visited recently, your starred pages, open tasks assigned to you
  and the activity: everything, what you watch, or what the people you follow did.
- **Spaces** hold the pages of a team or project, as a tree in the sidebar. Drag pages in the
  tree to reorder or nest them.
- **Search** (`Ctrl K` / `Cmd K` jumps straight to a page or runs an action: create a page or
  folder, open the trash, dark mode …; start with `>` for actions only) finds pages, blog posts
  and comments; filter by space, type, label, contributor and date, and save searches you repeat.
- **Star** pages and spaces you use often: they come first in the sidebar and the space
  directory.
- **Folders** group pages without content of their own; create one with the folder icon in the
  page tree.
- **My tasks** lists every task assigned to you, across spaces.

## Writing

New pages start as **private drafts**: only you see them (and pages you add below them) until
you press **Publish**; nobody is notified before that. Your drafts are listed on Home. Untick
*Start as a private draft* in the Create dialog to publish right away. **Schedule…** in the draft
banner publishes it by itself at a time you pick (up to a year ahead), as long as you can still
edit the page then.

Click into a page you may edit and start typing: there's no edit mode and no "save" button.
Everyone on the page edits together and sees each other's cursors; click a person's avatar to
jump to where they are.

- Type **`/`** for anything you can insert: headings, lists, task lists, tables, callouts (info,
  note, success, warning, error), expand blocks, code, Mermaid diagrams, drawings, images,
  files, embeds and link previews, table of contents, child pages, columns, status, dates, math,
  decisions and a decision log, task reports.
- Markdown shortcuts work as you type: `#`, `##`, `-`, `1.`, `[]`, `>`, ```` ``` ````, `---`.
- **`@`** mentions a person (they are notified) or, from two letters on, links a page.
- **`[[`** links a page (or creates it), `[[#` a heading on this page; attachments too.
- **`:`** followed by a word inserts an emoticon (`:tick`, `:warning`, …).
- Select text for the formatting bar: bold, italic, code, highlight, links, or a comment.
- Paste Markdown, a table from a spreadsheet or an image: it becomes formatted content.
- The handle left of a block moves the block; the link icon next to it copies a **link to that
  block**. Opening such a link scrolls to the block and highlights it.
- `Ctrl F` / `Ctrl H` find and replace within the page.
- **Tab** at the start of a paragraph or heading indents it (or `Ctrl ]`); **Shift Tab** (or
  `Ctrl [`) takes it back. The selection menu's text style button has indent buttons too.

**Diagrams:** `/drawing` opens a sketchpad (Excalidraw); `/draw.io diagram` opens draw.io for
flowcharts, architecture and network diagrams. Readers see a picture of it; editors click it to
change it. Diagrams imported from Confluence's draw.io app stay editable.

Tables have a toolbar for rows, columns, header rows, merged cells, cell colours, sorting and the
width (normal, wide, full). Images can be resized, aligned, captioned, annotated and opened full
size.

### Page properties

Properties are typed fields above the page body: *Status*, *Owner*, *Due date* and so on. **More
(…) → Add property** adds one; pick a type: text, select, number, date, person, web address or
page. A name already used in the space brings its type along, and select properties offer the
values used before. Click a property's name to change or remove it. Properties are searchable,
copied with the page, and part of exports (a table in HTML, front matter in Markdown).

`/properties` inserts a **properties report**: a table of the pages in this space and their
properties. Editors set its labels, columns and conditions with the filter button, for example
`Status = Done; Effort >= 3; Owner ~ Ann` (`=`, `!=`, `~` contains, and `<`, `>`, `<=`, `>=` for
numbers and dates). Confluence-style properties tables (a two-column table with the names in
header cells) count as properties too. Markdown files with front matter (`status: draft`,
`due: 2026-12-01`) get their keys as properties when imported, and so do the pages of a Notion
database (from its columns).

## Templates

*Create* offers blank pages and templates (meeting notes, decision record, how-to,
retrospective, runbook, product requirements …), plus your space's own templates and those for
every space. Grey instructional text in a template disappears when you start typing there.

Page menu → *Save as template* turns what a page holds now into a template for its space, with a
name and a description; workspace admins can offer it to every space instead. Templates are
renamed, described or deleted in *Space settings → Details → Templates* (their creator and space
admins) and *Administration → Templates* (the ones for every space).

Meeting notes, decisions, retrospectives, postmortems, status reports, how-tos, runbooks and
requirements made from *Create* go under an index page of their kind (for example *Meeting notes*,
made the first time, listing them newest first) and get a matching label; create them below a page
to put them there instead.

A space can have a template new pages start from (*Space settings → Details*), and so can a page
for the pages below it (page menu → *Template for pages below…*, e.g. *Incident postmortem* on an
*Incidents* page); the nearest one above wins. *Create* lists it first, and you can still pick
another one or a blank page.

Templates can ask questions: write `{{Customer}}` (any name in double braces) in the page you save
as a template, and *Create* asks for the customer before making the page, then puts the answer
everywhere `{{Customer}}` appears, with its formatting. What you leave empty stays as a grey hint
in the page. Text in code blocks and inline code is never a question. The built-in templates ask
for what their title needs (*Postmortem: {{Incident}}*, *How to {{Task}}*, …).

## History

Every editing session ends in a version. The clock icon shows all versions with what changed
(per block, changed words marked) and lets you restore an older one; restoring creates a new
version, nothing is lost. *Compared with* picks any other version; *Side by side* shows both next
to each other.
After you've been away, a banner offers *what changed since your last visit*, and the new and
changed parts of the page are tinted until you hide the highlights or dismiss the banner. Uploading a file with
the same name as an attachment adds a new version of it; the attachments panel lists the versions
and can make an earlier one current again.

## Comments and tasks

- Select text and choose the comment icon for an **inline comment**, or comment on the whole
  page in the comments panel. Reply, @mention, like, resolve and reopen; filter by open, resolved
  or your own threads. Comments take Markdown, and images: paste or drop them in the comment
  box, or use its image button (PNG, JPEG, GIF or WebP; they're stored with the page's
  attachments).
- In a task list, `@`-mention someone to assign the task and add a date for the due date. On
  that day (from 8:00 in the time zone of their profile) they get a reminder, in the bell and by
  e-mail like other notifications; mute it under Settings if you don't want it.

## Appearance

The moon icon switches between light and dark. *Settings → Appearance* also offers a high-contrast
theme (black text, strong borders, underlined links, a clear focus ring); with *System*, WikiCraft
follows the device, including its "more contrast" setting.

## People

*People* in the sidebar lists everybody with an account: search by name, job, department or
location, or pick a department. A profile shows what someone does, where they are and their local
time, their personal space and the pages they worked on lately (only the ones you can see). Click
an @mention (Ctrl/Cmd+click while editing) or an author's name to open their profile. **Follow**
on a profile puts their edits and comments in *Home → Activity → People I follow* (only on pages
you can see). Fill in
yours under *Settings → About you*, where you can also upload a picture (cropped to a square and
scaled down; photo metadata such as the location is not kept).

## Watching and notifications

You automatically watch pages you create, edit or are mentioned on (once you stop watching a
page, editing it doesn't sign you up again). Watch other pages, a page *and the pages below it*
(changes and new pages there), or whole spaces from the page menu, and mute a page you no longer
care about. Stopping to watch one page below a watched one leaves out just that page. The bell lists mentions, replies and
changes; e-mail summaries of what you missed come a few minutes after, hourly or daily (your
profile settings).

Fixing a typo? Switch on **Minor change** under the page title while you edit: when everybody who
edited in that session did, watchers aren't notified and the version is marked *minor* in the
history.

## Blogs

Every space has a blog (sidebar → *Blog*) for news, release notes and announcements; posts start as
drafts like pages. **Feed** on the blog page is an Atom feed of the latest posts for feed readers:
public spaces work as is, for others give the feed reader a read-only API token (Settings).

## Sharing

- **Share → Copy link**: a permanent link that survives renames and moves.
- **Public link** (if your admins allow it): readable without an account, optionally with the
  pages below it, an expiry date and a password (send it separately; a browser that entered it
  can read for 12 hours). Revoke it any time.
- **Restrictions** (lock icon): limit who can see or edit a page. View restrictions also apply
  to the pages below.
- **Inspect permissions** explains who can see a page and why.
- **Request access**: a page you can't open offers to ask its space admins. They get a
  notification with **Grant access** (you can then read it; restricted pages above it open for you
  too, and you're added to the space as a viewer if you couldn't see it) or **Dismiss**. You're
  notified when it's granted.

## Exporting

Page menu → *Export*: Word (.docx), Markdown, HTML or print / PDF. Space admins can export a whole
space as a browsable HTML site, a Markdown folder tree or a full backup.

## Keeping pages healthy

- **Owner**: who to ask about a page.
- **Verify** a page to mark it as accurate (optionally until a date); readers see when it was last
  verified, and you can ask someone to review it.
- **Archive** pages that are done but worth keeping; they leave the page tree and search but stay
  readable (Space settings → Archived).
- **Labels** group pages across spaces (suggestions show the labels already in use); click a
  label for all its pages.
- **Status** (page menu → *Set status…*): rough draft, in progress, ready for review, or your own.
- **Copy** or **move** a page (page menu), with the pages below it and its attachments, within the
  space or to another one.

## Spaces

Space admins manage a space in *Space settings → Details*: name, icon, description, category,
key, the template new pages start from, and its **owner**, the person responsible for it (hand
it over with *Change owner…*; the new owner becomes a space admin). **Sidebar shortcuts** pin links (a web address or a page,
label or search of this wiki) under the space name in the sidebar; the page menu's *Add to space
shortcuts* adds the page you're on. **Look** gives the space its own accent colour (instead of
the workspace's, while you're in the space) and can leave the blog out of its sidebar.
**Copy space…** makes a new
space with the published pages you can see, the settings, templates and (optionally) the
people of this one; links between its pages point at the copies. **Archive space** makes it
read-only and takes it out of search; workspace admins can restore it.

## Trash

Deleting a page moves it, with the pages below it, to the space's trash (*Space settings →
Trash*). Restore it from there; space admins can also delete pages for good.

## Your profile

Profile menu → *Profile, API tokens & AI*: name, password, two-factor authentication (an
authenticator app), active sessions, e-mail notifications and API tokens (for scripts and AI
assistants that work with the wiki on your behalf).
