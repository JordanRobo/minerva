//! Persistence port for [`Goal`]s.

use chrono::NaiveDate;
use domain::{Goal, GoalId, Status};

use crate::pagination::{Page, PageRequest};
use crate::ports::RepositoryError;

/// The filters the goal list accepts (roadmap 3.10). Every field is optional;
/// an all-`None` filter matches every goal, and the set fields combine with
/// AND. Filters never fail: a search that matches nothing simply yields an
/// empty page.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct GoalListFilter {
    /// Restrict to goals whose effective status is one of these (the manual
    /// override when set, otherwise the automatic value); empty means any.
    pub statuses: Vec<Status>,
    /// Case-insensitive substring match on the title, in which `%`, `_` and
    /// `\` are matched literally rather than as wildcards.
    pub q: Option<String>,
    /// Inclusive lower bound on `target_date`; goals without a target date
    /// never match a date bound.
    pub target_after: Option<NaiveDate>,
    /// Inclusive upper bound on `target_date` (see [`Self::target_after`]).
    pub target_before: Option<NaiveDate>,
}

#[async_trait::async_trait]
pub trait GoalRepository: Send + Sync {
    async fn create(&self, goal: Goal) -> Result<Goal, RepositoryError>;

    /// Returns `None` if no goal has this id.
    async fn find_by_id(&self, id: GoalId) -> Result<Option<Goal>, RepositoryError>;

    async fn list(&self) -> Result<Vec<Goal>, RepositoryError>;

    async fn update(&self, goal: Goal) -> Result<Goal, RepositoryError>;

    /// Set or clear the goal's manual status override; `None` clears it.
    /// Never touches the automatic status (roadmap 3.2).
    async fn set_status_override(
        &self,
        id: GoalId,
        status_override: Option<Status>,
    ) -> Result<(), RepositoryError>;

    async fn delete(&self, id: GoalId) -> Result<(), RepositoryError>;

    /// One page of the goals matching `filter` (roadmap 3.10), in the default
    /// order (roadmap 3.16): target date ascending with nulls last, then
    /// created_at, then id. The status filter matches the goal's effective
    /// status — the manual override when set, otherwise the automatic value
    /// (roadmap 3.2). [`Page::total`] counts every row matching `filter`,
    /// ignoring `page`.
    async fn list_page(
        &self,
        filter: &GoalListFilter,
        page: &PageRequest,
    ) -> Result<Page<Goal>, RepositoryError>;
}

#[cfg(test)]
pub mod fakes {
    use std::cmp::Ordering;
    use std::sync::Mutex;

    use chrono::{DateTime, NaiveDate, Utc};
    use domain::{Goal, GoalId, Status};

    use super::{GoalListFilter, GoalRepository};
    use crate::pagination::{Page, PageRequest};
    use crate::ports::RepositoryError;

    /// The goal-list filters and default order (roadmap 3.10, 3.16) applied to
    /// an in-memory set of goals: the same rules as the Postgres
    /// implementation — ANDed filters, a case-insensitive literal substring
    /// for `q`, inclusive date bounds that exclude undated goals, and a status
    /// filter on the effective status (override when set) — then the default
    /// order. Returns every match; page it with [`page_goals`].
    pub fn apply_goal_list_filter<'a, I>(goals: I, filter: &GoalListFilter) -> Vec<Goal>
    where
        I: IntoIterator<Item = &'a Goal>,
    {
        let matched = goals
            .into_iter()
            .filter(|goal| {
                (filter.statuses.is_empty() || filter.statuses.contains(&goal.effective_status()))
                    && match &filter.q {
                        Some(needle) => goal.title.to_lowercase().contains(&needle.to_lowercase()),
                        None => true,
                    }
                    && match filter.target_after {
                        Some(after) => goal.target_date.is_some_and(|date| date >= after),
                        None => true,
                    }
                    && match filter.target_before {
                        Some(before) => goal.target_date.is_some_and(|date| date <= before),
                        None => true,
                    }
            })
            .cloned()
            .collect();
        default_goal_order(matched)
    }

    /// The default list order (roadmap 3.16): target date ascending with nulls
    /// last, then created_at, then id.
    pub fn default_goal_order(mut goals: Vec<Goal>) -> Vec<Goal> {
        goals.sort_by(|a, b| {
            let by_date = match (a.target_date, b.target_date) {
                (Some(a), Some(b)) => a.cmp(&b),
                (Some(_), None) => Ordering::Less,
                (None, Some(_)) => Ordering::Greater,
                (None, None) => Ordering::Equal,
            };
            by_date
                .then_with(|| a.created_at.cmp(&b.created_at))
                .then_with(|| a.id.0.cmp(&b.id.0))
        });
        goals
    }

    /// Slice an already-filtered, default-ordered match list into one page.
    pub fn page_goals(matched: Vec<Goal>, page: &PageRequest) -> Page<Goal> {
        let total = matched.len() as u64;
        let items = matched
            .into_iter()
            .skip(page.offset as usize)
            .take(page.limit as usize)
            .collect();
        Page {
            items,
            total,
            limit: page.limit,
            offset: page.offset,
        }
    }

    /// In-memory [`GoalRepository`] mirroring the Postgres implementation, for
    /// tests that need a goal store without a database.
    #[derive(Default)]
    pub struct InMemoryGoalRepository {
        goals: Mutex<Vec<Goal>>,
    }

    impl InMemoryGoalRepository {
        pub fn new() -> Self {
            Self::default()
        }
    }

    #[async_trait::async_trait]
    impl GoalRepository for InMemoryGoalRepository {
        async fn create(&self, goal: Goal) -> Result<Goal, RepositoryError> {
            self.goals.lock().unwrap().push(goal.clone());
            Ok(goal)
        }

        async fn find_by_id(&self, id: GoalId) -> Result<Option<Goal>, RepositoryError> {
            Ok(self
                .goals
                .lock()
                .unwrap()
                .iter()
                .find(|goal| goal.id == id)
                .cloned())
        }

        async fn list(&self) -> Result<Vec<Goal>, RepositoryError> {
            Ok(self.goals.lock().unwrap().clone())
        }

        async fn update(&self, goal: Goal) -> Result<Goal, RepositoryError> {
            let mut goals = self.goals.lock().unwrap();
            match goals.iter_mut().find(|existing| existing.id == goal.id) {
                Some(existing) => {
                    *existing = goal.clone();
                    Ok(goal)
                }
                None => Err(RepositoryError::NotFound),
            }
        }

        async fn set_status_override(
            &self,
            id: GoalId,
            status_override: Option<Status>,
        ) -> Result<(), RepositoryError> {
            let mut goals = self.goals.lock().unwrap();
            let Some(goal) = goals.iter_mut().find(|existing| existing.id == id) else {
                return Err(RepositoryError::NotFound);
            };
            goal.status_override = status_override;
            Ok(())
        }

        async fn delete(&self, id: GoalId) -> Result<(), RepositoryError> {
            let mut goals = self.goals.lock().unwrap();
            let before = goals.len();
            goals.retain(|goal| goal.id != id);
            if goals.len() == before {
                return Err(RepositoryError::NotFound);
            }
            Ok(())
        }

        async fn list_page(
            &self,
            filter: &GoalListFilter,
            page: &PageRequest,
        ) -> Result<Page<Goal>, RepositoryError> {
            let matched = apply_goal_list_filter(self.goals.lock().unwrap().iter(), filter);
            Ok(page_goals(matched, page))
        }
    }

    #[cfg(test)]
    mod tests {
        use uuid::Uuid;

        use super::*;

        fn date(month: u32, day: u32) -> NaiveDate {
            NaiveDate::from_ymd_opt(2026, month, day).expect("valid date")
        }

        fn at(day: u32) -> DateTime<Utc> {
            date(1, day)
                .and_hms_opt(9, 0, 0)
                .expect("valid time")
                .and_utc()
        }

        /// A goal with a deterministic id and the fields the filters read.
        fn goal(
            id: u128,
            title: &str,
            status: Status,
            status_override: Option<Status>,
            target_date: Option<NaiveDate>,
            created_at: DateTime<Utc>,
        ) -> Goal {
            Goal {
                id: GoalId(Uuid::from_u128(id)),
                title: title.to_owned(),
                description: None,
                status,
                status_override,
                target_date,
                created_at,
                updated_at: created_at,
            }
        }

        fn ids(page: &Page<Goal>) -> Vec<u128> {
            page.items.iter().map(|goal| goal.id.0.as_u128()).collect()
        }

        /// The fixture every filter test starts from: four statuses (one of
        /// them only via an override), dates on both sides of the 5th (and one
        /// undated goal).
        fn fixtures() -> Vec<Goal> {
            vec![
                goal(1, "Alpha", Status::OnTrack, None, Some(date(1, 5)), at(1)),
                // Automatic OnTrack, manually overridden to AtRisk: the status
                // filter must see AtRisk and not OnTrack.
                goal(
                    2,
                    "Beta",
                    Status::OnTrack,
                    Some(Status::AtRisk),
                    Some(date(1, 10)),
                    at(2),
                ),
                goal(3, "Gamma", Status::OffTrack, None, Some(date(1, 1)), at(3)),
                goal(4, "Delta", Status::Complete, None, None, at(4)),
                goal(
                    5,
                    "Epsilon",
                    Status::OnTrack,
                    None,
                    Some(date(1, 20)),
                    at(5),
                ),
            ]
        }

        async fn list(goals: &[Goal], filter: &GoalListFilter) -> Page<Goal> {
            let repo = InMemoryGoalRepository::new();
            for goal in goals {
                repo.create(goal.clone()).await.unwrap();
            }
            repo.list_page(filter, &PageRequest::new(None, None).unwrap())
                .await
                .unwrap()
        }

        #[tokio::test]
        async fn the_status_filter_matches_the_effective_status() {
            let goals = fixtures();
            // The overridden goal counts as AtRisk...
            let page = list(
                &goals,
                &GoalListFilter {
                    statuses: vec![Status::AtRisk],
                    ..Default::default()
                },
            )
            .await;
            assert_eq!(ids(&page), vec![2]);

            // ...and does not count as its automatic OnTrack.
            let page = list(
                &goals,
                &GoalListFilter {
                    statuses: vec![Status::OnTrack],
                    ..Default::default()
                },
            )
            .await;
            assert_eq!(ids(&page), vec![1, 5]);
        }

        #[tokio::test]
        async fn the_status_filter_accepts_several_statuses() {
            let goals = fixtures();
            let page = list(
                &goals,
                &GoalListFilter {
                    statuses: vec![Status::OffTrack, Status::Complete],
                    ..Default::default()
                },
            )
            .await;
            assert_eq!(ids(&page), vec![3, 4]);
        }

        #[tokio::test]
        async fn the_title_search_is_case_insensitive_and_literal() {
            let goals = fixtures();
            // Case-insensitive substring...
            let page = list(
                &goals,
                &GoalListFilter {
                    q: Some("ALPH".to_owned()),
                    ..Default::default()
                },
            )
            .await;
            assert_eq!(ids(&page), vec![1]);

            // ...and the LIKE wildcards are matched literally: "a_b" finds
            // nothing here (no title contains that exact text), while a real
            // substring does.
            let page = list(
                &goals,
                &GoalListFilter {
                    q: Some("a_b".to_owned()),
                    ..Default::default()
                },
            )
            .await;
            assert!(page.items.is_empty());

            let page = list(
                &goals,
                &GoalListFilter {
                    q: Some("amm".to_owned()),
                    ..Default::default()
                },
            )
            .await;
            assert_eq!(ids(&page), vec![3]);
        }

        #[tokio::test]
        async fn wildcard_characters_in_the_search_are_matched_literally() {
            let goals = vec![
                goal(1, "100% ready", Status::OnTrack, None, None, at(1)),
                goal(2, "50 percent ready", Status::OnTrack, None, None, at(2)),
                goal(3, "a_b done", Status::OnTrack, None, None, at(3)),
                goal(4, "axb done", Status::OnTrack, None, None, at(4)),
            ];
            let repo = InMemoryGoalRepository::new();
            for goal in &goals {
                repo.create(goal.clone()).await.unwrap();
            }

            let page = repo
                .list_page(
                    &GoalListFilter {
                        q: Some("100%".to_owned()),
                        ..Default::default()
                    },
                    &PageRequest::new(None, None).unwrap(),
                )
                .await
                .unwrap();
            assert_eq!(ids(&page), vec![1]);

            let page = repo
                .list_page(
                    &GoalListFilter {
                        q: Some("a_b".to_owned()),
                        ..Default::default()
                    },
                    &PageRequest::new(None, None).unwrap(),
                )
                .await
                .unwrap();
            assert_eq!(ids(&page), vec![3]);
        }

        #[tokio::test]
        async fn the_date_bounds_are_inclusive_and_exclude_undated_goals() {
            let goals = fixtures();
            // Dates 1, 5, 10, 20 exist; goal 4 has none.
            let page = list(
                &goals,
                &GoalListFilter {
                    target_after: Some(date(1, 5)),
                    ..Default::default()
                },
            )
            .await;
            assert_eq!(ids(&page), vec![1, 2, 5]);

            let page = list(
                &goals,
                &GoalListFilter {
                    target_before: Some(date(1, 5)),
                    ..Default::default()
                },
            )
            .await;
            assert_eq!(ids(&page), vec![3, 1]);

            let page = list(
                &goals,
                &GoalListFilter {
                    target_after: Some(date(1, 5)),
                    target_before: Some(date(1, 10)),
                    ..Default::default()
                },
            )
            .await;
            assert_eq!(ids(&page), vec![1, 2]);
        }

        #[tokio::test]
        async fn the_filters_combine_with_and() {
            let goals = fixtures();
            let page = list(
                &goals,
                &GoalListFilter {
                    statuses: vec![Status::OnTrack, Status::AtRisk],
                    target_after: Some(date(1, 5)),
                    ..Default::default()
                },
            )
            .await;
            assert_eq!(ids(&page), vec![1, 2, 5]);

            // ...and a search that matches nothing empties the page.
            let page = list(
                &goals,
                &GoalListFilter {
                    statuses: vec![Status::OnTrack],
                    q: Some("no such title".to_owned()),
                    ..Default::default()
                },
            )
            .await;
            assert!(page.items.is_empty());
            assert_eq!(page.total, 0);
        }

        #[tokio::test]
        async fn the_default_order_is_target_date_nulls_last_then_created_at_then_id() {
            let goals = vec![
                goal(1, "a", Status::OnTrack, None, Some(date(1, 5)), at(3)),
                goal(2, "b", Status::OnTrack, None, Some(date(1, 5)), at(3)),
                goal(3, "c", Status::OnTrack, None, Some(date(1, 10)), at(1)),
                goal(4, "d", Status::OnTrack, None, None, at(1)),
                goal(5, "e", Status::OnTrack, None, None, at(9)),
            ];
            let page = list(&goals, &GoalListFilter::default()).await;
            assert_eq!(ids(&page), vec![1, 2, 3, 4, 5]);
        }

        #[tokio::test]
        async fn paging_walks_every_match_exactly_once() {
            let goals: Vec<Goal> = (1..=7)
                .map(|id| {
                    goal(
                        id,
                        &format!("g{id}"),
                        Status::OnTrack,
                        None,
                        Some(date(1, 5)),
                        at(1),
                    )
                })
                .collect();
            let repo = InMemoryGoalRepository::new();
            for goal in &goals {
                repo.create(goal.clone()).await.unwrap();
            }

            let mut seen = Vec::new();
            for offset in [0i64, 3, 6] {
                let page = repo
                    .list_page(
                        &GoalListFilter::default(),
                        &PageRequest::new(Some(3), Some(offset)).unwrap(),
                    )
                    .await
                    .unwrap();
                // The total ignores limit and offset...
                assert_eq!(page.total, 7);
                // ...and the pages do not overlap.
                seen.extend(ids(&page));
            }
            assert_eq!(seen, vec![1, 2, 3, 4, 5, 6, 7]);

            // An offset past the end is an empty page with the right total.
            let page = repo
                .list_page(
                    &GoalListFilter::default(),
                    &PageRequest::new(None, Some(100)).unwrap(),
                )
                .await
                .unwrap();
            assert!(page.items.is_empty());
            assert_eq!(page.total, 7);
        }
    }
}
