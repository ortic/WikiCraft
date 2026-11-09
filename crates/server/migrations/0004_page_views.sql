-- Aggregated page views (privacy-friendly: per page, user and day)
CREATE TABLE page_views (
    page_id  uuid NOT NULL REFERENCES pages(id) ON DELETE CASCADE,
    user_id  uuid REFERENCES users(id) ON DELETE CASCADE,
    day      date NOT NULL DEFAULT current_date,
    views    integer NOT NULL DEFAULT 1
);
CREATE UNIQUE INDEX page_views_key ON page_views (page_id, COALESCE(user_id, '00000000-0000-0000-0000-000000000000'::uuid), day);
