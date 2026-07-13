//! Resource governor for parallel pre-push hooks (#678).
//!
//! `daft sync --push` over N branches runs the repo's pre-push hook once
//! per branch, concurrently. Heavy hooks are internally parallel — they
//! assume they own the machine — so the aggregate footprint multiplies to
//! N × cores threads and N × peak-RSS, enough to swap the machine to
//! death. The governor keeps that fan-out inside the machine's memory
//! budget while staying invisible when hooks are light or absent.
//!
//! Layout mirrors the coordinator's hexagonal split (`ARCHITECTURE.md`):
//!
//! - [`domain`] — pure decision core (slow-start + AIMD admission over a
//!   traffic-light pressure signal); no I/O, fully deterministic.
//! - [`ports`] — trait surfaces ([`ports::ResourceProbe`]).
//! - [`adapters`] — sysinfo/PSI probe, plus the `DAFT_GOVERNOR_FORCE_*`
//!   probe for deterministic tests.
//! - This module — the imperative shell: [`SyncGovernor`] wires the
//!   probe, the controller, a sampling thread, and the DAG executor's
//!   [`DagGovernor`](crate::core::worktree::sync_dag::DagGovernor) seam
//!   together.
//!
//! The governor only exists while a sync push phase runs — it is never
//! constructed on shell-eval hot paths, and a no-hook push never
//! constructs it at all (zero-overhead gate in `commands/sync.rs`).

pub mod adapters;
pub mod domain;
pub mod ports;

use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

use crate::core::worktree::sync_dag::{AdmitDecision, DagGovernor, DeferReason, SyncTask, TaskId};
use crate::git::cancel::CancelFlag;
use domain::{Controller, GovernorParams, HoldReason, ResourceSample};
use ports::ResourceProbe;

/// Governor tick cadence. Memory is probed every other tick (500 ms);
/// the intermediate ticks keep shutdown/cancel latency low and leave
/// headroom for the faster containment decisions of stage 3.
const TICK: Duration = Duration::from_millis(250);

/// A push unit the governor tracks from admission to completion.
#[derive(Debug)]
struct UnitEntry {
    branch: String,
    /// Root pid of the unit's `git push` (group leader under
    /// `SupervisionMode::Isolated`); attached shortly after spawn.
    pid: Option<u32>,
}

/// Shared state between the governor handle, its unit guards, and the
/// sampling thread.
struct Shared {
    controller: Mutex<Controller>,
    latest: Mutex<ResourceSample>,
    /// Units currently holding an admission slot (Seam A accounting —
    /// incremented on admit, decremented on release).
    admitted: AtomicUsize,
    /// Registered units (Seam B — carries pids for the sampler and the
    /// stage-3 containment tier).
    units: Mutex<Vec<UnitEntry>>,
    /// Monotonic origin for the controller's injected timestamps.
    start: Instant,
    cancel: Arc<CancelFlag>,
}

impl Shared {
    fn now_ms(&self) -> u64 {
        u64::try_from(self.start.elapsed().as_millis()).unwrap_or(u64::MAX)
    }
}

/// The dynamic resource governor for one sync run's push phase.
///
/// Construct with [`SyncGovernor::spawn`], hand it to the DAG executor via
/// `with_governor`, register each push through [`SyncGovernor::begin_unit`],
/// and call [`SyncGovernor::shutdown`] after the executor returns.
pub struct SyncGovernor {
    shared: Arc<Shared>,
    tick_thread: Mutex<Option<JoinHandle<()>>>,
    stop: Arc<AtomicBool>,
}

impl SyncGovernor {
    /// Probe once (seeding the controller and resolving `params_of` against
    /// real machine totals), then start the sampling thread.
    pub fn spawn(
        probe: Box<dyn ResourceProbe>,
        cancel: Arc<CancelFlag>,
        params_of: impl FnOnce(&ResourceSample) -> GovernorParams,
    ) -> Arc<Self> {
        let first = probe.sample();
        let params = params_of(&first);
        let shared = Arc::new(Shared {
            controller: Mutex::new(Controller::new(params)),
            latest: Mutex::new(first),
            admitted: AtomicUsize::new(0),
            units: Mutex::new(Vec::new()),
            start: Instant::now(),
            cancel,
        });
        let stop = Arc::new(AtomicBool::new(false));

        let thread_shared = Arc::clone(&shared);
        let thread_stop = Arc::clone(&stop);
        let handle = std::thread::spawn(move || tick_loop(&thread_shared, probe, &thread_stop));

        Arc::new(Self {
            shared,
            tick_thread: Mutex::new(Some(handle)),
            stop,
        })
    }

    /// Stop and join the sampling thread. Idempotent; also runs on drop.
    pub fn shutdown(&self) {
        self.stop.store(true, Ordering::Relaxed);
        if let Some(handle) = self.tick_thread.lock().unwrap().take() {
            let _ = handle.join();
        }
    }

    /// Register a push unit that is about to spawn its `git push`.
    /// Dropping the guard deregisters the unit.
    pub fn begin_unit(&self, branch: &str) -> UnitGuard {
        self.shared.units.lock().unwrap().push(UnitEntry {
            branch: branch.to_string(),
            pid: None,
        });
        UnitGuard {
            shared: Arc::clone(&self.shared),
            branch: branch.to_string(),
        }
    }
}

impl Drop for SyncGovernor {
    fn drop(&mut self) {
        self.shutdown();
    }
}

/// Follows one push unit's lifetime (RAII: drop = unit gone).
pub struct UnitGuard {
    shared: Arc<Shared>,
    branch: String,
}

impl UnitGuard {
    /// Record the unit's `git push` root pid (called from the spawn
    /// callback threaded through `GitCommand::with_push_supervision`).
    pub fn attach_pid(&self, pid: u32) {
        let mut units = self.shared.units.lock().unwrap();
        if let Some(unit) = units.iter_mut().find(|u| u.branch == self.branch) {
            unit.pid = Some(pid);
        }
    }
}

impl Drop for UnitGuard {
    fn drop(&mut self) {
        let mut units = self.shared.units.lock().unwrap();
        if let Some(pos) = units.iter().position(|u| u.branch == self.branch) {
            units.remove(pos);
        }
    }
}

fn tick_loop(shared: &Shared, probe: Box<dyn ResourceProbe>, stop: &AtomicBool) {
    let mut tick_count: u64 = 0;
    loop {
        std::thread::sleep(TICK);
        if stop.load(Ordering::Relaxed) {
            return;
        }
        // A cancelled run tears its pushes down through the supervisor
        // cascade; the governor stands down immediately (stage 3 will also
        // thaw anything frozen here).
        if shared.cancel.is_cancelled() {
            return;
        }
        tick_count += 1;
        if !tick_count.is_multiple_of(2) {
            continue;
        }
        let sample = probe.sample();
        let running = shared.admitted.load(Ordering::Relaxed);
        shared.controller.lock().unwrap().tick(&sample, running);
        *shared.latest.lock().unwrap() = sample;
    }
}

impl DagGovernor for SyncGovernor {
    fn try_admit(&self, task: &SyncTask) -> AdmitDecision {
        if !matches!(task.id, TaskId::Push(_)) {
            return AdmitDecision::Admit;
        }
        let running = self.shared.admitted.load(Ordering::Relaxed);
        let sample = *self.shared.latest.lock().unwrap();
        let now_ms = self.shared.now_ms();
        let decision = self
            .shared
            .controller
            .lock()
            .unwrap()
            .try_admit(now_ms, running, &sample, None);
        match decision {
            Ok(()) => {
                self.shared.admitted.fetch_add(1, Ordering::Relaxed);
                AdmitDecision::Admit
            }
            Err(HoldReason::AtCap) => AdmitDecision::Defer(DeferReason::ClassCap),
            Err(HoldReason::KillCooldown) => AdmitDecision::Defer(DeferReason::KillCooldown),
            Err(HoldReason::AtTarget | HoldReason::Memory | HoldReason::Stagger) => {
                AdmitDecision::Defer(DeferReason::MemoryPressure)
            }
        }
    }

    fn release(&self, task: &SyncTask) {
        if matches!(task.id, TaskId::Push(_)) {
            self.shared.admitted.fetch_sub(1, Ordering::Relaxed);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A probe returning a fixed sample — keeps shell tests off sysinfo.
    struct FixedProbe(ResourceSample);

    impl ResourceProbe for FixedProbe {
        fn sample(&self) -> ResourceSample {
            self.0
        }
    }

    const GIB: u64 = 1 << 30;

    fn green_sample() -> ResourceSample {
        ResourceSample {
            mem_total: 32 * GIB,
            mem_available: 20 * GIB,
            swap_used: 0,
            psi_some_avg10: None,
        }
    }

    fn push_task(branch: &str) -> SyncTask {
        SyncTask {
            id: TaskId::Push(branch.into()),
            phase: crate::core::worktree::sync_dag::OperationPhase::Push,
            worktree_path: None,
            branch_name: branch.into(),
        }
    }

    #[test]
    fn admits_and_releases_through_the_seam() {
        let governor = SyncGovernor::spawn(
            Box::new(FixedProbe(green_sample())),
            Arc::new(CancelFlag::new()),
            |first| {
                assert_eq!(first.mem_total, 32 * GIB);
                GovernorParams::new(2, 2 * GIB)
            },
        );
        let a = push_task("a");
        let b = push_task("b");
        let c = push_task("c");
        assert_eq!(governor.try_admit(&a), AdmitDecision::Admit);
        // Second admission holds on the stagger (same instant), then the
        // hard cap once the target is reached — both surface as Defer.
        assert!(matches!(governor.try_admit(&b), AdmitDecision::Defer(_)));
        std::thread::sleep(Duration::from_millis(domain::STAGGER_MS + 50));
        assert_eq!(governor.try_admit(&b), AdmitDecision::Admit);
        assert!(matches!(governor.try_admit(&c), AdmitDecision::Defer(_)));
        // Releasing a slot re-opens admission once the stagger (re-stamped
        // on every admit) has elapsed again.
        governor.release(&a);
        std::thread::sleep(Duration::from_millis(domain::STAGGER_MS + 50));
        assert_eq!(governor.try_admit(&c), AdmitDecision::Admit);
        governor.shutdown();
    }

    #[test]
    fn non_push_tasks_bypass_the_governor() {
        let governor = SyncGovernor::spawn(
            Box::new(FixedProbe(green_sample())),
            Arc::new(CancelFlag::new()),
            |_| GovernorParams::new(1, 2 * GIB),
        );
        let fetch = SyncTask {
            id: TaskId::Fetch,
            phase: crate::core::worktree::sync_dag::OperationPhase::Fetch,
            worktree_path: None,
            branch_name: String::new(),
        };
        assert_eq!(governor.try_admit(&push_task("a")), AdmitDecision::Admit);
        assert_eq!(governor.try_admit(&fetch), AdmitDecision::Admit);
        governor.shutdown();
    }

    #[test]
    fn unit_registry_tracks_pid_until_drop() {
        let governor = SyncGovernor::spawn(
            Box::new(FixedProbe(green_sample())),
            Arc::new(CancelFlag::new()),
            |_| GovernorParams::new(2, 2 * GIB),
        );
        let guard = governor.begin_unit("feat/a");
        guard.attach_pid(4242);
        {
            let units = governor.shared.units.lock().unwrap();
            assert_eq!(units.len(), 1);
            assert_eq!(units[0].pid, Some(4242));
        }
        drop(guard);
        assert!(governor.shared.units.lock().unwrap().is_empty());
        governor.shutdown();
    }

    #[test]
    fn shutdown_joins_the_tick_thread() {
        let governor = SyncGovernor::spawn(
            Box::new(FixedProbe(green_sample())),
            Arc::new(CancelFlag::new()),
            |_| GovernorParams::new(2, 2 * GIB),
        );
        governor.shutdown();
        assert!(governor.tick_thread.lock().unwrap().is_none());
        // Idempotent.
        governor.shutdown();
    }
}
