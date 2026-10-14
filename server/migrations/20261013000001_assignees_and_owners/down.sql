-- Roadmap 3.11 (D17): drop the assignee and owner columns again.
DROP INDEX idx_tasks_assignee_id;
ALTER TABLE tasks DROP COLUMN assignee_id;
ALTER TABLE goals DROP COLUMN owner_id;
ALTER TABLE milestones DROP COLUMN owner_id;
