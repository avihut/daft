//! Ports for the governor subsystem — the trait surfaces its imperative
//! shell talks through, modeled on the coordinator's ports
//! (`src/coordinator/ports/clock.rs`): minimal, `Send + Sync`, primitive
//! value types (or store row models — the row is the contract). Platform
//! specifics live in the adapters.

use crate::governor::domain::ResourceSample;
use crate::store::models::{GovernorEventRow, HookProfileRow};

/// What kind of work a governed unit is — the containment policy's one
/// discriminator.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum UnitClass {
    /// A unit the user is actively waiting on in the foreground (a merge
    /// gate's ring, an attended hook job). Subject to admission like any
    /// unit, but NEVER frozen or killed — a stopped foreground unit is a
    /// hung terminal from the user's point of view.
    ForegroundInteractive,
    /// Deferrable or fan-out work (sync pushes, background jobs). Eligible
    /// for the full containment ladder: freeze under sustained pressure,
    /// kill-and-requeue when a freeze doesn't relieve it.
    Background,
}

/// A unit of work as the admission port sees it: an opaque label (unique
/// within the run — a branch name, `<invocation>:<job>`, …) plus its class.
#[derive(Debug, Clone, Copy)]
pub struct WorkUnit<'a> {
    pub label: &'a str,
    pub class: UnitClass,
}

/// Admission decision returned by [`WorkAdmission::try_admit`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AdmitDecision {
    /// Run the unit now. The governor reserved a slot; the caller pairs
    /// this with exactly one [`WorkAdmission::release`] when the unit
    /// leaves the running set.
    Admit,
    /// Keep the unit queued and re-check admission later.
    Defer(DeferReason),
}

/// Why the governor deferred a ready unit.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DeferReason {
    /// The concurrency cap for the unit's class is reached.
    ClassCap,
    /// Not enough memory headroom to admit another unit.
    MemoryPressure,
    /// A governor kill just happened; waiting out the post-kill cooldown.
    KillCooldown,
}

/// Admission gate consulted before a governed unit starts.
///
/// Contract (every caller relies on every point):
/// - `try_admit` returning [`AdmitDecision::Admit`] reserves one slot and is
///   paired with exactly one [`WorkAdmission::release`]; returning
///   [`AdmitDecision::Defer`] must be side-effect-free.
/// - A governor that currently tracks zero admitted units must admit — the
///   caller's liveness guarantee (an all-deferred queue with nothing running
///   would otherwise never make progress).
/// - Both methods may be called with the caller's internal lock held: they
///   must return promptly and never call back into the caller.
pub trait WorkAdmission: Send + Sync {
    /// Decide whether `unit` may start now.
    fn try_admit(&self, unit: &WorkUnit<'_>) -> AdmitDecision;
    /// Return the slot reserved by a successful `try_admit`.
    fn release(&self, unit: &WorkUnit<'_>);
}

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
