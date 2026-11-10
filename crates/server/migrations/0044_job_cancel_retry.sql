-- Jobs can be cancelled (queued ones at once, running ones at their next progress update) and
-- retried (failed and cancelled ones keep their upload for a while).
ALTER TABLE jobs ADD COLUMN cancel_requested boolean NOT NULL DEFAULT false;
ALTER TABLE jobs ADD COLUMN attempts int NOT NULL DEFAULT 1;
