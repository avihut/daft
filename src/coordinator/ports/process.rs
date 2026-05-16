//! Port: process-group control.
//!
//! The reconciler asks "is this process group still alive?" and may need
//! to send signals to one. Keeping these primitives behind a trait lets
//! domain logic stay platform-agnostic and lets tests assert reconcile
//! behavior without spawning real processes.

pub trait ProcessControl: Send + Sync {
    /// Returns `true` if `pgid` still names a live process group on this
    /// host. A `false` answer means the leader and every descendant have
    /// exited and the kernel has released the group's identity.
    fn process_group_alive(&self, pgid: u32) -> bool;

    /// Send `signal` (the libc/nix SIGINT/SIGTERM/... number) to every
    /// process in `pgid`. `Ok(())` on success; `Err(_)` on EPERM/ESRCH/
    /// EINVAL — callers usually log and proceed.
    fn signal_process_group(&self, pgid: u32, signal: i32) -> anyhow::Result<()>;
}
