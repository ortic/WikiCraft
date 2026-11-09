-- Page templates (workspace-wide when space_id is NULL)
CREATE TABLE templates (
    id            uuid PRIMARY KEY,
    workspace_id  uuid NOT NULL REFERENCES workspaces(id) ON DELETE CASCADE,
    space_id      uuid REFERENCES spaces(id) ON DELETE CASCADE,
    name          text NOT NULL,
    description   text,
    icon          text,
    title         text,
    content       jsonb NOT NULL,
    created_by    uuid REFERENCES users(id) ON DELETE SET NULL,
    created_at    timestamptz NOT NULL DEFAULT now()
);
CREATE INDEX templates_ws ON templates (workspace_id, space_id);

-- Mentions already notified per page (so editing a page doesn't re-notify)
CREATE TABLE page_mentions (
    page_id  uuid NOT NULL REFERENCES pages(id) ON DELETE CASCADE,
    user_id  uuid NOT NULL REFERENCES users(id) ON DELETE CASCADE,
    PRIMARY KEY (page_id, user_id)
);
