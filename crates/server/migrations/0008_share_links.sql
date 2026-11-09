-- Public, read-only links to a page (optionally with its sub-pages).
CREATE TABLE share_links (
    id                uuid PRIMARY KEY,
    token             text NOT NULL UNIQUE,
    workspace_id      uuid NOT NULL REFERENCES workspaces(id) ON DELETE CASCADE,
    page_id           uuid NOT NULL REFERENCES pages(id) ON DELETE CASCADE,
    include_children  boolean NOT NULL DEFAULT false,
    created_by        uuid REFERENCES users(id) ON DELETE SET NULL,
    created_at        timestamptz NOT NULL DEFAULT now(),
    expires_at        timestamptz,
    revoked_at        timestamptz,
    views             bigint NOT NULL DEFAULT 0,
    last_viewed_at    timestamptz
);
CREATE INDEX share_links_page ON share_links (page_id);
