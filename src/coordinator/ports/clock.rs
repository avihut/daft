//! Port: wall-clock time.
//!
//! Domain code that needs "now" depends on this trait so unit tests can
//! pin time without setting envs or freezing the system clock.

use chrono::{DateTime, Utc};

pub trait Clock: Send + Sync {
    fn now(&self) -> DateTime<Utc>;
}
