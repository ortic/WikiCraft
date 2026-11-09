-- Task items (taskItem nodes) extracted from page content, for "my tasks" views
CREATE TABLE tasks (
    page_id      uuid NOT NULL REFERENCES pages(id) ON DELETE CASCADE,
    idx          integer NOT NULL,          -- position among the page's task items
    text         text NOT NULL,
    done         boolean NOT NULL,
    assignee_id  uuid REFERENCES users(id) ON DELETE SET NULL,
    due          date,
    updated_at   timestamptz NOT NULL DEFAULT now(),
    PRIMARY KEY (page_id, idx)
);
CREATE INDEX tasks_assignee ON tasks (assignee_id, done);
