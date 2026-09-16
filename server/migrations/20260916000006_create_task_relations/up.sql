CREATE TABLE task_relations (
    id UUID PRIMARY KEY,
    source_task_id UUID NOT NULL REFERENCES tasks(id) ON DELETE CASCADE,
    target_task_id UUID NOT NULL REFERENCES tasks(id) ON DELETE CASCADE,
    relation_type TEXT NOT NULL CHECK (relation_type IN ('blocks', 'blocked_by', 'relates_to')),
    created_at TIMESTAMPTZ NOT NULL
);

CREATE INDEX idx_task_relations_source_task_id ON task_relations (source_task_id);
CREATE INDEX idx_task_relations_target_task_id ON task_relations (target_task_id);
