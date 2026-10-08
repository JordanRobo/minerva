//! Persistence port for [`Task`]s.

use chrono::NaiveDate;
use domain::{MilestoneId, Task, TaskId, TaskStatus};

use crate::pagination::{Page, PageRequest};
use crate::ports::RepositoryError;

/// The filters the task list accepts (roadmap 3.10). Every field is optional;
/// an all-`None` filter matches every task, and the set fields combine with
/// AND. Filters never fail: an unknown id or a search that matches nothing
/// simply yields an empty page.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct TaskListFilter {
    /// Restrict to these board columns; empty means any column.
    pub statuses: Vec<TaskStatus>,
    /// Restrict to the tasks assigned to this milestone; `None` means any,
    /// including unassigned ones.
    pub milestone_id: Option<MilestoneId>,
    /// Case-insensitive substring match on the title, in which `%`, `_` and
    /// `\` are matched literally rather than as wildcards.
    pub q: Option<String>,
    /// Inclusive lower bound on `target_date`; tasks without a target date
    /// never match a date bound.
    pub target_after: Option<NaiveDate>,
    /// Inclusive upper bound on `target_date` (see [`Self::target_after`]).
    pub target_before: Option<NaiveDate>,
}

#[async_trait::async_trait]
pub trait TaskRepository: Send + Sync {
    async fn create(&self, task: Task) -> Result<Task, RepositoryError>;

    /// Returns `None` if no task has this id.
    async fn find_by_id(&self, id: TaskId) -> Result<Option<Task>, RepositoryError>;

    /// The tasks with the given ids, in id order; ids with no matching task
    /// are skipped. An empty slice yields an empty list.
    async fn find_by_ids(&self, ids: &[TaskId]) -> Result<Vec<Task>, RepositoryError>;

    async fn update(&self, task: Task) -> Result<Task, RepositoryError>;

    /// Change only the task's board column (and `updated_at`), in a single
    /// write that cannot clobber fields another writer changed in between;
    /// returns the updated task. An unknown id is a `NotFound`.
    async fn set_status(&self, id: TaskId, status: TaskStatus) -> Result<Task, RepositoryError>;

    async fn delete(&self, id: TaskId) -> Result<(), RepositoryError>;

    /// One page of the tasks matching `filter` (roadmap 3.10), in the
    /// default order (roadmap 3.16): target date ascending with nulls last,
    /// then created_at, then id — the deterministic v1 board order, since
    /// there is no manual card ordering. [`Page::total`] counts every row
    /// matching `filter`, ignoring `page`.
    async fn list_page(
        &self,
        filter: &TaskListFilter,
        page: &PageRequest,
    ) -> Result<Page<Task>, RepositoryError>;
}

#[cfg(test)]
pub mod fakes {
    use std::cmp::Ordering;
    use std::sync::Mutex;

    use chrono::{DateTime, NaiveDate, Utc};
    use domain::{MilestoneId, Task, TaskId, TaskStatus};

    use super::{TaskListFilter, TaskRepository};
    use crate::pagination::{Page, PageRequest};
    use crate::ports::RepositoryError;

    /// The task-list filters and default order (roadmap 3.10, 3.16) applied
    /// to an in-memory set of tasks: the same rules as the Postgres
    /// implementation — ANDed filters, a case-insensitive literal substring
    /// for `q`, inclusive date bounds that exclude undated tasks — then the
    /// default order. Returns every match; page it with [`page_tasks`].
    pub fn apply_task_list_filter<'a, I>(tasks: I, filter: &TaskListFilter) -> Vec<Task>
    where
        I: IntoIterator<Item = &'a Task>,
    {
        let matched = tasks
            .into_iter()
            .filter(|task| {
                (filter.statuses.is_empty() || filter.statuses.contains(&task.status))
                    && match filter.milestone_id {
                        Some(milestone_id) => task.milestone_id == Some(milestone_id),
                        None => true,
                    }
                    && match &filter.q {
                        Some(needle) => task.title.to_lowercase().contains(&needle.to_lowercase()),
                        None => true,
                    }
                    && match filter.target_after {
                        Some(after) => task.target_date.is_some_and(|date| date >= after),
                        None => true,
                    }
                    && match filter.target_before {
                        Some(before) => task.target_date.is_some_and(|date| date <= before),
                        None => true,
                    }
            })
            .cloned()
            .collect();
        default_task_order(matched)
    }

    /// The default list order (roadmap 3.16): target date ascending with
    /// nulls last, then created_at, then id.
    pub fn default_task_order(mut tasks: Vec<Task>) -> Vec<Task> {
        tasks.sort_by(|a, b| {
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
        tasks
    }

    /// Slice an already-filtered, default-ordered match list into one page.
    pub fn page_tasks(matched: Vec<Task>, page: &PageRequest) -> Page<Task> {
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

    /// In-memory [`TaskRepository`] mirroring the Postgres implementation,
    /// for tests that need a task store without a database.
    #[derive(Default)]
    pub struct InMemoryTaskRepository {
        tasks: Mutex<Vec<Task>>,
    }

    impl InMemoryTaskRepository {
        pub fn new() -> Self {
            Self::default()
        }
    }

    #[async_trait::async_trait]
    impl TaskRepository for InMemoryTaskRepository {
        async fn create(&self, task: Task) -> Result<Task, RepositoryError> {
            self.tasks.lock().unwrap().push(task.clone());
            Ok(task)
        }

        async fn find_by_id(&self, id: TaskId) -> Result<Option<Task>, RepositoryError> {
            Ok(self
                .tasks
                .lock()
                .unwrap()
                .iter()
                .find(|task| task.id == id)
                .cloned())
        }

        async fn find_by_ids(&self, ids: &[TaskId]) -> Result<Vec<Task>, RepositoryError> {
            let tasks = self.tasks.lock().unwrap();
            let mut found: Vec<Task> = ids
                .iter()
                .filter_map(|id| tasks.iter().find(|task| task.id == *id).cloned())
                .collect();
            found.sort_by_key(|task| task.id.0);
            Ok(found)
        }

        async fn update(&self, task: Task) -> Result<Task, RepositoryError> {
            let mut tasks = self.tasks.lock().unwrap();
            match tasks.iter_mut().find(|existing| existing.id == task.id) {
                Some(existing) => {
                    *existing = task.clone();
                    Ok(task)
                }
                None => Err(RepositoryError::NotFound),
            }
        }

        async fn set_status(
            &self,
            id: TaskId,
            status: TaskStatus,
        ) -> Result<Task, RepositoryError> {
            let mut tasks = self.tasks.lock().unwrap();
            let Some(task) = tasks.iter_mut().find(|existing| existing.id == id) else {
                return Err(RepositoryError::NotFound);
            };
            // Mirror Postgres: the write touches only the column and the
            // updated_at timestamp.
            task.status = status;
            task.updated_at = Utc::now();
            Ok(task.clone())
        }

        async fn delete(&self, id: TaskId) -> Result<(), RepositoryError> {
            let mut tasks = self.tasks.lock().unwrap();
            let before = tasks.len();
            tasks.retain(|task| task.id != id);
            if tasks.len() == before {
                return Err(RepositoryError::NotFound);
            }
            Ok(())
        }

        async fn list_page(
            &self,
            filter: &TaskListFilter,
            page: &PageRequest,
        ) -> Result<Page<Task>, RepositoryError> {
            let matched = apply_task_list_filter(self.tasks.lock().unwrap().iter(), filter);
            Ok(page_tasks(matched, page))
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

        /// A task with a deterministic id and the fields the filters read.
        fn task(
            id: u128,
            title: &str,
            status: TaskStatus,
            milestone_id: Option<MilestoneId>,
            target_date: Option<NaiveDate>,
            created_at: DateTime<Utc>,
        ) -> Task {
            Task {
                id: TaskId(Uuid::from_u128(id)),
                milestone_id,
                title: title.to_owned(),
                description: None,
                status,
                target_date,
                created_at,
                updated_at: created_at,
            }
        }

        fn ids(page: &Page<Task>) -> Vec<u128> {
            page.items.iter().map(|task| task.id.0.as_u128()).collect()
        }

        /// The fixture every filter test starts from: two milestones, four
        /// statuses, dates on both sides of the 5th (and one undated task).
        fn fixtures() -> (MilestoneId, Vec<Task>) {
            let milestone_a = MilestoneId::new();
            let milestone_b = MilestoneId::new();
            let tasks = vec![
                task(
                    1,
                    "Alpha",
                    TaskStatus::Backlog,
                    Some(milestone_a),
                    Some(date(1, 5)),
                    at(1),
                ),
                task(
                    2,
                    "Beta",
                    TaskStatus::ToDo,
                    Some(milestone_a),
                    Some(date(1, 10)),
                    at(2),
                ),
                task(
                    3,
                    "Gamma",
                    TaskStatus::InProgress,
                    Some(milestone_b),
                    Some(date(1, 1)),
                    at(3),
                ),
                task(4, "Delta", TaskStatus::Done, Some(milestone_b), None, at(4)),
                task(
                    5,
                    "Epsilon",
                    TaskStatus::Backlog,
                    None,
                    Some(date(1, 20)),
                    at(5),
                ),
            ];
            (milestone_a, tasks)
        }

        async fn list(tasks: &[Task], filter: &TaskListFilter) -> Page<Task> {
            let repo = InMemoryTaskRepository::new();
            for task in tasks {
                repo.create(task.clone()).await.unwrap();
            }
            repo.list_page(filter, &PageRequest::new(None, None).unwrap())
                .await
                .unwrap()
        }

        #[tokio::test]
        async fn the_status_filter_matches_only_its_columns() {
            let (_milestone, tasks) = fixtures();
            let page = list(
                &tasks,
                &TaskListFilter {
                    statuses: vec![TaskStatus::Backlog],
                    ..Default::default()
                },
            )
            .await;
            assert_eq!(ids(&page), vec![1, 5]);
        }

        #[tokio::test]
        async fn the_status_filter_accepts_several_columns() {
            let (_milestone, tasks) = fixtures();
            let page = list(
                &tasks,
                &TaskListFilter {
                    statuses: vec![TaskStatus::ToDo, TaskStatus::Done],
                    ..Default::default()
                },
            )
            .await;
            assert_eq!(ids(&page), vec![2, 4]);
        }

        #[tokio::test]
        async fn the_milestone_filter_matches_only_that_milestone() {
            let (milestone_a, tasks) = fixtures();
            let page = list(
                &tasks,
                &TaskListFilter {
                    milestone_id: Some(milestone_a),
                    ..Default::default()
                },
            )
            .await;
            assert_eq!(ids(&page), vec![1, 2]);
        }

        #[tokio::test]
        async fn an_unknown_milestone_matches_nothing() {
            let (_milestone, tasks) = fixtures();
            let page = list(
                &tasks,
                &TaskListFilter {
                    milestone_id: Some(MilestoneId::new()),
                    ..Default::default()
                },
            )
            .await;
            assert!(page.items.is_empty());
            assert_eq!(page.total, 0);
        }

        #[tokio::test]
        async fn the_title_search_is_case_insensitive_and_literal() {
            let (_milestone, tasks) = fixtures();
            // Case-insensitive substring...
            let page = list(
                &tasks,
                &TaskListFilter {
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
                &tasks,
                &TaskListFilter {
                    q: Some("a_b".to_owned()),
                    ..Default::default()
                },
            )
            .await;
            assert!(page.items.is_empty());

            let page = list(
                &tasks,
                &TaskListFilter {
                    q: Some("amm".to_owned()),
                    ..Default::default()
                },
            )
            .await;
            assert_eq!(ids(&page), vec![3]);
        }

        #[tokio::test]
        async fn wildcard_characters_in_the_search_are_matched_literally() {
            let (_milestone, _tasks) = fixtures();
            let tasks = vec![
                task(1, "100% ready", TaskStatus::Backlog, None, None, at(1)),
                task(
                    2,
                    "50 percent ready",
                    TaskStatus::Backlog,
                    None,
                    None,
                    at(2),
                ),
                task(3, "a_b done", TaskStatus::Backlog, None, None, at(3)),
                task(4, "axb done", TaskStatus::Backlog, None, None, at(4)),
            ];
            let repo = InMemoryTaskRepository::new();
            for task in &tasks {
                repo.create(task.clone()).await.unwrap();
            }

            let page = repo
                .list_page(
                    &TaskListFilter {
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
                    &TaskListFilter {
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
        async fn the_date_bounds_are_inclusive_and_exclude_undated_tasks() {
            let (_milestone, tasks) = fixtures();
            // Dates 1, 5, 10, 20 exist; task 4 has none.
            let page = list(
                &tasks,
                &TaskListFilter {
                    target_after: Some(date(1, 5)),
                    ..Default::default()
                },
            )
            .await;
            assert_eq!(ids(&page), vec![1, 2, 5]);

            let page = list(
                &tasks,
                &TaskListFilter {
                    target_before: Some(date(1, 5)),
                    ..Default::default()
                },
            )
            .await;
            assert_eq!(ids(&page), vec![3, 1]);

            let page = list(
                &tasks,
                &TaskListFilter {
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
            let (milestone_a, tasks) = fixtures();
            let page = list(
                &tasks,
                &TaskListFilter {
                    statuses: vec![TaskStatus::Backlog, TaskStatus::ToDo],
                    milestone_id: Some(milestone_a),
                    target_after: Some(date(1, 5)),
                    ..Default::default()
                },
            )
            .await;
            assert_eq!(ids(&page), vec![1, 2]);

            // ...and a search that matches nothing empties the page.
            let page = list(
                &tasks,
                &TaskListFilter {
                    statuses: vec![TaskStatus::Backlog],
                    milestone_id: Some(milestone_a),
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
            let tasks = vec![
                task(1, "a", TaskStatus::Backlog, None, Some(date(1, 5)), at(3)),
                task(2, "b", TaskStatus::Backlog, None, Some(date(1, 5)), at(3)),
                task(3, "c", TaskStatus::Backlog, None, Some(date(1, 10)), at(1)),
                task(4, "d", TaskStatus::Backlog, None, None, at(1)),
                task(5, "e", TaskStatus::Backlog, None, None, at(9)),
            ];
            let page = list(&tasks, &TaskListFilter::default()).await;
            assert_eq!(ids(&page), vec![1, 2, 3, 4, 5]);
        }

        #[tokio::test]
        async fn paging_walks_every_match_exactly_once() {
            let tasks: Vec<Task> = (1..=7)
                .map(|id| {
                    task(
                        id,
                        &format!("t{id}"),
                        TaskStatus::Backlog,
                        None,
                        Some(date(1, 5)),
                        at(1),
                    )
                })
                .collect();
            let repo = InMemoryTaskRepository::new();
            for task in &tasks {
                repo.create(task.clone()).await.unwrap();
            }

            let mut seen = Vec::new();
            for offset in [0i64, 3, 6] {
                let page = repo
                    .list_page(
                        &TaskListFilter::default(),
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
                    &TaskListFilter::default(),
                    &PageRequest::new(None, Some(100)).unwrap(),
                )
                .await
                .unwrap();
            assert!(page.items.is_empty());
            assert_eq!(page.total, 7);
        }
    }
}
