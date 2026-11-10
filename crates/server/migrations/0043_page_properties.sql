-- Page properties: typed key/value fields shown in a panel above the page body.
-- `value` is JSON (a string, number, ISO date, user id or page id depending on `kind`, or
-- null while unset); `text_value` is its searchable text, kept at save time.
CREATE TABLE page_properties (
    page_id    uuid NOT NULL REFERENCES pages(id) ON DELETE CASCADE,
    position   int NOT NULL,
    key        text NOT NULL CHECK (length(key) BETWEEN 1 AND 100),
    kind       text NOT NULL CHECK (kind IN ('text', 'number', 'date', 'user', 'select', 'url', 'page')),
    value      jsonb,
    text_value text NOT NULL DEFAULT ''
);
CREATE UNIQUE INDEX page_properties_key ON page_properties (page_id, lower(key));
-- suggestions: the keys and values used in a space
CREATE INDEX page_properties_lookup ON page_properties (lower(key), kind);
