-- People someone follows: their edits and comments make up Home → Activity → People I follow.
CREATE TABLE user_follows (
    follower_id uuid NOT NULL REFERENCES users (id) ON DELETE CASCADE,
    followee_id uuid NOT NULL REFERENCES users (id) ON DELETE CASCADE,
    created_at timestamptz NOT NULL DEFAULT now(),
    PRIMARY KEY (follower_id, followee_id),
    CHECK (follower_id <> followee_id)
);
CREATE INDEX user_follows_followee ON user_follows (followee_id);
