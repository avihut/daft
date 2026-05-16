//! Hexagonal *adapters* for the coordinator.
//!
//! Each adapter satisfies one of the [`crate::coordinator::ports`] traits
//! by gluing it to a concrete dependency: SQLite for storage, `chrono::Utc`
//! for time, `nix` for process-group signals. Adapters are the only place
//! where store row models or platform-specific calls are allowed to leak
//! into coordinator code.

pub mod sqlite_store;
pub mod system_clock;
pub mod unix_process;

pub use sqlite_store::SqliteJobsStore;
pub use system_clock::SystemClock;

#[cfg(unix)]
pub use unix_process::UnixProcess;
