-- The template new pages below a page start from (the nearest such page above wins, then the
-- space's default template).
ALTER TABLE pages ADD COLUMN child_template text;
