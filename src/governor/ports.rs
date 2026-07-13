//! Ports for the governor subsystem — the trait surfaces its imperative
//! shell talks through, modeled on the coordinator's ports
//! (`src/coordinator/ports/clock.rs`): minimal, `Send + Sync`, primitive
//! value types only. Platform specifics live in the adapters.

use crate::governor::domain::ResourceSample;

/// Reads system memory state.
///
/// Implementations must be cheap enough to call every few hundred
/// milliseconds and must never block on anything slower than a syscall.
pub trait ResourceProbe: Send + Sync {
    /// A fresh reading of system memory.
    fn sample(&self) -> ResourceSample;
}
