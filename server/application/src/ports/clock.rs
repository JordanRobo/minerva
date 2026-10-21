//! The current date, behind a port so date-dependent rules (roadmap 3.13's
//! status computation) can be tested against a fixed clock.

use chrono::NaiveDate;

/// Supplies "today" for date-dependent rules. UTC in v1 (D19).
pub trait Clock: Send + Sync {
    /// The current UTC calendar date.
    fn today(&self) -> NaiveDate;
}

/// The system clock, reading the real current UTC date.
#[derive(Default)]
pub struct SystemClock;

impl Clock for SystemClock {
    fn today(&self) -> NaiveDate {
        chrono::Utc::now().date_naive()
    }
}
