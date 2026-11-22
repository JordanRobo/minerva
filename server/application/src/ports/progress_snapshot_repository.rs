//! Persistence port for [`ProgressSnapshot`]s.
//!
//! Snapshots are an append-only historical record, so this port has no
//! update or delete methods.

use chrono::NaiveDate;
use domain::{GoalId, MilestoneId, NewSnapshot, ProgressSnapshot};

use crate::ports::RepositoryError;

#[async_trait::async_trait]
pub trait ProgressSnapshotRepository: Send + Sync {
    /// Store a snapshot (manual status changes, roadmap 3.14). Duplicates are
    /// allowed — several manual-change snapshots may land on one day.
    async fn record(&self, snapshot: NewSnapshot) -> Result<ProgressSnapshot, RepositoryError>;

    /// Store daily snapshots, skipping any entity that already has one for
    /// its `snapshot_date` (the partial unique index); returns how many rows
    /// were actually inserted. Callers pass at most 200 per call; an empty
    /// slice makes no database call.
    async fn record_daily_batch(&self, snapshots: &[NewSnapshot])
    -> Result<usize, RepositoryError>;

    /// The ids of the goals and milestones with no daily snapshot for
    /// `date`, each in id order — the sweep's work list (roadmap 3.14). A
    /// manual-change row does not count as a daily one.
    async fn entity_ids_missing_daily(
        &self,
        date: NaiveDate,
    ) -> Result<(Vec<GoalId>, Vec<MilestoneId>), RepositoryError>;

    /// A goal's snapshots, oldest first (`taken_at`, then id). Unpaginated
    /// and internal: 3.15 adds what the dashboard needs.
    async fn list_for_goal(
        &self,
        goal_id: GoalId,
    ) -> Result<Vec<ProgressSnapshot>, RepositoryError>;

    /// A milestone's snapshots, oldest first (`taken_at`, then id).
    async fn list_for_milestone(
        &self,
        milestone_id: MilestoneId,
    ) -> Result<Vec<ProgressSnapshot>, RepositoryError>;
}

#[cfg(test)]
pub mod fakes {
    use std::sync::Mutex;

    use chrono::NaiveDate;
    use domain::{
        GoalId, MilestoneId, NewSnapshot, ProgressSnapshot, ProgressTarget, SnapshotReason,
    };

    use super::ProgressSnapshotRepository;
    use crate::ports::RepositoryError;

    /// In-memory [`ProgressSnapshotRepository`] mirroring the Postgres rules:
    /// at most one daily snapshot per entity per UTC day (manual-change rows
    /// never conflict), history oldest first (`taken_at`, then id), and the
    /// missing-daily anti-join over the registered entities. Rows are not
    /// removed when an entity disappears — the real table cascades with it;
    /// tests own both fakes.
    #[derive(Default)]
    pub struct InMemoryProgressSnapshotRepository {
        snapshots: Mutex<Vec<ProgressSnapshot>>,
        goals: Mutex<Vec<GoalId>>,
        milestones: Mutex<Vec<MilestoneId>>,
    }

    impl InMemoryProgressSnapshotRepository {
        pub fn new() -> Self {
            Self::default()
        }

        /// Register a goal as existing, the fake's stand-in for the `goals`
        /// table the anti-join reads.
        pub fn register_goal(&self, id: GoalId) {
            self.goals.lock().unwrap().push(id);
        }

        /// Register a milestone as existing (see [`Self::register_goal`]).
        pub fn register_milestone(&self, id: MilestoneId) {
            self.milestones.lock().unwrap().push(id);
        }
    }

    #[async_trait::async_trait]
    impl ProgressSnapshotRepository for InMemoryProgressSnapshotRepository {
        async fn record(&self, snapshot: NewSnapshot) -> Result<ProgressSnapshot, RepositoryError> {
            let stored = ProgressSnapshot::from_new(snapshot);
            self.snapshots.lock().unwrap().push(stored.clone());
            Ok(stored)
        }

        async fn record_daily_batch(
            &self,
            snapshots: &[NewSnapshot],
        ) -> Result<usize, RepositoryError> {
            if snapshots.is_empty() {
                return Ok(0);
            }
            let mut stored = self.snapshots.lock().unwrap();
            let mut inserted = 0;
            for snapshot in snapshots {
                // The partial unique index: a daily row never duplicates an
                // existing daily row for the same entity and day; manual
                // rows sit outside it.
                if snapshot.reason == SnapshotReason::Daily
                    && stored.iter().any(|existing| {
                        existing.target == snapshot.target
                            && existing.snapshot_date == snapshot.snapshot_date
                            && existing.reason == SnapshotReason::Daily
                    })
                {
                    continue;
                }
                stored.push(ProgressSnapshot::from_new(snapshot.clone()));
                inserted += 1;
            }
            Ok(inserted)
        }

        async fn entity_ids_missing_daily(
            &self,
            date: NaiveDate,
        ) -> Result<(Vec<GoalId>, Vec<MilestoneId>), RepositoryError> {
            let stored = self.snapshots.lock().unwrap();
            let has_daily = |target: &ProgressTarget| {
                stored.iter().any(|snapshot| {
                    snapshot.target == *target
                        && snapshot.reason == SnapshotReason::Daily
                        && snapshot.snapshot_date == date
                })
            };
            let mut goals: Vec<GoalId> = self
                .goals
                .lock()
                .unwrap()
                .iter()
                .copied()
                .filter(|id| !has_daily(&ProgressTarget::Goal(*id)))
                .collect();
            goals.sort_by_key(|id| id.0);
            let mut milestones: Vec<MilestoneId> = self
                .milestones
                .lock()
                .unwrap()
                .iter()
                .copied()
                .filter(|id| !has_daily(&ProgressTarget::Milestone(*id)))
                .collect();
            milestones.sort_by_key(|id| id.0);
            Ok((goals, milestones))
        }

        async fn list_for_goal(
            &self,
            goal_id: GoalId,
        ) -> Result<Vec<ProgressSnapshot>, RepositoryError> {
            let stored = self.snapshots.lock().unwrap();
            Ok(oldest_first(stored.iter().filter(|snapshot| {
                snapshot.target == ProgressTarget::Goal(goal_id)
            })))
        }

        async fn list_for_milestone(
            &self,
            milestone_id: MilestoneId,
        ) -> Result<Vec<ProgressSnapshot>, RepositoryError> {
            let stored = self.snapshots.lock().unwrap();
            Ok(oldest_first(stored.iter().filter(|snapshot| {
                snapshot.target == ProgressTarget::Milestone(milestone_id)
            })))
        }
    }

    /// The history order: `taken_at`, then id.
    fn oldest_first<'a>(
        snapshots: impl Iterator<Item = &'a ProgressSnapshot>,
    ) -> Vec<ProgressSnapshot> {
        let mut listed: Vec<ProgressSnapshot> = snapshots.cloned().collect();
        listed.sort_by_key(|a| (a.taken_at, a.id.0));
        listed
    }
}
