-- Uploads in parts, for multi-GB files (Confluence exports): resumable after a network error or a
-- reload, any replica takes the next part. The parts live in file storage (`uploads/<id>/<n>`)
-- until the upload is used (dry run, then import, from the same upload) or abandoned (a day).
CREATE TABLE uploads (
    id           uuid PRIMARY KEY,
    workspace_id uuid NOT NULL REFERENCES workspaces(id) ON DELETE CASCADE,
    created_by   uuid NOT NULL REFERENCES users(id) ON DELETE CASCADE,
    filename     text NOT NULL,
    size         bigint NOT NULL CHECK (size >= 0),
    part_size    integer NOT NULL CHECK (part_size > 0),
    -- parts received so far (in order) and their bytes
    parts        integer NOT NULL DEFAULT 0,
    received     bigint NOT NULL DEFAULT 0,
    created_at   timestamptz NOT NULL DEFAULT now(),
    updated_at   timestamptz NOT NULL DEFAULT now()
);
CREATE INDEX uploads_updated ON uploads (updated_at);
