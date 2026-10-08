//! Persistence port for [`Milestone`]s.

use chrono::NaiveDate;
use domain::{Milestone, MilestoneId, Status};

use crate::pagination::{Page, PageRequest};
use crate::ports::RepositoryError;

/// The filters the milestone list accepts (roadmap 3.10). Every field is
/// optional; an all-`None` filter matches every milestone, and the set fields
/// combine with AND. Filters never fail: a search that matches nothing simply
/// yields an empty page.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct MilestoneListFilter {
    /// Restrict to milestones whose effective status is one of these (the
    /// manual override when set, otherwise the automatic value); empty means
    /// any.
    pub statuses: Vec<Status>,
    /// Case-insensitive substring match on the title, in which `%`, `_` and
    /// `\` are matched literally rather than as wildcards.
    pub q: Option<String>,
    /// Inclusive lower bound on `target_date`; milestones without a target
    /// date never match a date bound.
    pub target_after: Option<NaiveDate>,
    /// Inclusive upper bound on `target_date` (see [`Self::target_after`]).
    pub target_before: Option<NaiveDate>,
}

#[async_trait::async_trait]
pub trait MilestoneRepository: Send + Sync {
    async fn create(&self, milestone: Milestone) -> Result<Milestone, RepositoryError>;

    /// Returns `None` if no milestone has this id.
    async fn find_by_id(&self, id: MilestoneId) -> Result<Option<Milestone>, RepositoryError>;

    async fn list(&self) -> Result<Vec<Milestone>, RepositoryError>;

    async fn update(&self, milestone: Milestone) -> Result<Milestone, RepositoryError>;

    /// Set or clear the milestone's manual status override; `None` clears
    /// it. Never touches the automatic status (roadmap 3.2).
    async fn set_status_override(
        &self,
        id: MilestoneId,
        status_override: Option<Status>,
    ) -> Result<(), RepositoryError>;

    async fn delete(&self, id: MilestoneId) -> Result<(), RepositoryError>;

    /// One page of the milestones matching `filter` (roadmap 3.10), in the
    /// default order (roadmap 3.16): target date ascending with nulls last,
    /// then created_at, then id. The status filter matches the milestone's
    /// effective status — the manual override when set, otherwise the
    /// automatic value (roadmap 3.2). [`Page::total`] counts every row
    /// matching `filter`, ignoring `page`.
    async fn list_page(
        &self,
        filter: &MilestoneListFilter,
        page: &PageRequest,
    ) -> Result<Page<Milestone>, RepositoryError>;
}

#[cfg(test)]
pub mod fakes {
    use std::cmp::Ordering;
    use std::sync::Mutex;

    use chrono::{DateTime, NaiveDate, Utc};
    use domain::{Milestone, MilestoneId, Status};

    use super::{MilestoneListFilter, MilestoneRepository};
    use crate::pagination::{Page, PageRequest};
    use crate::ports::RepositoryError;

    /// The milestone-list filters and default order (roadmap 3.10, 3.16)
    /// applied to an in-memory set of milestones: the same rules as the
    /// Postgres implementation — ANDed filters, a case-insensitive literal
    /// substring for `q`, inclusive date bounds that exclude undated
    /// milestones, and a status filter on the effective status (override when
    /// set) — then the default order. Returns every match; page it with
    /// [`page_milestones`].
    pub fn apply_milestone_list_filter<'a, I>(
        milestones: I,
        filter: &MilestoneListFilter,
    ) -> Vec<Milestone>
    where
        I: IntoIterator<Item = &'a Milestone>,
    {
        let matched = milestones
            .into_iter()
            .filter(|milestone| {
                (filter.statuses.is_empty()
                    || filter.statuses.contains(&milestone.effective_status()))
                    && match &filter.q {
                        Some(needle) => milestone
                            .title
                            .to_lowercase()
                            .contains(&needle.to_lowercase()),
                        None => true,
                    }
                    && match filter.target_after {
                        Some(after) => milestone.target_date.is_some_and(|date| date >= after),
                        None => true,
                    }
                    && match filter.target_before {
                        Some(before) => milestone.target_date.is_some_and(|date| date <= before),
                        None => true,
                    }
            })
            .cloned()
            .collect();
        default_milestone_order(matched)
    }

    /// The default list order (roadmap 3.16): target date ascending with nulls
    /// last, then created_at, then id.
    pub fn default_milestone_order(mut milestones: Vec<Milestone>) -> Vec<Milestone> {
        milestones.sort_by(|a, b| {
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
        milestones
    }

    /// Slice an already-filtered, default-ordered match list into one page.
    pub fn page_milestones(matched: Vec<Milestone>, page: &PageRequest) -> Page<Milestone> {
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

    /// In-memory [`MilestoneRepository`] mirroring the Postgres
    /// implementation, for tests that need a milestone store without a
    /// database.
    #[derive(Default)]
    pub struct InMemoryMilestoneRepository {
        milestones: Mutex<Vec<Milestone>>,
    }

    impl InMemoryMilestoneRepository {
        pub fn new() -> Self {
            Self::default()
        }
    }

    #[async_trait::async_trait]
    impl MilestoneRepository for InMemoryMilestoneRepository {
        async fn create(&self, milestone: Milestone) -> Result<Milestone, RepositoryError> {
            self.milestones.lock().unwrap().push(milestone.clone());
            Ok(milestone)
        }

        async fn find_by_id(&self, id: MilestoneId) -> Result<Option<Milestone>, RepositoryError> {
            Ok(self
                .milestones
                .lock()
                .unwrap()
                .iter()
                .find(|milestone| milestone.id == id)
                .cloned())
        }

        async fn list(&self) -> Result<Vec<Milestone>, RepositoryError> {
            Ok(self.milestones.lock().unwrap().clone())
        }

        async fn update(&self, milestone: Milestone) -> Result<Milestone, RepositoryError> {
            let mut milestones = self.milestones.lock().unwrap();
            match milestones
                .iter_mut()
                .find(|existing| existing.id == milestone.id)
            {
                Some(existing) => {
                    *existing = milestone.clone();
                    Ok(milestone)
                }
                None => Err(RepositoryError::NotFound),
            }
        }

        async fn set_status_override(
            &self,
            id: MilestoneId,
            status_override: Option<Status>,
        ) -> Result<(), RepositoryError> {
            let mut milestones = self.milestones.lock().unwrap();
            let Some(milestone) = milestones.iter_mut().find(|existing| existing.id == id) else {
                return Err(RepositoryError::NotFound);
            };
            milestone.status_override = status_override;
            Ok(())
        }

        async fn delete(&self, id: MilestoneId) -> Result<(), RepositoryError> {
            let mut milestones = self.milestones.lock().unwrap();
            let before = milestones.len();
            milestones.retain(|milestone| milestone.id != id);
            if milestones.len() == before {
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
                apply_milestone_list_filter(self.milestones.lock().unwrap().iter(), filter);
            Ok(page_milestones(matched, page))
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

        /// A milestone with a deterministic id and the fields the filters read.
        fn milestone(
            id: u128,
            title: &str,
            status: Status,
            status_override: Option<Status>,
            target_date: Option<NaiveDate>,
            created_at: DateTime<Utc>,
        ) -> Milestone {
            Milestone {
                id: MilestoneId(Uuid::from_u128(id)),
                title: title.to_owned(),
                description: None,
                status,
                status_override,
                target_date,
                created_at,
                updated_at: created_at,
            }
        }

        fn ids(page: &Page<Milestone>) -> Vec<u128> {
            page.items.iter().map(|m| m.id.0.as_u128()).collect()
        }

        /// The fixture every filter test starts from: four statuses (one of
        /// them only via an override), dates on both sides of the 5th (and one
        /// undated milestone).
        fn fixtures() -> Vec<Milestone> {
            vec![
                milestone(1, "Alpha", Status::OnTrack, None, Some(date(1, 5)), at(1)),
                // Automatic OnTrack, manually overridden to AtRisk: the status
                // filter must see AtRisk and not OnTrack.
                milestone(
                    2,
                    "Beta",
                    Status::OnTrack,
                    Some(Status::AtRisk),
                    Some(date(1, 10)),
                    at(2),
                ),
                milestone(3, "Gamma", Status::OffTrack, None, Some(date(1, 1)), at(3)),
                milestone(4, "Delta", Status::Complete, None, None, at(4)),
                milestone(
                    5,
                    "Epsilon",
                    Status::OnTrack,
                    None,
                    Some(date(1, 20)),
                    at(5),
                ),
            ]
        }

        async fn list(milestones: &[Milestone], filter: &MilestoneListFilter) -> Page<Milestone> {
            let repo = InMemoryMilestoneRepository::new();
            for milestone in milestones {
                repo.create(milestone.clone()).await.unwrap();
            }
            repo.list_page(filter, &PageRequest::new(None, None).unwrap())
                .await
                .unwrap()
        }

        #[tokio::test]
        async fn the_status_filter_matches_the_effective_status() {
            let milestones = fixtures();
            // The overridden milestone counts as AtRisk...
            let page = list(
                &milestones,
                &MilestoneListFilter {
                    statuses: vec![Status::AtRisk],
                    ..Default::default()
                },
            )
            .await;
            assert_eq!(ids(&page), vec![2]);

            // ...and does not count as its automatic OnTrack.
            let page = list(
                &milestones,
                &MilestoneListFilter {
                    statuses: vec![Status::OnTrack],
                    ..Default::default()
                },
            )
            .await;
            assert_eq!(ids(&page), vec![1, 5]);
        }

        #[tokio::test]
        async fn the_status_filter_accepts_several_statuses() {
            let milestones = fixtures();
            let page = list(
                &milestones,
                &MilestoneListFilter {
                    statuses: vec![Status::OffTrack, Status::Complete],
                    ..Default::default()
                },
            )
            .await;
            assert_eq!(ids(&page), vec![3, 4]);
        }

        #[tokio::test]
        async fn the_title_search_is_case_insensitive_and_literal() {
            let milestones = fixtures();
            // Case-insensitive substring...
            let page = list(
                &milestones,
                &MilestoneListFilter {
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
                &milestones,
                &MilestoneListFilter {
                    q: Some("a_b".to_owned()),
                    ..Default::default()
                },
            )
            .await;
            assert!(page.items.is_empty());

            let page = list(
                &milestones,
                &MilestoneListFilter {
                    q: Some("amm".to_owned()),
                    ..Default::default()
                },
            )
            .await;
            assert_eq!(ids(&page), vec![3]);
        }

        #[tokio::test]
        async fn wildcard_characters_in_the_search_are_matched_literally() {
            let milestones = vec![
                milestone(1, "100% ready", Status::OnTrack, None, None, at(1)),
                milestone(2, "50 percent ready", Status::OnTrack, None, None, at(2)),
                milestone(3, "a_b done", Status::OnTrack, None, None, at(3)),
                milestone(4, "axb done", Status::OnTrack, None, None, at(4)),
            ];
            let repo = InMemoryMilestoneRepository::new();
            for milestone in &milestones {
                repo.create(milestone.clone()).await.unwrap();
            }

            let page = repo
                .list_page(
                    &MilestoneListFilter {
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
                    &MilestoneListFilter {
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
        async fn the_date_bounds_are_inclusive_and_exclude_undated_milestones() {
            let milestones = fixtures();
            // Dates 1, 5, 10, 20 exist; milestone 4 has none.
            let page = list(
                &milestones,
                &MilestoneListFilter {
                    target_after: Some(date(1, 5)),
                    ..Default::default()
                },
            )
            .await;
            assert_eq!(ids(&page), vec![1, 2, 5]);

            let page = list(
                &milestones,
                &MilestoneListFilter {
                    target_before: Some(date(1, 5)),
                    ..Default::default()
                },
            )
            .await;
            assert_eq!(ids(&page), vec![3, 1]);

            let page = list(
                &milestones,
                &MilestoneListFilter {
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
            let milestones = fixtures();
            let page = list(
                &milestones,
                &MilestoneListFilter {
                    statuses: vec![Status::OnTrack, Status::AtRisk],
                    target_after: Some(date(1, 5)),
                    ..Default::default()
                },
            )
            .await;
            assert_eq!(ids(&page), vec![1, 2, 5]);

            // ...and a search that matches nothing empties the page.
            let page = list(
                &milestones,
                &MilestoneListFilter {
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
            let milestones = vec![
                milestone(1, "a", Status::OnTrack, None, Some(date(1, 5)), at(3)),
                milestone(2, "b", Status::OnTrack, None, Some(date(1, 5)), at(3)),
                milestone(3, "c", Status::OnTrack, None, Some(date(1, 10)), at(1)),
                milestone(4, "d", Status::OnTrack, None, None, at(1)),
                milestone(5, "e", Status::OnTrack, None, None, at(9)),
            ];
            let page = list(&milestones, &MilestoneListFilter::default()).await;
            assert_eq!(ids(&page), vec![1, 2, 3, 4, 5]);
        }

        #[tokio::test]
        async fn paging_walks_every_match_exactly_once() {
            let milestones: Vec<Milestone> = (1..=7)
                .map(|id| {
                    milestone(
                        id,
                        &format!("m{id}"),
                        Status::OnTrack,
                        None,
                        Some(date(1, 5)),
                        at(1),
                    )
                })
                .collect();
            let repo = InMemoryMilestoneRepository::new();
            for milestone in &milestones {
                repo.create(milestone.clone()).await.unwrap();
            }

            let mut seen = Vec::new();
            for offset in [0i64, 3, 6] {
                let page = repo
                    .list_page(
                        &MilestoneListFilter::default(),
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
                    &MilestoneListFilter::default(),
                    &PageRequest::new(None, Some(100)).unwrap(),
                )
                .await
                .unwrap();
            assert!(page.items.is_empty());
            assert_eq!(page.total, 7);
        }
    }
}
