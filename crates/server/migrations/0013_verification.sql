-- Page verification: someone who may edit a page vouches that it is accurate, optionally until
-- a date. Review requests are notifications.
CREATE TABLE page_verifications (
    page_id      uuid PRIMARY KEY REFERENCES pages(id) ON DELETE CASCADE,
    verified_by  uuid REFERENCES users(id) ON DELETE SET NULL,
    verified_at  timestamptz NOT NULL DEFAULT now(),
    expires_at   timestamptz,
    note         text
);
CREATE INDEX page_verifications_expiry ON page_verifications (expires_at);
