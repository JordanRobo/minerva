-- Pagination & filtering for the task list (roadmap 3.10, 3.16): the list
-- filters by status and orders by target date (nulls last), created_at, id.
-- The milestone_id filter already has idx_tasks_milestone_id; a title search
-- (ILIKE '%…%') cannot use a b-tree index, so none is added for it.
CREATE INDEX idx_tasks_status ON tasks (status);
CREATE INDEX idx_tasks_list_order ON tasks (target_date NULLS LAST, created_at, id);
