-- Human-readable page addresses: /wiki/<SPACE KEY>/<slug>, the slug made from the title
-- (lower case, runs of spaces and punctuation turned into one '-'; letters of every script kept).
-- The character class is spelled out instead of [[:alnum:]] so slugs don't depend on the
-- database's locale.
CREATE FUNCTION wc_slug(t text) RETURNS text
    LANGUAGE sql IMMUTABLE STRICT PARALLEL SAFE
    RETURN trim(both '-' from left(regexp_replace(lower(t), '[\s!-/:-@\[-`{-~ -¿×÷ -⁯←-⯿　-〿！-／]+', '-', 'g'), 100));

-- the page an address leads to: the oldest published page with that slug in the space
CREATE INDEX pages_slug ON pages (space_id, wc_slug(title)) WHERE deleted_at IS NULL;

-- addresses pages had before a rename or a move, so links to them keep working (as long as no
-- other page takes the address over)
CREATE TABLE page_paths (
    space_id uuid NOT NULL REFERENCES spaces(id) ON DELETE CASCADE,
    slug     text NOT NULL,
    page_id  uuid NOT NULL REFERENCES pages(id) ON DELETE CASCADE,
    left_at  timestamptz NOT NULL DEFAULT now(),
    PRIMARY KEY (space_id, slug, page_id)
);
CREATE INDEX page_paths_page ON page_paths (page_id);

CREATE FUNCTION wc_remember_path() RETURNS trigger LANGUAGE plpgsql AS $$
BEGIN
    IF wc_slug(OLD.title) <> '' AND (wc_slug(OLD.title) <> wc_slug(NEW.title) OR OLD.space_id <> NEW.space_id) THEN
        INSERT INTO page_paths (space_id, slug, page_id) VALUES (OLD.space_id, wc_slug(OLD.title), OLD.id)
        ON CONFLICT (space_id, slug, page_id) DO UPDATE SET left_at = now();
    END IF;
    RETURN NEW;
END
$$;
CREATE TRIGGER pages_remember_path AFTER UPDATE OF title, space_id ON pages
    FOR EACH ROW EXECUTE FUNCTION wc_remember_path();

-- the titles pages had in earlier versions
INSERT INTO page_paths (space_id, slug, page_id, left_at)
SELECT p.space_id, wc_slug(v.title), p.id, max(v.created_at)
FROM page_versions v JOIN pages p ON p.id = v.page_id
WHERE wc_slug(v.title) <> '' AND wc_slug(v.title) <> wc_slug(p.title)
GROUP BY 1, 2, 3
ON CONFLICT DO NOTHING;

-- The space with this key, or that had it before a rename.
CREATE FUNCTION wc_space_by_key(ws uuid, k text) RETURNS uuid
    LANGUAGE sql STABLE PARALLEL SAFE
    RETURN (SELECT id FROM (SELECT id, 0 AS o FROM spaces WHERE workspace_id = ws AND lower(key) = lower(k)
                            UNION ALL SELECT space_id, 1 FROM space_key_aliases WHERE workspace_id = ws AND lower(key) = lower(k)) s
            ORDER BY o LIMIT 1);

-- The page a name (title or slug) leads to in a space: the oldest published page with exactly
-- that title, else the oldest with that slug, else the page that last had that address.
CREATE FUNCTION wc_resolve(sp uuid, name text) RETURNS uuid
    LANGUAGE sql STABLE PARALLEL SAFE
    RETURN coalesce(
        (SELECT id FROM pages WHERE space_id = sp AND deleted_at IS NULL AND NOT draft
            AND wc_slug(title) = wc_slug(name) AND lower(title) = lower(trim(name)) ORDER BY created_at, id LIMIT 1),
        (SELECT id FROM pages WHERE space_id = sp AND deleted_at IS NULL AND NOT draft
            AND wc_slug(title) = wc_slug(name) AND wc_slug(name) <> '' ORDER BY created_at, id LIMIT 1),
        (SELECT h.page_id FROM page_paths h JOIN pages p ON p.id = h.page_id AND p.deleted_at IS NULL
            WHERE h.space_id = sp AND h.slug = wc_slug(name) ORDER BY h.left_at DESC LIMIT 1));

-- links to pages by name (page_links.missing_title) are matched by slug
DROP INDEX page_links_missing;
CREATE INDEX page_links_slug ON page_links (wc_slug(missing_title)) WHERE missing_title IS NOT NULL;
