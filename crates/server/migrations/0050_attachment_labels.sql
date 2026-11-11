-- Labels on attachments (like page labels): files show up on the label's page.
CREATE TABLE attachment_labels (
    attachment_id uuid NOT NULL REFERENCES attachments (id) ON DELETE CASCADE,
    name text NOT NULL,
    PRIMARY KEY (attachment_id, name)
);
CREATE INDEX attachment_labels_name ON attachment_labels (name);
