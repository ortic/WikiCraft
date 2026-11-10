-- Watching a page and everything below it (changes and new pages).
ALTER TABLE watches ADD COLUMN include_children boolean NOT NULL DEFAULT false;
