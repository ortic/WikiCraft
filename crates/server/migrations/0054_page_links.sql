-- The link graph: the pages each page links to (links in the body, included pages, page-link
-- properties), one row per distinct target, refreshed whenever the search index picks the
-- page up; a 'children' row (target = the page itself) marks a body that lists its child pages.
-- A target is a page id (which may since have been trashed or deleted), or, for links made to
-- pages that did not exist (`/missing?space=KEY&title=T`, `/missing?source=confluence:ID`),
-- the space and title or the source id.
CREATE TABLE page_links (
    source_id      uuid NOT NULL REFERENCES pages(id) ON DELETE CASCADE,
    ord            integer NOT NULL,
    kind           text NOT NULL CHECK (kind IN ('link', 'include', 'property', 'children')),
    target_id      uuid,
    missing_space  text,
    missing_title  text,
    missing_source text,
    -- the block the link sits in and its text (for reports)
    block_id       text,
    label          text NOT NULL DEFAULT '',
    PRIMARY KEY (source_id, ord),
    CHECK (target_id IS NOT NULL OR missing_title IS NOT NULL OR missing_source IS NOT NULL)
);
CREATE INDEX page_links_target ON page_links (target_id) WHERE target_id IS NOT NULL;
CREATE INDEX page_links_missing ON page_links (lower(missing_space), lower(missing_title)) WHERE missing_title IS NOT NULL;

-- when the page's links were last extracted (null = never: picked up at startup)
ALTER TABLE pages ADD COLUMN linked_at timestamptz;
-- link extractor version (a new one scans every page again)
ALTER TABLE retrieval_state ADD COLUMN linker integer NOT NULL DEFAULT 0;
