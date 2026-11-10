-- What a personal API token may do: 'write' (everything its owner may do, the default) or
-- 'read' (no changes: for read-only integrations and AI assistants).
ALTER TABLE api_tokens ADD COLUMN scope text NOT NULL DEFAULT 'write';
