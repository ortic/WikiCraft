-- Outgoing e-mail: an outbox drained by every replica's mail worker (retries with backoff).
CREATE TABLE outbox (
    id               uuid PRIMARY KEY,
    to_address       text NOT NULL,
    subject          text NOT NULL,
    body_text        text NOT NULL,
    body_html        text,
    created_at       timestamptz NOT NULL DEFAULT now(),
    next_attempt_at  timestamptz NOT NULL DEFAULT now(),
    attempts         integer NOT NULL DEFAULT 0,
    sent_at          timestamptz,
    last_error       text
);
CREATE INDEX outbox_pending ON outbox (next_attempt_at) WHERE sent_at IS NULL;

-- Single-use links sent by e-mail (password reset, invitation); only hashes are stored.
CREATE TABLE user_tokens (
    token_hash  text PRIMARY KEY,
    user_id     uuid NOT NULL REFERENCES users(id) ON DELETE CASCADE,
    purpose     text NOT NULL,
    created_by  uuid REFERENCES users(id) ON DELETE SET NULL,
    created_at  timestamptz NOT NULL DEFAULT now(),
    expires_at  timestamptz NOT NULL,
    used_at     timestamptz
);
CREATE INDEX user_tokens_user ON user_tokens (user_id, purpose);

-- all | important (mentions, comments, replies) | none
ALTER TABLE users ADD COLUMN email_notifications text NOT NULL DEFAULT 'important';

-- notification e-mails are sent as a digest of what's still unread after a short delay
ALTER TABLE notifications ADD COLUMN emailed_at timestamptz;
UPDATE notifications SET emailed_at = created_at;
CREATE INDEX notifications_email_pending ON notifications (created_at) WHERE emailed_at IS NULL;
