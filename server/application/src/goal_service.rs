//! Goal create (roadmap 3.13, step 4): the handler's write goes through this
//! service so the best-effort status computation after the mutation lives with
//! the mutation, not in the handler.
//!
//! Only create is here: a goal's automatic status depends solely on its linked
//! milestones (the link service and the milestone-side triggers recompute it
//! when they change), so updating or deleting a goal changes no stored status.
//! The computation is best-effort, like the snapshot hook on overrides: the
//! write already committed, so a failure only warns (the scheduled sweep in
//! roadmap 3.13 heals any miss) and never fails the request.

use std::sync::Arc;

use domain::Goal;

use crate::ports::{GoalRepository, RepositoryError};
use crate::status_computation::StatusRecomputation;

/// Creates goals (roadmap 3.13, step 4), computing the new goal's initial
/// status after the write.
pub struct GoalService {
    goals: Arc<dyn GoalRepository>,
    computation: Arc<dyn StatusRecomputation>,
}

impl GoalService {
    pub fn new(goals: Arc<dyn GoalRepository>, computation: Arc<dyn StatusRecomputation>) -> Self {
        Self { goals, computation }
    }

    /// Create the goal and compute its initial status.
    pub async fn create(&self, goal: Goal) -> Result<Goal, RepositoryError> {
        let goal = self.goals.create(goal).await?;
        // Best-effort: the write committed, so a failure only warns (roadmap
        // 3.13's sweep heals any miss).
        if let Err(err) = self
            .computation
            .recompute_goals(std::slice::from_ref(&goal.id))
            .await
        {
            eprintln!("warning: status recompute after a goal change failed: {err}");
        }
        Ok(goal)
    }
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use chrono::Utc;
    use domain::{GoalId, Status};

    use crate::ports::goal_repository::fakes::InMemoryGoalRepository;
    use crate::status_computation::fakes::{RecomputeCall, RecordingRecomputation};

    use super::*;

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

    fn setup() -> (Arc<RecordingRecomputation>, GoalService) {
        let goals = Arc::new(InMemoryGoalRepository::new());
        let computation = Arc::new(RecordingRecomputation::new());
        let service = GoalService::new(goals, computation.clone());
        (computation, service)
    }

    #[tokio::test]
    async fn create_computes_the_new_goals_status() {
        let (computation, service) = setup();

        let created = service.create(test_goal()).await.unwrap();

        assert_eq!(
            computation.calls(),
            vec![RecomputeCall::Goals(vec![created.id])]
        );
    }

    #[tokio::test]
    async fn a_failing_recompute_does_not_fail_the_create() {
        let (computation, service) = setup();
        computation.set_fail(true);

        // The write committed; the recompute failure only warns.
        let created = service.create(test_goal()).await.unwrap();
        assert_eq!(created.title, "Test goal");
    }
}
