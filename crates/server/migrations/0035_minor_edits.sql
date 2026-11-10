-- A minor change: watchers were not notified about this version.
ALTER TABLE page_versions ADD COLUMN minor boolean NOT NULL DEFAULT false;
