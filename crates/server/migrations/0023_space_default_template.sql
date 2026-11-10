-- The template new pages in a space start from (a built-in id or a template id).
ALTER TABLE spaces ADD COLUMN default_template text;
