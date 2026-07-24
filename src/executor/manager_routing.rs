//! Routes recognized hook-manager output into first-class job rows (#753).
//!
//! Path A runs the pre-push hook inside `git push` and reports it as one
//! synthetic `pre-push` job — a single opaque row for what may be a whole
//! lefthook run. [`ManagerRoutingPresenter`] wraps the real presenter at the
//! construction site and rewrites that story at the event level:
//!
//! - The synthetic `on_job_start("pre-push")` is **held**, not forwarded.
//! - Each `on_job_output("pre-push", line)` feeds the manager
//!   [`Detector`]. If a manager engages, its jobs become ordinary presenter
//!   events — `on_job_start`/`on_job_output`/`on_job_skipped`/
//!   `on_job_success`/`on_job_failure` with the *manager's* job names — and
//!   the synthetic job never materializes. Every renderer (rail, verbose
//!   thread, block, TUI) gets first-class rows for free.
//! - If every recognizer declines, the held start and the withheld lines are
//!   replayed in arrival order and the stream passes through untouched:
//!   unrecognized hooks render byte-identically to today. The only visible
//!   difference on the declined path is that the synthetic row appears with
//!   the hook's first output line (or its verdict) instead of at spawn time —
//!   phase-level liveness (the gate's section header) is unaffected.
//! - The synthetic verdict (`on_job_success|failure("pre-push")`) becomes the
//!   **reconciliation signal**: recognized jobs the summary never resolved
//!   (manager killed mid-run, summary suppressed) settle with the phase
//!   verdict, so no row is left spinning. When the manager's summary already
//!   resolved every job, the synthetic verdict is swallowed — a push that
//!   fails *after* the hook passed (non-fast-forward) must not paint the
//!   hook's jobs red (#752's trap). Exit policy is untouched either way:
//!   `HookVerdict` derives from `PushIo` + porcelain, never from presenter
//!   events.
//!
//! Concurrency: `git push`'s stdout and stderr are drained by two threads
//! sharing one tee. Feeding the detector and emitting to the inner presenter
//! happen under a single mutex so translated events cannot reorder (a job's
//! output must not race ahead of its start). Inner presenter locks are leaf
//! locks; nothing re-enters this wrapper.

use crate::executor::JobResult;
use crate::executor::presenter::JobPresenter;
use crate::hooks::manager_output::{Detector, DetectorEnd, DetectorStep, ManagerEvent};
use crate::settings::HookOutputConfig;
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::Duration;

/// The synthetic gate job Path A emits (`push_with_hooks`): phase and job are
/// both named `pre-push`, and it is the only job on the gate path.
const GATE_JOB: &str = "pre-push";

/// The held synthetic `on_job_start`, replayed verbatim on decline.
struct HeldStart {
    name: String,
    description: Option<String>,
    command_preview: Option<String>,
}

struct RoutingState {
    detector: Detector,
    held_start: Option<HeldStart>,
    engaged: bool,
    declined: bool,
    /// Every job row that has appeared: the config-seeded roster (#753) plus
    /// any stream-only job. The census counts these; the sweep settles the
    /// ones the summary never resolved.
    started: Vec<String>,
    /// Jobs seeded up front from the manager's config (a subset of `started`).
    /// Kept distinct so a seeded-but-never-run job settles as skipped instead
    /// of borrowing a passing hook's verdict.
    seeded: Vec<String>,
    /// Jobs the stream actually mentioned running (a block header flushed). A
    /// seeded job absent here never ran — a glob/condition skip or an
    /// over-listing config.
    appeared: Vec<String>,
    /// Jobs the manager's summary (or a skip notice) has resolved.
    resolved: Vec<String>,
}

impl RoutingState {
    fn fresh() -> Self {
        Self {
            detector: Detector::new(),
            held_start: None,
            engaged: false,
            declined: false,
            started: Vec::new(),
            seeded: Vec::new(),
            appeared: Vec::new(),
            resolved: Vec::new(),
        }
    }
}

/// Wraps a presenter, turning a recognized manager's gate stream into
/// first-class job events. See the module docs for the contract.
pub struct ManagerRoutingPresenter {
    inner: Arc<dyn JobPresenter>,
    state: Mutex<RoutingState>,
    /// The pushing worktree's root, when the call site can supply it — the
    /// directory whose lefthook config names the jobs to seed (#753). `None`
    /// disables seeding: jobs then reveal as the manager completes them.
    roster_dir: Option<PathBuf>,
}

impl ManagerRoutingPresenter {
    /// Wrap without a roster seed — jobs reveal as the manager completes them.
    pub fn wrap(inner: Arc<dyn JobPresenter>) -> Arc<Self> {
        Self::wrap_seeded(inner, None)
    }

    /// Wrap, seeding the job roster from `roster_dir` when one is given.
    pub fn wrap_seeded(inner: Arc<dyn JobPresenter>, roster_dir: Option<PathBuf>) -> Arc<Self> {
        Arc::new(Self {
            inner,
            state: Mutex::new(RoutingState::fresh()),
            roster_dir,
        })
    }

    /// Wrap `presenter` when `daft.hooks.output.parseManagers` allows it,
    /// seeding the job roster from `roster_dir` when one is given. The knob is
    /// the kill switch back to today's synthetic-job rendering.
    pub fn wrap_if_enabled(
        config: &HookOutputConfig,
        roster_dir: Option<&Path>,
        presenter: Option<Arc<dyn JobPresenter>>,
    ) -> Option<Arc<dyn JobPresenter>> {
        Self::wrap_when(config.parse_managers, roster_dir, presenter)
    }

    /// [`Self::wrap_if_enabled`] for call sites that carry the resolved knob
    /// as a bare flag (sync's task workers thread it into their closures).
    pub fn wrap_when(
        enabled: bool,
        roster_dir: Option<&Path>,
        presenter: Option<Arc<dyn JobPresenter>>,
    ) -> Option<Arc<dyn JobPresenter>> {
        match presenter {
            Some(inner) if enabled => {
                Some(Self::wrap_seeded(inner, roster_dir.map(Path::to_path_buf)))
            }
            other => other,
        }
    }

    /// Emit the held synthetic start (if any) — the moment a stream turns
    /// out not to be a manager's, the gate row materializes exactly as
    /// `push_with_hooks` announced it.
    fn flush_held(&self, state: &mut RoutingState) {
        if let Some(held) = state.held_start.take() {
            self.inner.on_job_start(
                &held.name,
                held.description.as_deref(),
                held.command_preview.as_deref(),
            );
        }
    }

    /// Seed the manager's job roster as pending rows the moment it engages, so
    /// long-running jobs are visible before they complete (#753). lefthook's
    /// buffered output reveals a job only when it *finishes*, so without this a
    /// slow job would be invisible for its whole run. Display-only: the rows
    /// resolve from the stream (or the sweep), and the push verdict stays
    /// `PushIo`-derived. No-op without a `roster_dir` or when the config names
    /// nothing (an unreadable/unknown config → the stream reveals jobs as
    /// before).
    fn seed_roster(&self, state: &mut RoutingState, manager: &str, hook: Option<&str>) {
        let Some(dir) = self.roster_dir.as_deref() else {
            return;
        };
        let roster = crate::hooks::manager_output::roster(manager, dir, hook.unwrap_or_default());
        if roster.is_empty() {
            return;
        }
        // One grow-only width seed for the whole roster, then a live row each.
        self.inner.on_jobs_planned(&roster);
        for name in roster {
            self.inner.on_job_start(&name, None, None);
            state.started.push(name.clone());
            state.seeded.push(name);
        }
    }

    fn translate(&self, state: &mut RoutingState, events: Vec<ManagerEvent>) {
        for event in events {
            match event {
                ManagerEvent::Engaged {
                    manager,
                    version,
                    hook,
                } => {
                    state.engaged = true;
                    // The synthetic job never materializes on the engaged
                    // path; the manager's own jobs are the story now.
                    state.held_start = None;
                    self.inner
                        .on_manager_engaged(None, manager, version.as_deref());
                    self.seed_roster(state, manager, hook.as_deref());
                }
                ManagerEvent::JobStarted { name } => {
                    // A block header flushed: the job ran. Record that so the
                    // sweep can tell run-but-unresolved from never-run.
                    state.appeared.push(name.clone());
                    // A seeded job already has its live row — the flush means
                    // it finished (its output follows), not a second start.
                    if !state.seeded.contains(&name) {
                        state.started.push(name.clone());
                        // Grow-only width seeding: receipts persist
                        // immediately, so renderers must learn the widest name
                        // as soon as it is known, not after it resolves.
                        self.inner.on_jobs_planned(&state.started);
                        self.inner.on_job_start(&name, None, None);
                    }
                    // In lefthook's default (buffered) mode the block flushes
                    // when the job completes, so this is a real-time "finished
                    // running" signal: settle the row to the neutral grey `✓`
                    // now, ahead of the summary's confirmed verdict. (Under
                    // `follow: true` the header prints at job start — the row
                    // settles early and the summary self-corrects.)
                    self.inner.on_manager_job_flushed(&name);
                }
                ManagerEvent::JobOutput { name, line } => {
                    self.inner.on_job_output(&name, &line);
                }
                ManagerEvent::JobSkipped { name, reason } => {
                    self.inner
                        .on_job_skipped(&name, &reason, Duration::ZERO, false, None);
                    // Settled: the sweep must not re-resolve it (it may be in
                    // the seeded roster).
                    state.resolved.push(name);
                }
                ManagerEvent::JobResolved { name, ok, duration } => {
                    state.resolved.push(name.clone());
                    let duration = duration.unwrap_or(Duration::ZERO);
                    if ok {
                        self.inner.on_job_success(&name, duration);
                    } else {
                        self.inner.on_job_failure(&name, duration);
                    }
                }
                // The rail's section close renders from the real
                // `on_phase_complete`; the manager's own total is surfaced
                // by the engaged annotation (separate change).
                ManagerEvent::PhaseDone { .. } => {}
            }
        }
    }

    /// A gate verdict arrived. Returns `true` when the verdict was consumed
    /// (engaged path) and must not be forwarded as the synthetic job's.
    fn settle_gate(&self, verdict: GateVerdict) -> bool {
        let mut state = self.state.lock().expect("routing state poisoned");
        // A stream that never produced enough output to decide (or produced
        // none at all) settles as declined: held start + withheld lines out,
        // exactly as announced.
        if !state.engaged && !state.declined {
            match state.detector.finish() {
                DetectorEnd::Engaged(events) => self.translate(&mut state, events),
                DetectorEnd::Undecided { replay } => {
                    state.declined = true;
                    self.flush_held(&mut state);
                    for line in replay {
                        self.inner.on_job_output(GATE_JOB, &line);
                    }
                }
                DetectorEnd::Declined => {}
            }
        }
        if !state.engaged {
            return false;
        }
        // Reconcile: jobs the summary never resolved settle with the phase
        // verdict so no row is left spinning. Jobs the summary already
        // resolved keep their own outcome — a push that dies after the hook
        // passed must not repaint the hook's jobs (#752).
        let unresolved: Vec<String> = state
            .started
            .iter()
            .filter(|name| !state.resolved.contains(name))
            .cloned()
            .collect();
        for name in unresolved {
            // A seeded job the stream never mentioned did not run (a glob or
            // condition skip that printed no notice, or an over-listing
            // config). On a passing hook it settles as skipped rather than
            // borrowing the green; on a failed or cancelled phase every
            // unresolved row follows the verdict, since a job in flight when
            // the manager died has no known outcome.
            let never_ran = state.seeded.contains(&name) && !state.appeared.contains(&name);
            match verdict {
                GateVerdict::Success if never_ran => {
                    self.inner
                        .on_job_skipped(&name, "not run", Duration::ZERO, false, None);
                }
                GateVerdict::Success => self.inner.on_job_success(&name, Duration::ZERO),
                GateVerdict::Failure => self.inner.on_job_failure(&name, Duration::ZERO),
                GateVerdict::Cancelled => self.inner.on_job_cancelled(&name, Duration::ZERO),
            }
            state.resolved.push(name);
        }
        true
    }
}

#[derive(Clone, Copy)]
enum GateVerdict {
    Success,
    Failure,
    Cancelled,
}

// ─────────────────────────────────────────────────────────────────────────
// Lifecycle nesting: a manager running INSIDE a lifecycle job
// ─────────────────────────────────────────────────────────────────────────

/// Per-parent recognition state for [`LifecycleRoutingPresenter`].
struct NestState {
    detector: Detector,
    /// Recognized child names in appearance order.
    started: Vec<String>,
    /// Children the manager's summary has resolved.
    resolved: Vec<String>,
}

impl NestState {
    fn fresh() -> Self {
        Self {
            detector: Detector::new(),
            started: Vec::new(),
            resolved: Vec::new(),
        }
    }
}

/// Wraps a lifecycle presenter, annotating jobs whose output turns out to be
/// a hook manager's with nested child sub-structure (#753).
///
/// Unlike the gate wrapper this is a pure tee: **every** raw line is
/// forwarded to `on_job_output` under its parent exactly as today — the
/// parent's buffers, verbose threads, and failure dumps are untouched, and a
/// declined stream needs no replay because nothing was ever withheld. On
/// engagement the manager's jobs surface *additionally* as `on_child_job_*`
/// events under the parent (`on_manager_engaged(Some(parent), …)` announces
/// whose they are). Children never synthesize `JobResult`s — outcome policy
/// stays the parent's — and children the summary never resolved settle with
/// the parent's own verdict so no row outlives its parent.
pub struct LifecycleRoutingPresenter {
    inner: Arc<dyn JobPresenter>,
    /// Per-parent detectors; one mutex orders feed + child emission across
    /// the runner's per-job reader threads.
    state: Mutex<HashMap<String, NestState>>,
}

impl LifecycleRoutingPresenter {
    pub fn wrap(inner: Arc<dyn JobPresenter>) -> Arc<Self> {
        Arc::new(Self {
            inner,
            state: Mutex::new(HashMap::new()),
        })
    }

    /// Wrap when `daft.hooks.output.parseManagers` allows it — the same
    /// kill switch as the gate path.
    pub fn wrap_when(enabled: bool, presenter: Arc<dyn JobPresenter>) -> Arc<dyn JobPresenter> {
        if enabled {
            Self::wrap(presenter)
        } else {
            presenter
        }
    }

    /// Settle a parent's unresolved children with the parent's verdict.
    fn settle_children(&self, parent: &str, verdict: GateVerdict) {
        let mut state = self.state.lock().expect("lifecycle routing poisoned");
        let Some(nest) = state.get_mut(parent) else {
            return;
        };
        let unresolved: Vec<String> = nest
            .started
            .iter()
            .filter(|name| !nest.resolved.contains(name))
            .cloned()
            .collect();
        for name in unresolved {
            match verdict {
                GateVerdict::Success => {
                    self.inner
                        .on_child_job_success(parent, &name, Duration::ZERO);
                }
                GateVerdict::Failure => {
                    self.inner
                        .on_child_job_failure(parent, &name, Duration::ZERO);
                }
                GateVerdict::Cancelled => {
                    self.inner
                        .on_child_job_cancelled(parent, &name, Duration::ZERO);
                }
            }
            nest.resolved.push(name);
        }
    }
}

impl JobPresenter for LifecycleRoutingPresenter {
    fn on_phase_start(&self, phase_name: &str, target: Option<&str>) {
        self.state
            .lock()
            .expect("lifecycle routing poisoned")
            .clear();
        self.inner.on_phase_start(phase_name, target);
    }

    fn on_job_start(&self, name: &str, description: Option<&str>, command_preview: Option<&str>) {
        self.inner.on_job_start(name, description, command_preview);
    }

    fn on_job_output(&self, name: &str, line: &str) {
        // The raw line always reaches the parent first — evidence stays
        // parent-scoped and byte-identical to an unwrapped run.
        let mut state = self.state.lock().expect("lifecycle routing poisoned");
        self.inner.on_job_output(name, line);
        let nest = state
            .entry(name.to_string())
            .or_insert_with(NestState::fresh);
        match nest.detector.feed(line) {
            // Declined replay is redundant here: nothing was withheld.
            DetectorStep::Buffering | DetectorStep::Passthrough | DetectorStep::Declined { .. } => {
            }
            DetectorStep::Events(events) => {
                for event in events {
                    match event {
                        ManagerEvent::Engaged {
                            manager, version, ..
                        } => {
                            self.inner
                                .on_manager_engaged(Some(name), manager, version.as_deref());
                        }
                        ManagerEvent::JobStarted { name: child } => {
                            nest.started.push(child.clone());
                            self.inner.on_child_job_start(name, &child);
                        }
                        ManagerEvent::JobResolved {
                            name: child,
                            ok,
                            duration,
                        } => {
                            nest.resolved.push(child.clone());
                            let duration = duration.unwrap_or(Duration::ZERO);
                            if ok {
                                self.inner.on_child_job_success(name, &child, duration);
                            } else {
                                self.inner.on_child_job_failure(name, &child, duration);
                            }
                        }
                        // Child lines already reached the parent raw; skips
                        // and totals stay parent-level noise here.
                        ManagerEvent::JobOutput { .. }
                        | ManagerEvent::JobSkipped { .. }
                        | ManagerEvent::PhaseDone { .. } => {}
                    }
                }
            }
        }
    }

    fn on_job_success(&self, name: &str, duration: Duration) {
        self.settle_children(name, GateVerdict::Success);
        self.inner.on_job_success(name, duration);
    }

    fn on_job_failure(&self, name: &str, duration: Duration) {
        self.settle_children(name, GateVerdict::Failure);
        self.inner.on_job_failure(name, duration);
    }

    fn on_job_failure_with_exit(&self, name: &str, duration: Duration, exit_code: Option<i32>) {
        self.settle_children(name, GateVerdict::Failure);
        self.inner
            .on_job_failure_with_exit(name, duration, exit_code);
    }

    fn on_job_skipped(
        &self,
        name: &str,
        reason: &str,
        duration: Duration,
        show_duration: bool,
        command_preview: Option<&str>,
    ) {
        self.inner
            .on_job_skipped(name, reason, duration, show_duration, command_preview);
    }

    fn on_job_cancelled(&self, name: &str, duration: Duration) {
        self.settle_children(name, GateVerdict::Cancelled);
        self.inner.on_job_cancelled(name, duration);
    }

    fn on_job_background(&self, name: &str, description: Option<&str>) {
        self.inner.on_job_background(name, description);
    }

    fn on_message(&self, msg: &str) {
        self.inner.on_message(msg);
    }

    fn on_jobs_planned(&self, names: &[String]) {
        self.inner.on_jobs_planned(names);
    }

    fn on_manager_engaged(&self, scope: Option<&str>, manager: &str, version: Option<&str>) {
        self.inner.on_manager_engaged(scope, manager, version);
    }

    fn on_child_job_start(&self, parent: &str, name: &str) {
        self.inner.on_child_job_start(parent, name);
    }

    fn on_child_job_success(&self, parent: &str, name: &str, duration: Duration) {
        self.inner.on_child_job_success(parent, name, duration);
    }

    fn on_child_job_failure(&self, parent: &str, name: &str, duration: Duration) {
        self.inner.on_child_job_failure(parent, name, duration);
    }

    fn on_child_job_cancelled(&self, parent: &str, name: &str, duration: Duration) {
        self.inner.on_child_job_cancelled(parent, name, duration);
    }

    fn on_phase_complete(&self, total_duration: Duration) {
        self.inner.on_phase_complete(total_duration);
    }

    fn take_results(&self) -> Vec<JobResult> {
        self.inner.take_results()
    }
}

impl JobPresenter for ManagerRoutingPresenter {
    fn on_phase_start(&self, phase_name: &str, target: Option<&str>) {
        // One detector serves one stream: a presenter reused across phases
        // (multi_remote pushes to several remotes sequentially) starts each
        // phase undecided again.
        *self.state.lock().expect("routing state poisoned") = RoutingState::fresh();
        self.inner.on_phase_start(phase_name, target);
    }

    fn on_job_start(&self, name: &str, description: Option<&str>, command_preview: Option<&str>) {
        if name == GATE_JOB {
            let mut state = self.state.lock().expect("routing state poisoned");
            state.held_start = Some(HeldStart {
                name: name.to_string(),
                description: description.map(str::to_string),
                command_preview: command_preview.map(str::to_string),
            });
            return;
        }
        self.inner.on_job_start(name, description, command_preview);
    }

    fn on_job_output(&self, name: &str, line: &str) {
        if name != GATE_JOB {
            self.inner.on_job_output(name, line);
            return;
        }
        let mut state = self.state.lock().expect("routing state poisoned");
        match state.detector.feed(line) {
            DetectorStep::Buffering => {}
            DetectorStep::Declined { replay } => {
                state.declined = true;
                self.flush_held(&mut state);
                for withheld in replay {
                    self.inner.on_job_output(GATE_JOB, &withheld);
                }
            }
            DetectorStep::Passthrough => self.inner.on_job_output(GATE_JOB, line),
            DetectorStep::Events(events) => self.translate(&mut state, events),
        }
    }

    fn on_job_success(&self, name: &str, duration: Duration) {
        if name == GATE_JOB && self.settle_gate(GateVerdict::Success) {
            return;
        }
        self.inner.on_job_success(name, duration);
    }

    fn on_job_failure(&self, name: &str, duration: Duration) {
        if name == GATE_JOB && self.settle_gate(GateVerdict::Failure) {
            return;
        }
        self.inner.on_job_failure(name, duration);
    }

    fn on_job_failure_with_exit(&self, name: &str, duration: Duration, exit_code: Option<i32>) {
        if name == GATE_JOB && self.settle_gate(GateVerdict::Failure) {
            return;
        }
        self.inner
            .on_job_failure_with_exit(name, duration, exit_code);
    }

    fn on_job_skipped(
        &self,
        name: &str,
        reason: &str,
        duration: Duration,
        show_duration: bool,
        command_preview: Option<&str>,
    ) {
        self.inner
            .on_job_skipped(name, reason, duration, show_duration, command_preview);
    }

    fn on_job_cancelled(&self, name: &str, duration: Duration) {
        if name == GATE_JOB && self.settle_gate(GateVerdict::Cancelled) {
            return;
        }
        self.inner.on_job_cancelled(name, duration);
    }

    fn on_job_background(&self, name: &str, description: Option<&str>) {
        self.inner.on_job_background(name, description);
    }

    fn on_message(&self, msg: &str) {
        self.inner.on_message(msg);
    }

    fn on_jobs_planned(&self, names: &[String]) {
        self.inner.on_jobs_planned(names);
    }

    fn on_phase_complete(&self, total_duration: Duration) {
        self.inner.on_phase_complete(total_duration);
    }

    fn take_results(&self) -> Vec<JobResult> {
        self.inner.take_results()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Mutex as StdMutex;

    /// Records every event with enough shape to assert order and payloads.
    #[derive(Default)]
    struct Recording {
        events: StdMutex<Vec<String>>,
    }

    impl Recording {
        fn arc() -> Arc<Self> {
            Arc::new(Self::default())
        }
        fn log(&self, entry: String) {
            self.events.lock().unwrap().push(entry);
        }
        fn events(&self) -> Vec<String> {
            self.events.lock().unwrap().clone()
        }
    }

    impl JobPresenter for Recording {
        fn on_phase_start(&self, phase_name: &str, _target: Option<&str>) {
            self.log(format!("phase_start:{phase_name}"));
        }
        fn on_job_start(&self, name: &str, _d: Option<&str>, preview: Option<&str>) {
            self.log(format!("start:{name}:{}", preview.unwrap_or("-")));
        }
        fn on_job_output(&self, name: &str, line: &str) {
            self.log(format!("output:{name}:{line}"));
        }
        fn on_job_success(&self, name: &str, _duration: Duration) {
            self.log(format!("success:{name}"));
        }
        fn on_job_failure(&self, name: &str, _duration: Duration) {
            self.log(format!("failure:{name}"));
        }
        fn on_job_skipped(
            &self,
            name: &str,
            reason: &str,
            _duration: Duration,
            _show: bool,
            _preview: Option<&str>,
        ) {
            self.log(format!("skipped:{name}:{reason}"));
        }
        fn on_job_cancelled(&self, name: &str, _duration: Duration) {
            self.log(format!("cancelled:{name}"));
        }
        fn on_job_background(&self, name: &str, _description: Option<&str>) {
            self.log(format!("background:{name}"));
        }
        fn on_message(&self, msg: &str) {
            self.log(format!("message:{msg}"));
        }
        fn on_jobs_planned(&self, names: &[String]) {
            self.log(format!("planned:{}", names.join(",")));
        }
        fn on_manager_engaged(&self, scope: Option<&str>, manager: &str, version: Option<&str>) {
            self.log(format!(
                "manager_engaged:{}:{manager}:{}",
                scope.unwrap_or("-"),
                version.unwrap_or("-")
            ));
        }
        fn on_manager_job_flushed(&self, name: &str) {
            self.log(format!("flushed:{name}"));
        }
        fn on_child_job_start(&self, parent: &str, name: &str) {
            self.log(format!("child_start:{parent}:{name}"));
        }
        fn on_child_job_success(&self, parent: &str, name: &str, _duration: Duration) {
            self.log(format!("child_success:{parent}:{name}"));
        }
        fn on_child_job_failure(&self, parent: &str, name: &str, _duration: Duration) {
            self.log(format!("child_failure:{parent}:{name}"));
        }
        fn on_child_job_cancelled(&self, parent: &str, name: &str, _duration: Duration) {
            self.log(format!("child_cancelled:{parent}:{name}"));
        }
        fn on_phase_complete(&self, _total: Duration) {
            self.log("phase_complete".to_string());
        }
        fn take_results(&self) -> Vec<JobResult> {
            Vec::new()
        }
    }

    const BANNER: &str = "│ 🥊 lefthook  v2.1.10   hook:  pre-push │";

    fn gate_start(wrapper: &ManagerRoutingPresenter) {
        wrapper.on_phase_start("pre-push", Some("feat/x"));
        wrapper.on_job_start(GATE_JOB, None, Some("git push origin feat/x"));
    }

    #[test]
    fn an_engaged_stream_renders_manager_jobs_and_swallows_the_synthetic() {
        let recording = Recording::arc();
        let wrapper = ManagerRoutingPresenter::wrap(recording.clone());
        gate_start(&wrapper);
        for line in [
            "╭──────╮",
            BANNER,
            "╰──────╯",
            "┃  fmt ❯ ",
            "fmt output line",
            "",
            "  ────────────",
            "summary: (done in 0.4 seconds)",
            "✔️ fmt (0.36 seconds)",
        ] {
            wrapper.on_job_output(GATE_JOB, line);
        }
        wrapper.on_job_success(GATE_JOB, Duration::from_secs(1));
        wrapper.on_phase_complete(Duration::from_secs(1));

        assert_eq!(
            recording.events(),
            vec![
                "phase_start:pre-push".to_string(),
                // The engagement fact precedes every job event — renderers
                // fold the manager into the section header before the first
                // row appears.
                "manager_engaged:-:lefthook:2.1.10".to_string(),
                "planned:fmt".to_string(),
                "start:fmt:-".to_string(),
                // The block header flushed at completion (#753): a done-pending
                // signal, right after the row is revealed, ahead of the verdict.
                "flushed:fmt".to_string(),
                "output:fmt:fmt output line".to_string(),
                "output:fmt:".to_string(),
                "success:fmt".to_string(),
                "phase_complete".to_string(),
            ],
            "no synthetic pre-push row on the engaged path"
        );
    }

    #[test]
    fn a_declined_stream_is_byte_identical_to_today() {
        let recording = Recording::arc();
        let wrapper = ManagerRoutingPresenter::wrap(recording.clone());
        gate_start(&wrapper);
        wrapper.on_job_output(GATE_JOB, "Running pre-push checks...");
        wrapper.on_job_output(GATE_JOB, "All checks passed.");
        wrapper.on_job_success(GATE_JOB, Duration::from_secs(1));
        wrapper.on_phase_complete(Duration::from_secs(1));

        assert_eq!(
            recording.events(),
            vec![
                "phase_start:pre-push".to_string(),
                // The held start flushes with the first (declining) line —
                // same events, same order, same payloads as an unwrapped run.
                "start:pre-push:git push origin feat/x".to_string(),
                "output:pre-push:Running pre-push checks...".to_string(),
                "output:pre-push:All checks passed.".to_string(),
                "success:pre-push".to_string(),
                "phase_complete".to_string(),
            ]
        );
    }

    #[test]
    fn a_held_box_prefix_replays_in_arrival_order_on_decline() {
        let recording = Recording::arc();
        let wrapper = ManagerRoutingPresenter::wrap(recording.clone());
        gate_start(&wrapper);
        wrapper.on_job_output(GATE_JOB, "╭───╮");
        wrapper.on_job_output(GATE_JOB, "🔧 my custom pre-push");
        wrapper.on_job_output(GATE_JOB, "linting...");
        wrapper.on_job_failure(GATE_JOB, Duration::from_secs(1));

        assert_eq!(
            recording.events(),
            vec![
                "phase_start:pre-push".to_string(),
                "start:pre-push:git push origin feat/x".to_string(),
                "output:pre-push:╭───╮".to_string(),
                "output:pre-push:🔧 my custom pre-push".to_string(),
                "output:pre-push:linting...".to_string(),
                "failure:pre-push".to_string(),
            ]
        );
    }

    #[test]
    fn a_silent_stream_settles_as_declined_at_the_verdict() {
        // A hook that printed nothing: no output events ever arrive, so the
        // held start must flush when the verdict lands — the row exists.
        let recording = Recording::arc();
        let wrapper = ManagerRoutingPresenter::wrap(recording.clone());
        gate_start(&wrapper);
        wrapper.on_job_success(GATE_JOB, Duration::from_secs(1));

        assert_eq!(
            recording.events(),
            vec![
                "phase_start:pre-push".to_string(),
                "start:pre-push:git push origin feat/x".to_string(),
                "success:pre-push".to_string(),
            ]
        );
    }

    #[test]
    fn a_killed_manager_reconciles_unresolved_jobs_with_the_phase_verdict() {
        // Banner + blocks, no summary (manager SIGKILL'd): every appeared job
        // must settle with the gate verdict — no row left spinning.
        let recording = Recording::arc();
        let wrapper = ManagerRoutingPresenter::wrap(recording.clone());
        gate_start(&wrapper);
        for line in [BANNER, "┃  fmt ❯ ", "fmt ok", "┃  clippy ❯ "] {
            wrapper.on_job_output(GATE_JOB, line);
        }
        wrapper.on_job_failure(GATE_JOB, Duration::from_secs(1));

        let events = recording.events();
        assert!(events.contains(&"failure:fmt".to_string()), "{events:?}");
        assert!(events.contains(&"failure:clippy".to_string()), "{events:?}");
        assert!(
            !events.contains(&"failure:pre-push".to_string()),
            "synthetic verdict must be swallowed once engaged: {events:?}"
        );
    }

    #[test]
    fn a_push_rejected_after_a_passing_hook_repaints_no_job() {
        // #752's trap: hook passed (summary resolved everything), then the
        // push died non-fast-forward → the gate reports failure. Rows must
        // keep their real outcomes; the synthetic failure is swallowed.
        let recording = Recording::arc();
        let wrapper = ManagerRoutingPresenter::wrap(recording.clone());
        gate_start(&wrapper);
        for line in [
            BANNER,
            "┃  quick ❯ ",
            "quick done",
            "summary: (done in 10.06 seconds)",
            "✔️ quick (0.25 seconds)",
            "To /tmp/remote.git",
            " ! [rejected]        master -> master (non-fast-forward)",
        ] {
            wrapper.on_job_output(GATE_JOB, line);
        }
        wrapper.on_job_failure(GATE_JOB, Duration::from_secs(11));

        let events = recording.events();
        assert!(events.contains(&"success:quick".to_string()), "{events:?}");
        assert!(
            !events.iter().any(|e| e.starts_with("failure:")),
            "nothing may be repainted red by the post-hook rejection: {events:?}"
        );
        assert!(
            !events.iter().any(|e| e.contains("[rejected]")),
            "git's trailing lines are not job output: {events:?}"
        );
    }

    #[test]
    fn skip_notices_become_native_skipped_rows() {
        let recording = Recording::arc();
        let wrapper = ManagerRoutingPresenter::wrap(recording.clone());
        gate_start(&wrapper);
        for line in [
            BANNER,
            "│  biome-check (skip) no files for inspection",
            "┃  runs ❯ ",
            "summary: (done in 0.2 seconds)",
            "✔️ runs (0.17 seconds)",
        ] {
            wrapper.on_job_output(GATE_JOB, line);
        }
        wrapper.on_job_success(GATE_JOB, Duration::from_secs(1));

        assert!(
            recording
                .events()
                .contains(&"skipped:biome-check:no files for inspection".to_string()),
            "{:?}",
            recording.events()
        );
    }

    #[test]
    fn a_reused_presenter_starts_each_phase_undecided() {
        // multi_remote pushes to several remotes with one presenter: a
        // declined first phase must not leave the second phase passthrough.
        let recording = Recording::arc();
        let wrapper = ManagerRoutingPresenter::wrap(recording.clone());
        gate_start(&wrapper);
        wrapper.on_job_output(GATE_JOB, "plain hook output");
        wrapper.on_job_success(GATE_JOB, Duration::from_secs(1));

        gate_start(&wrapper);
        wrapper.on_job_output(GATE_JOB, BANNER);
        wrapper.on_job_output(GATE_JOB, "┃  fmt ❯ ");
        wrapper.on_job_output(GATE_JOB, "summary: (done in 0.1 seconds)");
        wrapper.on_job_output(GATE_JOB, "✔️ fmt (0.1 seconds)");
        wrapper.on_job_success(GATE_JOB, Duration::from_secs(1));

        let events = recording.events();
        assert!(events.contains(&"output:pre-push:plain hook output".to_string()));
        assert!(
            events.contains(&"success:fmt".to_string()),
            "second phase must engage on its own stream: {events:?}"
        );
    }

    #[test]
    fn the_knob_disables_wrapping_entirely() {
        let recording = Recording::arc();
        let mut config = HookOutputConfig::default();
        assert!(config.parse_managers, "default is on");
        config.parse_managers = false;
        let presenter = ManagerRoutingPresenter::wrap_if_enabled(
            &config,
            None,
            Some(recording.clone() as Arc<dyn JobPresenter>),
        )
        .expect("presenter preserved");
        // Not wrapped: the synthetic events flow through verbatim, banner
        // and all — today's rendering exactly.
        presenter.on_job_start(GATE_JOB, None, None);
        presenter.on_job_output(GATE_JOB, BANNER);
        assert_eq!(
            recording.events(),
            vec![
                format!("start:{GATE_JOB}:-"),
                format!("output:{GATE_JOB}:{BANNER}"),
            ]
        );
        assert!(ManagerRoutingPresenter::wrap_if_enabled(&config, None, None).is_none());
    }

    // ── roster seeding (#753) ─────────────────────────────────────────────

    /// A temp repo directory holding a lefthook config with the given
    /// `pre-push` command names, for the seeding tests.
    fn seed_dir(commands: &[&str]) -> tempfile::TempDir {
        let dir = tempfile::tempdir().expect("tempdir");
        let mut yaml = String::from("pre-push:\n  parallel: true\n  commands:\n");
        for name in commands {
            yaml.push_str(&format!("    {name}:\n      run: true\n"));
        }
        std::fs::write(dir.path().join("lefthook.yml"), yaml).expect("write config");
        dir
    }

    fn seeded_wrapper(
        inner: Arc<dyn JobPresenter>,
        dir: &tempfile::TempDir,
    ) -> Arc<ManagerRoutingPresenter> {
        ManagerRoutingPresenter::wrap_seeded(inner, Some(dir.path().to_path_buf()))
    }

    #[test]
    fn the_roster_seeds_every_job_the_moment_the_manager_engages() {
        // The headline fix: a long job (`slow`) is visible from engagement,
        // not only when its block finally flushes. Here only `fast` produces a
        // block before the summary; `slow` must still have a row from t=0.
        let recording = Recording::arc();
        let dir = seed_dir(&["fast", "slow"]);
        let wrapper = seeded_wrapper(recording.clone(), &dir);
        gate_start(&wrapper);
        for line in [
            BANNER,
            "┃  fast ❯ ",
            "fast done",
            "summary: (done in 5.0 seconds)",
            "✔️ fast (0.2 seconds)",
            "✔️ slow (4.9 seconds)",
        ] {
            wrapper.on_job_output(GATE_JOB, line);
        }
        wrapper.on_job_success(GATE_JOB, Duration::from_secs(5));

        let events = recording.events();
        let engaged = events
            .iter()
            .position(|e| e.starts_with("manager_engaged"))
            .expect("engaged");
        let fast_start = events
            .iter()
            .position(|e| e == "start:fast:-")
            .expect("fast row");
        let slow_start = events
            .iter()
            .position(|e| e == "start:slow:-")
            .expect("slow row");
        // Both rows appear right after engagement — before any block flush.
        let fast_output = events
            .iter()
            .position(|e| e == "output:fast:fast done")
            .expect("fast output");
        assert!(
            engaged < slow_start && slow_start < fast_output,
            "{events:?}"
        );
        assert!(
            engaged < fast_start && fast_start < fast_output,
            "{events:?}"
        );
        // The census sees both up front, and neither is started twice.
        assert!(
            events.contains(&"planned:fast,slow".to_string()),
            "{events:?}"
        );
        assert_eq!(
            events.iter().filter(|e| *e == "start:fast:-").count(),
            1,
            "a seeded job must not restart when its block flushes: {events:?}"
        );
        assert_eq!(
            events.iter().filter(|e| e.starts_with("success:")).count(),
            2
        );
    }

    #[test]
    fn a_block_flush_emits_the_done_pending_signal_ahead_of_the_verdict() {
        // #753: the block header flushes at the job's completion in default
        // piped mode, so a `flushed` signal must reach the presenter before
        // the summary — that is what lets a live row settle to the grey ✓ in
        // real time. Only `fast` flushes a block here; `slow` resolves from
        // the summary alone and so has no flush signal.
        let recording = Recording::arc();
        let dir = seed_dir(&["fast", "slow"]);
        let wrapper = seeded_wrapper(recording.clone(), &dir);
        gate_start(&wrapper);
        for line in [
            BANNER,
            "┃  fast ❯ ",
            "fast done",
            "summary: (done in 5.0 seconds)",
            "✔️ fast (0.2 seconds)",
            "✔️ slow (4.9 seconds)",
        ] {
            wrapper.on_job_output(GATE_JOB, line);
        }
        wrapper.on_job_success(GATE_JOB, Duration::from_secs(5));

        let events = recording.events();
        let fast_flush = events
            .iter()
            .position(|e| e == "flushed:fast")
            .expect("fast's block flush signals done-pending");
        let fast_success = events
            .iter()
            .position(|e| e == "success:fast")
            .expect("fast's verdict");
        assert!(
            fast_flush < fast_success,
            "the flush signal precedes the summary verdict: {events:?}"
        );
        assert!(
            !events.contains(&"flushed:slow".to_string()),
            "slow never flushed a block before the summary — no early signal: {events:?}"
        );
        assert!(events.contains(&"success:slow".to_string()), "{events:?}");
    }

    #[test]
    fn a_seeded_job_the_run_skips_flips_to_skipped_not_the_verdict() {
        // `only-rs` is seeded but lefthook skips it (glob) — the leading skip
        // notice must resolve it as skipped, and it must not also be swept.
        let recording = Recording::arc();
        let dir = seed_dir(&["always", "only-rs"]);
        let wrapper = seeded_wrapper(recording.clone(), &dir);
        gate_start(&wrapper);
        for line in [
            BANNER,
            "│  only-rs (skip) no matching push files",
            "┃  always ❯ ",
            "always done",
            "summary: (done in 0.2 seconds)",
            "✔️ always (0.17 seconds)",
        ] {
            wrapper.on_job_output(GATE_JOB, line);
        }
        wrapper.on_job_success(GATE_JOB, Duration::from_secs(1));

        let events = recording.events();
        assert!(
            events.contains(&"skipped:only-rs:no matching push files".to_string()),
            "{events:?}"
        );
        // Seeded once, skipped once, and never swept into a success/failure.
        assert_eq!(events.iter().filter(|e| *e == "start:only-rs:-").count(), 1);
        assert!(
            !events
                .iter()
                .any(|e| e == "success:only-rs" || e == "failure:only-rs"),
            "a skipped job must not also be swept by the verdict: {events:?}"
        );
        assert!(events.contains(&"success:always".to_string()));
    }

    #[test]
    fn a_seeded_phantom_settles_skipped_on_a_passing_hook() {
        // The config over-lists (`ghost` exists in lefthook.yml but the run
        // never touches it — e.g. a silent condition skip). On a passing hook
        // it must settle as skipped, never borrow the green.
        let recording = Recording::arc();
        let dir = seed_dir(&["real", "ghost"]);
        let wrapper = seeded_wrapper(recording.clone(), &dir);
        gate_start(&wrapper);
        for line in [
            BANNER,
            "┃  real ❯ ",
            "real done",
            "summary: (done in 0.2 seconds)",
            "✔️ real (0.17 seconds)",
        ] {
            wrapper.on_job_output(GATE_JOB, line);
        }
        wrapper.on_job_success(GATE_JOB, Duration::from_secs(1));

        let events = recording.events();
        assert!(events.contains(&"success:real".to_string()), "{events:?}");
        assert!(
            events.contains(&"skipped:ghost:not run".to_string()),
            "an unrun seeded job settles as skipped, not success: {events:?}"
        );
        assert!(
            !events.contains(&"success:ghost".to_string()),
            "a phantom must not borrow the passing verdict: {events:?}"
        );
    }

    #[test]
    fn a_failing_hook_settles_an_unrun_seeded_job_with_the_verdict() {
        // On failure the fate of a never-run seeded job is unknown; it follows
        // the phase verdict rather than being labelled skipped.
        let recording = Recording::arc();
        let dir = seed_dir(&["ran", "never"]);
        let wrapper = seeded_wrapper(recording.clone(), &dir);
        gate_start(&wrapper);
        for line in [BANNER, "┃  ran ❯ ", "boom"] {
            wrapper.on_job_output(GATE_JOB, line);
        }
        wrapper.on_job_failure(GATE_JOB, Duration::from_secs(1));

        let events = recording.events();
        assert!(events.contains(&"failure:ran".to_string()), "{events:?}");
        assert!(events.contains(&"failure:never".to_string()), "{events:?}");
        assert!(
            !events.iter().any(|e| e.contains("skipped:never")),
            "a failed phase does not relabel an unrun job as skipped: {events:?}"
        );
    }

    #[test]
    fn seeding_with_no_config_falls_back_to_reveal_on_completion() {
        // A dir without a lefthook config seeds nothing: identical to the
        // unseeded path — jobs appear as their blocks flush.
        let recording = Recording::arc();
        let empty = tempfile::tempdir().expect("tempdir");
        let wrapper = ManagerRoutingPresenter::wrap_seeded(
            recording.clone(),
            Some(empty.path().to_path_buf()),
        );
        gate_start(&wrapper);
        for line in [
            BANNER,
            "┃  fmt ❯ ",
            "fmt done",
            "summary: (done in 0.1 seconds)",
            "✔️ fmt (0.1 seconds)",
        ] {
            wrapper.on_job_output(GATE_JOB, line);
        }
        wrapper.on_job_success(GATE_JOB, Duration::from_secs(1));

        let events = recording.events();
        // No pre-seeded rows: fmt's start comes with its block, as today.
        assert!(
            !events.iter().any(|e| e.starts_with("planned:"))
                || events.contains(&"planned:fmt".to_string())
        );
        let start = events.iter().position(|e| e == "start:fmt:-").expect("fmt");
        let output = events
            .iter()
            .position(|e| e == "output:fmt:fmt done")
            .expect("out");
        assert!(start < output, "fmt starts with its block: {events:?}");
        assert_eq!(events.iter().filter(|e| *e == "start:fmt:-").count(), 1);
    }

    // ── lifecycle nesting ─────────────────────────────────────────────────

    #[test]
    fn a_manager_inside_a_lifecycle_job_gains_children_and_keeps_raw_lines() {
        let recording = Recording::arc();
        let wrapper = LifecycleRoutingPresenter::wrap(recording.clone());
        wrapper.on_phase_start("post-clone", None);
        wrapper.on_job_start("setup", None, None);
        for line in [
            "╭──────╮",
            BANNER,
            "╰──────╯",
            "┃  install ❯ ",
            "packages linked",
            "summary: (done in 0.5 seconds)",
            "✔️ install (0.4 seconds)",
        ] {
            wrapper.on_job_output("setup", line);
        }
        wrapper.on_job_success("setup", Duration::from_secs(1));

        let events = recording.events();
        // Every raw line reached the parent untouched — evidence unchanged.
        for line in ["╭──────╮", BANNER, "┃  install ❯ ", "packages linked"] {
            assert!(
                events.contains(&format!("output:setup:{line}")),
                "raw line {line:?} must reach the parent: {events:?}"
            );
        }
        // And the manager's job surfaced as a child alongside.
        assert!(events.contains(&"manager_engaged:setup:lefthook:2.1.10".to_string()));
        assert!(events.contains(&"child_start:setup:install".to_string()));
        assert!(events.contains(&"child_success:setup:install".to_string()));
        assert!(events.contains(&"success:setup".to_string()));
    }

    #[test]
    fn a_plain_lifecycle_job_gains_no_children() {
        let recording = Recording::arc();
        let wrapper = LifecycleRoutingPresenter::wrap(recording.clone());
        wrapper.on_job_start("build", None, None);
        wrapper.on_job_output("build", "compiling daft");
        wrapper.on_job_output("build", "done");
        wrapper.on_job_success("build", Duration::from_secs(1));

        assert_eq!(
            recording.events(),
            vec![
                "start:build:-".to_string(),
                "output:build:compiling daft".to_string(),
                "output:build:done".to_string(),
                "success:build".to_string(),
            ],
            "declined streams are byte-identical passthrough"
        );
    }

    #[test]
    fn a_cancelled_parent_settles_open_children_as_cancelled() {
        // A ⊘ parent must strand no spinning child rows.
        let recording = Recording::arc();
        let wrapper = LifecycleRoutingPresenter::wrap(recording.clone());
        wrapper.on_job_start("setup", None, None);
        for line in [BANNER, "┃  install ❯ ", "linking..."] {
            wrapper.on_job_output("setup", line);
        }
        wrapper.on_job_cancelled("setup", Duration::from_secs(1));

        let events = recording.events();
        assert!(
            events.contains(&"child_cancelled:setup:install".to_string()),
            "{events:?}"
        );
        assert!(events.contains(&"cancelled:setup".to_string()));
    }

    #[test]
    fn two_parents_recognize_independently() {
        // Parallel lifecycle jobs each get their own detector; one engaging
        // must not bleed children into the other.
        let recording = Recording::arc();
        let wrapper = LifecycleRoutingPresenter::wrap(recording.clone());
        wrapper.on_job_start("managed", None, None);
        wrapper.on_job_start("plain", None, None);
        wrapper.on_job_output("plain", "ordinary output");
        wrapper.on_job_output("managed", BANNER);
        wrapper.on_job_output("managed", "┃  fmt ❯ ");
        wrapper.on_job_output("plain", "more output");
        wrapper.on_job_failure("managed", Duration::from_secs(1));
        wrapper.on_job_success("plain", Duration::from_secs(1));

        let events = recording.events();
        assert!(events.contains(&"child_start:managed:fmt".to_string()));
        assert!(
            events.contains(&"child_failure:managed:fmt".to_string()),
            "no-summary children settle with the parent verdict: {events:?}"
        );
        assert!(
            !events
                .iter()
                .any(|e| e.starts_with("child_") && e.contains(":plain:")),
            "the plain parent gains no children: {events:?}"
        );
    }

    #[test]
    fn non_gate_jobs_pass_through_untouched() {
        // Lifecycle phases (post-clone etc.) share presenter types; only the
        // gate job is intercepted.
        let recording = Recording::arc();
        let wrapper = ManagerRoutingPresenter::wrap(recording.clone());
        wrapper.on_phase_start("post-clone", None);
        wrapper.on_job_start("setup", None, None);
        wrapper.on_job_output("setup", BANNER);
        wrapper.on_job_success("setup", Duration::from_secs(1));

        assert_eq!(
            recording.events(),
            vec![
                "phase_start:post-clone".to_string(),
                "start:setup:-".to_string(),
                format!("output:setup:{BANNER}"),
                "success:setup".to_string(),
            ]
        );
    }
}
