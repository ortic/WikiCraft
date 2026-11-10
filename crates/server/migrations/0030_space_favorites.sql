-- Starred spaces (per person), shown first in the space directory and in the sidebar.
CREATE TABLE space_favorites (
    user_id     uuid NOT NULL REFERENCES users(id) ON DELETE CASCADE,
    space_id    uuid NOT NULL REFERENCES spaces(id) ON DELETE CASCADE,
    created_at  timestamptz NOT NULL DEFAULT now(),
    PRIMARY KEY (user_id, space_id)
);
