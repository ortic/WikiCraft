-- Pending self sign-ups: the address lives here until the confirmation link is used, so a
-- sign-up request creates no user and never touches invitations.
CREATE TABLE signup_requests (
    token_hash    text PRIMARY KEY,
    workspace_id  uuid NOT NULL REFERENCES workspaces(id) ON DELETE CASCADE,
    email         text NOT NULL,
    created_at    timestamptz NOT NULL DEFAULT now(),
    expires_at    timestamptz NOT NULL,
    used_at       timestamptz
);
CREATE INDEX signup_requests_email ON signup_requests (workspace_id, lower(email), created_at);
