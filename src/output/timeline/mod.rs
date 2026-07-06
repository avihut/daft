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
mod region;
mod render;

pub use bridge::RegionOutput;
pub use bridge::{error_line, warning_line};
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
        inner.core = Some(TimelineCore::new(
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
                // Shared-file reasons are self-contained sentences (missing,
                // conflict, error) — the generic prefix would stutter.
                let annotation = if key.id == crate::core::stage::StageId::SharedFile {
                    reason
                } else {
                    format!("skipped \u{2014} {reason}")
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
        let core = {
            let mut inner = self.handle.inner.lock().expect("timeline lock poisoned");
            inner.core.take()
        };
        if let Some(core) = core {
            core.finish(footer_text, policy);
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
}
