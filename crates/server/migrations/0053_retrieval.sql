-- Passages of pages and their attachments for retrieval (RAG): cut along the document's
-- structure, with the headings they sit under and the block they start at. `hash` addresses
-- the text a passage is embedded as; the vectors (optional, pgvector) live in `chunk_vectors`,
-- created at runtime once the embedding model's size is known.
CREATE TABLE page_chunks (
    page_id       uuid NOT NULL REFERENCES pages(id) ON DELETE CASCADE,
    ord           integer NOT NULL,
    attachment_id uuid REFERENCES attachments(id) ON DELETE CASCADE,
    block_id      text,
    headings      text[] NOT NULL DEFAULT '{}',
    content       text NOT NULL,
    hash          text NOT NULL,
    PRIMARY KEY (page_id, ord)
);
CREATE INDEX page_chunks_hash ON page_chunks (hash);

-- when the page's passages were last cut (null = never: picked up at startup)
ALTER TABLE pages ADD COLUMN chunked_at timestamptz;

-- chunker version (a new one re-cuts every page) and the model the stored vectors come from
CREATE TABLE retrieval_state (
    id      boolean PRIMARY KEY DEFAULT true CHECK (id),
    chunker integer NOT NULL DEFAULT 0,
    model   text,
    dims    integer
);
INSERT INTO retrieval_state DEFAULT VALUES;
