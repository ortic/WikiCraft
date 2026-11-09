-- External identities (SSO): one row per (provider, subject)
CREATE TABLE identities (
    provider    text NOT NULL,
    subject     text NOT NULL,
    user_id     uuid NOT NULL REFERENCES users(id) ON DELETE CASCADE,
    email       text,
    created_at  timestamptz NOT NULL DEFAULT now(),
    last_login  timestamptz,
    PRIMARY KEY (provider, subject)
);
CREATE INDEX identities_user ON identities (user_id);
