-- What an imported item looked like at its last import, so a later import of the same export
-- source can tell what changed: in Confluence (source_version) and here (target_version).
ALTER TABLE import_mappings ADD COLUMN source_version integer, ADD COLUMN target_version integer;
