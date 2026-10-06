//! Postgres implementation of [`RateLimiter`].
//!
//! Counters are rows in `rate_limit_counters`, one per (key, window start).
//! A hit is a single atomic upsert, so concurrent hits increment the same
//! row without interleaving; windows that have fully passed are removed by
//! the hourly maintenance job (`purge_before`).

use application::rate_limit::{
    RateLimitDecision, RateLimitError, RateLimitPolicy, RateLimiter, decide, window_start,
};
use chrono::{DateTime, Utc};
use diesel::prelude::*;

use crate::db::{PgPool, run_on_postgres};
use crate::error::map_diesel_error;
use crate::schema::rate_limit_counters;

/// [`RateLimiter`] backed by the `rate_limit_counters` table.
pub struct PostgresRateLimiter {
    pool: PgPool,
}

impl PostgresRateLimiter {
    pub fn new(pool: PgPool) -> Self {
        Self { pool }
    }
}

#[async_trait::async_trait]
impl RateLimiter for PostgresRateLimiter {
    async fn hit(
        &self,
        policy: &RateLimitPolicy,
        subject: &str,
        now: DateTime<Utc>,
    ) -> Result<RateLimitDecision, RateLimitError> {
        let pool = self.pool.clone();
        // An owned copy: the blocking closure below is 'static and cannot
        // capture the borrowed policy.
        let policy = *policy;
        // The key embeds the policy name so different policies never share a
        // counter; `subject` is already hashed by the service.
        let key = format!("{}:{}", policy.name, subject);
        let start = window_start(now, policy.window);
        run_on_postgres(pool, move |conn| {
            // One statement: insert-or-increment and read back the new count
            // atomically, so two concurrent hits cannot both see the same
            // count.
            let count: i32 = diesel::insert_into(rate_limit_counters::table)
                .values((
                    rate_limit_counters::key.eq(key),
                    rate_limit_counters::window_start.eq(start),
                    rate_limit_counters::count.eq(1),
                ))
                .on_conflict((rate_limit_counters::key, rate_limit_counters::window_start))
                .do_update()
                .set(rate_limit_counters::count.eq(rate_limit_counters::count + 1))
                .returning(rate_limit_counters::count)
                .get_result(conn)
                .map_err(map_diesel_error)?;
            Ok(decide(count as u64, &policy, now))
        })
        .await
        .map_err(|err| RateLimitError::Store(err.to_string()))
    }

    async fn purge_before(&self, cutoff: DateTime<Utc>) -> Result<u64, RateLimitError> {
        let pool = self.pool.clone();
        run_on_postgres(pool, move |conn| {
            let removed = diesel::delete(
                rate_limit_counters::table.filter(rate_limit_counters::window_start.lt(cutoff)),
            )
            .execute(conn)
            .map_err(map_diesel_error)?;
            Ok(removed as u64)
        })
        .await
        .map_err(|err| RateLimitError::Store(err.to_string()))
    }
}
