-- Watching a whole space: notifications for new and updated pages in it.
CREATE TABLE space_watches (
    user_id    uuid NOT NULL REFERENCES users(id) ON DELETE CASCADE,
    space_id   uuid NOT NULL REFERENCES spaces(id) ON DELETE CASCADE,
    created_at timestamptz NOT NULL DEFAULT now(),
    PRIMARY KEY (user_id, space_id)
);
