-- A space's own look: {accentColor: "#rrggbb", hideBlog: true}; empty = the workspace's.
ALTER TABLE spaces ADD COLUMN look jsonb NOT NULL DEFAULT '{}';
