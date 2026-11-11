-- Granular space permissions: a grant may leave out some of the permissions its role brings
-- (keys like 'page.delete', 'export'; see authz::Perm). Admin grants always carry all of them.
ALTER TABLE space_permissions ADD COLUMN denied text[] NOT NULL DEFAULT '{}';
-- the same for the space's default role (all workspace members except guests)
ALTER TABLE spaces ADD COLUMN default_denied text[] NOT NULL DEFAULT '{}';
