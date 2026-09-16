CREATE TABLE tasks (
    id UUID PRIMARY KEY,
    -- Nullable: a task may exist unassigned to any milestone.
    milestone_id UUID REFERENCES milestones(id) ON DELETE SET NULL,
    title TEXT NOT NULL,
    description TEXT,
    status TEXT NOT NULL CHECK (status IN ('backlog', 'to_do', 'in_progress', 'done')),
    target_date DATE,
    created_at TIMESTAMPTZ NOT NULL,
    updated_at TIMESTAMPTZ NOT NULL
);

CREATE INDEX idx_tasks_milestone_id ON tasks (milestone_id);
