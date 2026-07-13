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

use std::collections::HashMap;
use std::sync::atomic::{AtomicBool, AtomicU64, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

use crate::core::worktree::sync_dag::{AdmitDecision, DagGovernor, DeferReason, SyncTask, TaskId};
use crate::git::cancel::CancelFlag;
use crate::store::models::{GovernorEventRow, HookProfileRow};
use domain::{Controller, GovernorParams, HoldReason, HookProfile, ResourceSample};
use ports::{ProfileKey, ProfileStore, ResourceProbe};

/// Content hash of the resolved hook file — the profile cache key. Not a
/// security boundary: `DefaultHasher`'s algorithm may change across Rust
/// releases, which merely invalidates the cache and re-profiles the hook.
pub fn hook_script_hash(path: &std::path::Path) -> Option<String> {
    use std::hash::{Hash, Hasher};
    let bytes = std::fs::read(path).ok()?;
    let mut hasher = std::collections::hash_map::DefaultHasher::new();
    bytes.hash(&mut hasher);
    Some(format!("{:016x}", hasher.finish()))
}

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
    /// Wall-clock start, for the profile's per-run duration.
    started: Instant,
    /// Peak tree-RSS the sampler has observed for this unit.
    peak_rss: u64,
}

/// Profile persistence for this run (`None` = profiling disabled).
struct ProfilePersistence {
    store: Box<dyn ProfileStore>,
    key: ProfileKey,
    /// The profile as loaded at spawn — the fold base at persist time.
    prior: Option<HookProfile>,
}

/// Admission-defer bookkeeping for the event log.
#[derive(Default)]
struct ThrottleLog {
    /// Branch → first deferral of its current wait.
    since: HashMap<String, Instant>,
    /// Resolved waits: (branch, held duration).
    held: Vec<(String, Duration)>,
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
    /// Learned peak for this hook (loaded at spawn), if profiled.
    profile_peak: Option<u64>,
    /// Highest tree-RSS observed across units this run (0 = none yet).
    live_peak: AtomicU64,
    /// `(peak_rss, wall_ms)` of completed units — folded into the profile
    /// at shutdown.
    completed: Mutex<Vec<(u64, u64)>>,
    throttle: Mutex<ThrottleLog>,
    profiles: Option<ProfilePersistence>,
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
    persisted: AtomicBool,
}

impl SyncGovernor {
    /// Probe once (seeding the controller and resolving `params_of` against
    /// real machine totals), load the hook's learned profile if
    /// `profiles` is given, then start the sampling thread.
    pub fn spawn(
        probe: Box<dyn ResourceProbe>,
        profiles: Option<(Box<dyn ProfileStore>, ProfileKey)>,
        cancel: Arc<CancelFlag>,
        params_of: impl FnOnce(&ResourceSample) -> GovernorParams,
    ) -> Arc<Self> {
        let first = probe.sample();
        let persistence = profiles.map(|(store, key)| {
            let prior = store.load(&key).map(|row| HookProfile {
                peak_rss: row.peak_rss_bytes,
                wall_ms: row.wall_ms,
                runs: row.runs,
            });
            ProfilePersistence { store, key, prior }
        });
        let prior = persistence.as_ref().and_then(|p| p.prior);
        let params = params_of(&first).with_profile(prior);
        let shared = Arc::new(Shared {
            controller: Mutex::new(Controller::new(params)),
            latest: Mutex::new(first),
            admitted: AtomicUsize::new(0),
            units: Mutex::new(Vec::new()),
            start: Instant::now(),
            cancel,
            profile_peak: prior.map(|p| p.peak_rss),
            live_peak: AtomicU64::new(0),
            completed: Mutex::new(Vec::new()),
            throttle: Mutex::new(ThrottleLog::default()),
            profiles: persistence,
        });
        let stop = Arc::new(AtomicBool::new(false));

        let thread_shared = Arc::clone(&shared);
        let thread_stop = Arc::clone(&stop);
        let handle = std::thread::spawn(move || tick_loop(&thread_shared, probe, &thread_stop));

        Arc::new(Self {
            shared,
            tick_thread: Mutex::new(Some(handle)),
            stop,
            persisted: AtomicBool::new(false),
        })
    }

    /// Stop and join the sampling thread, then persist the learned profile
    /// and the run's governor events (best-effort). Idempotent; also runs
    /// on drop.
    pub fn shutdown(&self) {
        self.stop.store(true, Ordering::Relaxed);
        if let Some(handle) = self.tick_thread.lock().unwrap().take() {
            let _ = handle.join();
        }
        self.persist();
    }

    /// Register a push unit that is about to spawn its `git push`.
    /// Dropping the guard deregisters the unit and folds its observed
    /// peak + wall time into the run's profile aggregate.
    pub fn begin_unit(&self, branch: &str) -> UnitGuard {
        self.shared.units.lock().unwrap().push(UnitEntry {
            branch: branch.to_string(),
            pid: None,
            started: Instant::now(),
            peak_rss: 0,
        });
        UnitGuard {
            shared: Arc::clone(&self.shared),
            branch: branch.to_string(),
        }
    }

    /// Fold the run into the stored profile and append the event log.
    /// Best-effort by contract; a cancelled run teaches nothing (truncated
    /// wall times, half-done units).
    fn persist(&self) {
        if self.persisted.swap(true, Ordering::Relaxed) {
            return;
        }
        if self.shared.cancel.is_cancelled() {
            return;
        }
        let Some(persistence) = &self.shared.profiles else {
            return;
        };
        let completed = std::mem::take(&mut *self.shared.completed.lock().unwrap());
        if !completed.is_empty() {
            let mut profile = persistence.prior;
            for (peak_rss, wall_ms) in &completed {
                profile = Some(HookProfile::fold(profile, *peak_rss, *wall_ms));
            }
            if let Some(folded) = profile {
                persistence.store.save(&HookProfileRow {
                    repo_hash: persistence.key.repo_hash.clone(),
                    stage: persistence.key.stage.clone(),
                    hook_hash: persistence.key.hook_hash.clone(),
                    peak_rss_bytes: folded.peak_rss,
                    wall_ms: folded.wall_ms,
                    runs: folded.runs,
                    updated_at: chrono::Utc::now(),
                });
            }
        }
        let held = std::mem::take(&mut self.shared.throttle.lock().unwrap().held);
        let events: Vec<GovernorEventRow> = held
            .iter()
            .map(|(branch, held_for)| GovernorEventRow {
                id: None,
                repo_hash: persistence.key.repo_hash.clone(),
                occurred_at: chrono::Utc::now(),
                kind: "throttle".into(),
                branch: Some(branch.clone()),
                detail_ms: Some(u64::try_from(held_for.as_millis()).unwrap_or(u64::MAX)),
                rss_bytes: None,
            })
            .collect();
        persistence.store.record_events(&events);
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
        let removed = {
            let mut units = self.shared.units.lock().unwrap();
            units
                .iter()
                .position(|u| u.branch == self.branch)
                .map(|pos| units.remove(pos))
        };
        if let Some(unit) = removed {
            let wall_ms = u64::try_from(unit.started.elapsed().as_millis()).unwrap_or(u64::MAX);
            self.shared
                .completed
                .lock()
                .unwrap()
                .push((unit.peak_rss, wall_ms));
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

        // Stage 2 sampler: fold each live unit's tree-RSS into its peak
        // and the run's live maximum. Pids are snapshotted first so the
        // (possibly milliseconds-long) walk runs without the units lock.
        let pids: Vec<u32> = shared
            .units
            .lock()
            .unwrap()
            .iter()
            .filter_map(|u| u.pid)
            .collect();
        if pids.is_empty() {
            continue;
        }
        let readings = probe.tree_rss(&pids);
        let mut units = shared.units.lock().unwrap();
        for (pid, reading) in pids.iter().zip(readings) {
            let Some(bytes) = reading else { continue };
            if let Some(unit) = units.iter_mut().find(|u| u.pid == Some(*pid)) {
                unit.peak_rss = unit.peak_rss.max(bytes);
            }
            shared.live_peak.fetch_max(bytes, Ordering::Relaxed);
        }
    }
}

impl DagGovernor for SyncGovernor {
    fn try_admit(&self, task: &SyncTask) -> AdmitDecision {
        let TaskId::Push(branch) = &task.id else {
            return AdmitDecision::Admit;
        };
        let running = self.shared.admitted.load(Ordering::Relaxed);
        let sample = *self.shared.latest.lock().unwrap();
        let now_ms = self.shared.now_ms();
        // Predicted peak: the learned profile wins; otherwise the highest
        // tree-RSS any unit has reached this run; otherwise the domain's
        // conservative default.
        let predicted_peak = {
            let live = self.shared.live_peak.load(Ordering::Relaxed);
            self.shared.profile_peak.or((live > 0).then_some(live))
        };
        let decision = self.shared.controller.lock().unwrap().try_admit(
            now_ms,
            running,
            &sample,
            predicted_peak,
        );
        match decision {
            Ok(()) => {
                self.shared.admitted.fetch_add(1, Ordering::Relaxed);
                // Close the branch's throttle window for the event log.
                let mut log = self.shared.throttle.lock().unwrap();
                if let Some(since) = log.since.remove(branch) {
                    log.held.push((branch.clone(), since.elapsed()));
                }
                AdmitDecision::Admit
            }
            Err(reason) => {
                self.shared
                    .throttle
                    .lock()
                    .unwrap()
                    .since
                    .entry(branch.clone())
                    .or_insert_with(Instant::now);
                AdmitDecision::Defer(match reason {
                    HoldReason::AtCap => DeferReason::ClassCap,
                    HoldReason::KillCooldown => DeferReason::KillCooldown,
                    HoldReason::AtTarget | HoldReason::Memory | HoldReason::Stagger => {
                        DeferReason::MemoryPressure
                    }
                })
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

        fn tree_rss(&self, roots: &[u32]) -> Vec<Option<u64>> {
            vec![None; roots.len()]
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
            None,
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
            None,
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
            None,
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

    /// Shareable fake profile store; the test keeps a handle to inspect
    /// what the governor persisted.
    #[derive(Default)]
    struct FakeProfiles {
        loaded: Mutex<Option<HookProfileRow>>,
        saved: Mutex<Option<HookProfileRow>>,
        events: Mutex<Vec<GovernorEventRow>>,
    }

    struct FakeProfileStore(Arc<FakeProfiles>);

    impl ProfileStore for FakeProfileStore {
        fn load(&self, _key: &ProfileKey) -> Option<HookProfileRow> {
            self.0.loaded.lock().unwrap().clone()
        }
        fn save(&self, row: &HookProfileRow) {
            *self.0.saved.lock().unwrap() = Some(row.clone());
        }
        fn record_events(&self, events: &[GovernorEventRow]) {
            self.0.events.lock().unwrap().extend_from_slice(events);
        }
    }

    fn key() -> ProfileKey {
        ProfileKey {
            repo_hash: "r".into(),
            stage: "pre-push".into(),
            hook_hash: "h".into(),
        }
    }

    fn profile_row(peak_rss_bytes: u64, wall_ms: u64, runs: u32) -> HookProfileRow {
        HookProfileRow {
            repo_hash: "r".into(),
            stage: "pre-push".into(),
            hook_hash: "h".into(),
            peak_rss_bytes,
            wall_ms,
            runs,
            updated_at: chrono::Utc::now(),
        }
    }

    #[test]
    fn heavy_profile_blocks_admission_without_headroom() {
        let profiles = Arc::new(FakeProfiles::default());
        // Learned peak 10 GiB; the machine has 8 GiB available over a
        // 2 GiB reserve — green, under target, but the hook can't fit.
        *profiles.loaded.lock().unwrap() = Some(profile_row(10 * GIB, 240_000, 2));
        let sample = ResourceSample {
            mem_total: 32 * GIB,
            mem_available: 8 * GIB,
            swap_used: 0,
            psi_some_avg10: None,
        };
        let governor = SyncGovernor::spawn(
            Box::new(FixedProbe(sample)),
            Some((Box::new(FakeProfileStore(Arc::clone(&profiles))), key())),
            Arc::new(CancelFlag::new()),
            |_| GovernorParams::new(4, 2 * GIB),
        );
        // Liveness: the first unit always admits.
        assert_eq!(governor.try_admit(&push_task("a")), AdmitDecision::Admit);
        // The second cannot fit a 10 GiB predicted peak into 6 GiB headroom.
        assert_eq!(
            governor.try_admit(&push_task("b")),
            AdmitDecision::Defer(DeferReason::MemoryPressure)
        );
        governor.shutdown();
    }

    #[test]
    fn shutdown_persists_folded_profile_and_throttle_events() {
        let profiles = Arc::new(FakeProfiles::default());
        *profiles.loaded.lock().unwrap() = Some(profile_row(10 * GIB, 240_000, 2));
        let sample = ResourceSample {
            mem_total: 32 * GIB,
            mem_available: 8 * GIB,
            swap_used: 0,
            psi_some_avg10: None,
        };
        let governor = SyncGovernor::spawn(
            Box::new(FixedProbe(sample)),
            Some((Box::new(FakeProfileStore(Arc::clone(&profiles))), key())),
            Arc::new(CancelFlag::new()),
            |_| GovernorParams::new(4, 2 * GIB),
        );
        // One unit admits and runs…
        assert_eq!(governor.try_admit(&push_task("a")), AdmitDecision::Admit);
        let guard = governor.begin_unit("a");
        // …the second is deferred while "a" holds its slot (6 GiB headroom
        // cannot fit the 10 GiB learned peak), opening a throttle window.
        assert!(matches!(
            governor.try_admit(&push_task("b")),
            AdmitDecision::Defer(_)
        ));
        std::thread::sleep(Duration::from_millis(5));
        // "a" completes; at zero running units the liveness rule admits
        // "b", closing its throttle window.
        drop(guard);
        governor.release(&push_task("a"));
        assert_eq!(governor.try_admit(&push_task("b")), AdmitDecision::Admit);
        governor.shutdown();

        let saved = profiles
            .saved
            .lock()
            .unwrap()
            .clone()
            .expect("profile saved");
        // One completed unit folded onto the prior (runs 2 → 3); the
        // sampler saw no RSS (no pid), so the peak decayed 10%.
        assert_eq!(saved.runs, 3);
        assert_eq!(saved.peak_rss_bytes, 10 * GIB - GIB);
        let events = profiles.events.lock().unwrap();
        assert_eq!(events.len(), 1, "one throttle window was closed");
        assert_eq!(events[0].kind, "throttle");
        assert_eq!(events[0].branch.as_deref(), Some("b"));
    }

    #[test]
    fn cancelled_run_persists_nothing() {
        let profiles = Arc::new(FakeProfiles::default());
        let cancel = Arc::new(CancelFlag::new());
        let governor = SyncGovernor::spawn(
            Box::new(FixedProbe(green_sample())),
            Some((Box::new(FakeProfileStore(Arc::clone(&profiles))), key())),
            Arc::clone(&cancel),
            |_| GovernorParams::new(4, 2 * GIB),
        );
        let guard = governor.begin_unit("a");
        drop(guard);
        cancel.escalate();
        governor.shutdown();
        assert!(profiles.saved.lock().unwrap().is_none());
        assert!(profiles.events.lock().unwrap().is_empty());
    }

    #[test]
    fn shutdown_joins_the_tick_thread() {
        let governor = SyncGovernor::spawn(
            Box::new(FixedProbe(green_sample())),
            None,
            Arc::new(CancelFlag::new()),
            |_| GovernorParams::new(2, 2 * GIB),
        );
        governor.shutdown();
        assert!(governor.tick_thread.lock().unwrap().is_none());
        // Idempotent.
        governor.shutdown();
    }
}
