//! Plan-then-execute rail timeline (#651).
//!
//! Renders a command's full step plan up front as a live stderr region, fills
//! each step in place as the core executes, and leaves a persistent
//! append-only receipt in scrollback. Hook phases expand in place into the
//! existing hook-progress block (welded onto the rail — see
//! `hook_progress::formatting::format_header_lines`).
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

pub use bridge::RegionOutput;
pub use bridge::{error_line, warning_line};
pub use rail_hook::RailHookRenderer;
pub use region::HookEmbed;

use crate::core::stage::{PlanCommit, StageEvent, StepKey};
use region::{FinalFace, Resolution, TimelineCore, UnresolvedPolicy};
use std::io::IsTerminal;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

/// How the timeline renders for this invocation. Decided once per command.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum TimelineMode {
    /// Live region on a TTY. `color: false` renders the same structure with
    /// zero ANSI (NO_COLOR).
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
    core: Option<TimelineCore>,
    /// Lines held back until the rail closes — a failed hook job's captured
    /// output belongs after the footer (the rail's errors-after pattern),
    /// not torn through the live bars. Drained by [`Timeline::teardown`];
    /// accumulates across hook phases. Lives here rather than on the core
    /// because `finish` consumes the core before the footer exists in
    /// scrollback.
    deferred_after_footer: Vec<String>,
    /// Test-only: a draw target injected before `commit_plan`, so sequence
    /// tests capture the persisted lines through an `InMemoryTerm` instead
    /// of the (unattended) stderr target.
    #[cfg(test)]
    test_draw_target: Option<indicatif::ProgressDrawTarget>,
}

/// Cloneable handle to the live region, for components that render into it
/// from outside the command thread (the embedded hook presenter).
#[derive(Clone)]
pub struct TimelineHandle {
    inner: Arc<Mutex<Inner>>,
}

impl TimelineHandle {
    /// Expand the given hook step into its block: the step's rail row is
    /// removed (the block replaces it) and the caller gets the shared
    /// `MultiProgress` plus a live insertion anchor. `None` when no region
    /// is live or the key is unknown.
    pub fn begin_hook_embed(&self, key: &StepKey) -> Option<HookEmbed> {
        let mut inner = self.inner.lock().expect("timeline lock poisoned");
        inner.core.as_mut()?.begin_hook_embed(key)
    }

    /// Print a permanent line above the live bars (no-op without a region).
    pub fn println_above(&self, line: &str) {
        let mut inner = self.inner.lock().expect("timeline lock poisoned");
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
        let inner = self.inner.lock().expect("timeline lock poisoned");
        inner.core.as_ref()?.resolve_key(id, scope)
    }

    /// `-v` free-text detail under the active step (no-op without a region).
    pub fn detail(&self, text: &str) {
        let mut inner = self.inner.lock().expect("timeline lock poisoned");
        if let Some(core) = inner.core.as_mut() {
            core.detail(text);
        }
    }

    /// Run `f` with the region cleared (for stdout writes that must not land
    /// mid-region). Runs `f` directly when no region is live.
    pub fn suspend<R>(&self, f: impl FnOnce() -> R) -> R {
        // Hold the lock for the duration: `f` is a short print, and the
        // region must not mutate underneath the cleared frame.
        let inner = self.inner.lock().expect("timeline lock poisoned");
        match inner.core.as_ref() {
            Some(core) => core.suspend(f),
            None => f(),
        }
    }

    /// Whether a live region currently owns the terminal.
    pub fn region_live(&self) -> bool {
        self.inner
            .lock()
            .expect("timeline lock poisoned")
            .core
            .is_some()
    }

    /// Hold `lines` back until the rail closes; the timeline prints them
    /// after the footer (blank-line separated). No-op semantics match the
    /// rest of the handle: with no region the lines still drain at teardown,
    /// but only region-embedded renderers ever defer.
    pub fn defer_after_footer(&self, lines: Vec<String>) {
        self.inner
            .lock()
            .expect("timeline lock poisoned")
            .deferred_after_footer
            .extend(lines);
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

    /// Materialize the region (Interactive only; no-op otherwise). Called by
    /// the bridge when the core commits its plan.
    pub fn commit_plan(&mut self, plan: PlanCommit) {
        if !self.is_interactive() {
            return;
        }
        let mut inner = self.handle.inner.lock().expect("timeline lock poisoned");
        if inner.core.is_some() {
            debug_assert!(false, "plan committed twice for one invocation");
            return;
        }
        // The core may replace the seeded header with the resolved intent
        // (`daft remove .` → `Removing <branch>`).
        let header = plan.header.clone().unwrap_or_else(|| inner.header.clone());
        #[cfg(not(test))]
        let mp = indicatif::MultiProgress::new();
        #[cfg(test)]
        let mp = match inner.test_draw_target.take() {
            Some(target) => indicatif::MultiProgress::with_draw_target(target),
            None => indicatif::MultiProgress::new(),
        };
        inner.core = Some(TimelineCore::new(
            mp,
            header,
            plan,
            inner.verbose,
            inner.use_color,
        ));
    }

    /// Route a stage event onto the region.
    pub fn on_stage(&mut self, key: &StepKey, event: StageEvent) {
        let mut inner = self.handle.inner.lock().expect("timeline lock poisoned");
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
            StageEvent::SkippedExpected { reason } => core.resolve(
                key,
                Resolution::Final {
                    face: FinalFace::SkippedExpected,
                    annotation: Some(reason),
                },
            ),
            StageEvent::SkippedAttention { reason } => {
                // Shared-file and fetch reasons are self-contained phrases
                // (missing, conflict, "failed — …") — the generic prefix
                // would stutter.
                let annotation = match key.id {
                    crate::core::stage::StageId::SharedFile
                    | crate::core::stage::StageId::Fetch
                    | crate::core::stage::StageId::Tracking => reason,
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

    /// The hook block for an embedded step finished rendering; reconnect
    /// the rail (no-op if no block opened).
    pub fn close_hook_embed(&mut self) {
        let mut inner = self.handle.inner.lock().expect("timeline lock poisoned");
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
            | None => self.resolve_silently(key),
            // Attention-worthy skips (trust refusal, --skip-hooks,
            // declined prompt): yellow row with the reason.
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
        let mut inner = self.handle.inner.lock().expect("timeline lock poisoned");
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
            let mut inner = self.handle.inner.lock().expect("timeline lock poisoned");
            (
                inner.core.take(),
                std::mem::take(&mut inner.deferred_after_footer),
            )
        };
        if let Some(core) = core {
            core.finish(footer_text, policy);
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

    /// Whether the live region has materialized (plan committed, not yet
    /// finished).
    pub fn region_live(&self) -> bool {
        self.handle.region_live()
    }

    pub fn handle(&self) -> TimelineHandle {
        self.handle.clone()
    }

    /// Test-only: route the region's draw calls into `target` (an
    /// `InMemoryTerm`) so tests assert the persisted line sequence, not just
    /// the state machine. Must be called before `commit_plan`.
    #[cfg(test)]
    pub(crate) fn set_test_draw_target(&self, target: indicatif::ProgressDrawTarget) {
        self.handle
            .inner
            .lock()
            .expect("timeline lock poisoned")
            .test_draw_target = Some(target);
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
    // `│` spacer then a `├  <label>` anchor, never doubled.

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
             \u{251c}  shared files\n\
             \u{2713}  .env\n\
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
             \u{251c}  feat/a\n\
             \u{2713}  Removed worktree\n\
             \u{2502}\n\
             \u{251c}  feat/b\n\
             \u{2713}  Removed worktree\n\
             \u{2502}\n\
             \u{2514}  Removed 2 worktrees in 0.1s"
        );
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
             \u{251c}  shared files\n\
             \u{2713}  .env\n\
             \u{2502}\n\
             \u{2514}  Ready in 0.1s"
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
}
