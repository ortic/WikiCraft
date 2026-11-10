-- A draft that publishes itself at this time (on behalf of its creator).
ALTER TABLE pages ADD COLUMN publish_at timestamptz;
CREATE INDEX pages_publish_at ON pages (publish_at) WHERE publish_at IS NOT NULL AND draft;
