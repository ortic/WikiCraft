# WikiCraft admin guide

For the people who install and run WikiCraft. The full list of settings is in the
[README](../README.md#configuration); this guide explains how the pieces fit together.

## Install

WikiCraft is one program plus PostgreSQL (15 or newer).

**Docker Compose** (simplest):

```bash
docker compose up -d          # WikiCraft on :8787 and PostgreSQL
```

**Single binary**: release builds carry the web app inside them.

```bash
export WIKICRAFT_DATABASE_URL=postgres://wikicraft:secret@db.internal:5432/wikicraft
export WIKICRAFT_BASE_URL=https://wiki.example.com
export WIKICRAFT_DATA_DIR=/var/lib/wikicraft
./wikicraft serve
```

**Kubernetes / several replicas**: see [`deploy/kubernetes`](../deploy/kubernetes) and
*Deployment & scaling* in the README (S3 for attachments, `WIKICRAFT_SEARCH=postgres`,
`WIKICRAFT_CLUSTER=postgres`; no sticky sessions needed).

Database migrations run on startup (serialized by an advisory lock, so rolling updates are safe).

### Behind a reverse proxy

Terminate TLS at your proxy (nginx, Traefik, Caddy, an ingress) and forward to port 8787,
**including WebSocket upgrades** for `/api/pages/*/collab` (live editing). Then set:

- `WIKICRAFT_BASE_URL=https://wiki.example.com`: used in links, e-mails, SSO redirects; with an
  `https://` URL WikiCraft also sends `Strict-Transport-Security`.
- `WIKICRAFT_SECURE_COOKIES=true`.

Example for nginx:

```nginx
location / {
    proxy_pass http://127.0.0.1:8787;
    proxy_http_version 1.1;
    proxy_set_header Upgrade $http_upgrade;
    proxy_set_header Connection $connection_upgrade;   # map $http_upgrade → upgrade/close
    proxy_set_header Host $host;
    proxy_set_header X-Forwarded-Proto $scheme;
    client_max_body_size 2g;                            # attachment and import uploads
}
```

## First start

The first visit shows the setup wizard: workspace name and the **owner** account. Everything
else is configured afterwards:

- *Administration → Overview*: workspace name and logo, announcement banner, sharing and feature
  switches (public links, exports, *Ask the wiki*, personal spaces), how long trashed pages are
  kept, e-mail status and a test message, usage numbers.
- *Administration → Users / Groups*: people and groups.
- *Administration → Import*: Confluence, Word, Markdown, HTML, Notion and Obsidian imports, space
  backups.

## People and access

**Workspace roles**: *owner* (one, can't be demoted), *admin* (administration), *member*
(normal use, can create spaces), *guest* (sees only spaces they are explicitly added to, no
member directory).

**Space roles**, per user or group: *viewer*, *commenter*, *editor*, *admin*. Each space has a
default role for all members (or *none* for a private space) and optional anonymous read access.

**Page restrictions** narrow this down per page: *view* restrictions apply to the page and
everything below it (as in Confluence), *edit* restrictions to the page itself. *Inspect
permissions* on a page explains who can see or edit it and why.

Adding people:

- *Invite* sends an e-mail with a link (needs outgoing mail), or copies the link for you.
- *Add with password* creates the account directly.
- *Open sign-up* (Administration → Users) lets anyone with an address at your domains create an
  account after confirming their e-mail.
- With single sign-on, accounts are created on first login (see below).

Deactivating someone signs them out everywhere and blocks every sign-in method. *Reset 2FA*
helps people who lost their authenticator. Imported Confluence authors without an account show
up when you tick *Show imported authors*; merge them into the right person with *Merge into…*
(see the [migration guide](migrating-from-confluence.md#5-people-imported-authors)).

## Sign-in

Local passwords work out of the box (turn them off with `WIKICRAFT_DISABLE_PASSWORD_LOGIN=true`
once SSO works). Add any number of providers through `WIKICRAFT_AUTH_PROVIDERS` (JSON, or a file
via `WIKICRAFT_AUTH_PROVIDERS_FILE`): OpenID Connect (Microsoft Entra ID, Google, GitLab, Okta,
Keycloak, Authentik, …), GitHub, LDAP / Active Directory and SAML 2.0. The README lists every
option with examples.

Things worth deciding up front:

- `allowedDomains` / `allowedOrgs`: who may sign in at all.
- `autoCreate`: create accounts on first login, or only let existing / invited people in.
- `syncGroups`: mirror the provider's groups (then grant space roles to those groups).
- `trustEmail`: only for providers whose e-mail addresses you trust; an account is linked to an
  existing one (or claims an imported author) by verified e-mail.

People can turn on two-factor authentication (TOTP) under their profile; it also applies after
SSO sign-ins.

## E-mail

Set `WIKICRAFT_SMTP_URL` (`smtps://user:pass@host`, `smtp://user:pass@host:587?tls=required` or
`smtp://relay:25`) and `WIKICRAFT_MAIL_FROM`. WikiCraft then sends invitations, password resets,
sign-up confirmations and notification digests (mentions, comments, watched pages and spaces).
Mail goes through an outbox with retries; *Administration → Overview → E-mail* shows failures and
sends a test message.

## Backups

Back up two things, at the same point in time if you can:

1. **PostgreSQL** (pages, history, comments, users, permissions, the live editing state):
   `pg_dump -Fc wikicraft > wikicraft-$(date +%F).dump`, or your managed database's snapshots /
   point-in-time recovery.
2. **Attachments**: `WIKICRAFT_DATA_DIR/blobs` (local storage), or the S3 bucket (turn on
   versioning there).

The search index (`WIKICRAFT_DATA_DIR/index`) doesn't need a backup: `wikicraft reindex`
rebuilds it.

To restore: restore the database dump and the blobs, start WikiCraft, run `wikicraft reindex`.

**Space backups** are a second layer: *Space settings → Export → Backup* produces a zip with
pages, history, comments, attachments and permissions, which *Administration → Import → Restore a
space backup* restores into a new space (also on another WikiCraft).

## Deleting content for good

Deleted pages go to their space's trash (*Space settings → Trash*), where editors can restore
them. Space admins can **Delete forever** single pages or **Empty trash**; that removes the pages
with their history, comments and attachments, and files no other page uses are removed from
storage. To do this automatically, set *Delete trashed pages for good after … days* under
*Administration → Overview* (0, the default, keeps them until someone deletes them). Purges are
recorded in the audit log.

## Upgrades

Stop the old version (or roll the deployment), start the new one. Migrations run automatically;
take a database backup first. Live editors reconnect on their own.

## Monitoring

- `GET /api/health`: liveness; `GET /api/ready`: readiness (503 while shutting down or when the
  database is unreachable). `wikicraft healthcheck` probes it from inside the container.
- `GET /api/metrics`: Prometheus metrics (HTTP, latency, WebSockets, live documents, DB pool,
  jobs). Protect it with `WIKICRAFT_METRICS_TOKEN`.
- Logs go to stdout; `WIKICRAFT_LOG_FORMAT=json` for log shippers. Each request carries an
  `x-request-id`.
- *Administration → Audit log*: sign-ins and 2FA changes, users and groups (incl. merges),
  space and page permissions, trash / restore / archive, imports, space exports, API tokens,
  webhooks and workspace settings.

## AI features (optional)

- **MCP server** at `/api/mcp`: AI assistants (Claude and others) search, read and write pages
  with a person's API token and exactly that person's permissions.
- **Ask the wiki**: set `WIKICRAFT_AI_API_KEY` (Anthropic). Questions are answered from the pages
  the asker can open, with citations. Without the key nothing leaves your server. A switch under
  *Sharing and features* turns it off again.

## Security checklist

- [ ] HTTPS in front, `WIKICRAFT_BASE_URL` with `https://`, `WIKICRAFT_SECURE_COOKIES=true`.
- [ ] Secrets (database password, SMTP, SSO client secrets, AI key) from your secret store via
      environment variables or mounted files, not in images or the repository.
- [ ] SSO with `allowedDomains`, password sign-in off once SSO works, 2FA for admins.
- [ ] Public links and exports: switch them off under *Sharing and features* if your data must
      not leave the wiki.
- [ ] Webhook and link-preview targets on private networks stay blocked unless you need them
      (`WIKICRAFT_WEBHOOK_ALLOW_PRIVATE`, `WIKICRAFT_UNFURL_ALLOW_PRIVATE`).
- [ ] Database backups tested by restoring them somewhere.

## Command line

```bash
wikicraft serve                                     # web server (default)
wikicraft migrate                                   # apply migrations and exit
wikicraft analyze-confluence export.zip             # Confluence dry run (JSON report)
wikicraft import-confluence export.zip --as-user admin@example.com [--space-key KEY] [--skip-history]
wikicraft reindex                                   # rebuild the search index
wikicraft healthcheck                               # exit code reflects /api/health
```
