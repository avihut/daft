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
    /// Recognized job names in appearance order (also the census seed).
    started: Vec<String>,
    /// Recognized jobs the manager's summary has resolved.
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
            resolved: Vec::new(),
        }
    }
}

/// Wraps a presenter, turning a recognized manager's gate stream into
/// first-class job events. See the module docs for the contract.
pub struct ManagerRoutingPresenter {
    inner: Arc<dyn JobPresenter>,
    state: Mutex<RoutingState>,
}

impl ManagerRoutingPresenter {
    pub fn wrap(inner: Arc<dyn JobPresenter>) -> Arc<Self> {
        Arc::new(Self {
            inner,
            state: Mutex::new(RoutingState::fresh()),
        })
    }

    /// Wrap `presenter` when `daft.hooks.output.parseManagers` allows it.
    /// The knob is the kill switch back to today's synthetic-job rendering.
    pub fn wrap_if_enabled(
        config: &HookOutputConfig,
        presenter: Option<Arc<dyn JobPresenter>>,
    ) -> Option<Arc<dyn JobPresenter>> {
        match presenter {
            Some(inner) if config.parse_managers => Some(Self::wrap(inner)),
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

    fn translate(&self, state: &mut RoutingState, events: Vec<ManagerEvent>) {
        for event in events {
            match event {
                ManagerEvent::Engaged { .. } => {
                    state.engaged = true;
                    // The synthetic job never materializes on the engaged
                    // path; the manager's own jobs are the story now.
                    state.held_start = None;
                }
                ManagerEvent::JobStarted { name } => {
                    state.started.push(name.clone());
                    // Grow-only width seeding: receipts persist immediately,
                    // so renderers must learn the widest name as soon as it
                    // is known, not after it resolves.
                    self.inner.on_jobs_planned(&state.started);
                    self.inner.on_job_start(&name, None, None);
                }
                ManagerEvent::JobOutput { name, line } => {
                    self.inner.on_job_output(&name, &line);
                }
                ManagerEvent::JobSkipped { name, reason } => {
                    self.inner
                        .on_job_skipped(&name, &reason, Duration::ZERO, false, None);
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
            match verdict {
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
                "planned:fmt".to_string(),
                "start:fmt:-".to_string(),
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
        assert!(ManagerRoutingPresenter::wrap_if_enabled(&config, None).is_none());
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
