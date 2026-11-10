-- Unpublished drafts: a new page only its creator sees (with everything below it) until they
-- publish it. Nobody else sees it, space admins included.
ALTER TABLE pages ADD COLUMN draft boolean NOT NULL DEFAULT false;
CREATE INDEX pages_drafts ON pages (space_id, created_by) WHERE draft;
