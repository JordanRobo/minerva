//! Listing tasks with pagination and filters (roadmap 3.10, 3.16).
//!
//! The handler parses and validates the query parameters; this service is
//! the one place that asks the repository for a page, so handlers never
//! touch the task store directly and the list's rules (filters, default
//! order, total) live in the application layer.

use std::sync::Arc;

use domain::Task;

use crate::pagination::{Page, PageRequest};
use crate::ports::{RepositoryError, TaskListFilter, TaskRepository};

/// Lists tasks with filters and pagination (roadmap 3.10).
pub struct TaskListService {
    tasks: Arc<dyn TaskRepository>,
}

impl TaskListService {
    pub fn new(tasks: Arc<dyn TaskRepository>) -> Self {
        Self { tasks }
    }

    /// One page of the tasks matching `filter`, in the default order (target
    /// date ascending with nulls last, then created_at, then id);
    /// [`Page::total`] counts every match, ignoring the page position.
    pub async fn list_page(
        &self,
        filter: &TaskListFilter,
        page: &PageRequest,
    ) -> Result<Page<Task>, RepositoryError> {
        self.tasks.list_page(filter, page).await
    }
}
