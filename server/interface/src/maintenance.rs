//! Periodic maintenance jobs (roadmap 2.8): background work that runs on
//! every node but is safe to do so because each tick first takes a Postgres
//! advisory lock and skips silently when another node already holds it, so
//! however many API nodes run, exactly one does the work per tick.

use application::ports::{RepositoryError, SessionRepository};
use application::progress_snapshots::{ProgressSnapshotError, ProgressSnapshotService};
use application::rate_limit::{RateLimiter, longest_policy_window};
use application::status_computation::{StatusComputationError, StatusComputationService};
use chrono::Utc;
use infrastructure::db::{PgPool, try_acquire_advisory_lock};
use std::future::Future;
use std::pin::Pin;
use std::sync::Arc;
use std::time::Duration;

/// How often maintenance jobs tick. A constant on purpose: nothing tunes
/// these per deployment yet (roadmap 2.8).
const MAINTENANCE_INTERVAL: Duration = Duration::from_secs(60 * 60);

/// The first tick runs shortly after startup, not an hour in, so a fresh
/// database does not wait a full interval for its first cleanup.
const FIRST_TICK_DELAY: Duration = Duration::from_secs(30);

/// Advisory lock key for the expired-session purge job. Arbitrary but stable
/// across nodes and releases; "MNVRPRGE" read as ASCII, like the migration
/// lock key (see `infrastructure::migrations`).
const PURGE_LOCK_KEY: i64 = 0x4D4E5652_50524745;

/// Advisory lock key for the rate-limit counter purge job; "MNVRRLIM" read as
/// ASCII, distinct from the session purge's key so the two jobs tick
/// independently.
const RATE_LIMIT_PURGE_LOCK_KEY: i64 = 0x4D4E5652_524C494D;

/// Advisory lock key for the status-recomputation job; "MNVRRCMP" read as
/// ASCII, distinct from the other keys so all four jobs tick independently.
const RECOMPUTE_LOCK_KEY: i64 = 0x4D4E5652_52434D50;

/// Advisory lock key for the daily-snapshot job; "MNVRSNAP" read as ASCII,
/// distinct from the other keys so all four jobs tick independently.
const SNAPSHOT_LOCK_KEY: i64 = 0x4D4E5652_534E4150;

/// The work a maintenance job does: how many items it processed, or the
/// storage error that stopped it.
type JobResult = Result<u64, RepositoryError>;

/// One unit of periodic maintenance: a name for its log lines, the advisory
/// lock key that serializes it across nodes, and the work itself. The job
/// returns how many items it processed so the runner stays silent when there
/// was nothing to do.
pub struct MaintenanceJob {
    pub name: &'static str,
    pub lock_key: i64,
    run: Box<dyn Fn() -> Pin<Box<dyn Future<Output = JobResult> + Send>> + Send + Sync>,
}

impl MaintenanceJob {
    pub fn new<Fut, F>(name: &'static str, lock_key: i64, run: F) -> Self
    where
        Fut: Future<Output = JobResult> + Send + 'static,
        F: Fn() -> Fut + Send + Sync + 'static,
    {
        Self {
            name,
            lock_key,
            run: Box::new(move || Box::pin(run())),
        }
    }
}

/// The expired-session purge job (roadmap 2.8): deletes session rows past
/// their expiry from the Postgres store. With Redis this is a no-op — native
/// TTLs evict keys — so the job is registered only when Postgres holds the
/// sessions.
pub fn purge_expired_sessions(sessions: Arc<dyn SessionRepository>) -> MaintenanceJob {
    MaintenanceJob::new("purge_expired_sessions", PURGE_LOCK_KEY, move || {
        let sessions = sessions.clone();
        async move { sessions.purge_expired(Utc::now()).await }
    })
}

/// The rate-limit counter purge job (roadmap 2.8): deletes Postgres rows for
/// windows that ended more than the longest policy window ago — older than
/// that, no policy can receive hits in them any more. With Redis this is a
/// no-op (native TTLs evict keys), so the job is registered only when
/// Postgres holds the counters.
pub fn purge_rate_limit_counters(limiter: Arc<dyn RateLimiter>) -> MaintenanceJob {
    MaintenanceJob::new(
        "purge_rate_limit_counters",
        RATE_LIMIT_PURGE_LOCK_KEY,
        move || {
            let limiter = limiter.clone();
            async move {
                let cutoff = Utc::now()
                    - chrono::Duration::seconds(longest_policy_window().as_secs() as i64);
                limiter
                    .purge_before(cutoff)
                    .await
                    .map_err(|err| RepositoryError::Unexpected(err.to_string()))
            }
        },
    )
}

/// The status-recomputation job (roadmap 3.13, step 5): automatic statuses
/// depend on today's date, so they go stale when a target date passes even
/// if nobody edits anything — this sweep refreshes every milestone and goal.
/// It needs only Postgres, so it is registered on every node regardless of
/// the session store (see `jobs_for`).
pub fn recompute_statuses(computation: Arc<StatusComputationService>) -> MaintenanceJob {
    MaintenanceJob::new("recompute_statuses", RECOMPUTE_LOCK_KEY, move || {
        let computation = computation.clone();
        async move {
            computation
                .recompute_all()
                .await
                .map(|counts| (counts.milestones + counts.goals) as u64)
                .map_err(|err| match err {
                    StatusComputationError::Repository(err) => err,
                })
        }
    })
}

/// The daily-snapshot job (roadmap 3.14): ensures every goal and milestone
/// has one `daily` snapshot for the current UTC date, recomputing statuses
/// first so a stale row is never recorded. It needs only Postgres, so it is
/// registered on every node regardless of the session store (see
/// `jobs_for`). Catch-up by design: an entity created mid-day, or a node
/// that was down since yesterday, gets its row on the next tick — no
/// midnight scheduler.
pub fn record_daily_snapshots(snapshots: Arc<ProgressSnapshotService>) -> MaintenanceJob {
    MaintenanceJob::new("record_daily_snapshots", SNAPSHOT_LOCK_KEY, move || {
        let snapshots = snapshots.clone();
        async move {
            snapshots
                .ensure_daily_snapshots()
                .await
                .map(|inserted| inserted as u64)
                .map_err(|err| match err {
                    ProgressSnapshotError::Repository(err) => err,
                })
        }
    })
}

/// The maintenance jobs for one node: the status recompute and the daily
/// snapshot sweep always run (they need only Postgres), while the session
/// and rate-limit purges run only when Postgres holds those stores — with
/// Redis, native TTLs evict them instead. Every node ticks every job it
/// registers; the advisory locks mean exactly one node does each job's work
/// per tick (roadmap 2.8).
pub fn jobs_for(
    computation: Arc<StatusComputationService>,
    snapshots: Arc<ProgressSnapshotService>,
    postgres_stores: Option<(Arc<dyn SessionRepository>, Arc<dyn RateLimiter>)>,
) -> Vec<MaintenanceJob> {
    let mut jobs = vec![
        recompute_statuses(computation),
        record_daily_snapshots(snapshots),
    ];
    if let Some((sessions, limiter)) = postgres_stores {
        jobs.push(purge_expired_sessions(sessions));
        jobs.push(purge_rate_limit_counters(limiter));
    }
    jobs
}

/// One maintenance tick: take the job's advisory lock (skipping silently
/// when another node holds it) and run the job while holding it. Errors are
/// logged, never propagated — a failing job must not take the server down,
/// and the next hourly tick retries.
pub async fn run_tick(pool: &PgPool, job: &MaintenanceJob) {
    match try_acquire_advisory_lock(pool, job.lock_key).await {
        Ok(None) => {} // another node is doing this work; skip silently
        Err(err) => eprintln!("warning: maintenance {}: {err}", job.name),
        Ok(Some(lock)) => {
            match (job.run)().await {
                Ok(0) => {} // nothing to report
                Ok(count) => println!("maintenance {}: {count}", job.name),
                Err(err) => eprintln!("warning: maintenance {}: {err}", job.name),
            }
            // Release the advisory lock before the next tick.
            drop(lock);
        }
    }
}

/// Start every maintenance job on the shared schedule. The tasks live until
/// the process exits; the server has no shutdown hook yet, and Ctrl-C ends
/// them with it.
pub fn start(pool: PgPool, jobs: Vec<MaintenanceJob>) {
    let count = jobs.len();
    for job in jobs {
        // The pool is an Arc-backed handle; each task takes its own clone.
        let pool = pool.clone();
        tokio::spawn(async move {
            let mut ticker = tokio::time::interval_at(
                tokio::time::Instant::now() + FIRST_TICK_DELAY,
                MAINTENANCE_INTERVAL,
            );
            loop {
                ticker.tick().await;
                run_tick(&pool, &job).await;
            }
        });
    }
    println!("started {count} maintenance job(s), hourly");
}

#[cfg(test)]
mod tests {
    use super::*;
    use application::ports::{
        Clock, GoalRepository, MilestoneRepository, ProgressSnapshotRepository,
    };
    use application::status_computation::StatusRecomputation;
    use chrono::{DateTime, NaiveDate};
    use domain::{
        Goal, GoalId, Milestone, MilestoneId, ProgressSnapshot, SnapshotReason, Status,
        StatusSource,
    };
    use infrastructure::migrations::run_migrations;
    use infrastructure::repositories::{
        PostgresGoalMilestoneRepository, PostgresGoalRepository, PostgresMilestoneRepository,
        PostgresProgressSnapshotRepository, PostgresRateLimiter, PostgresSessionRepository,
        PostgresTaskRepository,
    };
    use std::collections::HashSet;
    use std::sync::atomic::{AtomicUsize, Ordering};

    /// Two connections: the "lock denied" test holds one while `run_tick`
    /// tries for the other. No tables are needed — the fake job never
    /// touches the database and only the advisory lock is exercised.
    fn pool() -> Option<PgPool> {
        let Some(url) = std::env::var("DATABASE_URL").ok() else {
            // In CI these tests must run: a green build that skipped them proves nothing.
            if std::env::var_os("CI").is_some() {
                panic!("DATABASE_URL is not set; refusing to skip maintenance tests in CI");
            }
            return None;
        };
        Some(
            diesel::r2d2::Pool::builder()
                .max_size(2)
                .build(diesel::r2d2::ConnectionManager::<diesel::PgConnection>::new(&url))
                .expect("could not create test pool"),
        )
    }

    /// A fake job that counts its runs and returns `count` (or fails when
    /// `fail`). Each test uses its own lock key: advisory locks are global
    /// per database, so shared keys would make the parallel tests contend.
    fn fake_job(
        counter: Arc<AtomicUsize>,
        lock_key: i64,
        count: u64,
        fail: bool,
    ) -> MaintenanceJob {
        MaintenanceJob::new("fake_job", lock_key, move || {
            let counter = counter.clone();
            async move {
                counter.fetch_add(1, Ordering::SeqCst);
                if fail {
                    Err(RepositoryError::Unexpected(
                        "faking a job failure".to_owned(),
                    ))
                } else {
                    Ok(count)
                }
            }
        })
    }

    #[tokio::test]
    async fn a_tick_runs_the_job_when_it_can_take_the_lock() {
        let Some(pool) = pool() else { return };
        let counter = Arc::new(AtomicUsize::new(0));
        let job = fake_job(counter.clone(), 0x4D4E5652_4A4F4231, 3, false);

        run_tick(&pool, &job).await;

        assert_eq!(counter.load(Ordering::SeqCst), 1);
    }

    #[tokio::test]
    async fn a_tick_skips_silently_when_another_node_holds_the_lock() {
        let Some(pool) = pool() else { return };
        let counter = Arc::new(AtomicUsize::new(0));
        let job = fake_job(counter.clone(), 0x4D4E5652_4A4F4232, 3, false);

        // Stand in for another node by holding the lock ourselves...
        let held = try_acquire_advisory_lock(&pool, job.lock_key)
            .await
            .expect("take the lock");
        assert!(held.is_some());
        run_tick(&pool, &job).await;
        drop(held);

        assert_eq!(
            counter.load(Ordering::SeqCst),
            0,
            "the job must not run without the lock"
        );
    }

    #[tokio::test]
    async fn a_failing_job_never_panics_and_releases_the_lock() {
        let Some(pool) = pool() else { return };
        let counter = Arc::new(AtomicUsize::new(0));
        let job = fake_job(counter.clone(), 0x4D4E5652_4A4F4233, 0, true);

        run_tick(&pool, &job).await; // must not panic

        assert_eq!(counter.load(Ordering::SeqCst), 1);
        // The lock is released after the tick, so the next one can run.
        let free = try_acquire_advisory_lock(&pool, job.lock_key)
            .await
            .expect("re-take the lock");
        assert!(free.is_some(), "a finished tick must release its lock");
    }

    /// A clock pinned to one date, so the recompute test can advance "today"
    /// past a milestone's target date without waiting for it. The instant
    /// sits at 09:00 of that date, far enough from midnight that `today()`
    /// stays on it.
    struct FixedClock(NaiveDate);

    impl Clock for FixedClock {
        fn now(&self) -> DateTime<Utc> {
            self.0.and_hms_opt(9, 0, 0).unwrap().and_utc()
        }
    }

    /// A full recompute service over the real Postgres tables, with the
    /// clock pinned to `today`.
    fn recompute_service(pool: &PgPool, today: NaiveDate) -> Arc<StatusComputationService> {
        Arc::new(StatusComputationService::new(
            Arc::new(PostgresGoalRepository::new(pool.clone())),
            Arc::new(PostgresMilestoneRepository::new(pool.clone())),
            Arc::new(PostgresGoalMilestoneRepository::new(pool.clone())),
            Arc::new(PostgresTaskRepository::new(pool.clone())),
            Arc::new(FixedClock(today)),
        ))
    }

    /// A milestone whose target date has passed under the test clock, still
    /// carrying its old On Track status — exactly the row shape a stale
    /// database holds.
    fn past_due_milestone() -> Milestone {
        let created = NaiveDate::from_ymd_opt(2026, 1, 1)
            .expect("valid date")
            .and_hms_opt(9, 0, 0)
            .expect("valid time")
            .and_utc();
        Milestone {
            id: MilestoneId::new(),
            title: "Maintenance recompute milestone".into(),
            description: None,
            status: Status::OnTrack,
            status_override: None,
            target_date: Some(NaiveDate::from_ymd_opt(2026, 1, 31).expect("valid date")),
            owner_id: None,
            created_at: created,
            updated_at: created,
        }
    }

    /// A goal with no override and no milestones — its effective status is
    /// the stored automatic one (On Track), which the sweep should record as
    /// `automatic`.
    fn snapshot_test_goal() -> Goal {
        let created = NaiveDate::from_ymd_opt(2026, 1, 1)
            .expect("valid date")
            .and_hms_opt(9, 0, 0)
            .expect("valid time")
            .and_utc();
        Goal {
            id: GoalId::new(),
            title: "Maintenance snapshot goal".into(),
            description: None,
            status: Status::OnTrack,
            status_override: None,
            target_date: None,
            owner_id: None,
            created_at: created,
            updated_at: created,
        }
    }

    /// A full daily-snapshot service over the real Postgres tables, with the
    /// clock pinned to `today`. The recompute port is the real service so
    /// the sweep's freshness pass runs for real.
    fn daily_snapshot_service(pool: &PgPool, today: NaiveDate) -> Arc<ProgressSnapshotService> {
        let computation: Arc<dyn StatusRecomputation> = Arc::new(StatusComputationService::new(
            Arc::new(PostgresGoalRepository::new(pool.clone())),
            Arc::new(PostgresMilestoneRepository::new(pool.clone())),
            Arc::new(PostgresGoalMilestoneRepository::new(pool.clone())),
            Arc::new(PostgresTaskRepository::new(pool.clone())),
            Arc::new(FixedClock(today)),
        ));
        Arc::new(ProgressSnapshotService::new(
            Arc::new(PostgresGoalRepository::new(pool.clone())),
            Arc::new(PostgresMilestoneRepository::new(pool.clone())),
            Arc::new(PostgresTaskRepository::new(pool.clone())),
            Arc::new(PostgresProgressSnapshotRepository::new(pool.clone())),
            computation,
            Arc::new(FixedClock(today)),
        ))
    }

    /// The rows dated `today` only: a running dev server's own sweep writes
    /// rows for its real today, never the pinned test date, so scoping to it
    /// keeps these assertions immune to that traffic.
    fn rows_on(rows: Vec<ProgressSnapshot>, today: NaiveDate) -> Vec<ProgressSnapshot> {
        rows.into_iter()
            .filter(|row| row.snapshot_date == today)
            .collect()
    }

    #[tokio::test]
    async fn a_recompute_tick_refreshes_stale_statuses_and_skips_when_locked() {
        let Some(pool) = pool() else { return };
        run_migrations(&pool).expect("apply migrations");
        let milestones = PostgresMilestoneRepository::new(pool.clone());
        let job = recompute_statuses(recompute_service(
            &pool,
            NaiveDate::from_ymd_opt(2026, 2, 15).expect("valid date"),
        ));

        // Phase 1: the tick takes the lock itself and runs. A running dev
        // server's hourly sweep could hold the key for a moment; if it does,
        // bail rather than assert against its in-flight work.
        let stale = past_due_milestone();
        milestones.create(stale.clone()).await.expect("seed");
        if try_acquire_advisory_lock(&pool, job.lock_key)
            .await
            .unwrap()
            .is_none()
        {
            milestones.delete(stale.id).await.ok();
            return;
        }
        run_tick(&pool, &job).await;
        let refreshed = milestones.find_by_id(stale.id).await.unwrap().unwrap();
        assert_eq!(refreshed.status, Status::OffTrack);

        // Phase 2: with the lock held by this "node", the tick skips silently
        // and the milestone keeps its stale status. Seeded under the lock so
        // no concurrent sweep can touch it.
        let Some(held) = try_acquire_advisory_lock(&pool, job.lock_key)
            .await
            .unwrap()
        else {
            milestones.delete(stale.id).await.ok();
            return;
        };
        let untouched = past_due_milestone();
        milestones.create(untouched.clone()).await.expect("seed");
        run_tick(&pool, &job).await;
        drop(held);
        let still_stale = milestones.find_by_id(untouched.id).await.unwrap().unwrap();
        assert_eq!(still_stale.status, Status::OnTrack);

        milestones.delete(stale.id).await.ok();
        milestones.delete(untouched.id).await.ok();
    }

    #[test]
    fn jobs_register_for_the_session_store_in_use() {
        let Some(pool) = pool() else { return };
        let today = NaiveDate::from_ymd_opt(2026, 2, 15).expect("valid date");
        let computation = recompute_service(&pool, today);
        let snapshots = daily_snapshot_service(&pool, today);

        // Redis-backed sessions: the two always-on Postgres jobs — native
        // TTLs evict expired sessions and stale counters.
        let redis_node = jobs_for(computation.clone(), snapshots.clone(), None);
        assert_eq!(
            redis_node.iter().map(|job| job.name).collect::<Vec<_>>(),
            vec!["recompute_statuses", "record_daily_snapshots"]
        );

        // Postgres-backed: the purges join them, each under its own key.
        let postgres_node = jobs_for(
            computation,
            snapshots,
            Some((
                Arc::new(PostgresSessionRepository::new(pool.clone())),
                Arc::new(PostgresRateLimiter::new(pool.clone())),
            )),
        );
        assert_eq!(
            postgres_node.iter().map(|job| job.name).collect::<Vec<_>>(),
            vec![
                "recompute_statuses",
                "record_daily_snapshots",
                "purge_expired_sessions",
                "purge_rate_limit_counters"
            ]
        );
        let keys = postgres_node
            .iter()
            .map(|job| job.lock_key)
            .collect::<HashSet<_>>();
        assert_eq!(keys.len(), 4, "each job needs its own lock key");
    }

    #[tokio::test]
    async fn a_daily_snapshot_tick_records_one_row_per_entity_and_the_second_inserts_nothing() {
        let Some(pool) = pool() else { return };
        run_migrations(&pool).expect("apply migrations");
        let today = NaiveDate::from_ymd_opt(2026, 2, 15).expect("valid date");
        let service = daily_snapshot_service(&pool, today);
        let snapshots = PostgresProgressSnapshotRepository::new(pool.clone());
        let goals = PostgresGoalRepository::new(pool.clone());
        let milestones = PostgresMilestoneRepository::new(pool.clone());
        let job = record_daily_snapshots(service);

        // Phase 1: the tick takes the lock itself and runs. A running dev
        // server's sweep could hold the key for a moment; if it does, bail
        // rather than assert against its in-flight work. The pinned date is
        // far from "real" today, so the dev server never writes rows for it.
        let goal = snapshot_test_goal();
        let milestone = past_due_milestone();
        goals.create(goal.clone()).await.expect("seed");
        milestones.create(milestone.clone()).await.expect("seed");
        if try_acquire_advisory_lock(&pool, job.lock_key)
            .await
            .unwrap()
            .is_none()
        {
            goals.delete(goal.id).await.ok();
            milestones.delete(milestone.id).await.ok();
            return;
        }
        // A parallel test suite can delete an entity mid-recompute (the id
        // list and the status write race), which aborts the tick with
        // NotFound before it writes anything — so retry like the hourly
        // schedule would: a failed tick changes nothing, the next retries.
        for _ in 0..5 {
            run_tick(&pool, &job).await;
            let goal_has =
                !rows_on(snapshots.list_for_goal(goal.id).await.unwrap(), today).is_empty();
            let milestone_has = !rows_on(
                snapshots.list_for_milestone(milestone.id).await.unwrap(),
                today,
            )
            .is_empty();
            if goal_has && milestone_has {
                break;
            }
        }
        let goal_rows = rows_on(snapshots.list_for_goal(goal.id).await.unwrap(), today);
        assert_eq!(goal_rows.len(), 1, "one daily row for the goal");
        assert_eq!(goal_rows[0].status, Status::OnTrack);
        assert_eq!(goal_rows[0].source, StatusSource::Computed);
        assert_eq!(goal_rows[0].reason, SnapshotReason::Daily);
        let milestone_rows = rows_on(
            snapshots.list_for_milestone(milestone.id).await.unwrap(),
            today,
        );
        assert_eq!(milestone_rows.len(), 1, "one daily row for the milestone");
        // The recompute ran before the recording, so the past target date is
        // in the row, not the stale stored status.
        assert_eq!(milestone_rows[0].status, Status::OffTrack);
        assert_eq!(milestone_rows[0].source, StatusSource::Computed);
        assert_eq!(milestone_rows[0].reason, SnapshotReason::Daily);

        // Phase 2: a second tick the same day inserts nothing — the anti-join
        // finds no missing ids and returns before writing.
        run_tick(&pool, &job).await;
        assert_eq!(
            rows_on(snapshots.list_for_goal(goal.id).await.unwrap(), today).len(),
            1
        );
        assert_eq!(
            rows_on(
                snapshots.list_for_milestone(milestone.id).await.unwrap(),
                today,
            )
            .len(),
            1
        );

        // Phase 3: with the lock held by this "node", the tick skips silently
        // and a freshly seeded entity gets no row. Seeded under the lock so
        // no concurrent sweep can touch it.
        let Some(held) = try_acquire_advisory_lock(&pool, job.lock_key)
            .await
            .unwrap()
        else {
            goals.delete(goal.id).await.ok();
            milestones.delete(milestone.id).await.ok();
            return;
        };
        let late_goal = snapshot_test_goal();
        let late_milestone = past_due_milestone();
        goals.create(late_goal.clone()).await.expect("seed");
        milestones
            .create(late_milestone.clone())
            .await
            .expect("seed");
        run_tick(&pool, &job).await;
        drop(held);
        assert!(rows_on(snapshots.list_for_goal(late_goal.id).await.unwrap(), today).is_empty());
        assert!(
            rows_on(
                snapshots
                    .list_for_milestone(late_milestone.id)
                    .await
                    .unwrap(),
                today,
            )
            .is_empty()
        );

        // Deleting the entities cascades their snapshot rows away.
        goals.delete(goal.id).await.ok();
        goals.delete(late_goal.id).await.ok();
        milestones.delete(milestone.id).await.ok();
        milestones.delete(late_milestone.id).await.ok();
    }

    #[tokio::test]
    async fn concurrent_daily_sweeps_insert_each_entity_once() {
        let Some(pool) = pool() else { return };
        run_migrations(&pool).expect("apply migrations");
        let today = NaiveDate::from_ymd_opt(2026, 3, 10).expect("valid date");
        let service = daily_snapshot_service(&pool, today);
        let snapshots = PostgresProgressSnapshotRepository::new(pool.clone());
        let goals = PostgresGoalRepository::new(pool.clone());
        let milestones = PostgresMilestoneRepository::new(pool.clone());

        let goal = snapshot_test_goal();
        let milestone = past_due_milestone();
        goals.create(goal.clone()).await.expect("seed");
        milestones.create(milestone.clone()).await.expect("seed");

        // Two "nodes" sweep at once: each (entity, date) pair lands exactly
        // once — the partial unique index plus ON CONFLICT DO NOTHING makes
        // the losing insert a no-op. A parallel test suite can delete an
        // entity mid-recompute and abort one call before it writes anything,
        // so retry like the hourly schedule would until both complete; the
        // rows for other entities in the shared database are legitimate
        // sweep work, not duplicates, so only our own two are counted.
        for _ in 0..5 {
            let (first, second) = tokio::join!(
                service.ensure_daily_snapshots(),
                service.ensure_daily_snapshots()
            );
            if first.is_ok() && second.is_ok() {
                break;
            }
        }
        assert_eq!(
            rows_on(snapshots.list_for_goal(goal.id).await.unwrap(), today).len(),
            1
        );
        assert_eq!(
            rows_on(
                snapshots.list_for_milestone(milestone.id).await.unwrap(),
                today,
            )
            .len(),
            1
        );

        goals.delete(goal.id).await.ok();
        milestones.delete(milestone.id).await.ok();
    }
}
