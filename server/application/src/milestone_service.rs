//! Milestone create/update/delete (roadmap 3.13, step 4): the handlers' writes
//! go through this service so the best-effort status recompute after each
//! mutation lives with the mutation, not in the handler.
//!
//! The recompute is best-effort, like the snapshot hook on overrides: the
//! write already committed, so a failure only warns (the scheduled sweep in
//! roadmap 3.13 heals any miss) and never fails the request.

use std::sync::Arc;

use domain::{GoalId, Milestone, MilestoneId};

use crate::ports::{GoalMilestoneRepository, MilestoneRepository, RepositoryError};
use crate::status_computation::StatusRecomputation;

/// An error from creating, updating or deleting a milestone.
#[derive(Debug)]
pub enum MilestoneServiceError {
    /// No milestone with the given id exists.
    NotFound,
    /// The repository failed.
    Repository(RepositoryError),
}

impl std::fmt::Display for MilestoneServiceError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            MilestoneServiceError::NotFound => write!(f, "requested resource was not found"),
            MilestoneServiceError::Repository(err) => write!(f, "{err}"),
        }
    }
}

impl std::error::Error for MilestoneServiceError {}

/// Creates, updates and deletes milestones (roadmap 3.13, step 4), recomputing
/// the milestone's status — and the statuses of the goals linked to it — after
/// each successful write. Milestones first, then goals: a goal's rollup reads
/// its linked milestones' effective statuses.
pub struct MilestoneService {
    milestones: Arc<dyn MilestoneRepository>,
    goal_milestones: Arc<dyn GoalMilestoneRepository>,
    computation: Arc<dyn StatusRecomputation>,
}

impl MilestoneService {
    pub fn new(
        milestones: Arc<dyn MilestoneRepository>,
        goal_milestones: Arc<dyn GoalMilestoneRepository>,
        computation: Arc<dyn StatusRecomputation>,
    ) -> Self {
        Self {
            milestones,
            goal_milestones,
            computation,
        }
    }

    /// Create the milestone and recompute its status (a past target date starts
    /// it OffTrack) plus the goals linked to it. The returned row is re-read
    /// after the recompute: it may have moved the automatic status, so the
    /// pre-recompute row would contradict what was stored.
    pub async fn create(&self, milestone: Milestone) -> Result<Milestone, MilestoneServiceError> {
        let created = self
            .milestones
            .create(milestone)
            .await
            .map_err(MilestoneServiceError::Repository)?;
        self.recompute_milestone(created.id).await;
        self.find(created.id).await
    }

    /// Update the milestone and recompute its status plus the goals linked to
    /// it, returning the fresh row (the recompute may have moved the automatic
    /// status, e.g. a target date that just slipped into the past).
    pub async fn update(&self, milestone: Milestone) -> Result<Milestone, MilestoneServiceError> {
        let updated = self
            .milestones
            .update(milestone)
            .await
            .map_err(MilestoneServiceError::Repository)?;
        self.recompute_milestone(updated.id).await;
        self.find(updated.id).await
    }

    /// Delete the milestone and recompute the statuses of the goals linked to
    /// it. The goal ids are captured before the delete: the links cascade with
    /// the milestone, so after it there is nothing left to ask.
    pub async fn delete(&self, id: MilestoneId) -> Result<(), MilestoneServiceError> {
        self.find(id).await?;
        let goal_ids = self
            .goal_milestones
            .goals_for_milestone(id)
            .await
            .map_err(MilestoneServiceError::Repository)?
            .into_iter()
            .map(|goal| goal.id)
            .collect::<Vec<GoalId>>();
        self.milestones
            .delete(id)
            .await
            .map_err(MilestoneServiceError::Repository)?;
        if !goal_ids.is_empty() {
            self.recompute_goals(&goal_ids).await;
        }
        Ok(())
    }

    async fn find(&self, id: MilestoneId) -> Result<Milestone, MilestoneServiceError> {
        self.milestones
            .find_by_id(id)
            .await
            .map_err(MilestoneServiceError::Repository)?
            .ok_or(MilestoneServiceError::NotFound)
    }

    /// Best-effort: the mutation already committed, so a recompute failure
    /// only warns (roadmap 3.13's sweep heals any miss).
    async fn recompute_milestone(&self, milestone_id: MilestoneId) {
        let ids = std::slice::from_ref(&milestone_id);
        if let Err(err) = self.computation.recompute_milestones(ids).await {
            eprintln!("warning: status recompute after a milestone change failed: {err}");
        }
        if let Err(err) = self.computation.recompute_goals_for_milestones(ids).await {
            eprintln!("warning: status recompute after a milestone change failed: {err}");
        }
    }

    async fn recompute_goals(&self, goal_ids: &[GoalId]) {
        if let Err(err) = self.computation.recompute_goals(goal_ids).await {
            eprintln!("warning: status recompute after a milestone change failed: {err}");
        }
    }
}

#[cfg(test)]
mod tests {
    use std::collections::{HashMap, HashSet};
    use std::sync::{Arc, Mutex};

    use chrono::Utc;
    use domain::{Goal, GoalId, GoalMilestone, Milestone, Status};

    use crate::ports::milestone_repository::fakes::InMemoryMilestoneRepository;
    use crate::status_computation::fakes::{RecomputeCall, RecordingRecomputation};

    use super::*;

    fn test_milestone() -> Milestone {
        let now = Utc::now();
        Milestone {
            id: MilestoneId::new(),
            title: "Test milestone".to_owned(),
            description: None,
            status: Status::OnTrack,
            status_override: None,
            target_date: None,
            owner_id: None,
            created_at: now,
            updated_at: now,
        }
    }

    fn test_goal() -> Goal {
        let now = Utc::now();
        Goal {
            id: GoalId::new(),
            title: "Test goal".to_owned(),
            description: None,
            status: Status::OnTrack,
            status_override: None,
            target_date: None,
            owner_id: None,
            created_at: now,
            updated_at: now,
        }
    }

    /// An in-memory [`GoalMilestoneRepository`] with the behaviour the service
    /// uses: `goals_for_milestone` joins the pair set through a goal store. The
    /// listing methods the service never calls fail loudly if a test starts
    /// relying on them.
    #[derive(Default)]
    struct InMemoryGoalMilestones {
        pairs: Mutex<HashSet<(GoalId, MilestoneId)>>,
        goals: Mutex<HashMap<GoalId, Goal>>,
    }

    impl InMemoryGoalMilestones {
        fn add_goal(&self, goal: Goal) {
            self.goals.lock().unwrap().insert(goal.id, goal);
        }

        fn link(&self, goal_id: GoalId, milestone_id: MilestoneId) {
            self.pairs.lock().unwrap().insert((goal_id, milestone_id));
        }
    }

    #[async_trait::async_trait]
    impl GoalMilestoneRepository for InMemoryGoalMilestones {
        async fn link(&self, link: GoalMilestone) -> Result<bool, RepositoryError> {
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

        async fn goals_for_milestone(
            &self,
            milestone_id: MilestoneId,
        ) -> Result<Vec<Goal>, RepositoryError> {
            let goals = self
                .goals
                .lock()
                .unwrap()
                .values()
                .cloned()
                .collect::<Vec<_>>();
            // The guard must not be held across the await above.
            let pairs = self.pairs.lock().unwrap();
            Ok(goals
                .into_iter()
                .filter(|g| pairs.contains(&(g.id, milestone_id)))
                .collect())
        }

        async fn milestones_for_goal(
            &self,
            _goal_id: GoalId,
        ) -> Result<Vec<Milestone>, RepositoryError> {
            Err(RepositoryError::Unexpected(
                "not used by these tests".to_owned(),
            ))
        }

        async fn milestones_for_goal_page(
            &self,
            _goal_id: GoalId,
            _page: &crate::pagination::PageRequest,
        ) -> Result<crate::pagination::Page<Milestone>, RepositoryError> {
            Err(RepositoryError::Unexpected(
                "not used by these tests".to_owned(),
            ))
        }

        async fn goals_for_milestone_page(
            &self,
            _milestone_id: MilestoneId,
            _page: &crate::pagination::PageRequest,
        ) -> Result<crate::pagination::Page<Goal>, RepositoryError> {
            Err(RepositoryError::Unexpected(
                "not used by these tests".to_owned(),
            ))
        }
    }

    fn setup() -> (
        Arc<InMemoryMilestoneRepository>,
        Arc<InMemoryGoalMilestones>,
        Arc<RecordingRecomputation>,
        MilestoneService,
    ) {
        let milestones = Arc::new(InMemoryMilestoneRepository::new());
        let goal_milestones = Arc::new(InMemoryGoalMilestones::default());
        let computation = Arc::new(RecordingRecomputation::new());
        let service = MilestoneService::new(
            milestones.clone(),
            goal_milestones.clone(),
            computation.clone(),
        );
        (milestones, goal_milestones, computation, service)
    }

    #[tokio::test]
    async fn create_recomputes_the_milestone_and_its_goals() {
        let (_milestones, _links, computation, service) = setup();

        let created = service.create(test_milestone()).await.unwrap();

        assert_eq!(
            computation.calls(),
            vec![
                RecomputeCall::Milestones(vec![created.id]),
                RecomputeCall::GoalsForMilestones(vec![created.id])
            ]
        );
    }

    #[tokio::test]
    async fn update_recomputes_the_milestone_and_its_goals() {
        let (milestones, _links, computation, service) = setup();
        let milestone = test_milestone();
        milestones.create(milestone.clone()).await.unwrap();

        service.update(milestone.clone()).await.unwrap();

        assert_eq!(
            computation.calls(),
            vec![
                RecomputeCall::Milestones(vec![milestone.id]),
                RecomputeCall::GoalsForMilestones(vec![milestone.id])
            ]
        );
    }

    #[tokio::test]
    async fn delete_recomputes_only_the_linked_goals() {
        let (milestones, links, computation, service) = setup();
        let milestone = test_milestone();
        milestones.create(milestone.clone()).await.unwrap();
        let linked_goal = test_goal();
        links.add_goal(linked_goal.clone());
        links.link(linked_goal.id, milestone.id);
        // An unlinked goal must not be recomputed.
        let stray_goal = test_goal();
        links.add_goal(stray_goal.clone());

        service.delete(milestone.id).await.unwrap();

        assert!(milestones.find_by_id(milestone.id).await.unwrap().is_none());
        assert_eq!(
            computation.calls(),
            vec![RecomputeCall::Goals(vec![linked_goal.id])]
        );
    }

    #[tokio::test]
    async fn delete_of_an_unlinked_milestone_recomputes_nothing() {
        let (milestones, _links, computation, service) = setup();
        let milestone = test_milestone();
        milestones.create(milestone.clone()).await.unwrap();

        service.delete(milestone.id).await.unwrap();

        assert!(computation.calls().is_empty());
    }

    #[tokio::test]
    async fn unknown_ids_are_not_found() {
        let (_milestones, _links, _computation, service) = setup();

        // The delete checks existence first...
        assert!(matches!(
            service.delete(MilestoneId::new()).await,
            Err(MilestoneServiceError::NotFound)
        ));
        // ...and the repository's own absence answer surfaces for updates.
        assert!(matches!(
            service.update(test_milestone()).await,
            Err(MilestoneServiceError::Repository(RepositoryError::NotFound))
        ));
    }

    #[tokio::test]
    async fn a_failing_recompute_does_not_fail_the_mutation() {
        let (milestones, links, computation, service) = setup();
        computation.set_fail(true);
        let milestone = test_milestone();
        milestones.create(milestone.clone()).await.unwrap();
        let goal = test_goal();
        links.add_goal(goal.clone());
        links.link(goal.id, milestone.id);

        // The writes commit; the recompute failures only warn.
        let created = service.create(test_milestone()).await.unwrap();
        assert_eq!(created.title, "Test milestone");
        let updated = service.update(milestone.clone()).await.unwrap();
        assert_eq!(updated.id, milestone.id);
        service.delete(created.id).await.unwrap();
        assert!(milestones.find_by_id(created.id).await.unwrap().is_none());
    }
}
