-- Link preview cache: title/description of external pages (public metadata), per URL.
CREATE TABLE link_previews (
    url         text PRIMARY KEY,
    data        jsonb NOT NULL,
    fetched_at  timestamptz NOT NULL DEFAULT now()
);
