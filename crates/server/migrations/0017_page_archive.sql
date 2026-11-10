-- Archived pages (with their descendants) are read-only and leave the page tree and search.
ALTER TABLE pages ADD COLUMN archived_at timestamptz, ADD COLUMN archived_by uuid REFERENCES users(id) ON DELETE SET NULL;
CREATE INDEX pages_archived ON pages (space_id) WHERE archived_at IS NOT NULL;
