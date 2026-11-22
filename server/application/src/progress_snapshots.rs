//! Progress snapshots (roadmap 3.14): when a goal's or milestone's manual
//! status override is set or cleared, the entity's effective status at that
//! moment is recorded as an append-only `manual_change` snapshot; and the
//! daily sweep ensures every goal and milestone has one `daily` snapshot per
//! UTC day. Reading the history back is roadmap 3.15.

use std::sync::Arc;

use async_trait::async_trait;
use domain::{GoalId, MilestoneId, NewSnapshot, SnapshotReason, percent_complete};

use crate::ports::{
    Clock, GoalRepository, MilestoneRepository, ProgressSnapshotRepository, RepositoryError,
    StatusChangeTarget, StatusSnapshotError, StatusSnapshotTrigger, TaskCounts, TaskRepository,
};
use crate::status_computation::{BATCH_SIZE, StatusComputationError, StatusRecomputation};

/// An error from recording a manual-change snapshot.
#[derive(Debug)]
pub enum ProgressSnapshotError {
    /// The repository failed.
    Repository(RepositoryError),
}

impl std::fmt::Display for ProgressSnapshotError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            ProgressSnapshotError::Repository(err) => write!(f, "{err}"),
        }
    }
}

impl std::error::Error for ProgressSnapshotError {}

/// Records manual-change snapshots (roadmap 3.14): one row per override
/// set/clear, capturing the entity's effective status and where it came from
/// at that moment. The [`RecordingStatusSnapshotTrigger`] calls this after
/// the override write committed, so an entity deleted in between simply
/// leaves no snapshot — its history cascades away with it anyway.
pub struct ProgressSnapshotService {
    goals: Arc<dyn GoalRepository>,
    milestones: Arc<dyn MilestoneRepository>,
    tasks: Arc<dyn TaskRepository>,
    snapshots: Arc<dyn ProgressSnapshotRepository>,
    computation: Arc<dyn StatusRecomputation>,
    clock: Arc<dyn Clock>,
}

impl ProgressSnapshotService {
    pub fn new(
        goals: Arc<dyn GoalRepository>,
        milestones: Arc<dyn MilestoneRepository>,
        tasks: Arc<dyn TaskRepository>,
        snapshots: Arc<dyn ProgressSnapshotRepository>,
        computation: Arc<dyn StatusRecomputation>,
        clock: Arc<dyn Clock>,
    ) -> Self {
        Self {
            goals,
            milestones,
            tasks,
            snapshots,
            computation,
            clock,
        }
    }

    /// Record the snapshot for the entity whose manual override just changed.
    /// The status and source are the entity's current effective ones — after
    /// a set that is the override itself, after a clear the stored automatic
    /// status (roadmap 3.13 keeps it current) — so one code path serves both.
    /// A milestone's percentage uses the same rule as its responses: the
    /// share of its tasks in Done, from one grouped count lookup. An entity
    /// that no longer exists leaves no snapshot and is not an error.
    pub async fn record_manual_change(
        &self,
        target: StatusChangeTarget,
    ) -> Result<(), ProgressSnapshotError> {
        match target {
            StatusChangeTarget::Goal(id) => self.record_goal(id).await,
            StatusChangeTarget::Milestone(id) => self.record_milestone(id).await,
        }
    }

    async fn record_goal(&self, id: GoalId) -> Result<(), ProgressSnapshotError> {
        let Some(goal) = self
            .goals
            .find_by_id(id)
            .await
            .map_err(ProgressSnapshotError::Repository)?
        else {
            return Ok(());
        };
        let snapshot =
            NewSnapshot::from_goal(&goal, self.clock.now(), SnapshotReason::ManualChange);
        self.snapshots
            .record(snapshot)
            .await
            .map_err(ProgressSnapshotError::Repository)?;
        Ok(())
    }

    async fn record_milestone(&self, id: MilestoneId) -> Result<(), ProgressSnapshotError> {
        let Some(milestone) = self
            .milestones
            .find_by_id(id)
            .await
            .map_err(ProgressSnapshotError::Repository)?
        else {
            return Ok(());
        };
        // One grouped lookup, the same rule the milestone responses use.
        let counts = self
            .tasks
            .task_counts_for_milestones(std::slice::from_ref(&id))
            .await
            .map_err(ProgressSnapshotError::Repository)?;
        let count = counts
            .get(&id)
            .copied()
            .unwrap_or(TaskCounts { total: 0, done: 0 });
        let percent = (percent_complete(count.total, count.done) * 100.0).round() as u8;
        let snapshot = NewSnapshot::from_milestone(
            &milestone,
            self.clock.now(),
            SnapshotReason::ManualChange,
            percent,
        )
        .expect("done never exceeds total, so the percentage is 0-100");
        self.snapshots
            .record(snapshot)
            .await
            .map_err(ProgressSnapshotError::Repository)?;
        Ok(())
    }

    /// Ensure every goal and milestone has a daily snapshot for today's UTC
    /// date (roadmap 3.14, step 3). The anti-join is the only memory — no
    /// in-process "already ran" flag — so whichever tick first sees missing
    /// rows does the work: a node down at midnight, a restart, or an entity
    /// created mid-day is covered by the next hourly tick. Statuses are
    /// recomputed first, so a stale row records what the rules say today.
    /// Returns how many rows were inserted; rows another node inserted in
    /// between are skipped by the partial unique index, not an error.
    pub async fn ensure_daily_snapshots(&self) -> Result<usize, ProgressSnapshotError> {
        let today = self.clock.today();
        let (missing_goals, missing_milestones) = self
            .snapshots
            .entity_ids_missing_daily(today)
            .await
            .map_err(ProgressSnapshotError::Repository)?;
        if missing_goals.is_empty() && missing_milestones.is_empty() {
            return Ok(0);
        }
        // Fresh statuses before recording (roadmap 3.13): idempotent, and it
        // writes nothing when nothing changed.
        self.computation
            .recompute_all()
            .await
            .map_err(|err| match err {
                StatusComputationError::Repository(err) => ProgressSnapshotError::Repository(err),
            })?;
        let taken_at = self.clock.now();
        let mut inserted = 0usize;
        for chunk in missing_goals.chunks(BATCH_SIZE) {
            let goals = self
                .goals
                .find_by_ids(chunk)
                .await
                .map_err(ProgressSnapshotError::Repository)?;
            let snapshots = goals
                .iter()
                .map(|goal| NewSnapshot::from_goal(goal, taken_at, SnapshotReason::Daily))
                .collect::<Vec<_>>();
            inserted += self
                .snapshots
                .record_daily_batch(&snapshots)
                .await
                .map_err(ProgressSnapshotError::Repository)?;
        }
        for chunk in missing_milestones.chunks(BATCH_SIZE) {
            let milestones = self
                .milestones
                .find_by_ids(chunk)
                .await
                .map_err(ProgressSnapshotError::Repository)?;
            // One grouped lookup per chunk, the same rule the responses use.
            let ids: Vec<MilestoneId> = milestones.iter().map(|milestone| milestone.id).collect();
            let counts = self
                .tasks
                .task_counts_for_milestones(&ids)
                .await
                .map_err(ProgressSnapshotError::Repository)?;
            let snapshots = milestones
                .iter()
                .map(|milestone| {
                    let count = counts
                        .get(&milestone.id)
                        .copied()
                        .unwrap_or(TaskCounts { total: 0, done: 0 });
                    let percent = (percent_complete(count.total, count.done) * 100.0).round() as u8;
                    NewSnapshot::from_milestone(milestone, taken_at, SnapshotReason::Daily, percent)
                        .expect("done never exceeds total, so the percentage is 0-100")
                })
                .collect::<Vec<_>>();
            inserted += self
                .snapshots
                .record_daily_batch(&snapshots)
                .await
                .map_err(ProgressSnapshotError::Repository)?;
        }
        Ok(inserted)
    }
}

/// A [`StatusSnapshotTrigger`] that records the manual-change snapshot
/// through the service (roadmap 3.14, step 2). The override service already
/// treats a failure as a warning, so the service's error is only stringified
/// for that log line.
pub struct RecordingStatusSnapshotTrigger {
    snapshots: Arc<ProgressSnapshotService>,
}

impl RecordingStatusSnapshotTrigger {
    pub fn new(snapshots: Arc<ProgressSnapshotService>) -> Self {
        Self { snapshots }
    }
}

#[async_trait]
impl StatusSnapshotTrigger for RecordingStatusSnapshotTrigger {
    async fn snapshot(&self, target: StatusChangeTarget) -> Result<(), StatusSnapshotError> {
        self.snapshots
            .record_manual_change(target)
            .await
            .map_err(|err| StatusSnapshotError(err.to_string()))
    }
}

#[cfg(test)]
mod tests {
    use std::collections::HashMap;
    use std::sync::Mutex;
    use std::sync::atomic::{AtomicUsize, Ordering};

    use chrono::{DateTime, NaiveDate, Utc};
    use domain::{
        Goal, GoalId, GoalMilestone, Milestone, MilestoneId, ProgressSnapshot, Status,
        StatusSource, Task, TaskId, TaskStatus, UserId,
    };

    use crate::pagination::{Page, PageRequest};
    use crate::ports::goal_milestone_repository::GoalMilestoneRepository;
    use crate::ports::goal_repository::fakes::InMemoryGoalRepository;
    use crate::ports::milestone_repository::fakes::InMemoryMilestoneRepository;
    use crate::ports::progress_snapshot_repository::fakes::InMemoryProgressSnapshotRepository;
    use crate::ports::task_repository::fakes::InMemoryTaskRepository;
    use crate::ports::{TaskCounts, TaskListFilter};
    use crate::status_computation::{
        StatusComputationService,
        fakes::{RecomputeCall, RecordingRecomputation, noop_recomputation},
    };

    use super::*;

    /// A [`Clock`] pinned to one instant: `today()` falls out of it by
    /// default, so the two can never disagree.
    struct FixedInstant(DateTime<Utc>);

    impl Clock for FixedInstant {
        fn now(&self) -> DateTime<Utc> {
            self.0
        }
    }

    /// A [`Clock`] whose instant tests can advance between calls, so two
    /// snapshots recorded in one test get distinct `taken_at` values and the
    /// history order is deterministic.
    struct AdvancingClock(Mutex<DateTime<Utc>>);

    impl AdvancingClock {
        fn new(instant: DateTime<Utc>) -> Self {
            Self(Mutex::new(instant))
        }

        fn advance(&self, to: DateTime<Utc>) {
            *self.0.lock().unwrap() = to;
        }
    }

    impl Clock for AdvancingClock {
        fn now(&self) -> DateTime<Utc> {
            *self.0.lock().unwrap()
        }
    }

    /// A [`ProgressSnapshotRepository`] whose every call fails, to pin that a
    /// store failure surfaces as the typed error.
    struct FailingSnapshotRepository;

    #[async_trait]
    impl ProgressSnapshotRepository for FailingSnapshotRepository {
        async fn record(
            &self,
            _snapshot: NewSnapshot,
        ) -> Result<ProgressSnapshot, RepositoryError> {
            Err(RepositoryError::Unexpected(
                "faking a snapshot store failure".to_owned(),
            ))
        }

        async fn record_daily_batch(
            &self,
            _snapshots: &[NewSnapshot],
        ) -> Result<usize, RepositoryError> {
            Err(RepositoryError::Unexpected(
                "faking a snapshot store failure".to_owned(),
            ))
        }

        async fn entity_ids_missing_daily(
            &self,
            _date: NaiveDate,
        ) -> Result<(Vec<GoalId>, Vec<MilestoneId>), RepositoryError> {
            Err(RepositoryError::Unexpected(
                "faking a snapshot store failure".to_owned(),
            ))
        }

        async fn list_for_goal(
            &self,
            _goal_id: GoalId,
        ) -> Result<Vec<ProgressSnapshot>, RepositoryError> {
            Err(RepositoryError::Unexpected(
                "faking a snapshot store failure".to_owned(),
            ))
        }

        async fn list_for_milestone(
            &self,
            _milestone_id: MilestoneId,
        ) -> Result<Vec<ProgressSnapshot>, RepositoryError> {
            Err(RepositoryError::Unexpected(
                "faking a snapshot store failure".to_owned(),
            ))
        }
    }

    /// A [`TaskRepository`] that counts its grouped task-count lookups and
    /// answers them with no tasks; the sweep calls nothing else on it, so
    /// every other method fails loudly if that ever changes.
    struct CountingTaskRepository {
        counts_lookups: AtomicUsize,
    }

    impl CountingTaskRepository {
        fn new() -> Self {
            Self {
                counts_lookups: AtomicUsize::new(0),
            }
        }

        fn lookups(&self) -> usize {
            self.counts_lookups.load(Ordering::SeqCst)
        }
    }

    #[async_trait]
    impl TaskRepository for CountingTaskRepository {
        async fn create(&self, _task: Task) -> Result<Task, RepositoryError> {
            Err(RepositoryError::Unexpected(
                "not used by the sweep".to_owned(),
            ))
        }

        async fn find_by_id(&self, _id: TaskId) -> Result<Option<Task>, RepositoryError> {
            Err(RepositoryError::Unexpected(
                "not used by the sweep".to_owned(),
            ))
        }

        async fn find_by_ids(&self, _ids: &[TaskId]) -> Result<Vec<Task>, RepositoryError> {
            Err(RepositoryError::Unexpected(
                "not used by the sweep".to_owned(),
            ))
        }

        async fn task_counts_for_milestones(
            &self,
            _milestone_ids: &[MilestoneId],
        ) -> Result<HashMap<MilestoneId, TaskCounts>, RepositoryError> {
            self.counts_lookups.fetch_add(1, Ordering::SeqCst);
            Ok(HashMap::new())
        }

        async fn update(&self, _task: Task) -> Result<Task, RepositoryError> {
            Err(RepositoryError::Unexpected(
                "not used by the sweep".to_owned(),
            ))
        }

        async fn set_status(
            &self,
            _id: TaskId,
            _status: TaskStatus,
        ) -> Result<Task, RepositoryError> {
            Err(RepositoryError::Unexpected(
                "not used by the sweep".to_owned(),
            ))
        }

        async fn set_assignee(
            &self,
            _id: TaskId,
            _assignee_id: Option<UserId>,
        ) -> Result<Task, RepositoryError> {
            Err(RepositoryError::Unexpected(
                "not used by the sweep".to_owned(),
            ))
        }

        async fn delete(&self, _id: TaskId) -> Result<(), RepositoryError> {
            Err(RepositoryError::Unexpected(
                "not used by the sweep".to_owned(),
            ))
        }

        async fn list_page(
            &self,
            _filter: &TaskListFilter,
            _page: &PageRequest,
        ) -> Result<Page<Task>, RepositoryError> {
            Err(RepositoryError::Unexpected(
                "not used by the sweep".to_owned(),
            ))
        }
    }

    /// An empty [`GoalMilestoneRepository`]: the sweep tests link nothing, so
    /// every read is empty and the writers are never exercised.
    struct EmptyGoalMilestones;

    #[async_trait]
    impl GoalMilestoneRepository for EmptyGoalMilestones {
        async fn link(&self, _link: GoalMilestone) -> Result<bool, RepositoryError> {
            Ok(false)
        }

        async fn unlink(
            &self,
            _goal_id: GoalId,
            _milestone_id: MilestoneId,
        ) -> Result<bool, RepositoryError> {
            Ok(false)
        }

        async fn milestones_for_goal(
            &self,
            _goal_id: GoalId,
        ) -> Result<Vec<Milestone>, RepositoryError> {
            Ok(vec![])
        }

        async fn goals_for_milestone(
            &self,
            _milestone_id: MilestoneId,
        ) -> Result<Vec<Goal>, RepositoryError> {
            Ok(vec![])
        }

        async fn milestones_for_goal_page(
            &self,
            _goal_id: GoalId,
            page: &PageRequest,
        ) -> Result<Page<Milestone>, RepositoryError> {
            Ok(Page {
                items: vec![],
                total: 0,
                limit: page.limit,
                offset: page.offset,
            })
        }

        async fn goals_for_milestone_page(
            &self,
            _milestone_id: MilestoneId,
            page: &PageRequest,
        ) -> Result<Page<Goal>, RepositoryError> {
            Ok(Page {
                items: vec![],
                total: 0,
                limit: page.limit,
                offset: page.offset,
            })
        }
    }

    fn test_goal(status: Status) -> Goal {
        let now = Utc::now();
        Goal {
            id: GoalId::new(),
            title: "Snapshot goal".to_owned(),
            description: None,
            status,
            status_override: None,
            target_date: None,
            owner_id: None,
            created_at: now,
            updated_at: now,
        }
    }

    fn test_milestone(status: Status) -> Milestone {
        let now = Utc::now();
        Milestone {
            id: MilestoneId::new(),
            title: "Snapshot milestone".to_owned(),
            description: None,
            status,
            status_override: None,
            target_date: None,
            owner_id: None,
            created_at: now,
            updated_at: now,
        }
    }

    fn test_task(milestone_id: Option<MilestoneId>, status: TaskStatus) -> Task {
        let now = Utc::now();
        Task {
            id: TaskId::new(),
            milestone_id,
            title: "Snapshot task".to_owned(),
            description: None,
            status,
            target_date: None,
            assignee_id: None,
            created_at: now,
            updated_at: now,
        }
    }

    /// The service over the in-memory fakes. The service never writes
    /// overrides itself — the trigger follows a write the override service
    /// already committed — so tests set the stored state directly.
    fn setup(
        clock: Arc<dyn Clock>,
        computation: Arc<dyn StatusRecomputation>,
    ) -> (
        Arc<InMemoryGoalRepository>,
        Arc<InMemoryMilestoneRepository>,
        Arc<InMemoryTaskRepository>,
        Arc<InMemoryProgressSnapshotRepository>,
        ProgressSnapshotService,
    ) {
        let goals = Arc::new(InMemoryGoalRepository::new());
        let milestones = Arc::new(InMemoryMilestoneRepository::new());
        let tasks = Arc::new(InMemoryTaskRepository::new());
        let snapshots = Arc::new(InMemoryProgressSnapshotRepository::new());
        let service = ProgressSnapshotService::new(
            goals.clone(),
            milestones.clone(),
            tasks.clone(),
            snapshots.clone(),
            computation,
            clock,
        );
        (goals, milestones, tasks, snapshots, service)
    }

    #[tokio::test]
    async fn a_goal_override_set_records_a_manual_snapshot() {
        let (goals, _milestones, _tasks, snapshots, service) =
            setup(Arc::new(FixedInstant(Utc::now())), noop_recomputation());
        let goal = test_goal(Status::OnTrack);
        goals.create(goal.clone()).await.unwrap();
        // The override write the trigger follows: effective OffTrack, manual.
        goals
            .set_status_override(goal.id, Some(Status::OffTrack))
            .await
            .unwrap();

        service
            .record_manual_change(StatusChangeTarget::Goal(goal.id))
            .await
            .unwrap();

        let listed = snapshots.list_for_goal(goal.id).await.unwrap();
        let [snapshot] = listed.as_slice() else {
            panic!("exactly one snapshot expected");
        };
        assert_eq!(snapshot.status, Status::OffTrack);
        assert_eq!(snapshot.source, StatusSource::ManualOverride);
        assert_eq!(snapshot.reason, SnapshotReason::ManualChange);
        // Goals have no completion percentage.
        assert_eq!(snapshot.percent_complete(), None);
    }

    #[tokio::test]
    async fn a_goal_override_clear_records_the_automatic_snapshot() {
        // Two distinct instants so the two rows order deterministically.
        let set_at = Utc::now();
        let clock = Arc::new(AdvancingClock::new(set_at));
        let (goals, _milestones, _tasks, snapshots, service) =
            setup(clock.clone(), noop_recomputation());
        let goal = test_goal(Status::AtRisk);
        goals.create(goal.clone()).await.unwrap();
        goals
            .set_status_override(goal.id, Some(Status::Complete))
            .await
            .unwrap();
        service
            .record_manual_change(StatusChangeTarget::Goal(goal.id))
            .await
            .unwrap();
        // The clear: the effective status falls back to the stored automatic
        // one, so the snapshot is computed-sourced.
        goals.set_status_override(goal.id, None).await.unwrap();
        clock.advance(set_at + chrono::Duration::seconds(1));

        service
            .record_manual_change(StatusChangeTarget::Goal(goal.id))
            .await
            .unwrap();

        let listed = snapshots.list_for_goal(goal.id).await.unwrap();
        assert_eq!(listed.len(), 2);
        assert_eq!(listed[0].status, Status::Complete);
        assert_eq!(listed[0].source, StatusSource::ManualOverride);
        assert_eq!(listed[1].status, Status::AtRisk);
        assert_eq!(listed[1].source, StatusSource::Computed);
    }

    #[tokio::test]
    async fn a_milestone_snapshot_carries_the_same_percent_as_the_responses() {
        let (goals, milestones, tasks, snapshots, service) =
            setup(Arc::new(FixedInstant(Utc::now())), noop_recomputation());
        goals.create(test_goal(Status::OnTrack)).await.unwrap();
        // Zero tasks, partial, and all done.
        let empty = test_milestone(Status::OnTrack);
        milestones.create(empty.clone()).await.unwrap();
        let partial = test_milestone(Status::OnTrack);
        milestones.create(partial.clone()).await.unwrap();
        for status in [
            TaskStatus::Done,
            TaskStatus::InProgress,
            TaskStatus::InProgress,
            TaskStatus::InProgress,
        ] {
            tasks
                .create(test_task(Some(partial.id), status))
                .await
                .unwrap();
        }
        let done = test_milestone(Status::Complete);
        milestones.create(done.clone()).await.unwrap();
        for _ in 0..2 {
            tasks
                .create(test_task(Some(done.id), TaskStatus::Done))
                .await
                .unwrap();
        }

        for milestone in [&empty, &partial, &done] {
            service
                .record_manual_change(StatusChangeTarget::Milestone(milestone.id))
                .await
                .unwrap();
        }

        assert_eq!(
            snapshots.list_for_milestone(empty.id).await.unwrap()[0].percent_complete(),
            Some(0)
        );
        assert_eq!(
            snapshots.list_for_milestone(partial.id).await.unwrap()[0].percent_complete(),
            Some(25)
        );
        assert_eq!(
            snapshots.list_for_milestone(done.id).await.unwrap()[0].percent_complete(),
            Some(100)
        );
    }

    #[tokio::test]
    async fn a_deleted_entity_leaves_no_snapshot_and_is_not_an_error() {
        let (_goals, _milestones, _tasks, snapshots, service) =
            setup(Arc::new(FixedInstant(Utc::now())), noop_recomputation());

        service
            .record_manual_change(StatusChangeTarget::Goal(GoalId::new()))
            .await
            .unwrap();
        service
            .record_manual_change(StatusChangeTarget::Milestone(MilestoneId::new()))
            .await
            .unwrap();

        assert!(
            snapshots
                .list_for_goal(GoalId::new())
                .await
                .unwrap()
                .is_empty()
        );
    }

    #[tokio::test]
    async fn a_repository_failure_surfaces_as_the_typed_error() {
        let goals = Arc::new(InMemoryGoalRepository::new());
        let goal = test_goal(Status::OnTrack);
        goals.create(goal.clone()).await.unwrap();
        let service = ProgressSnapshotService::new(
            goals,
            Arc::new(InMemoryMilestoneRepository::new()),
            Arc::new(InMemoryTaskRepository::new()),
            Arc::new(FailingSnapshotRepository),
            noop_recomputation(),
            Arc::new(FixedInstant(Utc::now())),
        );

        let error = service
            .record_manual_change(StatusChangeTarget::Goal(goal.id))
            .await
            .expect_err("the faked store failure");

        assert!(matches!(error, ProgressSnapshotError::Repository(_)));
    }

    /// Record one goal snapshot with the clock pinned to `instant`.
    async fn record_at(instant: DateTime<Utc>) -> ProgressSnapshot {
        let (goals, _milestones, _tasks, snapshots, service) =
            setup(Arc::new(FixedInstant(instant)), noop_recomputation());
        let goal = test_goal(Status::OnTrack);
        goals.create(goal.clone()).await.unwrap();
        service
            .record_manual_change(StatusChangeTarget::Goal(goal.id))
            .await
            .unwrap();
        snapshots
            .list_for_goal(goal.id)
            .await
            .unwrap()
            .pop()
            .expect("one snapshot")
    }

    #[tokio::test]
    async fn the_snapshot_instant_comes_from_the_injected_clock() {
        let before_midnight = NaiveDate::from_ymd_opt(2026, 11, 19)
            .unwrap()
            .and_hms_opt(23, 59, 59)
            .unwrap()
            .and_utc();
        let after_midnight = NaiveDate::from_ymd_opt(2026, 11, 20)
            .unwrap()
            .and_hms_opt(0, 0, 1)
            .unwrap()
            .and_utc();

        // Just before UTC midnight: the row stays on the old day...
        let before = record_at(before_midnight).await;
        assert_eq!(before.taken_at, before_midnight);
        assert_eq!(
            before.snapshot_date,
            NaiveDate::from_ymd_opt(2026, 11, 19).unwrap()
        );

        // ...and just after it, the row lands on the new one.
        let after = record_at(after_midnight).await;
        assert_eq!(after.taken_at, after_midnight);
        assert_eq!(
            after.snapshot_date,
            NaiveDate::from_ymd_opt(2026, 11, 20).unwrap()
        );
    }

    #[tokio::test]
    async fn the_first_daily_sweep_records_one_row_per_entity() {
        let clock = Arc::new(FixedInstant(
            NaiveDate::from_ymd_opt(2026, 11, 19)
                .unwrap()
                .and_hms_opt(9, 0, 0)
                .unwrap()
                .and_utc(),
        ));
        let (goals, milestones, tasks, snapshots, service) = setup(clock, noop_recomputation());
        // An automatic goal, an overridden one, and a half-done milestone.
        let automatic = test_goal(Status::OnTrack);
        goals.create(automatic.clone()).await.unwrap();
        snapshots.register_goal(automatic.id);
        let overridden = test_goal(Status::OnTrack);
        goals.create(overridden.clone()).await.unwrap();
        goals
            .set_status_override(overridden.id, Some(Status::OffTrack))
            .await
            .unwrap();
        snapshots.register_goal(overridden.id);
        let milestone = test_milestone(Status::AtRisk);
        milestones.create(milestone.clone()).await.unwrap();
        for status in [TaskStatus::Done, TaskStatus::InProgress] {
            tasks
                .create(test_task(Some(milestone.id), status))
                .await
                .unwrap();
        }
        snapshots.register_milestone(milestone.id);

        assert_eq!(service.ensure_daily_snapshots().await.unwrap(), 3);

        // Each entity: one daily row of its effective status and source.
        let listed = snapshots.list_for_goal(automatic.id).await.unwrap();
        let [goal_snapshot] = listed.as_slice() else {
            panic!("exactly one snapshot expected");
        };
        assert_eq!(goal_snapshot.status, Status::OnTrack);
        assert_eq!(goal_snapshot.source, StatusSource::Computed);
        assert_eq!(goal_snapshot.reason, SnapshotReason::Daily);
        assert_eq!(goal_snapshot.percent_complete(), None);
        assert_eq!(
            goal_snapshot.snapshot_date,
            NaiveDate::from_ymd_opt(2026, 11, 19).unwrap()
        );

        let listed = snapshots.list_for_goal(overridden.id).await.unwrap();
        let [overridden_snapshot] = listed.as_slice() else {
            panic!("exactly one snapshot expected");
        };
        assert_eq!(overridden_snapshot.status, Status::OffTrack);
        assert_eq!(overridden_snapshot.source, StatusSource::ManualOverride);
        assert_eq!(overridden_snapshot.reason, SnapshotReason::Daily);

        let listed = snapshots.list_for_milestone(milestone.id).await.unwrap();
        let [milestone_snapshot] = listed.as_slice() else {
            panic!("exactly one snapshot expected");
        };
        assert_eq!(milestone_snapshot.status, Status::AtRisk);
        assert_eq!(milestone_snapshot.source, StatusSource::Computed);
        assert_eq!(milestone_snapshot.reason, SnapshotReason::Daily);
        assert_eq!(milestone_snapshot.percent_complete(), Some(50));
    }

    #[tokio::test]
    async fn a_second_daily_sweep_the_same_day_does_nothing_and_skips_recompute() {
        let recompute = Arc::new(RecordingRecomputation::new());
        let (goals, _milestones, _tasks, snapshots, service) =
            setup(Arc::new(FixedInstant(Utc::now())), recompute.clone());
        let goal = test_goal(Status::OnTrack);
        goals.create(goal.clone()).await.unwrap();
        snapshots.register_goal(goal.id);

        assert_eq!(service.ensure_daily_snapshots().await.unwrap(), 1);
        assert_eq!(recompute.calls(), vec![RecomputeCall::All]);

        // Nothing is missing any more: no work, and no recompute either.
        assert_eq!(service.ensure_daily_snapshots().await.unwrap(), 0);
        assert_eq!(recompute.calls(), vec![RecomputeCall::All]);

        let listed = snapshots.list_for_goal(goal.id).await.unwrap();
        assert_eq!(listed.len(), 1);
    }

    #[tokio::test]
    async fn the_daily_sweep_refreshes_stale_statuses_before_recording() {
        let clock = Arc::new(FixedInstant(
            NaiveDate::from_ymd_opt(2026, 11, 19)
                .unwrap()
                .and_hms_opt(9, 0, 0)
                .unwrap()
                .and_utc(),
        ));
        let (goals, milestones, tasks, snapshots, _service) =
            setup(clock.clone(), noop_recomputation());
        // A past-due milestone still carrying its stale On Track status — the
        // row shape a database holds when today moves past the target date.
        let created = NaiveDate::from_ymd_opt(2026, 1, 1)
            .unwrap()
            .and_hms_opt(9, 0, 0)
            .unwrap()
            .and_utc();
        let stale = Milestone {
            id: MilestoneId::new(),
            title: "Stale milestone".to_owned(),
            description: None,
            status: Status::OnTrack,
            status_override: None,
            target_date: Some(NaiveDate::from_ymd_opt(2026, 1, 31).unwrap()),
            owner_id: None,
            created_at: created,
            updated_at: created,
        };
        milestones.create(stale.clone()).await.unwrap();
        snapshots.register_milestone(stale.id);

        // The real recompute over the same fakes: it can actually refresh.
        let computation: Arc<dyn StatusRecomputation> = Arc::new(StatusComputationService::new(
            goals.clone(),
            milestones.clone(),
            Arc::new(EmptyGoalMilestones),
            tasks.clone(),
            clock.clone(),
        ));
        let service = ProgressSnapshotService::new(
            goals,
            milestones.clone(),
            tasks,
            snapshots.clone(),
            computation,
            clock,
        );

        assert_eq!(service.ensure_daily_snapshots().await.unwrap(), 1);

        // The recompute ran before the record: the stored status is fresh...
        let refreshed = milestones.find_by_id(stale.id).await.unwrap().unwrap();
        assert_eq!(refreshed.status, Status::OffTrack);
        // ...and the snapshot carries the fresh value, not the stale one.
        let listed = snapshots.list_for_milestone(stale.id).await.unwrap();
        let [snapshot] = listed.as_slice() else {
            panic!("exactly one snapshot expected");
        };
        assert_eq!(snapshot.status, Status::OffTrack);
        assert_eq!(snapshot.source, StatusSource::Computed);
        assert_eq!(snapshot.reason, SnapshotReason::Daily);
    }

    #[tokio::test]
    async fn an_entity_created_after_the_first_sweep_gets_a_row_on_the_next_call() {
        let (goals, _milestones, _tasks, snapshots, service) =
            setup(Arc::new(FixedInstant(Utc::now())), noop_recomputation());
        let first = test_goal(Status::OnTrack);
        goals.create(first.clone()).await.unwrap();
        snapshots.register_goal(first.id);

        assert_eq!(service.ensure_daily_snapshots().await.unwrap(), 1);

        // Created mid-day: the next tick's anti-join still sees it as missing.
        let second = test_goal(Status::Complete);
        goals.create(second.clone()).await.unwrap();
        snapshots.register_goal(second.id);

        assert_eq!(service.ensure_daily_snapshots().await.unwrap(), 1);

        // The new entity got its row; the existing one was left untouched.
        let listed = snapshots.list_for_goal(first.id).await.unwrap();
        assert_eq!(listed.len(), 1);
        let listed = snapshots.list_for_goal(second.id).await.unwrap();
        assert_eq!(listed.len(), 1);
        assert_eq!(listed[0].reason, SnapshotReason::Daily);
    }

    #[tokio::test]
    async fn a_manual_change_snapshot_does_not_prevent_the_daily_row() {
        let start = Utc::now();
        let clock = Arc::new(AdvancingClock::new(start));
        let (goals, _milestones, _tasks, snapshots, service) =
            setup(clock.clone(), noop_recomputation());
        let goal = test_goal(Status::OnTrack);
        goals.create(goal.clone()).await.unwrap();
        goals
            .set_status_override(goal.id, Some(Status::OffTrack))
            .await
            .unwrap();
        snapshots.register_goal(goal.id);

        service
            .record_manual_change(StatusChangeTarget::Goal(goal.id))
            .await
            .unwrap();
        clock.advance(start + chrono::Duration::seconds(1));

        // The manual row does not count as a daily one...
        assert_eq!(service.ensure_daily_snapshots().await.unwrap(), 1);

        let listed = snapshots.list_for_goal(goal.id).await.unwrap();
        assert_eq!(listed.len(), 2);
        assert_eq!(listed[0].reason, SnapshotReason::ManualChange);
        assert_eq!(listed[1].reason, SnapshotReason::Daily);
        // ...and the daily row records the override still in force.
        assert_eq!(listed[1].status, Status::OffTrack);
        assert_eq!(listed[1].source, StatusSource::ManualOverride);
    }

    #[tokio::test]
    async fn a_new_utc_date_records_a_fresh_row_set() {
        let day_one = NaiveDate::from_ymd_opt(2026, 11, 19)
            .unwrap()
            .and_hms_opt(9, 0, 0)
            .unwrap()
            .and_utc();
        let day_two = NaiveDate::from_ymd_opt(2026, 11, 20)
            .unwrap()
            .and_hms_opt(9, 0, 0)
            .unwrap()
            .and_utc();
        let clock = Arc::new(AdvancingClock::new(day_one));
        let (goals, _milestones, _tasks, snapshots, service) =
            setup(clock.clone(), noop_recomputation());
        let goal = test_goal(Status::OnTrack);
        goals.create(goal.clone()).await.unwrap();
        snapshots.register_goal(goal.id);

        assert_eq!(service.ensure_daily_snapshots().await.unwrap(), 1);
        clock.advance(day_two);
        assert_eq!(service.ensure_daily_snapshots().await.unwrap(), 1);

        // One row per day; the old rows remain.
        let listed = snapshots.list_for_goal(goal.id).await.unwrap();
        assert_eq!(listed.len(), 2);
        assert_eq!(
            listed[0].snapshot_date,
            NaiveDate::from_ymd_opt(2026, 11, 19).unwrap()
        );
        assert_eq!(
            listed[1].snapshot_date,
            NaiveDate::from_ymd_opt(2026, 11, 20).unwrap()
        );
        assert_eq!(listed[0].reason, SnapshotReason::Daily);
        assert_eq!(listed[1].reason, SnapshotReason::Daily);
    }

    #[tokio::test]
    async fn more_than_two_hundred_milestones_are_chunked() {
        let clock = Arc::new(FixedInstant(Utc::now()));
        let (goals, milestones, _tasks, snapshots, _service) =
            setup(clock.clone(), noop_recomputation());
        let counting_tasks = Arc::new(CountingTaskRepository::new());
        let service = ProgressSnapshotService::new(
            goals,
            milestones.clone(),
            counting_tasks.clone(),
            snapshots.clone(),
            noop_recomputation(),
            clock,
        );

        for _ in 0..250 {
            let milestone = test_milestone(Status::OnTrack);
            milestones.create(milestone.clone()).await.unwrap();
            snapshots.register_milestone(milestone.id);
        }

        assert_eq!(service.ensure_daily_snapshots().await.unwrap(), 250);
        // 250 milestones chunk into 200 + 50: one grouped counts call each.
        assert_eq!(counting_tasks.lookups(), 2);
    }
}
