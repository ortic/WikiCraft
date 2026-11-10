-- An uploaded profile picture (blob key; served at /api/users/{id}/avatar).
ALTER TABLE users ADD COLUMN avatar_key text;
