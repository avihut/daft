//! Adapters implementing the governor's ports against the real world
//! (and, for tests, against forced values).

mod sqlite_profiles;
mod sysinfo_probe;

pub use sqlite_profiles::SqliteProfileStore;
pub use sysinfo_probe::{ForcedProbe, SysinfoProbe, build_probe};
