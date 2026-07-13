//! Typed query layer for store tables.
//!
//! Repos take a `&rusqlite::Connection` (so callers can pick reader vs
//! writer themselves), prepare named statements, and return typed row
//! structs. All SQL is parameterized — no `format!` into queries — and a
//! CI grep-gate enforces that in `src/store/repos/`.
//!
//! Multi-row writes go through transactions; see [`with_write_txn`].

use crate::store::error::Result;
use rusqlite::Connection;

pub mod catalog_repos;
pub mod governor_events;
pub mod hook_profiles;
pub mod invocations;
pub mod jobs;
pub mod repo_policies;
pub mod repo_sizes;
pub mod visitor_seeds;
pub mod worktree_sizes;

pub use catalog_repos::CatalogReposRepo;
pub use governor_events::GovernorEventsRepo;
pub use hook_profiles::HookProfilesRepo;
pub use invocations::InvocationsRepo;
pub use jobs::JobsRepo;
pub use repo_policies::RepoPoliciesRepo;
pub use repo_sizes::RepoSizesRepo;
pub use visitor_seeds::VisitorSeedsRepo;
pub use worktree_sizes::WorktreeSizesRepo;

/// Run a closure inside a deferred transaction on `conn`, commit on
/// success, roll back on error. Use for multi-statement updates that must
/// land together.
pub fn with_write_txn<T, F>(conn: &mut Connection, f: F) -> Result<T>
where
    F: FnOnce(&rusqlite::Transaction<'_>) -> Result<T>,
{
    let tx = conn.transaction()?;
    let out = f(&tx)?;
    tx.commit()?;
    Ok(out)
}
