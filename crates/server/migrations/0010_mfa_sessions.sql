-- Addressable sessions (list / revoke in the UI) and TOTP two-factor authentication.
ALTER TABLE sessions ADD COLUMN id uuid NOT NULL DEFAULT gen_random_uuid();
CREATE UNIQUE INDEX sessions_id ON sessions (id);

ALTER TABLE users ADD COLUMN totp_secret text;
ALTER TABLE users ADD COLUMN totp_pending text;
ALTER TABLE users ADD COLUMN totp_enabled_at timestamptz;
-- last accepted 30-second step: a code can't be used twice
ALTER TABLE users ADD COLUMN totp_last_step bigint;
-- sha256 hashes of unused one-time recovery codes
ALTER TABLE users ADD COLUMN recovery_codes text[] NOT NULL DEFAULT '{}';
