-- Former space keys: links and API calls with an old key keep working after a rename.
CREATE TABLE space_key_aliases (
    workspace_id uuid NOT NULL REFERENCES workspaces(id) ON DELETE CASCADE,
    key          text NOT NULL,
    space_id     uuid NOT NULL REFERENCES spaces(id) ON DELETE CASCADE,
    created_at   timestamptz NOT NULL DEFAULT now()
);
CREATE UNIQUE INDEX space_key_aliases_key ON space_key_aliases (workspace_id, lower(key));
