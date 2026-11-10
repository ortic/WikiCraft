-- Reminders sent for tasks on their due date (once per task, person and due date; a task is
-- recognised by its page and text, since its position can change).
CREATE TABLE task_reminders (
    page_id   uuid NOT NULL REFERENCES pages(id) ON DELETE CASCADE,
    user_id   uuid NOT NULL REFERENCES users(id) ON DELETE CASCADE,
    due       date NOT NULL,
    task_key  text NOT NULL,
    sent_at   timestamptz NOT NULL DEFAULT now(),
    PRIMARY KEY (page_id, user_id, due, task_key)
);
