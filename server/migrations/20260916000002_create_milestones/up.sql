CREATE TABLE milestones (
    id UUID PRIMARY KEY,
    title TEXT NOT NULL,
    description TEXT,
    status TEXT NOT NULL CHECK (status IN ('on_track', 'at_risk', 'off_track', 'complete')),
    status_source TEXT NOT NULL CHECK (status_source IN ('computed', 'manual_override')),
    target_date DATE,
    created_at TIMESTAMPTZ NOT NULL,
    updated_at TIMESTAMPTZ NOT NULL
);
