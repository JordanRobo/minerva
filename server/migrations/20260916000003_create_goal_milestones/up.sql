CREATE TABLE goal_milestones (
    goal_id UUID NOT NULL REFERENCES goals(id) ON DELETE CASCADE,
    milestone_id UUID NOT NULL REFERENCES milestones(id) ON DELETE CASCADE,
    PRIMARY KEY (goal_id, milestone_id)
);

-- goal_id is already indexed as the leading PK column.
CREATE INDEX idx_goal_milestones_milestone_id ON goal_milestones (milestone_id);
