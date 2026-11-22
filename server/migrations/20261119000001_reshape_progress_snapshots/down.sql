-- Revert roadmap 3.14's snapshot reshaping. Data recorded under the new
-- shape (status_source, reason, snapshot_date) is dropped with its columns;
-- a goal snapshot's NULL percentage cannot go back into the old NOT NULL
-- column, so a rollback past any such row fails on purpose rather than
-- inventing a value.
DROP INDEX idx_progress_snapshots_snapshot_date;
DROP INDEX idx_progress_snapshots_milestone_taken_at;
DROP INDEX idx_progress_snapshots_goal_taken_at;
DROP INDEX idx_progress_snapshots_daily_milestone;
DROP INDEX idx_progress_snapshots_daily_goal;
ALTER TABLE progress_snapshots ALTER COLUMN percent_complete SET NOT NULL;
ALTER TABLE progress_snapshots DROP COLUMN snapshot_date;
ALTER TABLE progress_snapshots DROP COLUMN reason;
ALTER TABLE progress_snapshots DROP COLUMN status_source;
ALTER TABLE progress_snapshots ADD COLUMN note TEXT;
ALTER TABLE progress_snapshots RENAME COLUMN taken_at TO recorded_at;
CREATE INDEX idx_progress_snapshots_goal_id ON progress_snapshots (goal_id);
CREATE INDEX idx_progress_snapshots_milestone_id ON progress_snapshots (milestone_id);
