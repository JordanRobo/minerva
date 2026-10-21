//! Automatic status computation (roadmap 3.13): recompute the stored
//! automatic status of milestones and goals from their tasks, dates and
//! links, using the pure domain rules (D19).
//!
//! The rules live here, not in handlers: the mutation services call the
//! [`StatusRecomputation`] trait after each successful write (roadmap 3.13,
//! step 4), best-effort — a failure only warns, and the scheduled sweep
//! (step 5) heals whatever they miss. Unknown ids are skipped silently (an
//! entity deleted mid-flight is not an error), a computed value equal to the
//! stored one writes nothing (the writer's own no-op rule), overrides are
//! never touched (only `set_automatic_status` writes the automatic status),
//! and no snapshot fires — snapshots follow manual changes and the daily
//! sweep only (D5).

use std::sync::Arc;

use domain::{Goal, GoalId, MilestoneId, MilestoneProgress, compute_milestone_status};

use crate::ports::{
    Clock, GoalMilestoneRepository, GoalRepository, MilestoneRepository, RepositoryError,
    TaskRepository,
};

/// The recompute capability the mutation services call after a write
/// (roadmap 3.13, step 4). A trait rather than the concrete service so those
/// services depend on a small seam they can fake in unit tests, mirroring
/// [`crate::ports::StatusSnapshotTrigger`]. Implementors recompute stored
/// automatic statuses only; overrides and snapshots are never their concern.
#[async_trait::async_trait]
pub trait StatusRecomputation: Send + Sync {
    /// Recompute the automatic status of each given milestone from its task
    /// counts and dates (D19). Unknown ids are skipped.
    async fn recompute_milestones(
        &self,
        milestone_ids: &[MilestoneId],
    ) -> Result<usize, StatusComputationError>;

    /// Recompute the automatic status of each given goal from its linked
    /// milestones' effective statuses (D19's rollup). Unknown ids are skipped.
    async fn recompute_goals(&self, goal_ids: &[GoalId]) -> Result<usize, StatusComputationError>;

    /// Recompute the automatic status of every goal linked to any of the
    /// given milestones. Unknown milestone ids simply have no links.
    async fn recompute_goals_for_milestones(
        &self,
        milestone_ids: &[MilestoneId],
    ) -> Result<usize, StatusComputationError>;
}

/// An error from recomputing automatic statuses. Unknown ids are not an
/// error — they are skipped (an entity may be deleted mid-flight).
#[derive(Debug)]
pub enum StatusComputationError {
    /// The repository failed.
    Repository(RepositoryError),
}

impl std::fmt::Display for StatusComputationError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            StatusComputationError::Repository(err) => write!(f, "{err}"),
        }
    }
}

impl std::error::Error for StatusComputationError {}

impl From<RepositoryError> for StatusComputationError {
    fn from(err: RepositoryError) -> Self {
        StatusComputationError::Repository(err)
    }
}

/// How many rows a recompute actually changed, split by entity kind.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RecomputedCounts {
    pub milestones: usize,
    pub goals: usize,
}

/// How many ids one recompute batch holds (roadmap 3.13's sweep): each batch
/// is one milestone fetch and one grouped task-count lookup.
const BATCH_SIZE: usize = 200;

/// Recomputes the automatic status of milestones and goals (roadmap 3.13).
/// Milestones come from their task counts and dates; goals roll up their
/// linked milestones' effective statuses, so an overridden milestone counts
/// as its override (D19).
pub struct StatusComputationService {
    goals: Arc<dyn GoalRepository>,
    milestones: Arc<dyn MilestoneRepository>,
    goal_milestones: Arc<dyn GoalMilestoneRepository>,
    tasks: Arc<dyn TaskRepository>,
    clock: Arc<dyn Clock>,
}

impl StatusComputationService {
    pub fn new(
        goals: Arc<dyn GoalRepository>,
        milestones: Arc<dyn MilestoneRepository>,
        goal_milestones: Arc<dyn GoalMilestoneRepository>,
        tasks: Arc<dyn TaskRepository>,
        clock: Arc<dyn Clock>,
    ) -> Self {
        Self {
            goals,
            milestones,
            goal_milestones,
            tasks,
            clock,
        }
    }

    /// Recompute the automatic status of each given milestone from its task
    /// counts and dates (D19), with one grouped task-count lookup for the
    /// whole batch. Unknown ids are skipped; returns how many changed.
    pub async fn recompute_milestones(
        &self,
        milestone_ids: &[MilestoneId],
    ) -> Result<usize, StatusComputationError> {
        let milestones = self.milestones.find_by_ids(milestone_ids).await?;
        if milestones.is_empty() {
            return Ok(0);
        }
        let ids: Vec<MilestoneId> = milestones.iter().map(|milestone| milestone.id).collect();
        // One grouped lookup for the whole batch (roadmap 3.13).
        let counts = self.tasks.task_counts_for_milestones(&ids).await?;
        let today = self.clock.today();
        let mut changed = 0usize;
        for milestone in &milestones {
            // The start date is the creation date in v1 (D19); "today" comes
            // from the injected clock, UTC.
            let count = counts
                .get(&milestone.id)
                .expect("every requested id is present");
            let progress = MilestoneProgress {
                total_tasks: count.total,
                done_tasks: count.done,
                start_date: milestone.created_at.date_naive(),
                target_date: milestone.target_date,
            };
            let computed = compute_milestone_status(&progress, today);
            // The writer no-ops when the value is unchanged and reports it.
            if self
                .milestones
                .set_automatic_status(milestone.id, computed)
                .await?
            {
                changed += 1;
            }
        }
        Ok(changed)
    }

    /// Recompute the automatic status of each given goal from its linked
    /// milestones' effective statuses (D19's rollup). Unknown ids are
    /// skipped; returns how many changed.
    pub async fn recompute_goals(
        &self,
        goal_ids: &[GoalId],
    ) -> Result<usize, StatusComputationError> {
        let mut changed = 0usize;
        for &goal_id in goal_ids {
            let Some(goal) = self.goals.find_by_id(goal_id).await? else {
                continue;
            };
            if self.recompute_one_goal(&goal).await? {
                changed += 1;
            }
        }
        Ok(changed)
    }

    /// Recompute the automatic status of every goal linked to any of the
    /// given milestones. Unknown milestone ids simply have no links; a goal
    /// linked to several of them is recomputed once.
    pub async fn recompute_goals_for_milestones(
        &self,
        milestone_ids: &[MilestoneId],
    ) -> Result<usize, StatusComputationError> {
        let mut goal_ids = Vec::new();
        for &milestone_id in milestone_ids {
            for goal in self
                .goal_milestones
                .goals_for_milestone(milestone_id)
                .await?
            {
                if !goal_ids.contains(&goal.id) {
                    goal_ids.push(goal.id);
                }
            }
        }
        self.recompute_goals(&goal_ids).await
    }

    /// Recompute everything: all milestones first (a goal's rollup depends
    /// on its linked milestones' effective statuses), then all goals, in
    /// batches of [`BATCH_SIZE`]. Returns how many rows each phase changed.
    pub async fn recompute_all(&self) -> Result<RecomputedCounts, StatusComputationError> {
        let milestone_ids = self.milestones.list_all_ids().await?;
        let mut milestones_changed = 0usize;
        for batch in milestone_ids.chunks(BATCH_SIZE) {
            milestones_changed += self.recompute_milestones(batch).await?;
        }
        let goal_ids = self.goals.list_all_ids().await?;
        let goals_changed = self.recompute_goals(&goal_ids).await?;
        Ok(RecomputedCounts {
            milestones: milestones_changed,
            goals: goals_changed,
        })
    }

    /// Recompute one loaded goal from its linked milestones' effective
    /// statuses; returns whether the automatic status changed.
    async fn recompute_one_goal(&self, goal: &Goal) -> Result<bool, StatusComputationError> {
        let milestones = self.goal_milestones.milestones_for_goal(goal.id).await?;
        // The rollup runs on effective statuses: an overridden milestone
        // counts as its override (D19).
        let computed = goal.compute_status_from_milestones(&milestones);
        // The writer no-ops when the value is unchanged and reports it.
        Ok(self.goals.set_automatic_status(goal.id, computed).await?)
    }
}

#[async_trait::async_trait]
impl StatusRecomputation for StatusComputationService {
    async fn recompute_milestones(
        &self,
        milestone_ids: &[MilestoneId],
    ) -> Result<usize, StatusComputationError> {
        // The inherent method by the same name does the work; the explicit
        // path keeps the delegation unambiguous.
        Self::recompute_milestones(self, milestone_ids).await
    }

    async fn recompute_goals(&self, goal_ids: &[GoalId]) -> Result<usize, StatusComputationError> {
        Self::recompute_goals(self, goal_ids).await
    }

    async fn recompute_goals_for_milestones(
        &self,
        milestone_ids: &[MilestoneId],
    ) -> Result<usize, StatusComputationError> {
        Self::recompute_goals_for_milestones(self, milestone_ids).await
    }
}

#[cfg(test)]
/// A [`StatusRecomputation`] double for the mutation services' tests: it
/// records every call (method and ids) and fails on demand, so a test can
/// prove both what a mutation triggers and that a failure is only warned
/// about. Shared from here because every service's test module needs the
/// same double (the house pattern for shared test doubles is a `pub mod
/// fakes` in the defining module).
pub mod fakes {
    use std::sync::{Arc, Mutex};

    use domain::{GoalId, MilestoneId};

    use crate::ports::RepositoryError;
    use crate::status_computation::{StatusComputationError, StatusRecomputation};

    /// Which recompute a call asked for, with the ids it was given.
    #[derive(Debug, Clone, PartialEq, Eq)]
    pub enum RecomputeCall {
        Milestones(Vec<MilestoneId>),
        Goals(Vec<GoalId>),
        GoalsForMilestones(Vec<MilestoneId>),
    }

    #[derive(Default)]
    pub struct RecordingRecomputation {
        fail: Mutex<bool>,
        calls: Mutex<Vec<RecomputeCall>>,
    }

    impl RecordingRecomputation {
        pub fn new() -> Self {
            Self::default()
        }

        /// Make every recompute fail (or succeed again).
        pub fn set_fail(&self, fail: bool) {
            *self.fail.lock().unwrap() = fail;
        }

        /// The calls made, in order.
        pub fn calls(&self) -> Vec<RecomputeCall> {
            self.calls.lock().unwrap().clone()
        }

        fn record(&self, call: RecomputeCall) -> Result<(), StatusComputationError> {
            self.calls.lock().unwrap().push(call);
            if *self.fail.lock().unwrap() {
                return Err(StatusComputationError::Repository(
                    RepositoryError::Unexpected("faking a recompute failure".to_owned()),
                ));
            }
            Ok(())
        }
    }

    #[async_trait::async_trait]
    impl StatusRecomputation for RecordingRecomputation {
        async fn recompute_milestones(
            &self,
            milestone_ids: &[MilestoneId],
        ) -> Result<usize, StatusComputationError> {
            self.record(RecomputeCall::Milestones(milestone_ids.to_vec()))?;
            Ok(0)
        }

        async fn recompute_goals(
            &self,
            goal_ids: &[GoalId],
        ) -> Result<usize, StatusComputationError> {
            self.record(RecomputeCall::Goals(goal_ids.to_vec()))?;
            Ok(0)
        }

        async fn recompute_goals_for_milestones(
            &self,
            milestone_ids: &[MilestoneId],
        ) -> Result<usize, StatusComputationError> {
            self.record(RecomputeCall::GoalsForMilestones(milestone_ids.to_vec()))?;
            Ok(0)
        }
    }

    /// A [`StatusRecomputation`] that does nothing and never fails — the
    /// default for tests that do not care about recomputation.
    #[derive(Default)]
    pub struct NoopRecomputation;

    impl NoopRecomputation {
        pub fn new() -> Arc<Self> {
            Arc::new(Self)
        }
    }

    #[async_trait::async_trait]
    impl StatusRecomputation for NoopRecomputation {
        async fn recompute_milestones(
            &self,
            _milestone_ids: &[MilestoneId],
        ) -> Result<usize, StatusComputationError> {
            Ok(0)
        }

        async fn recompute_goals(
            &self,
            _goal_ids: &[GoalId],
        ) -> Result<usize, StatusComputationError> {
            Ok(0)
        }

        async fn recompute_goals_for_milestones(
            &self,
            _milestone_ids: &[MilestoneId],
        ) -> Result<usize, StatusComputationError> {
            Ok(0)
        }
    }

    /// The no-op as the trait object every service's `new` takes.
    pub fn noop_recomputation() -> Arc<dyn StatusRecomputation> {
        NoopRecomputation::new()
    }
}

#[cfg(test)]
mod tests {
    use std::collections::{HashMap, HashSet};
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::sync::{Arc, Mutex};

    use chrono::{DateTime, NaiveDate, Utc};
    use domain::{Goal, Milestone, Status, Task, TaskId, TaskStatus, UserId};

    use crate::pagination::{Page, PageRequest};
    use crate::ports::goal_repository::fakes::InMemoryGoalRepository;
    use crate::ports::milestone_repository::fakes::InMemoryMilestoneRepository;
    use crate::ports::task_repository::fakes::InMemoryTaskRepository;
    use crate::ports::{TaskCounts, TaskListFilter};

    use super::*;

    fn day(month: u32, day: u32) -> NaiveDate {
        NaiveDate::from_ymd_opt(2026, month, day).unwrap()
    }

    fn at(date: NaiveDate) -> DateTime<Utc> {
        date.and_hms_opt(9, 0, 0).unwrap().and_utc()
    }

    /// A fixed clock: "today" is whatever the test says it is (D19's UTC
    /// date, injectable for determinism).
    struct FixedClock(NaiveDate);

    impl Clock for FixedClock {
        fn today(&self) -> NaiveDate {
            self.0
        }
    }

    /// Wraps the canonical task fake, counting `task_counts_for_milestones`
    /// calls so a test can assert one grouped lookup per batch.
    struct CountingTaskRepository {
        inner: InMemoryTaskRepository,
        count_calls: AtomicUsize,
    }

    impl CountingTaskRepository {
        fn new() -> Self {
            Self {
                inner: InMemoryTaskRepository::new(),
                count_calls: AtomicUsize::new(0),
            }
        }

        fn count_calls(&self) -> usize {
            self.count_calls.load(Ordering::Relaxed)
        }
    }

    #[async_trait::async_trait]
    impl TaskRepository for CountingTaskRepository {
        async fn create(&self, task: Task) -> Result<Task, RepositoryError> {
            self.inner.create(task).await
        }

        async fn find_by_id(&self, id: TaskId) -> Result<Option<Task>, RepositoryError> {
            self.inner.find_by_id(id).await
        }

        async fn find_by_ids(&self, ids: &[TaskId]) -> Result<Vec<Task>, RepositoryError> {
            self.inner.find_by_ids(ids).await
        }

        async fn task_counts_for_milestones(
            &self,
            milestone_ids: &[MilestoneId],
        ) -> Result<HashMap<MilestoneId, TaskCounts>, RepositoryError> {
            self.count_calls.fetch_add(1, Ordering::Relaxed);
            self.inner.task_counts_for_milestones(milestone_ids).await
        }

        async fn update(&self, task: Task) -> Result<Task, RepositoryError> {
            self.inner.update(task).await
        }

        async fn set_status(
            &self,
            id: TaskId,
            status: TaskStatus,
        ) -> Result<Task, RepositoryError> {
            self.inner.set_status(id, status).await
        }

        async fn set_assignee(
            &self,
            id: TaskId,
            assignee_id: Option<UserId>,
        ) -> Result<Task, RepositoryError> {
            self.inner.set_assignee(id, assignee_id).await
        }

        async fn delete(&self, id: TaskId) -> Result<(), RepositoryError> {
            self.inner.delete(id).await
        }

        async fn list_page(
            &self,
            filter: &TaskListFilter,
            page: &PageRequest,
        ) -> Result<Page<Task>, RepositoryError> {
            self.inner.list_page(filter, page).await
        }
    }

    /// An in-memory [`GoalMilestoneRepository`]: the pair set joined through
    /// the entity stores for the two lookups. The service never depends on
    /// the link order, so — unlike the Postgres mirror — this double does not
    /// sort.
    #[derive(Default)]
    struct InMemoryGoalMilestoneRepository {
        pairs: Mutex<HashSet<(GoalId, MilestoneId)>>,
        goals: Arc<InMemoryGoalRepository>,
        milestones: Arc<InMemoryMilestoneRepository>,
    }

    impl InMemoryGoalMilestoneRepository {
        fn new(
            goals: Arc<InMemoryGoalRepository>,
            milestones: Arc<InMemoryMilestoneRepository>,
        ) -> Self {
            Self {
                pairs: Mutex::new(HashSet::new()),
                goals,
                milestones,
            }
        }

        /// Link a pair directly (the service never calls `link` itself).
        fn link_pair(&self, goal_id: GoalId, milestone_id: MilestoneId) {
            self.pairs.lock().unwrap().insert((goal_id, milestone_id));
        }
    }

    #[async_trait::async_trait]
    impl GoalMilestoneRepository for InMemoryGoalMilestoneRepository {
        async fn link(&self, link: domain::GoalMilestone) -> Result<bool, RepositoryError> {
            Ok(self
                .pairs
                .lock()
                .unwrap()
                .insert((link.goal_id, link.milestone_id)))
        }

        async fn unlink(
            &self,
            goal_id: GoalId,
            milestone_id: MilestoneId,
        ) -> Result<bool, RepositoryError> {
            Ok(self.pairs.lock().unwrap().remove(&(goal_id, milestone_id)))
        }

        async fn milestones_for_goal(
            &self,
            goal_id: GoalId,
        ) -> Result<Vec<Milestone>, RepositoryError> {
            let all = self.milestones.list().await?;
            // The guard must not be held across the await above.
            let pairs = self.pairs.lock().unwrap();
            Ok(all
                .into_iter()
                .filter(|m| pairs.contains(&(goal_id, m.id)))
                .collect())
        }

        async fn goals_for_milestone(
            &self,
            milestone_id: MilestoneId,
        ) -> Result<Vec<Goal>, RepositoryError> {
            let all = self.goals.list().await?;
            // The guard must not be held across the await above.
            let pairs = self.pairs.lock().unwrap();
            Ok(all
                .into_iter()
                .filter(|g| pairs.contains(&(g.id, milestone_id)))
                .collect())
        }

        async fn milestones_for_goal_page(
            &self,
            goal_id: GoalId,
            page: &PageRequest,
        ) -> Result<Page<Milestone>, RepositoryError> {
            let matched = self.milestones_for_goal(goal_id).await?;
            Ok(crate::ports::milestone_repository::fakes::page_milestones(
                matched, page,
            ))
        }

        async fn goals_for_milestone_page(
            &self,
            milestone_id: MilestoneId,
            page: &PageRequest,
        ) -> Result<Page<Goal>, RepositoryError> {
            let matched = self.goals_for_milestone(milestone_id).await?;
            Ok(crate::ports::goal_repository::fakes::page_goals(
                matched, page,
            ))
        }
    }

    fn milestone(
        created: NaiveDate,
        target: Option<NaiveDate>,
        status: Status,
        override_status: Option<Status>,
    ) -> Milestone {
        let now = at(created);
        Milestone {
            id: MilestoneId::new(),
            title: "Test milestone".to_owned(),
            description: None,
            status,
            status_override: override_status,
            target_date: target,
            owner_id: None,
            created_at: now,
            updated_at: now,
        }
    }

    fn goal(status: Status, override_status: Option<Status>) -> Goal {
        let now = at(day(1, 1));
        Goal {
            id: GoalId::new(),
            title: "Test goal".to_owned(),
            description: None,
            status,
            status_override: override_status,
            target_date: None,
            owner_id: None,
            created_at: now,
            updated_at: now,
        }
    }

    fn task(milestone_id: MilestoneId, status: TaskStatus) -> Task {
        let now = at(day(1, 2));
        Task {
            id: TaskId::new(),
            milestone_id: Some(milestone_id),
            title: "Test task".to_owned(),
            description: None,
            status,
            target_date: None,
            assignee_id: None,
            created_at: now,
            updated_at: now,
        }
    }

    /// The wiring every test shares; `today` is the fixed clock's date.
    struct Setup {
        goals: Arc<InMemoryGoalRepository>,
        milestones: Arc<InMemoryMilestoneRepository>,
        goal_milestones: Arc<InMemoryGoalMilestoneRepository>,
        tasks: Arc<CountingTaskRepository>,
        service: StatusComputationService,
    }

    fn setup(today: NaiveDate) -> Setup {
        let goals = Arc::new(InMemoryGoalRepository::new());
        let milestones = Arc::new(InMemoryMilestoneRepository::new());
        let goal_milestones = Arc::new(InMemoryGoalMilestoneRepository::new(
            goals.clone(),
            milestones.clone(),
        ));
        let tasks = Arc::new(CountingTaskRepository::new());
        let service = StatusComputationService::new(
            goals.clone(),
            milestones.clone(),
            goal_milestones.clone(),
            tasks.clone(),
            Arc::new(FixedClock(today)),
        );
        Setup {
            goals,
            milestones,
            goal_milestones,
            tasks,
            service,
        }
    }

    #[tokio::test]
    async fn milestone_recompute_uses_task_counts_dates_and_the_injected_clock() {
        // Today is 2026-01-16: halfway through the Jan 1 – Jan 31 window
        // with no work done, so the gap (0.5) is beyond the off-track
        // threshold and the milestone must move to OffTrack.
        let setup = setup(day(1, 16));
        let m = milestone(day(1, 1), Some(day(1, 31)), Status::OnTrack, None);
        setup.milestones.create(m.clone()).await.unwrap();
        for _ in 0..4 {
            setup
                .tasks
                .create(task(m.id, TaskStatus::Backlog))
                .await
                .unwrap();
        }

        let changed = setup.service.recompute_milestones(&[m.id]).await.unwrap();

        assert_eq!(changed, 1);
        let stored = setup.milestones.find_by_id(m.id).await.unwrap().unwrap();
        assert_eq!(stored.status, Status::OffTrack);
        // The override is never touched by a recompute.
        assert_eq!(stored.status_override, None);
    }

    #[tokio::test]
    async fn goal_rollup_counts_an_overridden_milestone_by_its_effective_status() {
        let setup = setup(day(1, 16));
        // No tasks and no target date: the automatic status stays OnTrack,
        // but the override makes the effective status OffTrack — which is
        // what the goal's rollup must see.
        let m = milestone(day(1, 1), None, Status::OnTrack, Some(Status::OffTrack));
        setup.milestones.create(m.clone()).await.unwrap();
        let g = goal(Status::OnTrack, None);
        setup.goals.create(g.clone()).await.unwrap();
        setup.goal_milestones.link_pair(g.id, m.id);

        let changed = setup.service.recompute_goals(&[g.id]).await.unwrap();

        // One incomplete milestone that is off track makes the goal off
        // track; had the rollup used the automatic status it would have
        // stayed OnTrack and written nothing.
        assert_eq!(changed, 1);
        let stored = setup.goals.find_by_id(g.id).await.unwrap().unwrap();
        assert_eq!(stored.status, Status::OffTrack);
    }

    #[tokio::test]
    async fn a_goal_override_survives_recomputation() {
        let setup = setup(day(1, 16));
        let m = milestone(day(1, 1), None, Status::OnTrack, Some(Status::OffTrack));
        setup.milestones.create(m.clone()).await.unwrap();
        let g = goal(Status::OnTrack, Some(Status::Complete));
        setup.goals.create(g.clone()).await.unwrap();
        setup.goal_milestones.link_pair(g.id, m.id);

        let changed = setup.service.recompute_goals(&[g.id]).await.unwrap();

        assert_eq!(changed, 1);
        let stored = setup.goals.find_by_id(g.id).await.unwrap().unwrap();
        // The automatic status moved to the rollup's result...
        assert_eq!(stored.status, Status::OffTrack);
        // ...but the override is intact and still wins.
        assert_eq!(stored.status_override, Some(Status::Complete));
        assert_eq!(stored.effective_status(), Status::Complete);
    }

    #[tokio::test]
    async fn an_unchanged_result_writes_nothing() {
        let setup = setup(day(1, 16));
        // No tasks and no target date computes to OnTrack — the stored value.
        let m = milestone(day(1, 1), None, Status::OnTrack, None);
        setup.milestones.create(m.clone()).await.unwrap();

        let changed = setup.service.recompute_milestones(&[m.id]).await.unwrap();

        assert_eq!(changed, 0);
        let stored = setup.milestones.find_by_id(m.id).await.unwrap().unwrap();
        assert_eq!(stored.status, Status::OnTrack);
        // The writer was a no-op: updated_at is exactly what create set.
        assert_eq!(stored.updated_at, m.updated_at);
    }

    #[tokio::test]
    async fn unknown_ids_are_skipped_silently() {
        let setup = setup(day(1, 16));
        // The known milestone computes to OffTrack (halfway, no work done).
        let m = milestone(day(1, 1), Some(day(1, 31)), Status::OnTrack, None);
        setup.milestones.create(m.clone()).await.unwrap();
        for _ in 0..4 {
            setup
                .tasks
                .create(task(m.id, TaskStatus::Backlog))
                .await
                .unwrap();
        }
        // The known goal has no links and computes to OnTrack — unchanged.
        let g = goal(Status::OnTrack, None);
        setup.goals.create(g.clone()).await.unwrap();

        // Unknown ids are not an error (an entity deleted mid-flight); the
        // known ones are still recomputed.
        assert_eq!(
            setup
                .service
                .recompute_milestones(&[m.id, MilestoneId::new()])
                .await
                .unwrap(),
            1
        );
        assert_eq!(
            setup
                .service
                .recompute_goals(&[g.id, GoalId::new()])
                .await
                .unwrap(),
            0
        );
        // An unknown milestone simply has no linked goals.
        assert_eq!(
            setup
                .service
                .recompute_goals_for_milestones(&[MilestoneId::new()])
                .await
                .unwrap(),
            0
        );
    }

    #[tokio::test]
    async fn recompute_all_recomputes_milestones_before_goals() {
        let setup = setup(day(1, 16));
        // Both start stale: the milestone's data says OffTrack (halfway, no
        // work done) and the goal's rollup depends on that result.
        let m = milestone(day(1, 1), Some(day(1, 31)), Status::OnTrack, None);
        setup.milestones.create(m.clone()).await.unwrap();
        for _ in 0..4 {
            setup
                .tasks
                .create(task(m.id, TaskStatus::Backlog))
                .await
                .unwrap();
        }
        let g = goal(Status::OnTrack, None);
        setup.goals.create(g.clone()).await.unwrap();
        setup.goal_milestones.link_pair(g.id, m.id);

        let counts = setup.service.recompute_all().await.unwrap();

        assert_eq!(
            counts,
            RecomputedCounts {
                milestones: 1,
                goals: 1
            }
        );
        assert_eq!(
            setup
                .milestones
                .find_by_id(m.id)
                .await
                .unwrap()
                .unwrap()
                .status,
            Status::OffTrack
        );
        // The goal's result reflects the milestone recomputed in this same
        // run — had the goals gone first it would have seen the stale OnTrack.
        assert_eq!(
            setup.goals.find_by_id(g.id).await.unwrap().unwrap().status,
            Status::OffTrack
        );
    }

    #[tokio::test]
    async fn one_grouped_count_call_per_batch() {
        let setup = setup(day(1, 16));
        // Three milestones in one batch: exactly one grouped count lookup.
        let a = milestone(day(1, 1), None, Status::OnTrack, None);
        let b = milestone(day(1, 1), None, Status::OnTrack, None);
        let c = milestone(day(1, 1), None, Status::OnTrack, None);
        setup.milestones.create(a.clone()).await.unwrap();
        setup.milestones.create(b.clone()).await.unwrap();
        setup.milestones.create(c.clone()).await.unwrap();

        assert_eq!(
            setup
                .service
                .recompute_milestones(&[a.id, b.id, c.id])
                .await
                .unwrap(),
            0
        );
        assert_eq!(setup.tasks.count_calls(), 1);

        // The sweep chunks by BATCH_SIZE: the 3 above plus 205 more make 208
        // milestones — two batches, so two grouped lookups (and the goal
        // phase makes none).
        for _ in 0..205 {
            setup
                .milestones
                .create(milestone(day(1, 1), None, Status::OnTrack, None))
                .await
                .unwrap();
        }
        let counts = setup.service.recompute_all().await.unwrap();
        assert_eq!(counts.milestones, 0);
        assert_eq!(counts.goals, 0);
        assert_eq!(setup.tasks.count_calls(), 1 + 2);
    }
}
