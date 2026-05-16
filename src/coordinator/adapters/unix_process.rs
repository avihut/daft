//! Adapter: process-group control via `nix`.
//!
//! Unix-only. Non-Unix builds will not compile the coordinator process
//! module at all (see the existing `#[cfg(unix)]` gates), so this adapter
//! mirrors that constraint.

#![cfg(unix)]

use crate::coordinator::ports::ProcessControl;
use anyhow::{Context, Result};
use nix::sys::signal::Signal;
use nix::unistd::Pid;

#[derive(Debug, Clone, Copy, Default)]
pub struct UnixProcess;

impl ProcessControl for UnixProcess {
    fn process_group_alive(&self, pgid: u32) -> bool {
        // `killpg(pgid, 0)` is the canonical "is this group alive" probe:
        // signal 0 performs the EPERM/ESRCH bookkeeping without actually
        // delivering a signal. EPERM (the group exists but we lack
        // permission) still counts as "alive" — the leader is out there
        // even if we can't signal it.
        let pid = Pid::from_raw(pgid as i32);
        match nix::sys::signal::killpg(pid, None) {
            Ok(()) => true,
            Err(nix::errno::Errno::EPERM) => true,
            Err(_) => false,
        }
    }

    fn signal_process_group(&self, pgid: u32, signal: i32) -> Result<()> {
        let sig =
            Signal::try_from(signal).with_context(|| format!("invalid signal number {signal}"))?;
        nix::sys::signal::killpg(Pid::from_raw(pgid as i32), sig)
            .with_context(|| format!("killpg({pgid}, {sig:?})"))?;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn current_process_group_is_alive() {
        // The test runner's own pgrp must report as alive.
        let pgid = unsafe { libc::getpgrp() } as u32;
        assert!(UnixProcess.process_group_alive(pgid));
    }

    #[test]
    fn nonexistent_process_group_is_dead() {
        // PID 2^31-1 is reserved-high and never assigned on Unix.
        // `killpg` returns ESRCH and we map that to "dead".
        let phantom = (i32::MAX - 1) as u32;
        assert!(!UnixProcess.process_group_alive(phantom));
    }
}
