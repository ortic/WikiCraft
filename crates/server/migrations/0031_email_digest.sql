-- How often notification e-mails go out: 'soon' (a few minutes after, the default), 'hourly'
-- or 'daily' (at most one summary per hour / day, of what's still unread then).
ALTER TABLE users ADD COLUMN email_digest text NOT NULL DEFAULT 'soon';
ALTER TABLE users ADD COLUMN last_digest_at timestamptz;
