-- Passkeys (WebAuthn credentials): a second factor after the password, or a sign-in on their own
-- when the authenticator verified the person (PIN, fingerprint, face). The WebAuthn user handle is
-- the user's id.
CREATE TABLE passkeys (
    id            uuid PRIMARY KEY,
    user_id       uuid NOT NULL REFERENCES users(id) ON DELETE CASCADE,
    credential_id bytea NOT NULL UNIQUE,
    -- COSE key and its algorithm (-7 ES256, -8 EdDSA, -257 RS256)
    public_key    bytea NOT NULL,
    algorithm     integer NOT NULL,
    sign_count    bigint NOT NULL DEFAULT 0,
    name          text NOT NULL,
    transports    text[] NOT NULL DEFAULT '{}',
    -- synced to the person's other devices (iCloud Keychain, Google Password Manager, …)
    backed_up     boolean NOT NULL DEFAULT false,
    created_at    timestamptz NOT NULL DEFAULT now(),
    last_used_at  timestamptz
);
CREATE INDEX passkeys_user ON passkeys (user_id);
