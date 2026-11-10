-- Expired link previews are pruned in batches.
CREATE INDEX link_previews_fetched ON link_previews (fetched_at);
