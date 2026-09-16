//! Conversions between domain types and their Postgres column values.
//!
//! Goals and milestones store their status as two separate TEXT columns
//! (`status` and `status_source`); the domain combines them into a single
//! [`GoalStatus`]. Tasks, task relations, and progress snapshots store
//! their enums as plain TEXT columns. These functions are the one place
//! that translation happens, so repository code never matches on raw
//! strings itself.

use chrono::{DateTime, NaiveDate, Utc};
use uuid::Uuid;

use application::ports::RepositoryError;
use domain::{
    Goal, GoalId, GoalStatus, Milestone, MilestoneId, ProgressSnapshot, ProgressSnapshotId,
    ProgressTarget, Status, StatusSource, Task, TaskId, TaskRelation, TaskRelationId,
    TaskRelationType, TaskStatus,
};

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

/// The [`Status`] stored in a `status` column.
///
/// A value that matches no known variant is a data-integrity problem — the
/// CHECK constraints should make it impossible — so it is reported as
/// [`RepositoryError::Unexpected`] rather than guessed at.
pub fn status_from_db(status: &str) -> Result<Status, RepositoryError> {
    match status {
        "on_track" => Ok(Status::OnTrack),
        "at_risk" => Ok(Status::AtRisk),
        "off_track" => Ok(Status::OffTrack),
        "complete" => Ok(Status::Complete),
        other => Err(RepositoryError::Unexpected(format!(
            "unknown status value {other:?} in database"
        ))),
    }
}

/// Rebuild a [`GoalStatus`] from the two TEXT columns.
pub fn goal_status_from_db(status: &str, source: &str) -> Result<GoalStatus, RepositoryError> {
    let source = match source {
        "computed" => StatusSource::Computed,
        "manual_override" => StatusSource::ManualOverride,
        other => {
            return Err(RepositoryError::Unexpected(format!(
                "unknown status_source value {other:?} in database"
            )))
        }
    };
    Ok(GoalStatus {
        status: status_from_db(status)?,
        source,
    })
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

/// A row of the `tasks` table: id, milestone_id, title, description,
/// status, target_date, created_at, updated_at — in that order.
pub type TaskRow = (
    Uuid,
    Option<Uuid>,
    String,
    Option<String>,
    String,
    Option<NaiveDate>,
    DateTime<Utc>,
    DateTime<Utc>,
);

/// The value stored in a `tasks.status` column for [`TaskStatus`].
pub fn task_status_to_db(status: TaskStatus) -> &'static str {
    match status {
        TaskStatus::Backlog => "backlog",
        TaskStatus::ToDo => "to_do",
        TaskStatus::InProgress => "in_progress",
        TaskStatus::Done => "done",
    }
}

/// The [`TaskStatus`] stored in a `tasks.status` column.
pub fn task_status_from_db(status: &str) -> Result<TaskStatus, RepositoryError> {
    match status {
        "backlog" => Ok(TaskStatus::Backlog),
        "to_do" => Ok(TaskStatus::ToDo),
        "in_progress" => Ok(TaskStatus::InProgress),
        "done" => Ok(TaskStatus::Done),
        other => Err(RepositoryError::Unexpected(format!(
            "unknown task status value {other:?} in database"
        ))),
    }
}

/// Build a [`Task`] from a `tasks` row.
pub fn task_from_row(row: TaskRow) -> Result<Task, RepositoryError> {
    let (id, milestone_id, title, description, status, target_date, created_at, updated_at) = row;
    Ok(Task {
        id: TaskId(id),
        milestone_id: milestone_id.map(MilestoneId),
        title,
        description,
        status: task_status_from_db(&status)?,
        target_date,
        created_at,
        updated_at,
    })
}

/// A row of the `task_relations` table: id, source_task_id,
/// target_task_id, relation_type, created_at — in that order.
pub type TaskRelationRow = (Uuid, Uuid, Uuid, String, DateTime<Utc>);

/// The value stored in a `relation_type` column for [`TaskRelationType`].
///
/// [`TaskRelationType`] is `#[non_exhaustive]`, so the wildcard arm is
/// required: a variant added to `domain` later has no database
/// representation until one is chosen here, and storing an invented string
/// would corrupt data, so it is reported instead.
pub fn relation_type_to_db(relation_type: TaskRelationType) -> Result<&'static str, RepositoryError> {
    match relation_type {
        TaskRelationType::Blocks => Ok("blocks"),
        TaskRelationType::BlockedBy => Ok("blocked_by"),
        TaskRelationType::RelatesTo => Ok("relates_to"),
        _ => Err(RepositoryError::Unexpected(format!(
            "no database representation for relation type {relation_type:?}"
        ))),
    }
}

/// The [`TaskRelationType`] stored in a `relation_type` column.
pub fn relation_type_from_db(relation_type: &str) -> Result<TaskRelationType, RepositoryError> {
    match relation_type {
        "blocks" => Ok(TaskRelationType::Blocks),
        "blocked_by" => Ok(TaskRelationType::BlockedBy),
        "relates_to" => Ok(TaskRelationType::RelatesTo),
        other => Err(RepositoryError::Unexpected(format!(
            "unknown relation_type value {other:?} in database"
        ))),
    }
}

/// Build a [`TaskRelation`] from a `task_relations` row.
pub fn task_relation_from_row(row: TaskRelationRow) -> Result<TaskRelation, RepositoryError> {
    let (id, source_task_id, target_task_id, relation_type, created_at) = row;
    Ok(TaskRelation {
        id: TaskRelationId(id),
        source_task_id: TaskId(source_task_id),
        target_task_id: TaskId(target_task_id),
        relation_type: relation_type_from_db(&relation_type)?,
        created_at,
    })
}

/// A row of the `progress_snapshots` table: id, goal_id, milestone_id,
/// recorded_at, status, percent_complete, note — in that order.
pub type ProgressSnapshotRow = (
    Uuid,
    Option<Uuid>,
    Option<Uuid>,
    DateTime<Utc>,
    String,
    i16,
    Option<String>,
);

/// Build a [`ProgressSnapshot`] from a `progress_snapshots` row.
///
/// The row is reconstructed through [`ProgressSnapshot::new`] so the
/// 0..=100 `percent_complete` invariant is enforced in Rust as well as by
/// the database CHECK constraint.
pub fn progress_snapshot_from_row(row: ProgressSnapshotRow) -> Result<ProgressSnapshot, RepositoryError> {
    let (id, goal_id, milestone_id, recorded_at, status, percent_complete, note) = row;
    // The CHECK constraint says exactly one of the two is set; a row that
    // violates it is a data-integrity problem, not something to guess at.
    let target = match (goal_id, milestone_id) {
        (Some(goal_id), None) => ProgressTarget::Goal(GoalId(goal_id)),
        (None, Some(milestone_id)) => ProgressTarget::Milestone(MilestoneId(milestone_id)),
        _ => {
            return Err(RepositoryError::Unexpected(
                "progress snapshot must reference exactly one of a goal or a milestone".to_owned(),
            ))
        }
    };
    let percent_complete = u8::try_from(percent_complete).map_err(|_| {
        RepositoryError::Unexpected(format!("percent_complete out of range: {percent_complete}"))
    })?;
    // `new` mints a fresh id; restore the row's own.
    let mut snapshot = ProgressSnapshot::new(
        target,
        recorded_at,
        status_from_db(&status)?,
        percent_complete,
        note,
    )
    .map_err(|err| RepositoryError::Unexpected(err.to_string()))?;
    snapshot.id = ProgressSnapshotId(id);
    Ok(snapshot)
}
