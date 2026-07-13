//! Ports for the governor subsystem — the trait surfaces its imperative
//! shell talks through, modeled on the coordinator's ports
//! (`src/coordinator/ports/clock.rs`): minimal, `Send + Sync`, primitive
//! value types (or store row models — the row is the contract). Platform
//! specifics live in the adapters.

use crate::governor::domain::ResourceSample;
use crate::store::models::{GovernorEventRow, HookProfileRow};

/// Reads system memory state and per-tree memory use.
///
/// Implementations must be cheap enough to call every few hundred
/// milliseconds and must never block on anything slower than a syscall.
pub trait ResourceProbe: Send + Sync {
    /// A fresh reading of system memory.
    fn sample(&self) -> ResourceSample;

    /// Total RSS of each root's process tree (root + all descendants),
    /// index-aligned with `roots`. `None` per root when the tree cannot be
    /// observed (process gone, platform limitation).
    fn tree_rss(&self, roots: &[u32]) -> Vec<Option<u64>>;
}

/// Identifies one hook script's profile row.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProfileKey {
    pub repo_hash: String,
    /// Hook stage, e.g. `pre-push`.
    pub stage: String,
    /// Content hash of the resolved hook file.
    pub hook_hash: String,
}

/// Persistence for learned hook profiles and governor events.
///
/// Strictly best-effort: implementations swallow storage errors (a
/// profile is a cache; an event log is advisory) — a store problem must
/// never fail, slow, or write to the terminal of a running push. `load`
/// returns `None` for both "no profile yet" and "store unavailable".
pub trait ProfileStore: Send + Sync {
    /// The stored profile for `key`, if one exists and is readable.
    fn load(&self, key: &ProfileKey) -> Option<HookProfileRow>;

    /// Insert or replace the profile for `key`.
    fn save(&self, row: &HookProfileRow);

    /// Append governor events (throttles, freezes, kills, timeouts).
    fn record_events(&self, events: &[GovernorEventRow]);
}
