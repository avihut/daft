//! Plan-then-execute rail timeline (#651).
//!
//! Renders a command's full step plan up front as a live stderr region, fills
//! each step in place as the core executes, and leaves a persistent
//! append-only receipt in scrollback. Hook phases expand in place into a
//! rail-native section ([`RailHookRenderer`]) — succinct receipt rows by
//! default, threading each job's log when verbose.
//!
//! Not to be confused with `crate::output::outline`, the *static* outline
//! renderer — this module owns the live plan-execute region.
//!
//! Commands hold a [`Timeline`] alongside their `Output`; cores stay unaware
//! of it and speak through `ProgressSink::{on_plan, on_stage}`
//! (`crate::core::stage`). In [`TimelineMode::Plain`] and
//! [`TimelineMode::Hidden`] every method is a no-op and commands keep their
//! legacy output byte-identical — which is what keeps the non-TTY and
//! `DAFT_TESTING` output contracts (and the whole YAML suite) unchanged.

mod bridge;
mod plan;
mod rail_hook;
mod region;
mod render;
mod thread_block;

pub use bridge::RegionOutput;
pub use bridge::{error_line, warning_line};
pub use rail_hook::RailHookRenderer;
pub use region::HookEmbed;

/// The annotation a row that never ran wears: the `○ … (not run)` receipt. Two
/// paths produce it and must read identically — the region's `NotReached`
/// teardown face ([`render::final_row`]) and `daft exec`'s presenter resolving
/// a fail-fast / never-dispatched command as an expected skip.
pub(crate) const NOT_RUN: &str = "(not run)";

use crate::core::stage::{PlanCommit, StageEvent, StepKey};
use region::{FinalFace, RegionSetup, Resolution, TimelineCore, UnresolvedPolicy};
use std::io::IsTerminal;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

/// Per-row output threads (`daft exec`): each worker's live output rides its
/// plan row, and its captured log threads into the receipt. Set via
/// [`Timeline::set_row_output`] before the region materializes; only meaningful
/// alongside [`Timeline::set_ordered_receipts`].
#[derive(Clone, Copy)]
pub struct RowOutputConfig {
    /// Thread every row's full log into the receipt (grey under success) and
    /// show a rolling live window while it runs. Off: only failed/cancelled
    /// rows thread their captured output, and the live view is one latest-line
    /// annotation per row.
    pub verbose: bool,
    /// Rolling live-window height (`daft.hooks.output.tailLines`).
    pub tail_lines: usize,
    /// Byte budget for a row's buffered log (a chatty worker keeps only the
    /// tail); `None` keeps everything.
    pub buffer_cap: Option<usize>,
}

/// How the timeline renders for this invocation. Decided once per command.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum TimelineMode {
    /// Live region on a TTY. `color: false` renders the same structure with
    /// zero ANSI — reachable only from tests and the example spike:
    /// production (`auto`) maps NO_COLOR to [`Self::Plain`], because
    /// indicatif hides its whole draw target without color support.
    Interactive { color: bool },
    /// stderr is not a terminal: no timeline; commands keep legacy output.
    Plain,
    /// Quiet mode or test suppression: no timeline at all.
    Hidden,
}

impl TimelineMode {
    /// Predicate order: quiet → Hidden; `cfg(test)`/`DAFT_TESTING` → Hidden;
    /// non-TTY stderr or colors disabled → Plain; else Interactive.
    ///
    /// NO_COLOR lands in Plain because indicatif 0.18 hides its entire draw
    /// target when colors are unsupported (`ProgressDrawTarget::term`
    /// returns `hidden()` — documented: "Progress bars will also be hidden
    /// if NO_COLOR is set or TERM is unset/dumb"), so an "Interactive
    /// without color" region cannot actually render. This matches the
    /// pre-timeline behavior: the legacy spinner gated on
    /// `colors_enabled_stderr` too.
    pub fn auto(quiet: bool) -> Self {
        if quiet || crate::output::palette::testing_suppressed() {
            Self::Hidden
        } else if !std::io::stderr().is_terminal() || !crate::styles::colors_enabled_stderr() {
            Self::Plain
        } else {
            Self::Interactive { color: true }
        }
    }
}

struct Inner {
    header: String,
    verbose: bool,
    use_color: bool,
    /// Ordered receipts (`daft exec`): out-of-order completion still leaves a
    /// plan-ordered scrollback receipt. Set via
    /// [`Timeline::set_ordered_receipts`] before the region materializes.
    ordered: bool,
    /// Per-row output threads (`daft exec`). Set via
    /// [`Timeline::set_row_output`] before the region materializes.
    row_output: Option<RowOutputConfig>,
    core: Option<TimelineCore>,
    /// Lines held back until the rail closes — a failed hook job's captured
    /// output belongs after the footer (the rail's errors-after pattern),
    /// not torn through the live bars. Drained by [`Timeline::teardown`];
    /// accumulates across hook phases. Lives here rather than on the core
    /// because `finish` consumes the core before the footer exists in
    /// scrollback.
    deferred_after_footer: Vec<String>,
    /// Test-only: a draw target injected before the region materializes
    /// (`open_planning` or `commit_plan`, whichever comes first), so
    /// sequence tests capture the persisted lines through an `InMemoryTerm`
    /// instead of the (unattended) stderr target.
    #[cfg(test)]
    test_draw_target: Option<indicatif::ProgressDrawTarget>,
}

impl Inner {
    /// Snapshot the region knobs for a constructor call.
    fn region_setup(&self) -> RegionSetup {
        RegionSetup {
            verbose: self.verbose,
            use_color: self.use_color,
            ordered: self.ordered,
            row_output: self.row_output,
        }
    }

    /// The region's `MultiProgress` — production stderr, or the injected
    /// test target.
    fn make_multi_progress(&mut self) -> indicatif::MultiProgress {
        #[cfg(not(test))]
        {
            indicatif::MultiProgress::new()
        }
        #[cfg(test)]
        {
            match self.test_draw_target.take() {
                Some(target) => indicatif::MultiProgress::with_draw_target(target),
                None => indicatif::MultiProgress::new(),
            }
        }
    }
}

/// Cloneable handle to the live region, for components that render into it
/// from outside the command thread (the embedded hook presenter).
#[derive(Clone)]
pub struct TimelineHandle {
    inner: Arc<Mutex<Inner>>,
}

impl TimelineHandle {
    /// Lock the shared state, recovering from poison. A panic while the lock
    /// was held (an EPIPE print inside `suspend`) poisons it, but the state
    /// is structurally sound render bookkeeping — whereas panicking again
    /// turns the original unwind into a double panic inside `Timeline::drop`
    /// and aborts the process: no destructors, terminal left with `^C` echo
    /// off, live bars stranded, the real panic message lost.
    fn lock(&self) -> std::sync::MutexGuard<'_, Inner> {
        self.inner
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
    }

    /// Expand the given hook step into its block: the step's rail row is
    /// removed (the block replaces it) and the caller gets the shared
    /// `MultiProgress` plus a live insertion anchor. `None` when no region
    /// is live or the key is unknown.
    pub fn begin_hook_embed(&self, key: &StepKey) -> Option<HookEmbed> {
        let mut inner = self.lock();
        inner.core.as_mut()?.begin_hook_embed(key)
    }

    /// Print a permanent line above the live bars (no-op without a region).
    pub fn println_above(&self, line: &str) {
        let mut inner = self.lock();
        if let Some(core) = inner.core.as_mut() {
            core.println_above(line);
        }
    }

    /// Resolve a stage id (+ candidate scope) to the committed plan's key,
    /// preferring the scoped row over the unscoped one. `None` when no
    /// region is live or the plan has no such step.
    pub fn resolve_key(
        &self,
        id: crate::core::stage::StageId,
        scope: Option<&str>,
    ) -> Option<StepKey> {
        let inner = self.lock();
        inner.core.as_ref()?.resolve_key(id, scope)
    }

    /// `-v` free-text detail under the active step (no-op without a region).
    pub fn detail(&self, text: &str) {
        let mut inner = self.lock();
        if let Some(core) = inner.core.as_mut() {
            core.detail(text);
        }
    }

    /// Run `f` with the region cleared (for stdout writes that must not land
    /// mid-region). Runs `f` directly when no region is live.
    pub fn suspend<R>(&self, f: impl FnOnce() -> R) -> R {
        // Hold the lock for the duration: `f` is a short print, and the
        // region must not mutate underneath the cleared frame.
        let inner = self.lock();
        match inner.core.as_ref() {
            Some(core) => core.suspend(f),
            None => f(),
        }
    }

    /// Yield the terminal to a blocking prompt: the region clears for the
    /// duration ([`Self::suspend`]) and Ctrl-C routes to the prompt-cancel
    /// exit for the *whole* window, not just the key read. The region's
    /// collapse behavior must not stay armed here — this thread holds the
    /// timeline mutex and indicatif's draw lock while suspended, and the
    /// collapse takes both, so a Ctrl-C landing between the suspend and the
    /// prompt's own interrupt swap would deadblock the dispatcher. The
    /// prompt exit leaves the (already cleared) region alone.
    pub fn suspend_for_prompt<R>(&self, f: impl FnOnce() -> R) -> R {
        let outer = crate::interrupt::swap_behavior(|| crate::prompt::exit_for_cancelled_prompt());
        let result = self.suspend(f);
        crate::interrupt::restore_behavior(outer);
        result
    }

    /// Whether a live region currently owns the terminal.
    pub fn region_live(&self) -> bool {
        self.lock().core.is_some()
    }

    /// Hold `lines` back until the rail closes; the timeline prints them
    /// after the footer (blank-line separated). No-op semantics match the
    /// rest of the handle: with no region the lines still drain at teardown,
    /// but only region-embedded renderers ever defer.
    pub fn defer_after_footer(&self, lines: Vec<String>) {
        self.lock().deferred_after_footer.extend(lines);
    }

    /// Feed one output line to a row's live thread (`daft exec` workers,
    /// driven from the stream-reader threads through the cloneable handle).
    /// Buffers the line for the receipt and repaints the row's live view.
    /// No-op without a live region, an unknown key, or a resolved row.
    pub fn push_row_output(&self, key: &StepKey, line: &str) {
        let mut inner = self.lock();
        if let Some(core) = inner.core.as_mut() {
            core.push_row_output(key, line);
        }
    }

    /// Route a stage event onto the region. Lives on the handle (not just
    /// [`Timeline`]) so a `JobPresenter` driving from worker threads — `daft
    /// exec`'s rail rows — can reach it through the cloneable handle without a
    /// `&mut Timeline`. No-op without a live region.
    pub fn on_stage(&self, key: &StepKey, event: StageEvent) {
        let mut inner = self.lock();
        let use_color = inner.use_color;
        let Some(core) = inner.core.as_mut() else {
            return;
        };
        // A shared-file outcome the plan never saw (clone without a probed
        // config, or a target branch declaring more than the source did):
        // persist its legacy line above the live bars — tear-free, and the
        // fact is not lost — instead of dropping the unknown key.
        if key.id == crate::core::stage::StageId::SharedFile
            && core.resolve_key(key.id, key.scope.as_deref()).is_none()
        {
            if let Some(line) =
                crate::core::shared::legacy_shared_stage_line(key, &event, use_color)
            {
                core.println_above(&line);
            }
            return;
        }
        match event {
            StageEvent::Started => core.activate(key),
            StageEvent::Completed { annotation } => core.resolve(
                key,
                Resolution::Final {
                    face: FinalFace::Done,
                    annotation,
                },
            ),
            StageEvent::Failed { detail } => core.resolve(
                key,
                Resolution::Final {
                    face: FinalFace::Failed,
                    annotation: Some(detail),
                },
            ),
            StageEvent::Cancelled => core.resolve(
                key,
                Resolution::Final {
                    face: FinalFace::Cancelled,
                    annotation: Some("cancelled".to_string()),
                },
            ),
            StageEvent::SkippedExpected { reason } => core.resolve(
                key,
                Resolution::Final {
                    face: FinalFace::SkippedExpected,
                    annotation: Some(reason),
                },
            ),
            StageEvent::SkippedAttention { reason } => {
                // Shared-file and fetch reasons are self-contained phrases
                // (missing, conflict, "failed — …"); exec's orphan-target
                // reason ("no worktree") and push's resolve fallback
                // ("no worktree — pushing from the current directory")
                // likewise. The generic "skipped — " prefix would stutter on
                // them — and on push's it would also lie: resolution ran, and
                // the push proceeds from the invoking directory.
                let annotation = match key.id {
                    crate::core::stage::StageId::SharedFile
                    | crate::core::stage::StageId::Fetch
                    | crate::core::stage::StageId::Tracking
                    | crate::core::stage::StageId::ExecCommand
                    | crate::core::stage::StageId::ResolveWorktree => reason,
                    _ => format!("skipped \u{2014} {reason}"),
                };
                core.resolve(
                    key,
                    Resolution::Final {
                        face: FinalFace::SkippedAttention,
                        annotation: Some(annotation),
                    },
                )
            }
            StageEvent::SkippedSilent => core.resolve(key, Resolution::Silent),
            StageEvent::Note(text) => core.set_annotation(key, text),
        }
    }
}

/// The timeline a command owns for one invocation.
pub struct Timeline {
    handle: TimelineHandle,
    mode: TimelineMode,
    started: Instant,
}

impl Timeline {
    /// `header` is the resolved intent line, seeded by the command layer
    /// ("Starting daft-652/x", "Removing 2 branches", …).
    pub fn new(mode: TimelineMode, verbose: bool, header: impl Into<String>) -> Self {
        let use_color = matches!(mode, TimelineMode::Interactive { color: true });
        Self {
            handle: TimelineHandle {
                inner: Arc::new(Mutex::new(Inner {
                    header: header.into(),
                    verbose,
                    use_color,
                    ordered: false,
                    row_output: None,
                    core: None,
                    deferred_after_footer: Vec::new(),
                    #[cfg(test)]
                    test_draw_target: None,
                })),
            },
            mode,
            started: Instant::now(),
        }
    }

    /// Open the live region before the plan is known (Interactive only):
    /// the seeded header, a static `⠹  <label>` planning row (grey), and
    /// the stopwatch footer. `commit_plan` replaces the middle in place; a
    /// command that returns without committing (navigation early exit,
    /// resolve-phase error) collapses the face without a trace — via
    /// [`Self::abandon_planning`], or implicitly through `finish`/`abort` —
    /// and keeps its legacy output.
    pub fn open_planning(&mut self, planning_label: &str) {
        if !self.is_interactive() {
            return;
        }
        let mut inner = self.handle.lock();
        if inner.core.is_some() {
            return;
        }
        let mp = inner.make_multi_progress();
        let header = inner.header.clone();
        let setup = inner.region_setup();
        inner.core = Some(TimelineCore::open_planning(
            mp,
            header,
            planning_label,
            setup,
            self.started,
        ));
    }

    /// Opt into plan-ordered receipts: rows may resolve out of completion
    /// order (parallel `daft exec` workers), but the scrollback receipt stays
    /// in plan order. Must be called before the region materializes
    /// (`open_planning` or `commit_plan`); a no-op afterward. Lifecycle
    /// commands never call this — their eager persistence is byte-identical.
    pub fn set_ordered_receipts(&self, ordered: bool) {
        self.handle.lock().ordered = ordered;
    }

    /// Enable per-row output threads (`daft exec`): live output on each row,
    /// captured logs threaded into the receipt. Must be called before the
    /// region materializes. Pairs with [`Self::set_ordered_receipts`].
    pub fn set_row_output(&self, config: RowOutputConfig) {
        self.handle.lock().row_output = Some(config);
    }

    /// Update the planning face's label in place ("Cloning repository" →
    /// "Resolving branches") — liveness for a resolve phase that moves
    /// between kinds of work. No-op without a face (Plain mode, plan already
    /// committed, abandoned region).
    pub fn set_planning_label(&mut self, label: &str) {
        let mut inner = self.handle.lock();
        if let Some(core) = inner.core.as_mut() {
            core.set_planning_label(label);
        }
    }

    /// Materialize the region (Interactive only; no-op otherwise). Called by
    /// the bridge when the core commits its plan. Over an open planning face
    /// the plan installs in place (the face's bars leave, the stopwatch
    /// footer carries on); with no face this is the direct path (a command
    /// that commits without opening a planning face first).
    pub fn commit_plan(&mut self, plan: PlanCommit) {
        if !self.is_interactive() {
            return;
        }
        let mut inner = self.handle.lock();
        if let Some(core) = inner.core.as_mut() {
            let installing_over_face = core.is_planning();
            if installing_over_face {
                core.install_plan(plan);
            }
            // Assert only after the lock is released: a debug-build panic
            // while the guard is held would poison the lock mid-unwind.
            drop(inner);
            debug_assert!(
                installing_over_face,
                "plan committed twice for one invocation"
            );
            return;
        }
        let mp = inner.make_multi_progress();
        let header = inner.header.clone();
        let setup = inner.region_setup();
        inner.core = Some(TimelineCore::new(mp, header, plan, setup, self.started));
    }

    /// Collapse a still-planning region without a trace (no footer, no
    /// receipt). Commands call this after the core returns without
    /// committing a plan — the legacy result/error lines can then print
    /// without tearing through live bars. No-op once the plan has committed
    /// (and without a region).
    pub fn abandon_planning(&mut self) {
        let core = {
            let mut inner = self.handle.lock();
            if inner.core.as_ref().is_some_and(TimelineCore::is_planning) {
                inner.core.take()
            } else {
                None
            }
        };
        if let Some(core) = core {
            core.collapse();
        }
    }

    /// Route a stage event onto the region. Delegates to
    /// [`TimelineHandle::on_stage`] so the command thread and a rail presenter
    /// running on worker threads speak one routing path.
    pub fn on_stage(&mut self, key: &StepKey, event: StageEvent) {
        self.handle.on_stage(key, event);
    }

    /// The hook block for an embedded step finished rendering; reconnect
    /// the rail (no-op if no block opened).
    pub fn close_hook_embed(&mut self) {
        let mut inner = self.handle.lock();
        if let Some(core) = inner.core.as_mut() {
            core.close_hook_embed();
        }
    }

    /// Resolve a hook step's row from the executor's outcome: benign
    /// non-events (nothing configured to run) remove the row silently;
    /// attention-worthy skips (trust refusal, --skip-hooks) render the
    /// yellow row; a hook that ran was already expanded into its block.
    /// Shared by `TimelineBridge::run_hook` and clone's hook helpers.
    pub fn resolve_hook_step(&mut self, key: &StepKey, skipped: bool, skip_reason: Option<&str>) {
        if !skipped {
            // Ran (success or failure): the block is the record; reconnect
            // the rail below it when rows remain.
            self.close_hook_embed();
            return;
        }
        match skip_reason {
            // Benign non-events — remove the row silently rather than
            // advertise machinery. ("All jobs skipped" may have rendered
            // its own attribution block, in which case the row is already
            // consumed and this is a no-op.)
            Some("No hook files found")
            | Some("Hooks are globally disabled")
            | Some("All jobs skipped")
            | Some("No jobs defined")
            | Some("No jobs match changed attributes")
            | None => self.resolve_silently(key),
            // A per-hook `enabled: false` (`daft.hooks.<type>.enabled`) is
            // deliberate configuration, exactly like the global kill-switch
            // above — pre-#651 it printed nothing, and a config the user
            // chose must not nag in the attention color on every run. The
            // executor's reason is "<hook-type> hook is disabled".
            Some(reason) if reason.ends_with(" hook is disabled") => self.resolve_silently(key),
            // A hook-level `skip:`/`only:` condition is the config working
            // as designed — the section vanishes like a per-job condition
            // skip (custom condition messages don't match the prefix and
            // stay visible below).
            Some(reason) if rail_hook::is_condition_skip(reason) => self.resolve_silently(key),
            // Attention-worthy skips (trust refusal, --skip-hooks — tag
            // exclusion included, declined prompt): yellow row with the
            // reason.
            Some(reason) => self.on_stage(
                key,
                StageEvent::SkippedAttention {
                    reason: reason.to_string(),
                },
            ),
        }
    }

    /// Resolve a step without leaving a row (benign hook skip: no hooks
    /// configured, hooks globally disabled).
    pub fn resolve_silently(&mut self, key: &StepKey) {
        let mut inner = self.handle.lock();
        if let Some(core) = inner.core.as_mut() {
            core.resolve(key, Resolution::Silent);
        }
    }

    /// `-v` free-text detail under the active step.
    pub fn detail(&mut self, text: &str) {
        self.handle.detail(text);
    }

    /// Print a permanent line above the live bars.
    pub fn println_above(&self, line: &str) {
        self.handle.println_above(line);
    }

    /// Resolve a stage id (+ candidate scope) to the committed plan's key.
    /// See [`TimelineHandle::resolve_key`].
    pub fn resolve_key(
        &self,
        id: crate::core::stage::StageId,
        scope: Option<&str>,
    ) -> Option<StepKey> {
        self.handle.resolve_key(id, scope)
    }

    /// Close the rail on success: `└  <text>`. No-op without a region.
    pub fn finish(&mut self, footer_text: &str) {
        self.teardown(footer_text, UnresolvedPolicy::Drop);
    }

    /// Close the rail after a failure: unresolved steps persist as dim
    /// `(not run)` rows, then `└  <text>`.
    pub fn abort(&mut self, footer_text: &str) {
        self.teardown(footer_text, UnresolvedPolicy::NotReached);
    }

    fn teardown(&mut self, footer_text: &str, policy: UnresolvedPolicy) {
        let (core, deferred) = {
            let mut inner = self.handle.lock();
            (
                inner.core.take(),
                std::mem::take(&mut inner.deferred_after_footer),
            )
        };
        if let Some(core) = core {
            if core.is_planning() {
                // The plan never landed: nothing painted earned a receipt.
                // Collapse silently so resolve errors and navigation early
                // exits keep their legacy single-line output.
                core.collapse();
            } else {
                core.finish(footer_text, policy);
            }
        }
        // Deferred content (failed hook-job output) lands below the footer,
        // blank-line separated — the region is gone, so plain stderr is the
        // channel. Drains on abort and the Drop safety net too: the receipt
        // may be truncated, the captured failure must not be.
        if !deferred.is_empty() {
            eprintln!();
            for line in deferred {
                eprintln!("{line}");
            }
        }
    }

    /// Elapsed wall-clock since the timeline was created.
    pub fn elapsed(&self) -> Duration {
        self.started.elapsed()
    }

    /// Format a footer duration in the house duration vocabulary.
    pub fn elapsed_display(&self) -> String {
        crate::output::hook_progress::format_duration(self.elapsed())
    }

    pub fn is_interactive(&self) -> bool {
        matches!(self.mode, TimelineMode::Interactive { .. })
    }

    /// Whether the rail replaces the legacy stdout record for this run.
    ///
    /// The receipt supersedes the record only for eyes on the terminal. The
    /// rail keys off *stderr*, so `daft go x > file` (or a pipe, or `$(…)`)
    /// still renders it — but the redirected stdout is the machine-readable
    /// outcome and must keep the record, exactly as it read before the
    /// timeline existed.
    pub fn replaces_stdout_record(&self) -> bool {
        self.is_interactive() && std::io::IsTerminal::is_terminal(&std::io::stdout())
    }

    /// Whether a live region owns the terminal — the planning face or the
    /// committed plan, until finish/abort/collapse.
    pub fn region_live(&self) -> bool {
        self.handle.region_live()
    }

    pub fn handle(&self) -> TimelineHandle {
        self.handle.clone()
    }

    /// Test-only: route the region's draw calls into `target` (an
    /// `InMemoryTerm`) so tests assert the persisted line sequence, not just
    /// the state machine. Must be called before the region materializes
    /// (`open_planning` or `commit_plan`, whichever comes first).
    #[cfg(test)]
    pub(crate) fn set_test_draw_target(&self, target: indicatif::ProgressDrawTarget) {
        self.handle.lock().test_draw_target = Some(target);
    }
}

impl Drop for Timeline {
    /// Safety net: a region abandoned by an early `?` return must not leave
    /// live bars behind. Persist nothing extra — just collapse.
    fn drop(&mut self) {
        if self.region_live() {
            self.abort("interrupted");
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::core::stage::{Row, StageId, StepSpec};

    // Under `cfg(test)` stderr is not a TTY, so indicatif's stderr draw
    // target is unattended and paints nothing — these tests exercise the
    // region state machine, not pixels.

    fn plan() -> PlanCommit {
        PlanCommit::new(vec![
            Row::Step(StepSpec::new(StepKey::new(StageId::CheckOut))),
            Row::Step(
                StepSpec::new(StepKey::new(StageId::CreateWorktree)).with_annotation("../feat/x"),
            ),
            Row::Step(StepSpec::new(StepKey::new(StageId::PostCreateHooks))),
        ])
    }

    fn interactive() -> Timeline {
        Timeline::new(
            TimelineMode::Interactive { color: false },
            false,
            "Opening feat/x",
        )
    }

    #[test]
    fn lifecycle_commit_run_finish() {
        let mut tl = interactive();
        assert!(!tl.region_live());
        tl.commit_plan(plan());
        assert!(tl.region_live());

        for id in [StageId::CheckOut, StageId::CreateWorktree] {
            let key = StepKey::new(id);
            tl.on_stage(&key, StageEvent::Started);
            tl.on_stage(&key, StageEvent::Completed { annotation: None });
        }
        tl.resolve_silently(&StepKey::new(StageId::PostCreateHooks));
        tl.finish("Ready in 0.1s");
        assert!(!tl.region_live());
    }

    #[test]
    fn poisoned_lock_recovers_instead_of_double_panicking() {
        // A panic while the Inner lock is held (an EPIPE print inside
        // `suspend`) poisons the mutex mid-unwind. Every later lock — most
        // critically the one in `Timeline::drop → region_live` — must
        // recover instead of panicking again: a panic inside Drop during
        // unwind aborts the process (no destructors, termios left broken,
        // the original panic message lost).
        let mut tl = interactive();
        tl.commit_plan(plan());
        let handle = tl.handle();
        std::thread::spawn(move || {
            let _guard = handle.lock();
            panic!("poison the timeline lock");
        })
        .join()
        .unwrap_err();
        assert!(tl.region_live(), "recovered read of the poisoned state");
        drop(tl); // The Drop safety net must survive the poisoned lock.
    }

    #[test]
    fn skipped_silent_resolves_without_a_row() {
        let mut tl = interactive();
        tl.commit_plan(plan());
        let key = StepKey::new(StageId::CheckOut);
        tl.on_stage(&key, StageEvent::Started);
        tl.on_stage(&key, StageEvent::SkippedSilent);
        // A silently resolved step is settled: a clean finish must not
        // re-render it (Drop policy applies to unresolved rows only).
        tl.finish("Ready in 0.1s");
        assert!(!tl.region_live());
    }

    fn grouped_plan() -> PlanCommit {
        PlanCommit::new(vec![
            Row::Step(StepSpec::new(StepKey::new(StageId::CreateWorktree))),
            Row::Group {
                label: "shared files".into(),
            },
            Row::Step(StepSpec::new(StepKey::scoped(StageId::CheckOut, ".env"))),
            Row::Step(StepSpec::new(StepKey::scoped(StageId::CheckOut, ".envrc"))),
            Row::EndGroup,
            Row::Step(StepSpec::new(StepKey::new(StageId::PostCreateHooks))),
        ])
    }

    #[test]
    fn all_silent_group_span_drops_its_anchor_midrun() {
        let mut tl = interactive();
        tl.commit_plan(grouped_plan());
        let wt = StepKey::new(StageId::CreateWorktree);
        tl.on_stage(&wt, StageEvent::Started);
        tl.on_stage(&wt, StageEvent::Completed { annotation: None });
        // Both grouped rows vanish: the second silent resolution settles the
        // span and must drop the never-printed anchor bar (not strand it).
        tl.on_stage(
            &StepKey::scoped(StageId::CheckOut, ".env"),
            StageEvent::SkippedSilent,
        );
        tl.on_stage(
            &StepKey::scoped(StageId::CheckOut, ".envrc"),
            StageEvent::SkippedSilent,
        );
        let hooks = StepKey::new(StageId::PostCreateHooks);
        tl.on_stage(&hooks, StageEvent::Started);
        tl.on_stage(&hooks, StageEvent::Completed { annotation: None });
        tl.finish("Ready in 0.1s");
        assert!(!tl.region_live());
    }

    #[test]
    fn group_span_with_visible_content_persists_through_finish() {
        let mut tl = interactive();
        tl.commit_plan(grouped_plan());
        // One row completes (anchor must flush before it), one vanishes.
        tl.on_stage(
            &StepKey::scoped(StageId::CheckOut, ".env"),
            StageEvent::Completed { annotation: None },
        );
        tl.on_stage(
            &StepKey::scoped(StageId::CheckOut, ".envrc"),
            StageEvent::SkippedSilent,
        );
        tl.finish("Ready in 0.1s");
        assert!(!tl.region_live());
    }

    #[test]
    fn abort_persists_group_over_not_reached_rows() {
        let mut tl = interactive();
        tl.commit_plan(grouped_plan());
        let wt = StepKey::new(StageId::CreateWorktree);
        tl.on_stage(&wt, StageEvent::Started);
        tl.on_stage(
            &wt,
            StageEvent::Failed {
                detail: "boom".into(),
            },
        );
        // Teardown with unresolved grouped rows: NotReached policy flushes
        // the anchor before printing its span's `(not run)` rows.
        tl.abort("Failed after 0.1s");
        assert!(!tl.region_live());
    }

    #[test]
    fn hook_embed_hands_out_region_and_anchor_stays_live() {
        let mut tl = interactive();
        tl.commit_plan(plan());
        let key = StepKey::new(StageId::CheckOut);
        tl.on_stage(&key, StageEvent::Started);
        tl.on_stage(&key, StageEvent::Completed { annotation: None });

        let embed = tl
            .handle()
            .begin_hook_embed(&StepKey::new(StageId::PostCreateHooks))
            .expect("region live, key known");
        // The consumed hook row's plan label names the section for the
        // succinct renderer.
        assert_eq!(embed.section_label.as_deref(), Some("post-create hooks"));
        // The anchor must be a linked member of the shared MultiProgress —
        // `insert_before` panics otherwise, so exercise it directly.
        let inserted = embed
            .mp
            .insert_before(&embed.anchor, indicatif::ProgressBar::new_spinner());
        embed.mp.remove(&inserted);

        tl.finish("Ready");
        assert!(!tl.region_live());
    }

    #[test]
    fn task_step_embeds_with_its_fixed_label_as_the_section() {
        // `daft run`'s multi-job rail: one Task step carrying the task name
        // as a fixed label; the embed consumes it into a `├─ <task>` section.
        let mut tl = interactive();
        tl.commit_plan(PlanCommit::new(vec![Row::Step(
            StepSpec::new(StepKey::new(StageId::Task)).with_label("dev-stack"),
        )]));
        let embed = tl
            .handle()
            .begin_hook_embed(&StepKey::new(StageId::Task))
            .expect("region live, key known");
        assert_eq!(embed.section_label.as_deref(), Some("dev-stack"));
        tl.finish("Done in 0.1s");
        assert!(!tl.region_live());
    }

    #[test]
    fn gate_embed_on_an_active_step_has_no_section_label() {
        let mut tl = interactive();
        tl.commit_plan(PlanCommit::new(vec![
            Row::Step(StepSpec::new(StepKey::new(StageId::CheckOut))),
            Row::Step(StepSpec::new(StepKey::new(StageId::Push))),
        ]));
        let push = StepKey::new(StageId::Push);
        tl.on_stage(&push, StageEvent::Started);
        // A pre-push gate hook embeds on the ACTIVE Push row: the step label
        // belongs to the outcome row below the section, not the anchor.
        let embed = tl
            .handle()
            .begin_hook_embed(&push)
            .expect("region live, key known");
        assert_eq!(embed.section_label, None);
        tl.on_stage(&push, StageEvent::Completed { annotation: None });
        tl.finish("Ready");
    }

    #[test]
    fn hidden_mode_never_materializes() {
        let mut tl = Timeline::new(TimelineMode::Hidden, false, "Opening feat/x");
        tl.commit_plan(plan());
        assert!(!tl.region_live());
        assert!(
            tl.handle()
                .begin_hook_embed(&StepKey::new(StageId::PostCreateHooks))
                .is_none()
        );
        tl.finish("Ready");
    }

    #[test]
    fn abort_collapses_with_pending_steps() {
        let mut tl = interactive();
        tl.commit_plan(plan());
        let key = StepKey::new(StageId::CheckOut);
        tl.on_stage(&key, StageEvent::Started);
        tl.on_stage(
            &key,
            StageEvent::Failed {
                detail: "boom".into(),
            },
        );
        tl.abort("Failed after 0.1s");
        assert!(!tl.region_live());
    }

    #[test]
    fn drop_collapses_a_live_region() {
        let mut tl = interactive();
        tl.commit_plan(plan());
        tl.on_stage(&StepKey::new(StageId::CheckOut), StageEvent::Started);
        drop(tl); // must not panic or strand bars
    }

    fn deferred_len(tl: &Timeline) -> usize {
        tl.handle()
            .inner
            .lock()
            .unwrap()
            .deferred_after_footer
            .len()
    }

    #[test]
    fn deferred_lines_accumulate_and_drain_on_finish() {
        let mut tl = interactive();
        tl.commit_plan(plan());
        tl.handle()
            .defer_after_footer(vec!["error: hook job 'x' failed:".into(), "  boom".into()]);
        tl.handle()
            .defer_after_footer(vec!["  second phase".into()]);
        assert_eq!(deferred_len(&tl), 3, "lines held until the rail closes");
        tl.finish("Ready");
        assert_eq!(deferred_len(&tl), 0, "teardown drains the buffer");
    }

    #[test]
    fn deferred_lines_drain_on_abort_and_drop() {
        let mut tl = interactive();
        tl.commit_plan(plan());
        tl.on_stage(&StepKey::new(StageId::CheckOut), StageEvent::Started);
        let handle = tl.handle();
        handle.defer_after_footer(vec!["captured failure".into()]);
        drop(tl); // Drop safety net aborts a live region
        assert!(
            handle
                .inner
                .lock()
                .unwrap()
                .deferred_after_footer
                .is_empty(),
            "a truncated receipt must still flush the captured failure"
        );
    }

    // ── persisted-sequence tests (InMemoryTerm) ──────────────────────────
    //
    // The tests above exercise the state machine; these capture the actual
    // persisted lines. The contract under test is the section dialect: a
    // `│` spacer, a `├─ <label>` anchor, and every row in the anchor's span
    // tucked into the rail gutter (`│  <row>`) — spacers never doubled.

    fn captured(header: &str) -> (Timeline, indicatif::InMemoryTerm) {
        let term = indicatif::InMemoryTerm::new(60, 100);
        let tl = Timeline::new(TimelineMode::Interactive { color: false }, false, header);
        tl.set_test_draw_target(indicatif::ProgressDrawTarget::term_like(Box::new(
            term.clone(),
        )));
        (tl, term)
    }

    fn complete(tl: &mut Timeline, key: &StepKey) {
        tl.on_stage(key, StageEvent::Started);
        tl.on_stage(key, StageEvent::Completed { annotation: None });
    }

    #[test]
    fn group_persists_as_spacer_then_edge_anchor() {
        let (mut tl, term) = captured("Opening feat/x");
        tl.commit_plan(PlanCommit::new(vec![
            Row::Step(StepSpec::new(StepKey::new(StageId::CreateWorktree))),
            Row::Group {
                label: "shared files".into(),
            },
            Row::Step(
                StepSpec::new(StepKey::scoped(StageId::SharedFile, ".env")).with_label(".env"),
            ),
            Row::EndGroup,
            Row::Step(StepSpec::new(StepKey::new(StageId::PostCreateHooks))),
        ]));
        complete(&mut tl, &StepKey::new(StageId::CreateWorktree));
        tl.on_stage(
            &StepKey::scoped(StageId::SharedFile, ".env"),
            StageEvent::Completed { annotation: None },
        );
        tl.resolve_silently(&StepKey::new(StageId::PostCreateHooks));
        tl.finish("Ready in 0.1s");
        assert_eq!(
            term.contents(),
            "\u{250c}  Opening feat/x\n\
             \u{2502}\n\
             \u{2713}  Created worktree\n\
             \u{2502}\n\
             \u{251c}\u{2500} shared files\n\
             \u{2502}  \u{2713}  .env\n\
             \u{2502}\n\
             \u{2514}  Ready in 0.1s"
        );
    }

    #[test]
    fn first_slot_group_leans_on_the_header_spacer() {
        let (mut tl, term) = captured("Removing 2 branches");
        tl.commit_plan(PlanCommit::new(vec![
            Row::Group {
                label: "feat/a".into(),
            },
            Row::Step(StepSpec::new(StepKey::scoped(StageId::RemoveWorktree, "a"))),
            Row::Group {
                label: "feat/b".into(),
            },
            Row::Step(StepSpec::new(StepKey::scoped(StageId::RemoveWorktree, "b"))),
        ]));
        complete(&mut tl, &StepKey::scoped(StageId::RemoveWorktree, "a"));
        complete(&mut tl, &StepKey::scoped(StageId::RemoveWorktree, "b"));
        tl.finish("Removed 2 worktrees in 0.1s");
        // Exactly one `│` between the header and the first anchor (the
        // header's own spacer), and a fresh one before the second section.
        assert_eq!(
            term.contents(),
            "\u{250c}  Removing 2 branches\n\
             \u{2502}\n\
             \u{251c}\u{2500} feat/a\n\
             \u{2502}  \u{2713}  Removed worktree\n\
             \u{2502}\n\
             \u{251c}\u{2500} feat/b\n\
             \u{2502}  \u{2713}  Removed worktree\n\
             \u{2502}\n\
             \u{2514}  Removed 2 worktrees in 0.1s"
        );
    }

    #[test]
    fn exec_command_row_cancels_to_the_ban_face() {
        // An exec worker interrupted mid-run resolves to the yellow `⊘` face
        // with a plain `cancelled` reason (the sub-second test duration is
        // below the display threshold, so none shows). The fixed label wins.
        let (mut tl, term) = captured("Running mise test in 1 worktree");
        tl.commit_plan(PlanCommit::new(vec![Row::Step(
            StepSpec::new(StepKey::new(StageId::ExecCommand)).with_label("master"),
        )]));
        let key = StepKey::new(StageId::ExecCommand);
        tl.on_stage(&key, StageEvent::Started);
        tl.on_stage(&key, StageEvent::Cancelled);
        tl.finish("Cancelled after 0.1s");
        assert_eq!(
            term.contents(),
            "\u{250c}  Running mise test in 1 worktree\n\
             \u{2502}\n\
             \u{2298}  master  cancelled\n\
             \u{2502}\n\
             \u{2514}  Cancelled after 0.1s"
        );
    }

    fn exec_key(scope: &str) -> StepKey {
        StepKey::scoped(StageId::ExecCommand, scope)
    }

    fn exec_row(scope: &str) -> Row {
        Row::Step(StepSpec::new(exec_key(scope)).with_label(scope))
    }

    #[test]
    fn ordered_receipts_persist_in_plan_order_despite_out_of_order_completion() {
        // The exec case: workers finish in completion order (c, a, b) but the
        // scrollback receipt must stay in plan order (a, b, c). Nothing
        // persists until the prefix is resolved — c waits behind a and b.
        let (mut tl, term) = captured("Running mise clean in 3 worktrees");
        tl.set_ordered_receipts(true);
        tl.commit_plan(PlanCommit::new(vec![
            exec_row("a"),
            exec_row("b"),
            exec_row("c"),
        ]));
        for s in ["c", "a", "b"] {
            tl.on_stage(&exec_key(s), StageEvent::Started);
            tl.on_stage(&exec_key(s), StageEvent::Completed { annotation: None });
        }
        tl.finish("Done in 0.1s");
        assert_eq!(
            term.contents(),
            "\u{250c}  Running mise clean in 3 worktrees\n\
             \u{2502}\n\
             \u{2713}  a\n\
             \u{2713}  b\n\
             \u{2713}  c\n\
             \u{2502}\n\
             \u{2514}  Done in 0.1s"
        );
    }

    #[test]
    fn ordered_grouped_receipts_stay_in_plan_order() {
        // Pipelines (m>1): a ├─ group per worktree. Worktree B's rows resolve
        // first, but the receipt keeps A's group ahead of B's.
        let (mut tl, term) = captured("Running 2 commands in 2 worktrees");
        tl.set_ordered_receipts(true);
        tl.commit_plan(PlanCommit::new(vec![
            Row::Group {
                label: "wtA".into(),
            },
            Row::Step(StepSpec::new(exec_key("wtA#0")).with_label("mise clean")),
            Row::Step(StepSpec::new(exec_key("wtA#1")).with_label("mise dev")),
            Row::Group {
                label: "wtB".into(),
            },
            Row::Step(StepSpec::new(exec_key("wtB#0")).with_label("mise clean")),
            Row::Step(StepSpec::new(exec_key("wtB#1")).with_label("mise dev")),
        ]));
        for s in ["wtB#0", "wtB#1", "wtA#0", "wtA#1"] {
            tl.on_stage(&exec_key(s), StageEvent::Started);
            tl.on_stage(&exec_key(s), StageEvent::Completed { annotation: None });
        }
        tl.finish("Done in 0.1s");
        assert_eq!(
            term.contents(),
            "\u{250c}  Running 2 commands in 2 worktrees\n\
             \u{2502}\n\
             \u{251c}\u{2500} wtA\n\
             \u{2502}  \u{2713}  mise clean\n\
             \u{2502}  \u{2713}  mise dev\n\
             \u{2502}\n\
             \u{251c}\u{2500} wtB\n\
             \u{2502}  \u{2713}  mise clean\n\
             \u{2502}  \u{2713}  mise dev\n\
             \u{2502}\n\
             \u{2514}  Done in 0.1s"
        );
    }

    #[test]
    fn ordered_fail_fast_persists_recorded_prefix_then_not_run_suffix() {
        // Sequential fail-fast: the first worker fails, the rest never launch.
        // The failed row shows `✗`, the never-launched rows persist as dim
        // `(not run)` under abort — all in plan order.
        let (mut tl, term) = captured("Running cargo test in 3 worktrees");
        tl.set_ordered_receipts(true);
        tl.commit_plan(PlanCommit::new(vec![
            exec_row("a"),
            exec_row("b"),
            exec_row("c"),
        ]));
        tl.on_stage(&exec_key("a"), StageEvent::Started);
        tl.on_stage(
            &exec_key("a"),
            StageEvent::Failed {
                detail: "exit 101".into(),
            },
        );
        // b and c never start; abort persists them as (not run).
        tl.abort("Failed after 0.1s");
        assert_eq!(
            term.contents(),
            "\u{250c}  Running cargo test in 3 worktrees\n\
             \u{2502}\n\
             \u{2717}  a  exit 101\n\
             \u{25cb}  b  (not run)\n\
             \u{25cb}  c  (not run)\n\
             \u{2502}\n\
             \u{2514}  Failed after 0.1s"
        );
    }

    fn exec_verbose() -> RowOutputConfig {
        RowOutputConfig {
            verbose: true,
            tail_lines: 6,
            buffer_cap: None,
        }
    }

    fn exec_default() -> RowOutputConfig {
        RowOutputConfig {
            verbose: false,
            tail_lines: 6,
            buffer_cap: None,
        }
    }

    #[test]
    fn verbose_row_threads_its_log_grey_under_success() {
        let (mut tl, term) = captured("Running mise clean in 1 worktree");
        tl.set_ordered_receipts(true);
        tl.set_row_output(exec_verbose());
        tl.commit_plan(PlanCommit::new(vec![exec_row("master")]));
        let k = exec_key("master");
        tl.on_stage(&k, StageEvent::Started);
        tl.handle().push_row_output(&k, "[clean] artifacts cleaned");
        tl.handle().push_row_output(&k, "[clean] done");
        tl.on_stage(&k, StageEvent::Completed { annotation: None });
        tl.finish("Done in 0.1s");
        assert_eq!(
            term.contents(),
            "\u{250c}  Running mise clean in 1 worktree\n\
             \u{2502}\n\
             \u{2713}  master\n\
             \u{2502}    [clean] artifacts cleaned\n\
             \u{2502}    [clean] done\n\
             \u{2502}\n\
             \u{2514}  Done in 0.1s"
        );
    }

    #[test]
    fn default_failure_threads_output_but_success_stays_compact() {
        let (mut tl, term) = captured("Running mise clean in 2 worktrees");
        tl.set_ordered_receipts(true);
        tl.set_row_output(exec_default());
        tl.commit_plan(PlanCommit::new(vec![exec_row("ok"), exec_row("bad")]));
        let ok = exec_key("ok");
        tl.on_stage(&ok, StageEvent::Started);
        tl.handle().push_row_output(&ok, "quiet success chatter");
        tl.on_stage(&ok, StageEvent::Completed { annotation: None });
        let bad = exec_key("bad");
        tl.on_stage(&bad, StageEvent::Started);
        tl.handle().push_row_output(&bad, "boom: permission denied");
        tl.on_stage(
            &bad,
            StageEvent::Failed {
                detail: "exit 1".into(),
            },
        );
        tl.finish("Finished with failures in 0.1s");
        // The success stays a compact row (its chatter is dropped); only the
        // failure threads its captured output, in default ink.
        assert_eq!(
            term.contents(),
            "\u{250c}  Running mise clean in 2 worktrees\n\
             \u{2502}\n\
             \u{2713}  ok\n\
             \u{2717}  bad  exit 1\n\
             \u{2502}    boom: permission denied\n\
             \u{2502}\n\
             \u{2514}  Finished with failures in 0.1s"
        );
    }

    #[test]
    fn default_silent_failure_stays_compact_no_placeholder() {
        // The blind spot: a default-mode failure with no output must NOT emit
        // a `(no output)` thread line — that placeholder is verbose-only.
        let (mut tl, term) = captured("Running true in 1 worktree");
        tl.set_ordered_receipts(true);
        tl.set_row_output(exec_default());
        tl.commit_plan(PlanCommit::new(vec![exec_row("q")]));
        let k = exec_key("q");
        tl.on_stage(&k, StageEvent::Started);
        tl.on_stage(
            &k,
            StageEvent::Failed {
                detail: "exit 2".into(),
            },
        );
        tl.finish("Finished with failures in 0.1s");
        assert_eq!(
            term.contents(),
            "\u{250c}  Running true in 1 worktree\n\
             \u{2502}\n\
             \u{2717}  q  exit 2\n\
             \u{2502}\n\
             \u{2514}  Finished with failures in 0.1s"
        );
    }

    #[test]
    fn verbose_silent_row_marks_no_output() {
        let (mut tl, term) = captured("Running true in 1 worktree");
        tl.set_ordered_receipts(true);
        tl.set_row_output(exec_verbose());
        tl.commit_plan(PlanCommit::new(vec![exec_row("q")]));
        let k = exec_key("q");
        tl.on_stage(&k, StageEvent::Started);
        tl.on_stage(&k, StageEvent::Completed { annotation: None });
        tl.finish("Done in 0.1s");
        assert_eq!(
            term.contents(),
            "\u{250c}  Running true in 1 worktree\n\
             \u{2502}\n\
             \u{2713}  q\n\
             \u{2502}    (no output)\n\
             \u{2502}\n\
             \u{2514}  Done in 0.1s"
        );
    }

    // ── planning face (the region opens before the plan) ─────────────────
    //
    // The rail opens at t=0 with the seeded header, a static planning row,
    // and the stopwatch footer; `commit_plan` replaces the middle in place.
    // A command that returns without committing collapses the face without
    // a trace — resolve errors and navigation early exits keep their legacy
    // single-line output.

    #[test]
    fn planning_face_paints_header_planning_row_and_stopwatch() {
        let (mut tl, term) = captured("Removing feat/x");
        tl.open_planning("Validating branches");
        assert!(tl.region_live());
        let contents = term.contents();
        let lines: Vec<&str> = contents.lines().collect();
        assert_eq!(lines[0], "\u{250c}  Removing feat/x");
        assert_eq!(lines[1], "\u{2502}");
        // The spinner glyph varies by tick; the label is the contract.
        assert!(
            lines[2].ends_with("  Validating branches"),
            "planning row: {:?}",
            lines[2]
        );
        assert_eq!(lines[3], "\u{2502}");
        assert_eq!(lines[4], "\u{2514}  0ms");
        assert_eq!(lines.len(), 5);
        tl.abandon_planning();
    }

    #[test]
    fn committing_over_the_planning_face_matches_the_direct_commit() {
        let (mut direct, direct_term) = captured("Opening feat/x");
        direct.commit_plan(plan());

        let (mut planned, planned_term) = captured("Opening feat/x");
        planned.open_planning("Resolving branch");
        planned.commit_plan(plan());

        // The face leaves no residue: both paths paint the same plan.
        assert!(!planned_term.contents().contains("Resolving branch"));
        assert_eq!(planned_term.contents(), direct_term.contents());

        direct.finish("Ready in 0.1s");
        planned.finish("Ready in 0.1s");
        assert_eq!(planned_term.contents(), direct_term.contents());
    }

    #[test]
    fn plan_header_override_replaces_the_planning_seed() {
        let (mut tl, term) = captured("Removing 1 branch");
        tl.open_planning("Validating branches");
        tl.commit_plan(
            PlanCommit::new(vec![Row::Step(StepSpec::new(StepKey::scoped(
                StageId::RemoveWorktree,
                "a",
            )))])
            .with_header("Removing feat/a"),
        );
        let contents = term.contents();
        assert!(contents.starts_with("\u{250c}  Removing feat/a"));
        assert!(!contents.contains("Removing 1 branch"));
        tl.finish("Removed in 0.1s");
    }

    #[test]
    fn finishing_while_planning_collapses_without_a_trace() {
        let (mut tl, term) = captured("Opening feat/x");
        tl.open_planning("Resolving branch");
        tl.finish("Ready in 0.1s");
        assert!(!tl.region_live());
        assert_eq!(term.contents(), "");
    }

    #[test]
    fn aborting_while_planning_collapses_without_a_trace() {
        let (mut tl, term) = captured("Opening feat/x");
        tl.open_planning("Resolving branch");
        tl.abort("Failed after 0.1s");
        assert!(!tl.region_live());
        assert_eq!(term.contents(), "");
    }

    #[test]
    fn abandoned_face_collapses_and_the_late_finish_is_a_noop() {
        let (mut tl, term) = captured("Opening feat/x");
        tl.open_planning("Resolving branch");
        tl.abandon_planning();
        assert!(!tl.region_live());
        // The command epilogue's ordinary finish must not resurrect a footer.
        tl.finish("Ready in 0.1s");
        assert_eq!(term.contents(), "");
    }

    #[test]
    fn dropped_while_planning_collapses_without_a_trace() {
        let (mut tl, term) = captured("Opening feat/x");
        tl.open_planning("Resolving branch");
        drop(tl);
        assert_eq!(term.contents(), "");
    }

    #[test]
    fn abandon_after_commit_leaves_the_region_alone() {
        let (mut tl, term) = captured("Opening feat/x");
        tl.open_planning("Resolving branch");
        tl.commit_plan(plan());
        tl.abandon_planning();
        assert!(tl.region_live());
        complete(&mut tl, &StepKey::new(StageId::CheckOut));
        complete(&mut tl, &StepKey::new(StageId::CreateWorktree));
        tl.resolve_silently(&StepKey::new(StageId::PostCreateHooks));
        tl.finish("Ready in 0.1s");
        assert!(term.contents().ends_with("\u{2514}  Ready in 0.1s"));
    }

    #[test]
    fn warning_during_planning_persists_above_the_face() {
        let (mut tl, term) = captured("Removing feat/x");
        tl.open_planning("Validating branches");
        tl.println_above("warning: remote is unreachable");
        assert!(
            term.contents()
                .starts_with("warning: remote is unreachable")
        );
        tl.abandon_planning();
        // The face vanished; the warning is scrollback.
        assert_eq!(term.contents(), "warning: remote is unreachable");
    }

    /// Clone's plan shape: a pre-completed row (the bare phase finished
    /// before the plan could commit) leading the pending rows.
    fn clone_plan() -> PlanCommit {
        PlanCommit::new(vec![
            Row::Step(
                StepSpec::new(StepKey::new(StageId::CloneBare))
                    .with_annotation("\u{2190} file://src/proj")
                    .pre_completed(Duration::from_secs(2)),
            ),
            Row::Step(
                StepSpec::new(StepKey::new(StageId::CreateBaseWorktree)).with_annotation("master"),
            ),
            Row::Step(StepSpec::new(StepKey::new(StageId::PostCloneHooks))),
            Row::Step(StepSpec::new(StepKey::new(StageId::PostCreateHooks))),
        ])
    }

    #[test]
    fn pre_completed_rows_over_the_face_match_the_direct_commit() {
        let (mut direct, direct_term) = captured("Cloning proj");
        direct.commit_plan(clone_plan());

        let (mut planned, planned_term) = captured("Cloning proj");
        planned.open_planning("Cloning repository");
        planned.commit_plan(clone_plan());

        // The pre-completed `✓` row persists above the pending bars on both
        // paths (the println-vs-live-bars ordering the face must not
        // disturb), and the face leaves no residue.
        assert!(!planned_term.contents().contains("Cloning repository"));
        assert_eq!(planned_term.contents(), direct_term.contents());
        assert!(
            planned_term
                .contents()
                .contains("\u{2713}  Cloned repository")
        );

        for tl in [&mut direct, &mut planned] {
            complete(tl, &StepKey::new(StageId::CreateBaseWorktree));
            tl.resolve_silently(&StepKey::new(StageId::PostCloneHooks));
            tl.resolve_silently(&StepKey::new(StageId::PostCreateHooks));
            tl.finish("Ready in 2.4s");
        }
        assert_eq!(planned_term.contents(), direct_term.contents());
    }

    #[test]
    fn planning_label_update_repaints_the_face() {
        let (mut tl, term) = captured("Cloning proj");
        tl.open_planning("Cloning repository");
        tl.set_planning_label("Resolving branches");
        let contents = term.contents();
        let lines: Vec<&str> = contents.lines().collect();
        assert!(
            lines[2].ends_with("  Resolving branches"),
            "planning row: {:?}",
            lines[2]
        );
        assert!(!contents.contains("Cloning repository"));
        tl.abandon_planning();
    }

    #[test]
    fn planning_label_update_after_commit_is_a_noop() {
        let (mut tl, term) = captured("Opening feat/x");
        tl.open_planning("Resolving branch");
        tl.commit_plan(plan());
        let committed = term.contents();
        tl.set_planning_label("Too late");
        assert_eq!(term.contents(), committed);
        tl.finish("Ready in 0.1s");
        assert!(!term.contents().contains("Too late"));
    }

    #[test]
    fn reopened_face_after_abandon_commits_cleanly() {
        // The layout-prompt path: the face steps aside for the prompt, then
        // returns with a fresh label, and the plan lands on the reopened
        // face. (The test draw target is consumed per region, so the reopen
        // re-arms it — production regions each get their own stderr target.)
        let (mut direct, direct_term) = captured("Cloning proj");
        direct.commit_plan(clone_plan());

        let (mut tl, term) = captured("Cloning proj");
        tl.open_planning("Cloning repository");
        tl.abandon_planning();
        assert!(!tl.region_live());
        assert_eq!(term.contents(), "");
        tl.set_test_draw_target(indicatif::ProgressDrawTarget::term_like(Box::new(
            term.clone(),
        )));
        tl.open_planning("Resolving branches");
        assert!(tl.region_live());
        tl.commit_plan(clone_plan());
        assert_eq!(term.contents(), direct_term.contents());
    }

    #[test]
    fn open_planning_is_plain_mode_inert() {
        let mut tl = Timeline::new(TimelineMode::Plain, false, "Opening feat/x");
        tl.open_planning("Resolving branch");
        assert!(!tl.region_live());
    }

    #[test]
    fn group_after_hook_block_reuses_the_reconnect_spacer() {
        let (mut tl, term) = captured("Opening feat/x");
        tl.commit_plan(PlanCommit::new(vec![
            Row::Step(StepSpec::new(StepKey::new(StageId::CheckOut))),
            Row::Step(StepSpec::new(StepKey::new(StageId::PostCreateHooks))),
            Row::Group {
                label: "shared files".into(),
            },
            Row::Step(
                StepSpec::new(StepKey::scoped(StageId::SharedFile, ".env")).with_label(".env"),
            ),
            Row::EndGroup,
        ]));
        complete(&mut tl, &StepKey::new(StageId::CheckOut));
        let embed = tl
            .handle()
            .begin_hook_embed(&StepKey::new(StageId::PostCreateHooks))
            .expect("region live, key known");
        embed.mp.println("block content").unwrap();
        tl.close_hook_embed();
        tl.on_stage(
            &StepKey::scoped(StageId::SharedFile, ".env"),
            StageEvent::Completed { annotation: None },
        );
        tl.finish("Ready in 0.1s");
        // One spacer between the block and the anchor — the reconnect `│`
        // from close_hook_embed; print_group_at must not add a second.
        assert_eq!(
            term.contents(),
            "\u{250c}  Opening feat/x\n\
             \u{2502}\n\
             \u{2713}  Checked out branch\n\
             \u{2502}\n\
             block content\n\
             \u{2502}\n\
             \u{251c}\u{2500} shared files\n\
             \u{2502}  \u{2713}  .env\n\
             \u{2502}\n\
             \u{2514}  Ready in 0.1s"
        );
    }

    #[test]
    fn in_group_hook_embed_keeps_the_branch_gutter() {
        // Multi-branch remove: a per-branch hook section renders inside the
        // branch's span — `│  ├─ pre-remove hooks` with double-tucked job
        // rows and no rail-level spacers — so the branch anchor keeps owning
        // the span's remaining rows (a rail-level `├─` anchor used to
        // re-attach them to the hooks section instead).
        let (mut tl, term) = captured("Removing 2 branches");
        let hook_key = StepKey::scoped(StageId::PreRemoveHooks, "feat/a");
        let remove_key = StepKey::scoped(StageId::RemoveWorktree, "feat/a");
        tl.commit_plan(PlanCommit::new(vec![
            Row::Group {
                label: "feat/a".into(),
            },
            Row::Step(StepSpec::new(hook_key.clone())),
            Row::Step(StepSpec::new(remove_key.clone())),
            Row::EndGroup,
        ]));
        let embed = tl
            .handle()
            .begin_hook_embed(&hook_key)
            .expect("region live, key known");
        let mut renderer = RailHookRenderer::new(
            embed,
            tl.handle(),
            &crate::settings::HookOutputConfig::default(),
        );
        renderer.print_header("worktree-pre-remove", Some("feat/a"));
        renderer.start_job("direnv-revoke", None);
        renderer.finish_job_success("direnv-revoke", Duration::from_millis(2100));
        tl.close_hook_embed();
        complete(&mut tl, &remove_key);
        tl.finish("Removed in 0.1s");
        assert_eq!(
            term.contents(),
            "\u{250c}  Removing 2 branches\n\
             \u{2502}\n\
             \u{251c}\u{2500} feat/a\n\
             \u{2502}  \u{251c}\u{2500} pre-remove hooks\n\
             \u{2502}  \u{2502}  \u{2713}  direnv-revoke  (2.1s)\n\
             \u{2502}  \u{2713}  Removed worktree\n\
             \u{2502}\n\
             \u{2514}  Removed in 0.1s"
        );
    }

    #[test]
    fn hook_level_condition_skip_vanishes_from_the_receipt() {
        let (mut tl, term) = captured("Opening feat/x");
        tl.commit_plan(plan());
        for id in [StageId::CheckOut, StageId::CreateWorktree] {
            complete(&mut tl, &StepKey::new(id));
        }
        // `hook_def.skip` fired: the config working as designed — no row.
        tl.resolve_hook_step(
            &StepKey::new(StageId::PostCreateHooks),
            true,
            Some("skip: true"),
        );
        tl.finish("Ready in 0.1s");
        assert!(
            !term.contents().contains("post-create hooks"),
            "condition-skipped hook must leave no receipt: {}",
            term.contents()
        );
    }

    #[test]
    fn done_tense_labels_stay_in_the_annotation_column() {
        // The column is sized over every tense: "Checked out branch" (18)
        // outgrows its pending form (16), and an over-width done label
        // cannot be padded — its annotation would shear out of the shared
        // column (#688 review).
        let (mut tl, term) = captured("Opening feat/x");
        tl.commit_plan(PlanCommit::new(vec![
            Row::Step(StepSpec::new(StepKey::new(StageId::CheckOut)).with_annotation("tracking")),
            Row::Step(
                StepSpec::new(StepKey::new(StageId::CreateWorktree)).with_annotation("../feat/x"),
            ),
        ]));
        for id in [StageId::CheckOut, StageId::CreateWorktree] {
            complete(&mut tl, &StepKey::new(id));
        }
        tl.finish("Ready in 0.1s");
        let contents = term.contents();
        let column = |needle: &str| {
            contents
                .lines()
                .find_map(|l| l.find(needle))
                .unwrap_or_else(|| panic!("{needle:?} missing from {contents}"))
        };
        assert_eq!(
            column("tracking"),
            column("../feat/x"),
            "annotations must share one column: {contents}"
        );
    }

    #[test]
    fn configured_off_hook_skips_vanish_from_the_receipt() {
        // Deliberate configuration is not an attention event: a per-hook
        // `enabled: false` ("<hook-type> hook is disabled") and an empty
        // jobs list ("No jobs defined") printed nothing pre-#651 and must
        // not nag in yellow on every run.
        for reason in [
            "worktree-post-create hook is disabled",
            "No jobs defined",
            "No jobs match changed attributes",
        ] {
            let (mut tl, term) = captured("Opening feat/x");
            tl.commit_plan(plan());
            for id in [StageId::CheckOut, StageId::CreateWorktree] {
                complete(&mut tl, &StepKey::new(id));
            }
            tl.resolve_hook_step(&StepKey::new(StageId::PostCreateHooks), true, Some(reason));
            tl.finish("Ready in 0.1s");
            assert!(
                !term.contents().contains("post-create hooks"),
                "a configured-off hook ({reason:?}) must leave no receipt: {}",
                term.contents()
            );
        }
    }

    #[test]
    fn hook_level_attention_skip_stays_yellow() {
        let (mut tl, term) = captured("Opening feat/x");
        tl.commit_plan(plan());
        for id in [StageId::CheckOut, StageId::CreateWorktree] {
            complete(&mut tl, &StepKey::new(id));
        }
        tl.resolve_hook_step(
            &StepKey::new(StageId::PostCreateHooks),
            true,
            Some("Repository not trusted"),
        );
        tl.finish("Ready in 0.1s");
        assert!(
            term.contents()
                .contains("post-create hooks     skipped \u{2014} Repository not trusted"),
            "trust refusal must stay visible: {}",
            term.contents()
        );
    }

    #[test]
    fn silent_group_span_leaves_no_spacer_residue() {
        let (mut tl, term) = captured("Opening feat/x");
        tl.commit_plan(PlanCommit::new(vec![
            Row::Step(StepSpec::new(StepKey::new(StageId::CreateWorktree))),
            Row::Group {
                label: "shared files".into(),
            },
            Row::Step(
                StepSpec::new(StepKey::scoped(StageId::SharedFile, ".env")).with_label(".env"),
            ),
            Row::EndGroup,
        ]));
        complete(&mut tl, &StepKey::new(StageId::CreateWorktree));
        tl.on_stage(
            &StepKey::scoped(StageId::SharedFile, ".env"),
            StageEvent::SkippedSilent,
        );
        tl.finish("Ready in 0.1s");
        assert_eq!(
            term.contents(),
            "\u{250c}  Opening feat/x\n\
             \u{2502}\n\
             \u{2713}  Created worktree\n\
             \u{2502}\n\
             \u{2514}  Ready in 0.1s"
        );
    }

    // ── planned section spacers (#651 field test) ────────────────────────
    //
    // A top-level hook row opens as a `├─` section when it runs; its `│`
    // spacers are laid down with the plan, so the committed plan shows the
    // receipt's rail rhythm and starting a section never shifts the rows
    // below it.

    #[test]
    fn plan_lays_down_section_spacers_and_sections_fill_in_place() {
        let (mut tl, term) = captured("Removing feat/x");
        tl.commit_plan(PlanCommit::new(vec![
            Row::Step(StepSpec::new(StepKey::new(StageId::PreRemoveHooks))),
            Row::Step(StepSpec::new(StepKey::new(StageId::RemoveWorktree))),
            Row::Step(StepSpec::new(StepKey::new(StageId::DeleteLocalBranch))),
            Row::Step(StepSpec::new(StepKey::new(StageId::PostRemoveHooks))),
        ]));
        // The committed plan already carries the receipt's rhythm: a `│`
        // everywhere a section will open — none doubled with the header's
        // spacer (first slot) or the bottom spacer (last slot).
        assert_eq!(
            term.contents(),
            "\u{250c}  Removing feat/x\n\
             \u{2502}\n\
             \u{25cb}  pre-remove hooks\n\
             \u{2502}\n\
             \u{25cb}  Remove worktree\n\
             \u{25cb}  Delete branch\n\
             \u{2502}\n\
             \u{25cb}  post-remove hooks\n\
             \u{2502}\n\
             \u{2514}  0ms"
        );

        let pre = StepKey::new(StageId::PreRemoveHooks);
        let embed = tl.handle().begin_hook_embed(&pre).expect("region live");
        embed
            .mp
            .println("\u{251c}\u{2500} pre-remove hooks")
            .unwrap();
        embed
            .mp
            .println("\u{2502}  \u{2713}  direnv-revoke")
            .unwrap();
        tl.close_hook_embed();
        complete(&mut tl, &StepKey::new(StageId::RemoveWorktree));
        complete(&mut tl, &StepKey::new(StageId::DeleteLocalBranch));
        let post = StepKey::new(StageId::PostRemoveHooks);
        let embed = tl.handle().begin_hook_embed(&post).expect("region live");
        embed
            .mp
            .println("\u{251c}\u{2500} post-remove hooks")
            .unwrap();
        embed.mp.println("\u{2502}  \u{2713}  scrub-cache").unwrap();
        tl.close_hook_embed();
        tl.finish("Removed in 0.1s");
        // Every `│` in the receipt was already in the plan: the sections
        // filled their pre-spaced slots without inserting a line.
        assert_eq!(
            term.contents(),
            "\u{250c}  Removing feat/x\n\
             \u{2502}\n\
             \u{251c}\u{2500} pre-remove hooks\n\
             \u{2502}  \u{2713}  direnv-revoke\n\
             \u{2502}\n\
             \u{2713}  Removed worktree\n\
             \u{2713}  Deleted branch\n\
             \u{2502}\n\
             \u{251c}\u{2500} post-remove hooks\n\
             \u{2502}  \u{2713}  scrub-cache\n\
             \u{2502}\n\
             \u{2514}  Removed in 0.1s"
        );
    }

    #[test]
    fn attention_skipped_hook_keeps_its_planned_frame() {
        let (mut tl, term) = captured("Removing feat/x");
        tl.commit_plan(PlanCommit::new(vec![
            Row::Step(StepSpec::new(StepKey::new(StageId::PreRemoveHooks))),
            Row::Step(StepSpec::new(StepKey::new(StageId::RemoveWorktree))),
        ]));
        tl.resolve_hook_step(
            &StepKey::new(StageId::PreRemoveHooks),
            true,
            Some("Repository not trusted"),
        );
        complete(&mut tl, &StepKey::new(StageId::RemoveWorktree));
        tl.finish("Removed in 0.1s");
        // The section never opened, but the plan promised it air — the `↓`
        // row keeps the planned frame instead of yanking lines out.
        assert_eq!(
            term.contents(),
            "\u{250c}  Removing feat/x\n\
             \u{2502}\n\
             \u{2193}  pre-remove hooks   skipped \u{2014} Repository not trusted\n\
             \u{2502}\n\
             \u{2713}  Removed worktree\n\
             \u{2502}\n\
             \u{2514}  Removed in 0.1s"
        );
    }

    #[test]
    fn silently_skipped_hook_takes_its_planned_spacers_along() {
        let (mut tl, term) = captured("Removing feat/x");
        tl.commit_plan(PlanCommit::new(vec![
            Row::Step(StepSpec::new(StepKey::new(StageId::PreRemoveHooks))),
            Row::Step(StepSpec::new(StepKey::new(StageId::RemoveWorktree))),
        ]));
        // Hook-level `skip:` condition — the row and both its planned
        // spacers vanish together, no residue.
        tl.resolve_hook_step(
            &StepKey::new(StageId::PreRemoveHooks),
            true,
            Some("skip: true"),
        );
        complete(&mut tl, &StepKey::new(StageId::RemoveWorktree));
        tl.finish("Removed in 0.1s");
        assert_eq!(
            term.contents(),
            "\u{250c}  Removing feat/x\n\
             \u{2502}\n\
             \u{2713}  Removed worktree\n\
             \u{2502}\n\
             \u{2514}  Removed in 0.1s"
        );
    }

    #[test]
    fn adjacent_hook_phases_share_one_planned_spacer() {
        let (mut tl, term) = captured("Removing feat/x");
        tl.commit_plan(PlanCommit::new(vec![
            Row::Step(StepSpec::new(StepKey::new(StageId::PreRemoveHooks))),
            Row::Step(StepSpec::new(StepKey::new(StageId::PostRemoveHooks))),
        ]));
        assert_eq!(
            term.contents(),
            "\u{250c}  Removing feat/x\n\
             \u{2502}\n\
             \u{25cb}  pre-remove hooks\n\
             \u{2502}\n\
             \u{25cb}  post-remove hooks\n\
             \u{2502}\n\
             \u{2514}  0ms"
        );
    }

    #[test]
    fn hook_row_before_a_group_leans_on_its_spacer() {
        let (mut tl, term) = captured("Opening feat/x");
        tl.commit_plan(PlanCommit::new(vec![
            Row::Step(StepSpec::new(StepKey::new(StageId::PostCreateHooks))),
            Row::Group {
                label: "shared files".into(),
            },
            Row::Step(
                StepSpec::new(StepKey::scoped(StageId::SharedFile, ".env")).with_label(".env"),
            ),
            Row::EndGroup,
        ]));
        // The group's own spacer provides the gap below the hook row —
        // exactly one `│` between them in the plan.
        assert_eq!(
            term.contents(),
            "\u{250c}  Opening feat/x\n\
             \u{2502}\n\
             \u{25cb}  post-create hooks\n\
             \u{2502}\n\
             \u{251c}\u{2500} shared files\n\
             \u{2502}  \u{25cb}  .env\n\
             \u{2502}\n\
             \u{2514}  0ms"
        );
    }

    #[test]
    fn section_reconnect_lands_before_a_trailing_note() {
        let (mut tl, term) = captured("Removing feat/x");
        tl.commit_plan(PlanCommit::new(vec![
            Row::Step(StepSpec::new(StepKey::new(StageId::PostRemoveHooks))),
            Row::Note {
                text: "no remote branch".into(),
            },
        ]));
        let post = StepKey::new(StageId::PostRemoveHooks);
        let embed = tl.handle().begin_hook_embed(&post).expect("region live");
        embed.mp.println("block content").unwrap();
        tl.close_hook_embed();
        tl.finish("Removed in 0.1s");
        // A note is visible content too: the planned below-`│` persists as
        // the reconnect even though no *step* remains.
        assert_eq!(
            term.contents(),
            "\u{250c}  Removing feat/x\n\
             \u{2502}\n\
             block content\n\
             \u{2502}\n\
             \u{25cb}  no remote branch\n\
             \u{2502}\n\
             \u{2514}  Removed in 0.1s"
        );
    }

    #[test]
    fn verbose_section_before_later_steps_leaves_no_footer_ghost() {
        // #651 field test: `daft remove -v` stranded the footer placeholder
        // (`│` + `└ …`) in scrollback above the real footer. Drive the real
        // verbose renderer over a remove-shaped plan (hook phase FIRST, spine
        // steps after) and assert the receipt contains exactly one footer.
        let term = indicatif::InMemoryTerm::new(60, 100);
        let mut tl = Timeline::new(
            TimelineMode::Interactive { color: false },
            true,
            "Removing feat-x",
        );
        tl.set_test_draw_target(indicatif::ProgressDrawTarget::term_like(Box::new(
            term.clone(),
        )));
        tl.commit_plan(PlanCommit::new(vec![
            Row::Step(StepSpec::new(StepKey::new(StageId::PreRemoveHooks))),
            Row::Step(StepSpec::new(StepKey::new(StageId::RemoveWorktree))),
            Row::Step(StepSpec::new(StepKey::new(StageId::DeleteLocalBranch))),
        ]));
        let pre = StepKey::new(StageId::PreRemoveHooks);
        let embed = tl.handle().begin_hook_embed(&pre).expect("region live");
        let config = crate::settings::HookOutputConfig {
            verbose: true,
            ..Default::default()
        };
        let mut renderer = RailHookRenderer::new(embed, tl.handle(), &config);
        renderer.print_header("worktree-pre-remove", None);
        renderer.start_job_with_description("pg-test", None, Some("echo pg-test"));
        renderer.update_job_output("pg-test", "pg-test");
        renderer.finish_job_success("pg-test", std::time::Duration::from_millis(400));
        renderer.print_summary(std::time::Duration::from_millis(400));
        drop(renderer);
        tl.close_hook_embed();
        // `-v` free-text detail rides under the active row — the real remove
        // emits these ("Removing worktree at …", "Deleting local branch …").
        let wt = StepKey::new(StageId::RemoveWorktree);
        tl.on_stage(&wt, StageEvent::Started);
        tl.detail("Removing worktree at /tmp/x/proj/feat-x...");
        tl.detail("Removed worktree 'feat-x'");
        tl.on_stage(&wt, StageEvent::Completed { annotation: None });
        let br = StepKey::new(StageId::DeleteLocalBranch);
        tl.on_stage(&br, StageEvent::Started);
        tl.detail("Deleting local branch feat-x...");
        tl.on_stage(&br, StageEvent::Completed { annotation: None });
        tl.detail("No worktree-post-remove hooks found");
        tl.finish("Removed in 0.1s");
        let contents = term.contents();
        assert!(
            !contents.contains('\u{2026}'),
            "the footer placeholder must not survive into the receipt:\n{contents}"
        );
        // Exactly one rail end on the spine (the section's own `└ all jobs…`
        // note sits inside the gutter, not at line start).
        assert_eq!(
            contents
                .lines()
                .filter(|l| l.starts_with('\u{2514}'))
                .count(),
            1,
            "exactly one rail end:\n{contents}"
        );
    }

    #[test]
    fn span_notes_and_not_reached_rows_stay_in_the_gutter() {
        // Every face a span row can persist with — a note, a completed step,
        // and a not-reached step on the failure teardown — renders tucked
        // into the gutter; the spine row before the section does not.
        let (mut tl, term) = captured("Removing feat/a");
        tl.commit_plan(PlanCommit::new(vec![
            Row::Group {
                label: "feat/a".into(),
            },
            Row::Note {
                text: "no remote branch".into(),
            },
            Row::Step(StepSpec::new(StepKey::scoped(StageId::RemoveWorktree, "a"))),
            Row::Step(StepSpec::new(StepKey::scoped(
                StageId::DeleteLocalBranch,
                "a",
            ))),
        ]));
        let key = StepKey::scoped(StageId::RemoveWorktree, "a");
        tl.on_stage(&key, StageEvent::Started);
        tl.on_stage(
            &key,
            StageEvent::Failed {
                detail: "dirty".into(),
            },
        );
        tl.abort("Failed after 0.1s");
        assert_eq!(
            term.contents(),
            "\u{250c}  Removing feat/a\n\
             \u{2502}\n\
             \u{251c}\u{2500} feat/a\n\
             \u{2502}  \u{25cb}  no remote branch\n\
             \u{2502}  \u{2717}  Remove worktree    dirty\n\
             \u{2502}  \u{25cb}  Delete branch      (not run)\n\
             \u{2502}\n\
             \u{2514}  Failed after 0.1s"
        );
    }
}
