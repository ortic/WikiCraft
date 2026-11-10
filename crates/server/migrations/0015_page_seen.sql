-- The version of each page a person last saw, for "changed since your last visit".
CREATE TABLE page_seen (
    user_id  uuid NOT NULL REFERENCES users(id) ON DELETE CASCADE,
    page_id  uuid NOT NULL REFERENCES pages(id) ON DELETE CASCADE,
    version  integer NOT NULL,
    seen_at  timestamptz NOT NULL DEFAULT now(),
    PRIMARY KEY (user_id, page_id)
);
