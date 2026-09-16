CREATE TABLE progress_snapshots (
    id UUID PRIMARY KEY,
    goal_id UUID REFERENCES goals(id) ON DELETE CASCADE,
    milestone_id UUID REFERENCES milestones(id) ON DELETE CASCADE,
    recorded_at TIMESTAMPTZ NOT NULL,
    status TEXT NOT NULL CHECK (status IN ('on_track', 'at_risk', 'off_track', 'complete')),
    percent_complete SMALLINT NOT NULL CHECK (percent_complete BETWEEN 0 AND 100),
    note TEXT,
    -- A snapshot is taken for exactly one of a goal or a milestone.
    CHECK (num_nonnulls(goal_id, milestone_id) = 1)
);

CREATE INDEX idx_progress_snapshots_goal_id ON progress_snapshots (goal_id);
CREATE INDEX idx_progress_snapshots_milestone_id ON progress_snapshots (milestone_id);
