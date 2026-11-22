//! The current time, behind a port so date-dependent rules (roadmap 3.13's
//! status computation) and timestamps (roadmap 3.14's snapshots) can be
//! tested against a fixed clock.

use chrono::{DateTime, NaiveDate, Utc};

/// Supplies the current time for date- and timestamp-dependent rules. UTC in
/// v1 (D19).
pub trait Clock: Send + Sync {
    /// The current UTC timestamp.
    fn now(&self) -> DateTime<Utc>;

    /// The current UTC calendar date, derived from [`Self::now`] so the two
    /// can never disagree.
    fn today(&self) -> NaiveDate {
        self.now().date_naive()
    }
}

/// The system clock, reading the real current UTC time.
#[derive(Default)]
pub struct SystemClock;

impl Clock for SystemClock {
    fn now(&self) -> DateTime<Utc> {
        Utc::now()
    }
}
