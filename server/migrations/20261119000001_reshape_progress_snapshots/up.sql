-- Roadmap 3.14 (D5): snapshots record the effective status, where it came
-- from and why they were taken, so the daily sweep can run at most once per
-- entity per UTC day. The goal/milestone foreign keys with ON DELETE CASCADE
-- already exist since M1 and are unchanged: a deleted entity drops out of
-- trends with its history.

-- The moment column takes the name the domain uses...
ALTER TABLE progress_snapshots RENAME COLUMN recorded_at TO taken_at;

-- ...and the free-text note goes: snapshots are machine-recorded (D5), not
-- annotated by hand.
ALTER TABLE progress_snapshots DROP COLUMN note;

-- Where the recorded status came from and why the snapshot was taken.
-- Pre-existing rows predate both, so they backfill as system-computed
-- 'manual_change' rows...
ALTER TABLE progress_snapshots
    ADD COLUMN status_source TEXT NOT NULL DEFAULT 'computed'
    CHECK (status_source IN ('computed', 'manual_override'));
ALTER TABLE progress_snapshots
    ADD COLUMN reason TEXT NOT NULL DEFAULT 'manual_change'
    CHECK (reason IN ('daily', 'manual_change'));

-- ...and carry the UTC calendar day of taken_at. AT TIME ZONE 'UTC' first,
-- because a bare cast would apply the session timezone. A column default
-- cannot reference another column, so pre-existing rows are backfilled in a
-- separate step before the column is made NOT NULL.
ALTER TABLE progress_snapshots ADD COLUMN snapshot_date DATE;
UPDATE progress_snapshots SET snapshot_date = (taken_at AT TIME ZONE 'UTC')::date;
ALTER TABLE progress_snapshots ALTER COLUMN snapshot_date SET NOT NULL;

-- Goals have no completion percentage; milestones keep the 0-100 one.
ALTER TABLE progress_snapshots ALTER COLUMN percent_complete DROP NOT NULL;

-- At most one daily snapshot per goal per UTC day, and per milestone: the
-- ON CONFLICT target of the sweep's batch insert. Manual-change snapshots
-- are never unique-constrained (several per day are fine). The two indexes
-- stay separate because a row always leaves the other entity's id NULL, and
-- NULLs never collide in a unique index.
CREATE UNIQUE INDEX idx_progress_snapshots_daily_goal
    ON progress_snapshots (goal_id, snapshot_date)
    WHERE reason = 'daily';
CREATE UNIQUE INDEX idx_progress_snapshots_daily_milestone
    ON progress_snapshots (milestone_id, snapshot_date)
    WHERE reason = 'daily';

-- One entity's history (taken_at, then id) and 3.15's per-day rollups. The
-- composite indexes replace the plain goal_id/milestone_id ones, whose
-- leading column they cover.
DROP INDEX idx_progress_snapshots_goal_id;
DROP INDEX idx_progress_snapshots_milestone_id;
CREATE INDEX idx_progress_snapshots_goal_taken_at
    ON progress_snapshots (goal_id, taken_at);
CREATE INDEX idx_progress_snapshots_milestone_taken_at
    ON progress_snapshots (milestone_id, taken_at);
CREATE INDEX idx_progress_snapshots_snapshot_date
    ON progress_snapshots (snapshot_date);
