//! Listing goals with pagination and filters (roadmap 3.10, 3.16).
//!
//! The handler parses and validates the query parameters; this service is the
//! one place that asks the repository for a page, so handlers never touch the
//! goal store directly and the list's rules (filters, default order, total)
//! live in the application layer.

use std::sync::Arc;

use domain::Goal;

use crate::pagination::{Page, PageRequest};
use crate::ports::{GoalListFilter, GoalRepository, RepositoryError};

/// Lists goals with filters and pagination (roadmap 3.10).
pub struct GoalListService {
    goals: Arc<dyn GoalRepository>,
}

impl GoalListService {
    pub fn new(goals: Arc<dyn GoalRepository>) -> Self {
        Self { goals }
    }

    /// One page of the goals matching `filter`, in the default order (target
    /// date ascending with nulls last, then created_at, then id);
    /// [`Page::total`] counts every match, ignoring the page position.
    pub async fn list_page(
        &self,
        filter: &GoalListFilter,
        page: &PageRequest,
    ) -> Result<Page<Goal>, RepositoryError> {
        self.goals.list_page(filter, page).await
    }
}
