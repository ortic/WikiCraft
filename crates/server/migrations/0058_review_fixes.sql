-- Security review 5.

-- Former addresses don't lead to unpublished drafts either.
CREATE OR REPLACE FUNCTION wc_resolve(sp uuid, name text) RETURNS uuid
    LANGUAGE sql STABLE PARALLEL SAFE
    RETURN coalesce(
        (SELECT id FROM pages WHERE space_id = sp AND deleted_at IS NULL AND NOT draft
            AND wc_slug(title) = wc_slug(name) AND lower(title) = lower(trim(name)) ORDER BY created_at, id LIMIT 1),
        (SELECT id FROM pages WHERE space_id = sp AND deleted_at IS NULL AND NOT draft
            AND wc_slug(title) = wc_slug(name) AND wc_slug(name) <> '' ORDER BY created_at, id LIMIT 1),
        (SELECT h.page_id FROM page_paths h JOIN pages p ON p.id = h.page_id AND p.deleted_at IS NULL AND NOT p.draft
            WHERE h.space_id = sp AND h.slug = wc_slug(name) ORDER BY h.left_at DESC LIMIT 1));

-- Every page a name may lead to, best first (exact title, slug, former addresses): readers get the
-- first one they may open, so a page they can't see neither shows nor hides one they can.
CREATE FUNCTION wc_resolve_candidates(sp uuid, name text) RETURNS SETOF uuid
    LANGUAGE sql STABLE PARALLEL SAFE AS $$
    SELECT id FROM (
        SELECT id, 1 AS rank, extract(epoch FROM created_at) AS ord FROM pages
         WHERE space_id = sp AND deleted_at IS NULL AND NOT draft AND wc_slug(title) = wc_slug(name) AND lower(title) = lower(trim(name))
        UNION ALL
        SELECT id, 2, extract(epoch FROM created_at) FROM pages
         WHERE space_id = sp AND deleted_at IS NULL AND NOT draft AND wc_slug(title) = wc_slug(name) AND wc_slug(name) <> ''
        UNION ALL
        SELECT h.page_id, 3, -extract(epoch FROM h.left_at) FROM page_paths h JOIN pages p ON p.id = h.page_id AND p.deleted_at IS NULL AND NOT p.draft
         WHERE h.space_id = sp AND h.slug = wc_slug(name)
    ) c ORDER BY rank, ord, id LIMIT 50
$$;

-- expired sign-in states are pruned on every write
CREATE INDEX IF NOT EXISTS auth_states_expires ON auth_states (expires_at);

-- an upload is used up by one import only (two at once would both start)
ALTER TABLE uploads ADD COLUMN importing boolean NOT NULL DEFAULT false;
