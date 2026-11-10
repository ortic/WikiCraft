-- The person responsible for a space (its creator unless handed over); always a space admin.
ALTER TABLE spaces ADD COLUMN owner_id uuid REFERENCES users(id) ON DELETE SET NULL;
UPDATE spaces SET owner_id = COALESCE(personal_owner_id, created_by);
