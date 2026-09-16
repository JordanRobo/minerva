//! Conversions between domain types and their Postgres column values.
//!
//! Goals and milestones store their status as two separate TEXT columns
//! (`status` and `status_source`); the domain combines them into a single
//! [`GoalStatus`]. These functions are the one place that translation
//! happens, so repository code never matches on raw strings itself.

use chrono::{DateTime, NaiveDate, Utc};
use uuid::Uuid;

use application::ports::RepositoryError;
use domain::{Goal, GoalId, GoalStatus, Milestone, MilestoneId, Status, StatusSource};

/// A row of the `goals` table: id, title, description, status,
/// status_source, target_date, created_at, updated_at — in that order.
pub type GoalRow = (
    Uuid,
    String,
    Option<String>,
    String,
    String,
    Option<NaiveDate>,
    DateTime<Utc>,
    DateTime<Utc>,
);

/// A row of the `milestones` table, which has the same columns as `goals`.
pub type MilestoneRow = GoalRow;

/// The value stored in a `status` column for [`Status`].
pub fn status_to_db(status: Status) -> &'static str {
    match status {
        Status::OnTrack => "on_track",
        Status::AtRisk => "at_risk",
        Status::OffTrack => "off_track",
        Status::Complete => "complete",
    }
}

/// The value stored in a `status_source` column for [`StatusSource`].
pub fn status_source_to_db(source: StatusSource) -> &'static str {
    match source {
        StatusSource::Computed => "computed",
        StatusSource::ManualOverride => "manual_override",
    }
}

/// Rebuild a [`GoalStatus`] from the two TEXT columns.
///
/// A value that matches no known variant is a data-integrity problem — the
/// CHECK constraints should make it impossible — so it is reported as
/// [`RepositoryError::Unexpected`] rather than guessed at.
pub fn goal_status_from_db(status: &str, source: &str) -> Result<GoalStatus, RepositoryError> {
    let status = match status {
        "on_track" => Status::OnTrack,
        "at_risk" => Status::AtRisk,
        "off_track" => Status::OffTrack,
        "complete" => Status::Complete,
        other => {
            return Err(RepositoryError::Unexpected(format!(
                "unknown status value {other:?} in database"
            )))
        }
    };
    let source = match source {
        "computed" => StatusSource::Computed,
        "manual_override" => StatusSource::ManualOverride,
        other => {
            return Err(RepositoryError::Unexpected(format!(
                "unknown status_source value {other:?} in database"
            )))
        }
    };
    Ok(GoalStatus { status, source })
}

/// Build a [`Goal`] from a `goals` row.
pub fn goal_from_row(row: GoalRow) -> Result<Goal, RepositoryError> {
    let (id, title, description, status, status_source, target_date, created_at, updated_at) = row;
    Ok(Goal {
        id: GoalId(id),
        title,
        description,
        status: goal_status_from_db(&status, &status_source)?,
        target_date,
        created_at,
        updated_at,
    })
}

/// Build a [`Milestone`] from a `milestones` row.
pub fn milestone_from_row(row: MilestoneRow) -> Result<Milestone, RepositoryError> {
    let (id, title, description, status, status_source, target_date, created_at, updated_at) = row;
    Ok(Milestone {
        id: MilestoneId(id),
        title,
        description,
        status: goal_status_from_db(&status, &status_source)?,
        target_date,
        created_at,
        updated_at,
    })
}
