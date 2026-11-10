-- A person's saved searches: the search page's query string (q, space, filters).
CREATE TABLE saved_searches (
    id          uuid PRIMARY KEY,
    user_id     uuid NOT NULL REFERENCES users(id) ON DELETE CASCADE,
    name        text NOT NULL,
    params      text NOT NULL,
    created_at  timestamptz NOT NULL DEFAULT now()
);
CREATE INDEX saved_searches_user ON saved_searches (user_id, created_at);
