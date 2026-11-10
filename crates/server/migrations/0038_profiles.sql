-- Profiles for the people directory (time zone and language already exist).
ALTER TABLE users ADD COLUMN job_title text, ADD COLUMN department text, ADD COLUMN location text, ADD COLUMN bio text;
CREATE INDEX users_department ON users (workspace_id, lower(department)) WHERE department IS NOT NULL;
