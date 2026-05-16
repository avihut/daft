//! Adapter: real wall-clock time.

use crate::coordinator::ports::Clock;
use chrono::{DateTime, Utc};

#[derive(Debug, Clone, Copy, Default)]
pub struct SystemClock;

impl Clock for SystemClock {
    fn now(&self) -> DateTime<Utc> {
        Utc::now()
    }
}
