-- Notification kinds a person doesn't want at all (neither in the app nor by e-mail).
ALTER TABLE users ADD COLUMN muted_notifications text[] NOT NULL DEFAULT '{}';
