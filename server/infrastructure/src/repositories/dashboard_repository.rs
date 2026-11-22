//! Postgres implementation of [`DashboardRepository`] (roadmap 3.15, D22).

use application::ports::{
    DashboardEntityId, DashboardKind, DashboardRepository, ManuallySetItem, RepositoryError,
    StatusRollup, TimelineData, TimelineFilter, TimelineGoal, TimelineMilestone, TrendRow,
};
use chrono::{DateTime, NaiveDate, Utc};
use diesel::prelude::*;
use domain::{GoalId, MilestoneId, Status, UserId};
use std::collections::BTreeMap;
use uuid::Uuid;

use crate::db::{PgPool, run_on_postgres};
use crate::error::map_diesel_error;
use crate::repositories::mapping::status_from_db;
use crate::schema::{goal_milestones, goals, milestones};

/// [`DashboardRepository`] backed by Postgres through Diesel.
pub struct PostgresDashboardRepository {
    pool: PgPool,
}

impl PostgresDashboardRepository {
    /// Wrap a connection pool in a dashboard repository.
    pub fn new(pool: PgPool) -> Self {
        Self { pool }
    }
}

#[async_trait::async_trait]
impl DashboardRepository for PostgresDashboardRepository {
    async fn status_rollup(&self, kind: DashboardKind) -> Result<StatusRollup, RepositoryError> {
        let pool = self.pool.clone();
        run_on_postgres(pool, move |conn| match kind {
            // One read of every row's two status columns; the counting is in
            // Rust, like the task counts (roadmap 3.13) — Diesel cannot
            // express the effective-status conditional aggregate without raw
            // SQL, and a school's tables are small.
            DashboardKind::Goal => {
                let rows: Vec<(Option<String>, String)> = goals::table
                    .select((goals::status_override, goals::status))
                    .load(conn)
                    .map_err(map_diesel_error)?;
                rollup_from_rows(&rows)
            }
            DashboardKind::Milestone => {
                let rows: Vec<(Option<String>, String)> = milestones::table
                    .select((milestones::status_override, milestones::status))
                    .load(conn)
                    .map_err(map_diesel_error)?;
                rollup_from_rows(&rows)
            }
        })
        .await
    }

    async fn manually_set_items(&self) -> Result<Vec<ManuallySetItem>, RepositoryError> {
        let pool = self.pool.clone();
        run_on_postgres(pool, |conn| {
            // Goals before milestones (D22), each in the shared default order
            // (roadmap 3.16); only rows with an override are selected.
            type Row = (Uuid, String, String, Option<String>, Option<NaiveDate>);
            let goal_rows: Vec<Row> = goals::table
                .filter(goals::status_override.is_not_null())
                .select((
                    goals::id,
                    goals::title,
                    goals::status,
                    goals::status_override,
                    goals::target_date,
                ))
                .order((
                    goals::target_date.asc().nulls_last(),
                    goals::created_at.asc(),
                    goals::id.asc(),
                ))
                .load(conn)
                .map_err(map_diesel_error)?;
            let milestone_rows: Vec<Row> = milestones::table
                .filter(milestones::status_override.is_not_null())
                .select((
                    milestones::id,
                    milestones::title,
                    milestones::status,
                    milestones::status_override,
                    milestones::target_date,
                ))
                .order((
                    milestones::target_date.asc().nulls_last(),
                    milestones::created_at.asc(),
                    milestones::id.asc(),
                ))
                .load(conn)
                .map_err(map_diesel_error)?;
            let mut items = Vec::with_capacity(goal_rows.len() + milestone_rows.len());
            for row in goal_rows {
                items.push(item_from_row(DashboardKind::Goal, row)?);
            }
            for row in milestone_rows {
                items.push(item_from_row(DashboardKind::Milestone, row)?);
            }
            Ok(items)
        })
        .await
    }

    async fn status_trend(
        &self,
        kind: DashboardKind,
        from: NaiveDate,
        to: NaiveDate,
    ) -> Result<Vec<TrendRow>, RepositoryError> {
        let pool = self.pool.clone();
        run_on_postgres(pool, move |conn| {
            // DISTINCT ON picks each entity's latest snapshot of the day
            // (taken_at, then id); Diesel cannot express it, so raw SQL. The
            // window filter rides the (snapshot_date) index; grouping and the
            // per-status counting happen in Rust, like the other dashboard reads.
            let sql = match kind {
                DashboardKind::Goal => {
                    "SELECT DISTINCT ON (goal_id, snapshot_date) \
                    snapshot_date, status, status_source, percent_complete \
                    FROM progress_snapshots \
                    WHERE goal_id IS NOT NULL AND snapshot_date BETWEEN $1 AND $2 \
                    ORDER BY goal_id, snapshot_date, taken_at DESC, id DESC"
                }
                DashboardKind::Milestone => {
                    "SELECT DISTINCT ON (milestone_id, snapshot_date) \
                    snapshot_date, status, status_source, percent_complete \
                    FROM progress_snapshots \
                    WHERE milestone_id IS NOT NULL AND snapshot_date BETWEEN $1 AND $2 \
                    ORDER BY milestone_id, snapshot_date, taken_at DESC, id DESC"
                }
            };
            let rows: Vec<TrendSnapshotRow> = diesel::sql_query(sql)
                .bind::<diesel::sql_types::Date, _>(from)
                .bind::<diesel::sql_types::Date, _>(to)
                .load(conn)
                .map_err(map_diesel_error)?;
            trend_rows_from(rows)
        })
        .await
    }

    async fn timeline(&self, filter: TimelineFilter) -> Result<TimelineData, RepositoryError> {
        let pool = self.pool.clone();
        run_on_postgres(pool, move |conn| {
            // The date bounds keep the list endpoints' semantics (roadmap
            // 3.10): inclusive, and a bound never matches an undated row —
            // NULL fails both comparisons. With no bound, undated rows are
            // included and the shared default order puts them last.
            type Row = (
                Uuid,
                String,
                String,
                Option<String>,
                Option<NaiveDate>,
                Option<Uuid>,
                DateTime<Utc>,
            );
            let mut goals_query = goals::table.into_boxed();
            if let Some(after) = filter.target_after {
                goals_query = goals_query.filter(goals::target_date.ge(after));
            }
            if let Some(before) = filter.target_before {
                goals_query = goals_query.filter(goals::target_date.le(before));
            }
            let goal_rows: Vec<Row> = goals_query
                .select((
                    goals::id,
                    goals::title,
                    goals::status,
                    goals::status_override,
                    goals::target_date,
                    goals::owner_id,
                    goals::created_at,
                ))
                .order((
                    goals::target_date.asc().nulls_last(),
                    goals::created_at.asc(),
                    goals::id.asc(),
                ))
                .load(conn)
                .map_err(map_diesel_error)?;

            let mut milestones_query = milestones::table.into_boxed();
            if let Some(after) = filter.target_after {
                milestones_query = milestones_query.filter(milestones::target_date.ge(after));
            }
            if let Some(before) = filter.target_before {
                milestones_query = milestones_query.filter(milestones::target_date.le(before));
            }
            let milestone_rows: Vec<Row> = milestones_query
                .select((
                    milestones::id,
                    milestones::title,
                    milestones::status,
                    milestones::status_override,
                    milestones::target_date,
                    milestones::owner_id,
                    milestones::created_at,
                ))
                .order((
                    milestones::target_date.asc().nulls_last(),
                    milestones::created_at.asc(),
                    milestones::id.asc(),
                ))
                .load(conn)
                .map_err(map_diesel_error)?;

            // Every link of the returned goals (D22): grouped per goal, each
            // group in the linked milestone's shared default order, so a
            // goal's milestone_ids are stable. Links to milestones outside
            // the window are included on purpose; no query at all when no
            // goal matched.
            let goal_ids: Vec<Uuid> = goal_rows.iter().map(|row| row.0).collect();
            let links: Vec<(Uuid, Uuid)> = if goal_ids.is_empty() {
                Vec::new()
            } else {
                goal_milestones::table
                    .inner_join(milestones::table)
                    .filter(goal_milestones::goal_id.eq_any(goal_ids))
                    .select((goal_milestones::goal_id, goal_milestones::milestone_id))
                    .order((
                        goal_milestones::goal_id.asc(),
                        milestones::target_date.asc().nulls_last(),
                        milestones::created_at.asc(),
                        milestones::id.asc(),
                    ))
                    .load(conn)
                    .map_err(map_diesel_error)?
            };

            Ok(TimelineData {
                goals: goal_rows
                    .into_iter()
                    .map(timeline_goal_from_row)
                    .collect::<Result<Vec<_>, _>>()?,
                milestones: milestone_rows
                    .into_iter()
                    .map(timeline_milestone_from_row)
                    .collect::<Result<Vec<_>, _>>()?,
                links: links
                    .into_iter()
                    .map(|(goal_id, milestone_id)| (GoalId(goal_id), MilestoneId(milestone_id)))
                    .collect(),
            })
        })
        .await
    }
}

/// One timeline row, as both the goal and milestone queries select it.
type TimelineRow = (
    Uuid,
    String,
    String,
    Option<String>,
    Option<NaiveDate>,
    Option<Uuid>,
    DateTime<Utc>,
);

fn timeline_goal_from_row(
    (id, title, automatic, override_status, target_date, owner_id, created_at): TimelineRow,
) -> Result<TimelineGoal, RepositoryError> {
    Ok(TimelineGoal {
        id: GoalId(id),
        title,
        automatic_status: status_from_db(&automatic)?,
        status_override: override_status.as_deref().map(status_from_db).transpose()?,
        target_date,
        owner_id: owner_id.map(UserId),
        created_at,
    })
}

fn timeline_milestone_from_row(
    (id, title, automatic, override_status, target_date, owner_id, created_at): TimelineRow,
) -> Result<TimelineMilestone, RepositoryError> {
    Ok(TimelineMilestone {
        id: MilestoneId(id),
        title,
        automatic_status: status_from_db(&automatic)?,
        status_override: override_status.as_deref().map(status_from_db).transpose()?,
        target_date,
        owner_id: owner_id.map(UserId),
        created_at,
    })
}

/// One latest-per-entity-per-day snapshot row, as the raw trend query selects
/// it. `status_source` stays a string: only `manual_override` matters here.
#[derive(QueryableByName)]
struct TrendSnapshotRow {
    #[diesel(sql_type = diesel::sql_types::Date)]
    snapshot_date: NaiveDate,
    #[diesel(sql_type = diesel::sql_types::Text)]
    status: String,
    #[diesel(sql_type = diesel::sql_types::Text)]
    status_source: String,
    #[diesel(sql_type = diesel::sql_types::Nullable<diesel::sql_types::SmallInt>)]
    percent_complete: Option<i16>,
}

/// Group the latest-per-entity-per-day rows into one [`TrendRow`] per date,
/// counting each item under its status and flagging the manually set ones. A
/// `BTreeMap` keeps the dates ascending for free; a date with no rows never
/// appears (no carry-forward).
fn trend_rows_from(rows: Vec<TrendSnapshotRow>) -> Result<Vec<TrendRow>, RepositoryError> {
    let mut by_date: BTreeMap<NaiveDate, TrendRow> = BTreeMap::new();
    for row in rows {
        let entry = by_date
            .entry(row.snapshot_date)
            .or_insert_with(|| TrendRow {
                date: row.snapshot_date,
                on_track: 0,
                at_risk: 0,
                off_track: 0,
                complete: 0,
                manual: 0,
                percent_sum: 0,
                percent_count: 0,
            });
        let manual = row.status_source == "manual_override";
        match status_from_db(&row.status)? {
            Status::OnTrack => entry.on_track += 1,
            Status::AtRisk => entry.at_risk += 1,
            Status::OffTrack => entry.off_track += 1,
            Status::Complete => entry.complete += 1,
        }
        if manual {
            entry.manual += 1;
        }
        if let Some(percent) = row.percent_complete {
            entry.percent_sum += i64::from(percent);
            entry.percent_count += 1;
        }
    }
    Ok(by_date.into_values().collect())
}

/// Count the rows of one table into a rollup: every row under its effective
/// status — the override when set, else the stored value — flagging the
/// manually set ones. All four statuses start at zero and stay present.
fn rollup_from_rows(rows: &[(Option<String>, String)]) -> Result<StatusRollup, RepositoryError> {
    let mut rollup = StatusRollup::default();
    for (override_status, automatic) in rows {
        let effective = match override_status.as_deref() {
            Some(value) => status_from_db(value)?,
            None => status_from_db(automatic)?,
        };
        rollup.record(effective, override_status.is_some());
    }
    Ok(rollup)
}

/// One manually-set row: the override is the effective status by definition.
fn item_from_row(
    kind: DashboardKind,
    (id, title, automatic, override_status, target_date): (
        Uuid,
        String,
        String,
        Option<String>,
        Option<NaiveDate>,
    ),
) -> Result<ManuallySetItem, RepositoryError> {
    // The query selected only rows with an override, so a `None` here means
    // the row changed shape under us — surface it, don't guess.
    let Some(override_status) = override_status else {
        return Err(RepositoryError::Unexpected(
            "manually-set row without an override".into(),
        ));
    };
    let entity = match kind {
        DashboardKind::Goal => DashboardEntityId::Goal(GoalId(id)),
        DashboardKind::Milestone => DashboardEntityId::Milestone(MilestoneId(id)),
    };
    Ok(ManuallySetItem {
        entity,
        title,
        status: status_from_db(&override_status)?,
        automatic_status: status_from_db(&automatic)?,
        target_date,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use domain::Status;

    fn row(automatic: &str, override_status: Option<&str>) -> (Option<String>, String) {
        (override_status.map(str::to_owned), automatic.to_owned())
    }

    #[test]
    fn no_rows_give_an_all_zero_rollup() {
        let rollup = rollup_from_rows(&[]).expect("rollup");
        assert_eq!(rollup, StatusRollup::default());
        assert_eq!(rollup.total(), 0);
        assert_eq!(rollup.manual(), 0);
    }

    #[test]
    fn rows_count_under_their_effective_status_with_manual_flagged() {
        // g2's override moves it from at_risk to complete; only the two
        // overridden rows are manual.
        let rollup = rollup_from_rows(&[
            row("on_track", None),
            row("at_risk", Some("complete")),
            row("off_track", Some("off_track")),
            row("complete", None),
        ])
        .expect("rollup");

        assert_eq!(rollup.on_track.count, 1);
        assert_eq!(rollup.at_risk.count, 0);
        assert_eq!(rollup.off_track.count, 1);
        assert_eq!(rollup.complete.count, 2);
        assert_eq!(rollup.on_track.manual, 0);
        assert_eq!(rollup.at_risk.manual, 0);
        assert_eq!(rollup.off_track.manual, 1);
        assert_eq!(rollup.complete.manual, 1);
        assert_eq!(rollup.total(), 4);
        assert_eq!(rollup.manual(), 2);
    }

    #[test]
    fn an_unknown_stored_status_is_reported_not_guessed() {
        let err = rollup_from_rows(&[row("sideways", None)]).unwrap_err();
        assert!(matches!(err, RepositoryError::Unexpected(_)));
        let err = rollup_from_rows(&[row("on_track", Some("sideways"))]).unwrap_err();
        assert!(matches!(err, RepositoryError::Unexpected(_)));
    }

    #[test]
    fn a_manually_set_row_without_an_override_is_reported() {
        let err = item_from_row(
            DashboardKind::Goal,
            (Uuid::new_v4(), "T".into(), "on_track".into(), None, None),
        )
        .unwrap_err();
        assert!(matches!(err, RepositoryError::Unexpected(_)));
    }

    #[test]
    fn a_manually_set_row_carries_both_statuses() {
        let target = NaiveDate::from_ymd_opt(2026, 3, 1).unwrap();
        let item = item_from_row(
            DashboardKind::Milestone,
            (
                Uuid::new_v4(),
                "T".into(),
                "on_track".into(),
                Some("off_track".to_owned()),
                Some(target),
            ),
        )
        .expect("item");
        assert!(matches!(item.entity, DashboardEntityId::Milestone(_)));
        assert_eq!(item.status, Status::OffTrack);
        assert_eq!(item.automatic_status, Status::OnTrack);
        assert_eq!(item.target_date, Some(target));
    }
}
