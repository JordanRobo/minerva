//! Manual status overrides (roadmap 3.2): setting and clearing a goal's or
//! milestone's manual override of its automatic status.
//!
//! The rules live here, not in the handlers: an unknown id is a typed
//! `NotFound`, repository failures surface as a typed error, a no-op change
//! (setting the value already set, clearing when none is set) still returns
//! the entity without touching storage, and the snapshot hook (roadmap 3.14)
//! fires exactly once after each successful write — a hook failure only
//! warns, it never fails the override.

use std::sync::Arc;

use domain::{Goal, GoalId, Milestone, MilestoneId, Status};

use crate::ports::{
    GoalRepository, MilestoneRepository, RepositoryError, StatusChangeTarget, StatusSnapshotTrigger,
};

/// An error from setting or clearing a manual status override.
#[derive(Debug)]
pub enum StatusOverrideError {
    /// No goal or milestone with the given id exists.
    NotFound,
    /// The repository failed.
    Repository(RepositoryError),
}

impl std::fmt::Display for StatusOverrideError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            StatusOverrideError::NotFound => write!(f, "requested resource was not found"),
            StatusOverrideError::Repository(err) => write!(f, "{err}"),
        }
    }
}

impl std::error::Error for StatusOverrideError {}

/// Sets and clears manual status overrides on goals and milestones (roadmap
/// 3.2). Nothing here touches the automatic status: recomputation owns it
/// (roadmap 3.13), and an override is sticky until explicitly cleared.
pub struct StatusOverrideService {
    goals: Arc<dyn GoalRepository>,
    milestones: Arc<dyn MilestoneRepository>,
    snapshots: Arc<dyn StatusSnapshotTrigger>,
}

impl StatusOverrideService {
    pub fn new(
        goals: Arc<dyn GoalRepository>,
        milestones: Arc<dyn MilestoneRepository>,
        snapshots: Arc<dyn StatusSnapshotTrigger>,
    ) -> Self {
        Self {
            goals,
            milestones,
            snapshots,
        }
    }

    /// Set the goal's manual override to `status`. Setting the value it
    /// already has is a no-op that still returns the goal.
    pub async fn set_goal_override(
        &self,
        id: GoalId,
        status: Status,
    ) -> Result<Goal, StatusOverrideError> {
        self.change_goal(id, Some(status)).await
    }

    /// Clear the goal's manual override, returning it to its automatic
    /// status. Clearing when none is set is a no-op that still returns the
    /// goal.
    pub async fn clear_goal_override(&self, id: GoalId) -> Result<Goal, StatusOverrideError> {
        self.change_goal(id, None).await
    }

    /// Set the milestone's manual override to `status`. Setting the value it
    /// already has is a no-op that still returns the milestone.
    pub async fn set_milestone_override(
        &self,
        id: MilestoneId,
        status: Status,
    ) -> Result<Milestone, StatusOverrideError> {
        self.change_milestone(id, Some(status)).await
    }

    /// Clear the milestone's manual override, returning it to its automatic
    /// status. Clearing when none is set is a no-op that still returns the
    /// milestone.
    pub async fn clear_milestone_override(
        &self,
        id: MilestoneId,
    ) -> Result<Milestone, StatusOverrideError> {
        self.change_milestone(id, None).await
    }

    async fn change_goal(
        &self,
        id: GoalId,
        status: Option<Status>,
    ) -> Result<Goal, StatusOverrideError> {
        let mut goal = self.find_goal(id).await?;
        if goal.status_override == status {
            // Idempotent no-op: nothing is written, so no snapshot is taken.
            return Ok(goal);
        }
        self.goals
            .set_status_override(id, status)
            .await
            .map_err(StatusOverrideError::Repository)?;
        self.snapshot(StatusChangeTarget::Goal(id)).await;
        match status {
            Some(status) => goal.set_status_override(status),
            None => goal.clear_status_override(),
        }
        Ok(goal)
    }

    async fn change_milestone(
        &self,
        id: MilestoneId,
        status: Option<Status>,
    ) -> Result<Milestone, StatusOverrideError> {
        let mut milestone = self.find_milestone(id).await?;
        if milestone.status_override == status {
            // Idempotent no-op: nothing is written, so no snapshot is taken.
            return Ok(milestone);
        }
        self.milestones
            .set_status_override(id, status)
            .await
            .map_err(StatusOverrideError::Repository)?;
        self.snapshot(StatusChangeTarget::Milestone(id)).await;
        match status {
            Some(status) => milestone.set_status_override(status),
            None => milestone.clear_status_override(),
        }
        Ok(milestone)
    }

    async fn find_goal(&self, id: GoalId) -> Result<Goal, StatusOverrideError> {
        self.goals
            .find_by_id(id)
            .await
            .map_err(StatusOverrideError::Repository)?
            .ok_or(StatusOverrideError::NotFound)
    }

    async fn find_milestone(&self, id: MilestoneId) -> Result<Milestone, StatusOverrideError> {
        self.milestones
            .find_by_id(id)
            .await
            .map_err(StatusOverrideError::Repository)?
            .ok_or(StatusOverrideError::NotFound)
    }

    /// The snapshot hook is best-effort: the override write already
    /// committed, so a failure only warns (roadmap 3.14).
    async fn snapshot(&self, target: StatusChangeTarget) {
        if let Err(err) = self.snapshots.snapshot(target).await {
            eprintln!("warning: status snapshot after an override change failed: {err}");
        }
    }
}

#[cfg(test)]
mod tests {
    use std::collections::HashMap;
    use std::sync::{Arc, Mutex, MutexGuard};

    use chrono::Utc;
    use domain::StatusSource;

    use crate::pagination::{Page, PageRequest};
    use crate::ports::goal_repository::fakes::{apply_goal_list_filter, page_goals};
    use crate::ports::milestone_repository::fakes::{apply_milestone_list_filter, page_milestones};
    use crate::ports::{GoalListFilter, MilestoneListFilter, StatusSnapshotError};

    use super::*;

    fn test_goal(status: Status) -> Goal {
        let now = Utc::now();
        Goal {
            id: GoalId::new(),
            title: "Test goal".to_owned(),
            description: None,
            status,
            status_override: None,
            target_date: None,
            created_at: now,
            updated_at: now,
        }
    }

    fn test_milestone(status: Status) -> Milestone {
        let now = Utc::now();
        Milestone {
            id: MilestoneId::new(),
            title: "Test milestone".to_owned(),
            description: None,
            status,
            status_override: None,
            target_date: None,
            created_at: now,
            updated_at: now,
        }
    }

    /// An in-memory [`GoalRepository`] mirroring Postgres: `update` never
    /// writes the override column, and `set_status_override` answers
    /// `NotFound` for an unknown id.
    #[derive(Default)]
    struct InMemoryGoalRepository {
        goals: Mutex<HashMap<GoalId, Goal>>,
        fail_overrides: Mutex<bool>,
    }

    impl InMemoryGoalRepository {
        fn new() -> Self {
            Self::default()
        }

        /// Make `set_status_override` fail (or succeed again).
        fn set_fail_overrides(&self, fail: bool) {
            *self.fail_overrides.lock().unwrap() = fail;
        }

        fn locked(&self) -> MutexGuard<'_, HashMap<GoalId, Goal>> {
            self.goals.lock().unwrap()
        }
    }

    #[async_trait::async_trait]
    impl GoalRepository for InMemoryGoalRepository {
        async fn create(&self, goal: Goal) -> Result<Goal, RepositoryError> {
            self.locked().insert(goal.id, goal.clone());
            Ok(goal)
        }

        async fn find_by_id(&self, id: GoalId) -> Result<Option<Goal>, RepositoryError> {
            Ok(self.locked().get(&id).cloned())
        }

        async fn list(&self) -> Result<Vec<Goal>, RepositoryError> {
            Ok(self.locked().values().cloned().collect())
        }

        async fn update(&self, goal: Goal) -> Result<Goal, RepositoryError> {
            let mut goals = self.locked();
            let Some(existing) = goals.get(&goal.id) else {
                return Err(RepositoryError::NotFound);
            };
            // Mirror Postgres: an ordinary update never writes
            // status_override (roadmap 3.2).
            let updated = Goal {
                status_override: existing.status_override,
                ..goal
            };
            goals.insert(updated.id, updated.clone());
            Ok(updated)
        }

        async fn set_status_override(
            &self,
            id: GoalId,
            status_override: Option<Status>,
        ) -> Result<(), RepositoryError> {
            if *self.fail_overrides.lock().unwrap() {
                return Err(RepositoryError::Unexpected(
                    "faking an override write failure".to_owned(),
                ));
            }
            let mut goals = self.locked();
            let Some(goal) = goals.get_mut(&id) else {
                return Err(RepositoryError::NotFound);
            };
            goal.status_override = status_override;
            Ok(())
        }

        async fn delete(&self, id: GoalId) -> Result<(), RepositoryError> {
            if self.locked().remove(&id).is_none() {
                return Err(RepositoryError::NotFound);
            }
            Ok(())
        }

        async fn list_page(
            &self,
            filter: &GoalListFilter,
            page: &PageRequest,
        ) -> Result<Page<Goal>, RepositoryError> {
            let matched = apply_goal_list_filter(self.locked().values(), filter);
            Ok(page_goals(matched, page))
        }
    }

    /// An in-memory [`MilestoneRepository`] mirroring Postgres: `update`
    /// never writes the override column, and `set_status_override` answers
    /// `NotFound` for an unknown id.
    #[derive(Default)]
    struct InMemoryMilestoneRepository {
        milestones: Mutex<HashMap<MilestoneId, Milestone>>,
        fail_overrides: Mutex<bool>,
    }

    impl InMemoryMilestoneRepository {
        fn new() -> Self {
            Self::default()
        }

        /// Make `set_status_override` fail (or succeed again).
        fn set_fail_overrides(&self, fail: bool) {
            *self.fail_overrides.lock().unwrap() = fail;
        }

        fn locked(&self) -> MutexGuard<'_, HashMap<MilestoneId, Milestone>> {
            self.milestones.lock().unwrap()
        }
    }

    #[async_trait::async_trait]
    impl MilestoneRepository for InMemoryMilestoneRepository {
        async fn create(&self, milestone: Milestone) -> Result<Milestone, RepositoryError> {
            self.locked().insert(milestone.id, milestone.clone());
            Ok(milestone)
        }

        async fn find_by_id(&self, id: MilestoneId) -> Result<Option<Milestone>, RepositoryError> {
            Ok(self.locked().get(&id).cloned())
        }

        async fn list(&self) -> Result<Vec<Milestone>, RepositoryError> {
            Ok(self.locked().values().cloned().collect())
        }

        async fn update(&self, milestone: Milestone) -> Result<Milestone, RepositoryError> {
            let mut milestones = self.locked();
            let Some(existing) = milestones.get(&milestone.id) else {
                return Err(RepositoryError::NotFound);
            };
            // Mirror Postgres: an ordinary update never writes
            // status_override (roadmap 3.2).
            let updated = Milestone {
                status_override: existing.status_override,
                ..milestone
            };
            milestones.insert(updated.id, updated.clone());
            Ok(updated)
        }

        async fn set_status_override(
            &self,
            id: MilestoneId,
            status_override: Option<Status>,
        ) -> Result<(), RepositoryError> {
            if *self.fail_overrides.lock().unwrap() {
                return Err(RepositoryError::Unexpected(
                    "faking an override write failure".to_owned(),
                ));
            }
            let mut milestones = self.locked();
            let Some(milestone) = milestones.get_mut(&id) else {
                return Err(RepositoryError::NotFound);
            };
            milestone.status_override = status_override;
            Ok(())
        }

        async fn delete(&self, id: MilestoneId) -> Result<(), RepositoryError> {
            if self.locked().remove(&id).is_none() {
                return Err(RepositoryError::NotFound);
            }
            Ok(())
        }

        async fn list_page(
            &self,
            filter: &MilestoneListFilter,
            page: &PageRequest,
        ) -> Result<Page<Milestone>, RepositoryError> {
            let matched = apply_milestone_list_filter(self.locked().values(), filter);
            Ok(page_milestones(matched, page))
        }
    }

    /// A [`StatusSnapshotTrigger`] that records every call and fails on
    /// demand.
    #[derive(Default)]
    struct RecordingSnapshotTrigger {
        fail: Mutex<bool>,
        calls: Mutex<Vec<StatusChangeTarget>>,
    }

    impl RecordingSnapshotTrigger {
        fn new() -> Self {
            Self::default()
        }

        /// Make every snapshot fail (or succeed again).
        fn set_fail(&self, fail: bool) {
            *self.fail.lock().unwrap() = fail;
        }

        /// The targets snapshotted, in call order.
        fn calls(&self) -> Vec<StatusChangeTarget> {
            self.calls.lock().unwrap().clone()
        }
    }

    #[async_trait::async_trait]
    impl StatusSnapshotTrigger for RecordingSnapshotTrigger {
        async fn snapshot(&self, target: StatusChangeTarget) -> Result<(), StatusSnapshotError> {
            if *self.fail.lock().unwrap() {
                return Err(StatusSnapshotError("faking a snapshot failure".to_owned()));
            }
            self.calls.lock().unwrap().push(target);
            Ok(())
        }
    }

    fn goal_setup() -> (
        Arc<InMemoryGoalRepository>,
        Arc<RecordingSnapshotTrigger>,
        StatusOverrideService,
    ) {
        let goals = Arc::new(InMemoryGoalRepository::new());
        let milestones = Arc::new(InMemoryMilestoneRepository::new());
        let snapshots = Arc::new(RecordingSnapshotTrigger::new());
        let service = StatusOverrideService::new(goals.clone(), milestones, snapshots.clone());
        (goals, snapshots, service)
    }

    fn milestone_setup() -> (
        Arc<InMemoryMilestoneRepository>,
        Arc<RecordingSnapshotTrigger>,
        StatusOverrideService,
    ) {
        let goals = Arc::new(InMemoryGoalRepository::new());
        let milestones = Arc::new(InMemoryMilestoneRepository::new());
        let snapshots = Arc::new(RecordingSnapshotTrigger::new());
        let service = StatusOverrideService::new(goals, milestones.clone(), snapshots.clone());
        (milestones, snapshots, service)
    }

    #[tokio::test]
    async fn set_goal_override_makes_it_the_effective_status() {
        let (goals, _snapshots, service) = goal_setup();
        let goal = test_goal(Status::OnTrack);
        goals.create(goal.clone()).await.unwrap();

        let updated = service
            .set_goal_override(goal.id, Status::OffTrack)
            .await
            .unwrap();

        assert_eq!(updated.effective_status(), Status::OffTrack);
        assert_eq!(updated.status_source(), StatusSource::ManualOverride);
        // The automatic status is untouched by the override.
        assert_eq!(updated.status, Status::OnTrack);
    }

    #[tokio::test]
    async fn clear_goal_override_returns_to_the_automatic_status() {
        let (goals, _snapshots, service) = goal_setup();
        let goal = test_goal(Status::AtRisk);
        goals.create(goal.clone()).await.unwrap();
        service
            .set_goal_override(goal.id, Status::Complete)
            .await
            .unwrap();

        let cleared = service.clear_goal_override(goal.id).await.unwrap();

        assert_eq!(cleared.effective_status(), Status::AtRisk);
        assert_eq!(cleared.status_source(), StatusSource::Computed);
    }

    #[tokio::test]
    async fn setting_twice_and_clearing_twice_is_idempotent() {
        let (goals, snapshots, service) = goal_setup();
        let goal = test_goal(Status::OnTrack);
        goals.create(goal.clone()).await.unwrap();

        service
            .set_goal_override(goal.id, Status::OffTrack)
            .await
            .unwrap();
        let repeated = service
            .set_goal_override(goal.id, Status::OffTrack)
            .await
            .unwrap();
        assert_eq!(repeated.effective_status(), Status::OffTrack);
        assert_eq!(repeated.status_source(), StatusSource::ManualOverride);
        service.clear_goal_override(goal.id).await.unwrap();
        let cleared_again = service.clear_goal_override(goal.id).await.unwrap();
        assert_eq!(cleared_again.effective_status(), Status::OnTrack);
        assert_eq!(cleared_again.status_source(), StatusSource::Computed);

        // Each real change snapshotted exactly once; the no-ops did not.
        assert_eq!(
            snapshots.calls(),
            vec![
                StatusChangeTarget::Goal(goal.id),
                StatusChangeTarget::Goal(goal.id)
            ]
        );
    }

    #[tokio::test]
    async fn unknown_goal_id_is_a_not_found() {
        let (_goals, _snapshots, service) = goal_setup();

        let set = service
            .set_goal_override(GoalId::new(), Status::OffTrack)
            .await;
        assert!(matches!(set, Err(StatusOverrideError::NotFound)));
        let clear = service.clear_goal_override(GoalId::new()).await;
        assert!(matches!(clear, Err(StatusOverrideError::NotFound)));
    }

    #[tokio::test]
    async fn goal_repository_failure_surfaces_and_skips_the_snapshot() {
        let (goals, snapshots, service) = goal_setup();
        goals.set_fail_overrides(true);
        let goal = test_goal(Status::OnTrack);
        goals.create(goal.clone()).await.unwrap();

        let error = service
            .set_goal_override(goal.id, Status::OffTrack)
            .await
            .expect_err("the faked write failure");

        assert!(matches!(error, StatusOverrideError::Repository(_)));
        assert!(snapshots.calls().is_empty());
    }

    #[tokio::test]
    async fn a_failing_snapshot_hook_does_not_fail_the_goal_override() {
        let (goals, snapshots, service) = goal_setup();
        snapshots.set_fail(true);
        let goal = test_goal(Status::OnTrack);
        goals.create(goal.clone()).await.unwrap();

        let updated = service
            .set_goal_override(goal.id, Status::OffTrack)
            .await
            .expect("a hook failure must not fail the override");

        assert_eq!(updated.effective_status(), Status::OffTrack);
        assert_eq!(updated.status_source(), StatusSource::ManualOverride);
    }

    #[tokio::test]
    async fn goal_override_survives_recomputation_and_save() {
        let (goals, _snapshots, service) = goal_setup();
        let goal = test_goal(Status::OnTrack);
        goals.create(goal.clone()).await.unwrap();
        let overridden = service
            .set_goal_override(goal.id, Status::OffTrack)
            .await
            .unwrap();

        // Simulate roadmap 3.13: the automatic status is recomputed and saved
        // through an ordinary update. The override must survive it.
        let mut recomputed = overridden;
        recomputed.apply_computed_status(Status::AtRisk);
        let saved = goals.update(recomputed).await.unwrap();

        assert_eq!(saved.status, Status::AtRisk);
        assert_eq!(saved.effective_status(), Status::OffTrack);
        assert_eq!(saved.status_source(), StatusSource::ManualOverride);
    }

    #[tokio::test]
    async fn set_milestone_override_makes_it_the_effective_status() {
        let (milestones, _snapshots, service) = milestone_setup();
        let milestone = test_milestone(Status::OnTrack);
        milestones.create(milestone.clone()).await.unwrap();

        let updated = service
            .set_milestone_override(milestone.id, Status::OffTrack)
            .await
            .unwrap();

        assert_eq!(updated.effective_status(), Status::OffTrack);
        assert_eq!(updated.status_source(), StatusSource::ManualOverride);
        // The automatic status is untouched by the override.
        assert_eq!(updated.status, Status::OnTrack);
    }

    #[tokio::test]
    async fn clear_milestone_override_returns_to_the_automatic_status() {
        let (milestones, _snapshots, service) = milestone_setup();
        let milestone = test_milestone(Status::AtRisk);
        milestones.create(milestone.clone()).await.unwrap();
        service
            .set_milestone_override(milestone.id, Status::Complete)
            .await
            .unwrap();

        let cleared = service
            .clear_milestone_override(milestone.id)
            .await
            .unwrap();

        assert_eq!(cleared.effective_status(), Status::AtRisk);
        assert_eq!(cleared.status_source(), StatusSource::Computed);
    }

    #[tokio::test]
    async fn setting_twice_and_clearing_twice_is_idempotent_for_milestones() {
        let (milestones, snapshots, service) = milestone_setup();
        let milestone = test_milestone(Status::OnTrack);
        milestones.create(milestone.clone()).await.unwrap();

        service
            .set_milestone_override(milestone.id, Status::OffTrack)
            .await
            .unwrap();
        let repeated = service
            .set_milestone_override(milestone.id, Status::OffTrack)
            .await
            .unwrap();
        assert_eq!(repeated.effective_status(), Status::OffTrack);
        assert_eq!(repeated.status_source(), StatusSource::ManualOverride);
        service
            .clear_milestone_override(milestone.id)
            .await
            .unwrap();
        let cleared_again = service
            .clear_milestone_override(milestone.id)
            .await
            .unwrap();
        assert_eq!(cleared_again.effective_status(), Status::OnTrack);
        assert_eq!(cleared_again.status_source(), StatusSource::Computed);

        // Each real change snapshotted exactly once; the no-ops did not.
        assert_eq!(
            snapshots.calls(),
            vec![
                StatusChangeTarget::Milestone(milestone.id),
                StatusChangeTarget::Milestone(milestone.id)
            ]
        );
    }

    #[tokio::test]
    async fn unknown_milestone_id_is_a_not_found() {
        let (_milestones, _snapshots, service) = milestone_setup();

        let set = service
            .set_milestone_override(MilestoneId::new(), Status::OffTrack)
            .await;
        assert!(matches!(set, Err(StatusOverrideError::NotFound)));
        let clear = service.clear_milestone_override(MilestoneId::new()).await;
        assert!(matches!(clear, Err(StatusOverrideError::NotFound)));
    }

    #[tokio::test]
    async fn milestone_repository_failure_surfaces_and_skips_the_snapshot() {
        let (milestones, snapshots, service) = milestone_setup();
        milestones.set_fail_overrides(true);
        let milestone = test_milestone(Status::OnTrack);
        milestones.create(milestone.clone()).await.unwrap();

        let error = service
            .set_milestone_override(milestone.id, Status::OffTrack)
            .await
            .expect_err("the faked write failure");

        assert!(matches!(error, StatusOverrideError::Repository(_)));
        assert!(snapshots.calls().is_empty());
    }

    #[tokio::test]
    async fn a_failing_snapshot_hook_does_not_fail_the_milestone_override() {
        let (milestones, snapshots, service) = milestone_setup();
        snapshots.set_fail(true);
        let milestone = test_milestone(Status::OnTrack);
        milestones.create(milestone.clone()).await.unwrap();

        let updated = service
            .set_milestone_override(milestone.id, Status::OffTrack)
            .await
            .expect("a hook failure must not fail the override");

        assert_eq!(updated.effective_status(), Status::OffTrack);
        assert_eq!(updated.status_source(), StatusSource::ManualOverride);
    }

    #[tokio::test]
    async fn milestone_override_survives_recomputation_and_save() {
        let (milestones, _snapshots, service) = milestone_setup();
        let milestone = test_milestone(Status::OnTrack);
        milestones.create(milestone.clone()).await.unwrap();
        let overridden = service
            .set_milestone_override(milestone.id, Status::OffTrack)
            .await
            .unwrap();

        // Simulate roadmap 3.13: the automatic status is recomputed and saved
        // through an ordinary update. The override must survive it.
        let mut recomputed = overridden;
        recomputed.apply_computed_status(Status::AtRisk);
        let saved = milestones.update(recomputed).await.unwrap();

        assert_eq!(saved.status, Status::AtRisk);
        assert_eq!(saved.effective_status(), Status::OffTrack);
        assert_eq!(saved.status_source(), StatusSource::ManualOverride);
    }

    #[tokio::test]
    async fn the_snapshot_hook_sees_the_right_kind_and_id_per_change() {
        let goals = Arc::new(InMemoryGoalRepository::new());
        let milestones = Arc::new(InMemoryMilestoneRepository::new());
        let snapshots = Arc::new(RecordingSnapshotTrigger::new());
        let service =
            StatusOverrideService::new(goals.clone(), milestones.clone(), snapshots.clone());
        let goal = test_goal(Status::OnTrack);
        goals.create(goal.clone()).await.unwrap();
        let milestone = test_milestone(Status::OnTrack);
        milestones.create(milestone.clone()).await.unwrap();

        service
            .set_goal_override(goal.id, Status::AtRisk)
            .await
            .unwrap();
        service
            .set_milestone_override(milestone.id, Status::OffTrack)
            .await
            .unwrap();
        service.clear_goal_override(goal.id).await.unwrap();
        service
            .clear_milestone_override(milestone.id)
            .await
            .unwrap();

        assert_eq!(
            snapshots.calls(),
            vec![
                StatusChangeTarget::Goal(goal.id),
                StatusChangeTarget::Milestone(milestone.id),
                StatusChangeTarget::Goal(goal.id),
                StatusChangeTarget::Milestone(milestone.id),
            ]
        );
    }
}
