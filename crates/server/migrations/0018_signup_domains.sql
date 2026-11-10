-- Open sign-up for people with an e-mail address at these domains (empty = invitation only).
ALTER TABLE workspaces ADD COLUMN signup_domains text[] NOT NULL DEFAULT '{}';
