-- Roadmap 3.2 (D12): manual status override for goals and milestones.
-- `status_override` is NULL while the automatic status in `status` applies,
-- and holds a sticky manual value until explicitly cleared; recomputation
-- never overwrites it (roadmap 3.13). The CHECK mirrors the `status` column's.
ALTER TABLE goals ADD COLUMN status_override TEXT CHECK (status_override IN ('on_track', 'at_risk', 'off_track', 'complete'));
ALTER TABLE milestones ADD COLUMN status_override TEXT CHECK (status_override IN ('on_track', 'at_risk', 'off_track', 'complete'));
