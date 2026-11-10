-- The person responsible for a page (defaults to its creator; can be handed over).
ALTER TABLE pages ADD COLUMN owner_id uuid REFERENCES users(id) ON DELETE SET NULL;
