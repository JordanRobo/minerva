//! Goal–milestone linkage (roadmap 3.4): linking a milestone to a goal,
//! unlinking it again, and listing each side of the relation.
//!
//! The rules live here, not in the handlers: both ids must exist before a
//! link or unlink (the goal is checked first), listing for an unknown goal
//! or milestone is a typed not-found, and repeating a link or unlink never
//! fails — it just reports that nothing changed. Linking and unlinking touch
//! only the relation table: status computation is roadmap 3.13, and progress
//! snapshots fire only on manual status changes (decision D5), so no
//! snapshot hook is involved here.

use std::sync::Arc;

use domain::{Goal, GoalId, GoalMilestone, Milestone, MilestoneId};

use crate::pagination::{Page, PageRequest};
use crate::ports::{GoalMilestoneRepository, GoalRepository, MilestoneRepository, RepositoryError};

/// An error from linking or unlinking a goal and milestone.
#[derive(Debug)]
pub enum GoalMilestoneLinkError {
    /// No goal with the given id exists.
    GoalNotFound,
    /// No milestone with the given id exists.
    MilestoneNotFound,
    /// The repository failed.
    Repository(RepositoryError),
}

impl std::fmt::Display for GoalMilestoneLinkError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            GoalMilestoneLinkError::GoalNotFound => write!(f, "goal was not found"),
            GoalMilestoneLinkError::MilestoneNotFound => write!(f, "milestone was not found"),
            GoalMilestoneLinkError::Repository(err) => write!(f, "{err}"),
        }
    }
}

impl std::error::Error for GoalMilestoneLinkError {}

/// Links and unlinks milestones on goals (roadmap 3.4), and lists each side
/// of the relation as full entities. Nothing here touches statuses.
pub struct GoalMilestoneLinkService {
    goals: Arc<dyn GoalRepository>,
    milestones: Arc<dyn MilestoneRepository>,
    links: Arc<dyn GoalMilestoneRepository>,
}

impl GoalMilestoneLinkService {
    pub fn new(
        goals: Arc<dyn GoalRepository>,
        milestones: Arc<dyn MilestoneRepository>,
        links: Arc<dyn GoalMilestoneRepository>,
    ) -> Self {
        Self {
            goals,
            milestones,
            links,
        }
    }

    /// Link the milestone to the goal. Linking a pair that is already linked
    /// is a no-op: it returns `false` instead of failing.
    pub async fn link(
        &self,
        goal_id: GoalId,
        milestone_id: MilestoneId,
    ) -> Result<bool, GoalMilestoneLinkError> {
        self.require_pair(goal_id, milestone_id).await?;
        self.links
            .link(GoalMilestone::new(goal_id, milestone_id))
            .await
            .map_err(GoalMilestoneLinkError::Repository)
    }

    /// Unlink the milestone from the goal. Unlinking a pair that is not
    /// linked is a no-op: it returns `false` instead of failing.
    pub async fn unlink(
        &self,
        goal_id: GoalId,
        milestone_id: MilestoneId,
    ) -> Result<bool, GoalMilestoneLinkError> {
        self.require_pair(goal_id, milestone_id).await?;
        self.links
            .unlink(goal_id, milestone_id)
            .await
            .map_err(GoalMilestoneLinkError::Repository)
    }

    /// The milestones linked to the goal, ordered by target date (nulls
    /// last), then created_at, then id.
    pub async fn milestones_for_goal(
        &self,
        goal_id: GoalId,
    ) -> Result<Vec<Milestone>, GoalMilestoneLinkError> {
        self.require_goal(goal_id).await?;
        self.links
            .milestones_for_goal(goal_id)
            .await
            .map_err(GoalMilestoneLinkError::Repository)
    }

    /// The goals linked to the milestone, in the same order.
    pub async fn goals_for_milestone(
        &self,
        milestone_id: MilestoneId,
    ) -> Result<Vec<Goal>, GoalMilestoneLinkError> {
        self.require_milestone(milestone_id).await?;
        self.links
            .goals_for_milestone(milestone_id)
            .await
            .map_err(GoalMilestoneLinkError::Repository)
    }

    /// One page of [`Self::milestones_for_goal`]: `total` counts only the
    /// linked rows and ignores paging (roadmap 3.10).
    pub async fn milestones_for_goal_page(
        &self,
        goal_id: GoalId,
        page: &PageRequest,
    ) -> Result<Page<Milestone>, GoalMilestoneLinkError> {
        self.require_goal(goal_id).await?;
        self.links
            .milestones_for_goal_page(goal_id, page)
            .await
            .map_err(GoalMilestoneLinkError::Repository)
    }

    /// One page of [`Self::goals_for_milestone`], same rules.
    pub async fn goals_for_milestone_page(
        &self,
        milestone_id: MilestoneId,
        page: &PageRequest,
    ) -> Result<Page<Goal>, GoalMilestoneLinkError> {
        self.require_milestone(milestone_id).await?;
        self.links
            .goals_for_milestone_page(milestone_id, page)
            .await
            .map_err(GoalMilestoneLinkError::Repository)
    }

    /// Both ids must exist before a link or unlink; the goal is checked
    /// first, so a pair missing both reports the goal's absence.
    async fn require_pair(
        &self,
        goal_id: GoalId,
        milestone_id: MilestoneId,
    ) -> Result<(), GoalMilestoneLinkError> {
        self.require_goal(goal_id).await?;
        self.require_milestone(milestone_id).await
    }

    async fn require_goal(&self, id: GoalId) -> Result<(), GoalMilestoneLinkError> {
        match self
            .goals
            .find_by_id(id)
            .await
            .map_err(GoalMilestoneLinkError::Repository)?
        {
            Some(_) => Ok(()),
            None => Err(GoalMilestoneLinkError::GoalNotFound),
        }
    }

    async fn require_milestone(&self, id: MilestoneId) -> Result<(), GoalMilestoneLinkError> {
        match self
            .milestones
            .find_by_id(id)
            .await
            .map_err(GoalMilestoneLinkError::Repository)?
        {
            Some(_) => Ok(()),
            None => Err(GoalMilestoneLinkError::MilestoneNotFound),
        }
    }
}

#[cfg(test)]
mod tests {
    use std::cmp::Ordering;
    use std::collections::{HashMap, HashSet};
    use std::sync::{Arc, Mutex};

    use chrono::{DateTime, NaiveDate, TimeZone, Utc};
    use domain::Status;

    use crate::pagination::{Page, PageRequest};
    use crate::ports::goal_repository::fakes::{apply_goal_list_filter, page_goals};
    use crate::ports::milestone_repository::fakes::{apply_milestone_list_filter, page_milestones};
    use crate::ports::{GoalListFilter, MilestoneListFilter};

    use super::*;

    /// A fixed timestamp in 2026, exact to the second.
    fn at(month: u32, day: u32) -> DateTime<Utc> {
        Utc.with_ymd_and_hms(2026, month, day, 9, 0, 0).unwrap()
    }

    fn date(month: u32, day: u32) -> NaiveDate {
        NaiveDate::from_ymd_opt(2026, month, day).unwrap()
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
            created_at: now,
            updated_at: now,
        }
    }

    fn goal_with(target_date: Option<NaiveDate>, created_at: DateTime<Utc>) -> Goal {
        let mut goal = test_goal();
        goal.target_date = target_date;
        goal.created_at = created_at;
        goal.updated_at = created_at;
        goal
    }

    fn test_milestone() -> Milestone {
        let now = Utc::now();
        Milestone {
            id: MilestoneId::new(),
            title: "Test milestone".to_owned(),
            description: None,
            status: Status::OnTrack,
            status_override: None,
            target_date: None,
            created_at: now,
            updated_at: now,
        }
    }

    fn milestone_with(target_date: Option<NaiveDate>, created_at: DateTime<Utc>) -> Milestone {
        let mut milestone = test_milestone();
        milestone.target_date = target_date;
        milestone.created_at = created_at;
        milestone.updated_at = created_at;
        milestone
    }

    /// An in-memory [`GoalRepository`] with the behaviour the service uses.
    #[derive(Default)]
    struct InMemoryGoalRepository {
        goals: Mutex<HashMap<GoalId, Goal>>,
    }

    #[async_trait::async_trait]
    impl GoalRepository for InMemoryGoalRepository {
        async fn create(&self, goal: Goal) -> Result<Goal, RepositoryError> {
            self.goals.lock().unwrap().insert(goal.id, goal.clone());
            Ok(goal)
        }

        async fn find_by_id(&self, id: GoalId) -> Result<Option<Goal>, RepositoryError> {
            Ok(self.goals.lock().unwrap().get(&id).cloned())
        }

        async fn list(&self) -> Result<Vec<Goal>, RepositoryError> {
            Ok(self.goals.lock().unwrap().values().cloned().collect())
        }

        async fn update(&self, goal: Goal) -> Result<Goal, RepositoryError> {
            if self.goals.lock().unwrap().contains_key(&goal.id) {
                Ok(goal)
            } else {
                Err(RepositoryError::NotFound)
            }
        }

        async fn set_status_override(
            &self,
            id: GoalId,
            status_override: Option<Status>,
        ) -> Result<(), RepositoryError> {
            let mut goals = self.goals.lock().unwrap();
            let Some(goal) = goals.get_mut(&id) else {
                return Err(RepositoryError::NotFound);
            };
            goal.status_override = status_override;
            Ok(())
        }

        async fn delete(&self, id: GoalId) -> Result<(), RepositoryError> {
            if self.goals.lock().unwrap().remove(&id).is_none() {
                return Err(RepositoryError::NotFound);
            }
            Ok(())
        }

        async fn list_page(
            &self,
            filter: &GoalListFilter,
            page: &PageRequest,
        ) -> Result<Page<Goal>, RepositoryError> {
            let matched = apply_goal_list_filter(self.goals.lock().unwrap().values(), filter);
            Ok(page_goals(matched, page))
        }
    }

    /// An in-memory [`MilestoneRepository`] with the behaviour the service
    /// uses.
    #[derive(Default)]
    struct InMemoryMilestoneRepository {
        milestones: Mutex<HashMap<MilestoneId, Milestone>>,
    }

    #[async_trait::async_trait]
    impl MilestoneRepository for InMemoryMilestoneRepository {
        async fn create(&self, milestone: Milestone) -> Result<Milestone, RepositoryError> {
            self.milestones
                .lock()
                .unwrap()
                .insert(milestone.id, milestone.clone());
            Ok(milestone)
        }

        async fn find_by_id(&self, id: MilestoneId) -> Result<Option<Milestone>, RepositoryError> {
            Ok(self.milestones.lock().unwrap().get(&id).cloned())
        }

        async fn list(&self) -> Result<Vec<Milestone>, RepositoryError> {
            Ok(self.milestones.lock().unwrap().values().cloned().collect())
        }

        async fn update(&self, milestone: Milestone) -> Result<Milestone, RepositoryError> {
            if self.milestones.lock().unwrap().contains_key(&milestone.id) {
                Ok(milestone)
            } else {
                Err(RepositoryError::NotFound)
            }
        }

        async fn set_status_override(
            &self,
            id: MilestoneId,
            status_override: Option<Status>,
        ) -> Result<(), RepositoryError> {
            let mut milestones = self.milestones.lock().unwrap();
            let Some(milestone) = milestones.get_mut(&id) else {
                return Err(RepositoryError::NotFound);
            };
            milestone.status_override = status_override;
            Ok(())
        }

        async fn delete(&self, id: MilestoneId) -> Result<(), RepositoryError> {
            if self.milestones.lock().unwrap().remove(&id).is_none() {
                return Err(RepositoryError::NotFound);
            }
            Ok(())
        }

        async fn list_page(
            &self,
            filter: &MilestoneListFilter,
            page: &PageRequest,
        ) -> Result<Page<Milestone>, RepositoryError> {
            let matched =
                apply_milestone_list_filter(self.milestones.lock().unwrap().values(), filter);
            Ok(page_milestones(matched, page))
        }
    }

    /// An in-memory [`GoalMilestoneRepository`] mirroring Postgres: the pair
    /// set makes link/unlink idempotent, and listings join through the
    /// entity stores with the same ordering rule.
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
    }

    #[async_trait::async_trait]
    impl GoalMilestoneRepository for InMemoryGoalMilestoneRepository {
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

        async fn milestones_for_goal(
            &self,
            goal_id: GoalId,
        ) -> Result<Vec<Milestone>, RepositoryError> {
            let all = self.milestones.list().await.unwrap();
            // The guard must not be held across the await above.
            let pairs = self.pairs.lock().unwrap();
            let mut found: Vec<Milestone> = all
                .into_iter()
                .filter(|m| pairs.contains(&(goal_id, m.id)))
                .collect();
            drop(pairs);
            order_by_target_then_created_then_id(&mut found, |m| {
                (m.target_date, m.created_at, m.id.0)
            });
            Ok(found)
        }

        async fn goals_for_milestone(
            &self,
            milestone_id: MilestoneId,
        ) -> Result<Vec<Goal>, RepositoryError> {
            let all = self.goals.list().await.unwrap();
            // The guard must not be held across the await above.
            let pairs = self.pairs.lock().unwrap();
            let mut found: Vec<Goal> = all
                .into_iter()
                .filter(|g| pairs.contains(&(g.id, milestone_id)))
                .collect();
            drop(pairs);
            order_by_target_then_created_then_id(&mut found, |g| {
                (g.target_date, g.created_at, g.id.0)
            });
            Ok(found)
        }

        async fn milestones_for_goal_page(
            &self,
            goal_id: GoalId,
            page: &PageRequest,
        ) -> Result<Page<Milestone>, RepositoryError> {
            let all = self.milestones.list().await.unwrap();
            // The guard must not be held across the await above.
            let pairs = self.pairs.lock().unwrap();
            let mut found: Vec<Milestone> = all
                .into_iter()
                .filter(|m| pairs.contains(&(goal_id, m.id)))
                .collect();
            drop(pairs);
            order_by_target_then_created_then_id(&mut found, |m| {
                (m.target_date, m.created_at, m.id.0)
            });
            Ok(page_milestones(found, page))
        }

        async fn goals_for_milestone_page(
            &self,
            milestone_id: MilestoneId,
            page: &PageRequest,
        ) -> Result<Page<Goal>, RepositoryError> {
            let all = self.goals.list().await.unwrap();
            // The guard must not be held across the await above.
            let pairs = self.pairs.lock().unwrap();
            let mut found: Vec<Goal> = all
                .into_iter()
                .filter(|g| pairs.contains(&(g.id, milestone_id)))
                .collect();
            drop(pairs);
            order_by_target_then_created_then_id(&mut found, |g| {
                (g.target_date, g.created_at, g.id.0)
            });
            Ok(page_goals(found, page))
        }
    }

    /// The listing order shared with the Postgres implementation: target
    /// date ascending with nulls last, then created_at, then id.
    fn order_by_target_then_created_then_id<T, I>(
        items: &mut [T],
        fields: impl Fn(&T) -> (Option<NaiveDate>, DateTime<Utc>, I),
    ) where
        I: Ord + Copy,
    {
        items.sort_by(|a, b| {
            let (ta, ca, ia) = fields(a);
            let (tb, cb, ib) = fields(b);
            target_date_cmp(&ta, &tb)
                .then(ca.cmp(&cb))
                .then(ia.cmp(&ib))
        });
    }

    /// `Option<NaiveDate>` ascending with `None` last.
    fn target_date_cmp(a: &Option<NaiveDate>, b: &Option<NaiveDate>) -> Ordering {
        match (a, b) {
            (Some(a), Some(b)) => a.cmp(b),
            (Some(_), None) => Ordering::Less,
            (None, Some(_)) => Ordering::Greater,
            (None, None) => Ordering::Equal,
        }
    }

    fn setup() -> (
        Arc<InMemoryGoalRepository>,
        Arc<InMemoryMilestoneRepository>,
        GoalMilestoneLinkService,
    ) {
        let goals = Arc::new(InMemoryGoalRepository::default());
        let milestones = Arc::new(InMemoryMilestoneRepository::default());
        let links = Arc::new(InMemoryGoalMilestoneRepository::new(
            goals.clone(),
            milestones.clone(),
        ));
        let service = GoalMilestoneLinkService::new(goals.clone(), milestones.clone(), links);
        (goals, milestones, service)
    }

    #[tokio::test]
    async fn link_creates_the_link_on_both_sides() {
        let (goals, milestones, service) = setup();
        let goal = test_goal();
        goals.create(goal.clone()).await.unwrap();
        let mut milestone = test_milestone();
        milestone.set_status_override(Status::OffTrack);
        milestones.create(milestone.clone()).await.unwrap();

        assert!(service.link(goal.id, milestone.id).await.unwrap());

        // Full entities come back, the status override included.
        let milestone_id = milestone.id;
        assert_eq!(
            service.milestones_for_goal(goal.id).await.unwrap(),
            vec![milestone]
        );
        assert_eq!(
            service.goals_for_milestone(milestone_id).await.unwrap(),
            vec![goal]
        );
    }

    #[tokio::test]
    async fn linking_twice_is_idempotent() {
        let (goals, milestones, service) = setup();
        let goal = test_goal();
        goals.create(goal.clone()).await.unwrap();
        let milestone = test_milestone();
        milestones.create(milestone.clone()).await.unwrap();

        assert!(service.link(goal.id, milestone.id).await.unwrap());
        assert!(!service.link(goal.id, milestone.id).await.unwrap());

        assert_eq!(service.milestones_for_goal(goal.id).await.unwrap().len(), 1);
    }

    #[tokio::test]
    async fn unlink_removes_the_link_and_is_idempotent() {
        let (goals, milestones, service) = setup();
        let goal = test_goal();
        goals.create(goal.clone()).await.unwrap();
        let milestone = test_milestone();
        milestones.create(milestone.clone()).await.unwrap();
        service.link(goal.id, milestone.id).await.unwrap();

        assert!(service.unlink(goal.id, milestone.id).await.unwrap());
        assert!(
            service
                .milestones_for_goal(goal.id)
                .await
                .unwrap()
                .is_empty()
        );
        assert!(
            service
                .goals_for_milestone(milestone.id)
                .await
                .unwrap()
                .is_empty()
        );
        // A second unlink is a no-op, not an error.
        assert!(!service.unlink(goal.id, milestone.id).await.unwrap());
    }

    #[tokio::test]
    async fn unknown_ids_give_the_right_typed_errors() {
        let (goals, milestones, service) = setup();
        let goal = test_goal();
        goals.create(goal.clone()).await.unwrap();
        let milestone = test_milestone();
        milestones.create(milestone).await.unwrap();

        // Both ids missing: the goal is checked first.
        assert!(matches!(
            service.link(GoalId::new(), MilestoneId::new()).await,
            Err(GoalMilestoneLinkError::GoalNotFound)
        ));
        assert!(matches!(
            service.unlink(GoalId::new(), MilestoneId::new()).await,
            Err(GoalMilestoneLinkError::GoalNotFound)
        ));
        // Known goal, unknown milestone.
        assert!(matches!(
            service.link(goal.id, MilestoneId::new()).await,
            Err(GoalMilestoneLinkError::MilestoneNotFound)
        ));
        assert!(matches!(
            service.unlink(goal.id, MilestoneId::new()).await,
            Err(GoalMilestoneLinkError::MilestoneNotFound)
        ));
        // Listings.
        assert!(matches!(
            service.milestones_for_goal(GoalId::new()).await,
            Err(GoalMilestoneLinkError::GoalNotFound)
        ));
        assert!(matches!(
            service.goals_for_milestone(MilestoneId::new()).await,
            Err(GoalMilestoneLinkError::MilestoneNotFound)
        ));
        // Paged listings.
        let page = PageRequest::new(None, None).unwrap();
        assert!(matches!(
            service.milestones_for_goal_page(GoalId::new(), &page).await,
            Err(GoalMilestoneLinkError::GoalNotFound)
        ));
        assert!(matches!(
            service
                .goals_for_milestone_page(MilestoneId::new(), &page)
                .await,
            Err(GoalMilestoneLinkError::MilestoneNotFound)
        ));
    }

    #[tokio::test]
    async fn a_goal_can_link_to_several_milestones_and_a_milestone_to_several_goals() {
        let (goals, milestones, service) = setup();
        let goal_one = goal_with(None, at(1, 1));
        goals.create(goal_one.clone()).await.unwrap();
        let goal_two = goal_with(None, at(1, 2));
        goals.create(goal_two.clone()).await.unwrap();
        let milestone_one = milestone_with(None, at(1, 3));
        milestones.create(milestone_one.clone()).await.unwrap();
        let milestone_two = milestone_with(None, at(1, 4));
        milestones.create(milestone_two.clone()).await.unwrap();

        assert!(service.link(goal_one.id, milestone_one.id).await.unwrap());
        assert!(service.link(goal_one.id, milestone_two.id).await.unwrap());
        assert!(service.link(goal_two.id, milestone_one.id).await.unwrap());

        let for_goal = service.milestones_for_goal(goal_one.id).await.unwrap();
        assert_eq!(
            for_goal.iter().map(|m| m.id).collect::<Vec<_>>(),
            vec![milestone_one.id, milestone_two.id]
        );
        let for_milestone = service.goals_for_milestone(milestone_one.id).await.unwrap();
        assert_eq!(
            for_milestone.iter().map(|g| g.id).collect::<Vec<_>>(),
            vec![goal_one.id, goal_two.id]
        );
    }

    #[tokio::test]
    async fn milestones_for_goal_is_ordered_by_target_date_nulls_last_then_created_at() {
        let (goals, milestones, service) = setup();
        let goal = test_goal();
        goals.create(goal.clone()).await.unwrap();
        // Dated first in date order, undated last in created_at order.
        let m_first = milestone_with(Some(date(1, 5)), at(1, 3));
        let m_second = milestone_with(Some(date(1, 10)), at(1, 1));
        let m_third = milestone_with(None, at(1, 2));
        let m_fourth = milestone_with(None, at(1, 4));
        for milestone in [&m_first, &m_second, &m_third, &m_fourth] {
            milestones.create(milestone.clone()).await.unwrap();
            service.link(goal.id, milestone.id).await.unwrap();
        }

        let listed = service.milestones_for_goal(goal.id).await.unwrap();

        assert_eq!(
            listed.iter().map(|m| m.id).collect::<Vec<_>>(),
            vec![m_first.id, m_second.id, m_third.id, m_fourth.id]
        );
    }

    #[tokio::test]
    async fn goals_for_milestone_is_ordered_the_same_way() {
        let (goals, milestones, service) = setup();
        let milestone = test_milestone();
        milestones.create(milestone.clone()).await.unwrap();
        let g_undated = goal_with(None, at(1, 2));
        let g_late = goal_with(Some(date(1, 20)), at(1, 5));
        let g_early = goal_with(Some(date(1, 1)), at(1, 9));
        for goal in [&g_undated, &g_late, &g_early] {
            goals.create(goal.clone()).await.unwrap();
            service.link(goal.id, milestone.id).await.unwrap();
        }

        let listed = service.goals_for_milestone(milestone.id).await.unwrap();

        assert_eq!(
            listed.iter().map(|g| g.id).collect::<Vec<_>>(),
            vec![g_early.id, g_late.id, g_undated.id]
        );
    }

    #[tokio::test]
    async fn the_paged_listings_count_only_linked_rows_and_page_in_order() {
        let (goals, milestones, service) = setup();
        let goal = test_goal();
        goals.create(goal.clone()).await.unwrap();
        // Two of three milestones are linked; the third must not appear in
        // total or items.
        let m_first = milestone_with(Some(date(1, 5)), at(1, 3));
        let m_second = milestone_with(None, at(1, 1));
        let unlinked_milestone = milestone_with(None, at(1, 2));
        for milestone in [&m_first, &m_second, &unlinked_milestone] {
            milestones.create(milestone.clone()).await.unwrap();
        }
        service.link(goal.id, m_first.id).await.unwrap();
        service.link(goal.id, m_second.id).await.unwrap();

        let page = PageRequest::new(Some(1), Some(0)).unwrap();
        let listed = service
            .milestones_for_goal_page(goal.id, &page)
            .await
            .unwrap();
        assert_eq!(listed.total, 2);
        assert_eq!(
            listed.items.iter().map(|m| m.id).collect::<Vec<_>>(),
            vec![m_first.id]
        );

        let page = PageRequest::new(Some(1), Some(1)).unwrap();
        let listed = service
            .milestones_for_goal_page(goal.id, &page)
            .await
            .unwrap();
        assert_eq!(listed.total, 2);
        assert_eq!(
            listed.items.iter().map(|m| m.id).collect::<Vec<_>>(),
            vec![m_second.id]
        );

        // The mirror side counts only the goals linked to the milestone.
        let other_goal = goal_with(None, at(1, 9));
        goals.create(other_goal.clone()).await.unwrap();
        let stray_goal = goal_with(None, at(1, 8));
        goals.create(stray_goal.clone()).await.unwrap();
        service.link(other_goal.id, m_first.id).await.unwrap();

        let page = PageRequest::new(Some(10), Some(0)).unwrap();
        let listed = service
            .goals_for_milestone_page(m_first.id, &page)
            .await
            .unwrap();
        assert_eq!(listed.total, 2);
        // Both undated: created_at order puts January before the goal's now.
        assert_eq!(
            listed.items.iter().map(|g| g.id).collect::<Vec<_>>(),
            vec![other_goal.id, goal.id]
        );
    }
}
