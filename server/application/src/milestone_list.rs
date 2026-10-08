//! Listing milestones with pagination and filters (roadmap 3.10, 3.16).
//!
//! The handler parses and validates the query parameters; this service is the
//! one place that asks the repository for a page, so handlers never touch the
//! milestone store directly and the list's rules (filters, default order,
//! total) live in the application layer.

use std::sync::Arc;

use domain::Milestone;

use crate::pagination::{Page, PageRequest};
use crate::ports::{MilestoneListFilter, MilestoneRepository, RepositoryError};

/// Lists milestones with filters and pagination (roadmap 3.10).
pub struct MilestoneListService {
    milestones: Arc<dyn MilestoneRepository>,
}

impl MilestoneListService {
    pub fn new(milestones: Arc<dyn MilestoneRepository>) -> Self {
        Self { milestones }
    }

    /// One page of the milestones matching `filter`, in the default order
    /// (target date ascending with nulls last, then created_at, then id);
    /// [`Page::total`] counts every match, ignoring the page position.
    pub async fn list_page(
        &self,
        filter: &MilestoneListFilter,
        page: &PageRequest,
    ) -> Result<Page<Milestone>, RepositoryError> {
        self.milestones.list_page(filter, page).await
    }
}
