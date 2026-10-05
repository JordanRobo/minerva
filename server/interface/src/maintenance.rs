//! Periodic maintenance jobs (roadmap 2.8): background work that runs on
//! every node but is safe to do so because each tick first takes a Postgres
//! advisory lock and skips silently when another node already holds it, so
//! however many API nodes run, exactly one does the work per tick.

use application::ports::{RepositoryError, SessionRepository};
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
}
