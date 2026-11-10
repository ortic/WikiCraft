-- Public links can need a password; unlocking one grants the browser 12 hours (a cookie whose
-- hash is stored here).
ALTER TABLE share_links ADD COLUMN password_hash text;
CREATE TABLE share_unlocks (
    token_hash  bytea PRIMARY KEY,
    link_id     uuid NOT NULL REFERENCES share_links(id) ON DELETE CASCADE,
    expires_at  timestamptz NOT NULL
);
CREATE INDEX share_unlocks_expiry ON share_unlocks (expires_at);
