-- Activity feed: newest versions and comments across a workspace.
CREATE INDEX page_versions_created ON page_versions (created_at DESC);
CREATE INDEX comments_created ON comments (created_at DESC);
