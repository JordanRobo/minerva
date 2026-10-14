-- Roadmap 3.11 (D17): one assignee per task, one owner per goal and per
-- milestone. Each is a nullable user reference; existing rows get NULL.
-- ON DELETE SET NULL: deleting a user leaves their assignments in place as
-- unassigned rather than failing or cascading (no auto-unassign in v1).
ALTER TABLE tasks ADD COLUMN assignee_id UUID REFERENCES users(id) ON DELETE SET NULL;
ALTER TABLE goals ADD COLUMN owner_id UUID REFERENCES users(id) ON DELETE SET NULL;
ALTER TABLE milestones ADD COLUMN owner_id UUID REFERENCES users(id) ON DELETE SET NULL;
-- "My tasks" (roadmap 4.5) filters the task list on the assignee, so it gets
-- an index; goals and milestones get none yet (no owner filter in v1).
CREATE INDEX idx_tasks_assignee_id ON tasks (assignee_id);
