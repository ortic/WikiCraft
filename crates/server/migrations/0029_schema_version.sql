-- Document format version per page (wikicraft_doc_model::schema). Existing pages are version 1
-- and get upgraded in the background; new pages are written in the current format.
ALTER TABLE pages ADD COLUMN schema_version integer NOT NULL DEFAULT 1;
ALTER TABLE pages ALTER COLUMN schema_version SET DEFAULT 2;
CREATE INDEX pages_schema_version ON pages (schema_version) WHERE schema_version < 2;
