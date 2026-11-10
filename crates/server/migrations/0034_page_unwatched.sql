-- People who stopped watching a page on purpose: editing it again doesn't sign them up again.
CREATE TABLE page_unwatched (
    user_id  uuid NOT NULL REFERENCES users(id) ON DELETE CASCADE,
    page_id  uuid NOT NULL REFERENCES pages(id) ON DELETE CASCADE,
    PRIMARY KEY (user_id, page_id)
);
