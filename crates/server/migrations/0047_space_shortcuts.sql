-- Links space admins pin to the space sidebar: [{title, url}] (web addresses or wiki paths).
ALTER TABLE spaces ADD COLUMN shortcuts jsonb NOT NULL DEFAULT '[]';
