//! Adapters bridging core traits to the command layer.

use super::{
    ConflictSide, ConsolidationChoice, ConsolidationPrompter, ConsolidationRequest, HookOutcome,
    HookRunner, ProgressSink,
};
use crate::executor::cli_presenter::CliPresenter;
use crate::executor::presenter::JobPresenter;
use crate::hooks::HookExecutor;
use crate::output::Output;
use crate::prompt::{PromptConfig, PromptOption, PromptResult, single_key_select};
use crate::settings::HookOutputConfig;
use std::sync::Arc;

/// Adapter that forwards `ProgressSink` calls to an `Output` implementation.
///
/// Use this for commands that do not need hook execution (e.g., carry, fetch).
///
/// # Example
///
/// ```ignore
/// let mut output = CliOutput::new(config);
/// let mut sink = OutputSink(&mut output);
/// core::worktree::carry::execute(&params, &git, &root, &mut sink)?;
/// ```
pub struct OutputSink<'a>(pub &'a mut dyn Output);

impl ProgressSink for OutputSink<'_> {
    fn on_step(&mut self, msg: &str) {
        self.0.step(msg);
    }

    fn on_warning(&mut self, msg: &str) {
        self.0.warning(msg);
    }

    fn on_debug(&mut self, msg: &str) {
        self.0.debug(msg);
    }

    fn pause_spinner(&mut self) {
        self.0.pause_spinner();
    }

    fn resume_spinner(&mut self) {
        self.0.resume_spinner();
    }
}

/// Combined adapter for commands that need both progress reporting and hook
/// execution (e.g., checkout, clone).
///
/// Wraps an `Output` implementation and a `HookExecutor`, implementing both
/// `ProgressSink` and `HookRunner` through a single mutable reference.
///
/// # Example
///
/// ```ignore
/// let executor = HookExecutor::new(HooksConfig::default())?;
/// let result = {
///     let mut bridge = CommandBridge::new(&mut output, executor);
///     core::worktree::checkout::execute(&params, &git, &root, &mut bridge)?
/// };
/// // bridge dropped — output is available again for rendering
/// render_checkout_result(&result, &mut output);
/// ```
pub struct CommandBridge<'a> {
    output: &'a mut dyn Output,
    executor: HookExecutor,
    output_config: HookOutputConfig,
}

impl<'a> CommandBridge<'a> {
    pub fn new(output: &'a mut dyn Output, executor: HookExecutor) -> Self {
        Self {
            output,
            executor,
            output_config: HookOutputConfig::default(),
        }
    }

    /// Create a bridge with a custom hook output configuration.
    pub fn with_output_config(
        output: &'a mut dyn Output,
        executor: HookExecutor,
        output_config: HookOutputConfig,
    ) -> Self {
        Self {
            output,
            executor,
            output_config,
        }
    }
}

impl ProgressSink for CommandBridge<'_> {
    fn on_step(&mut self, msg: &str) {
        self.output.step(msg);
    }

    fn on_warning(&mut self, msg: &str) {
        self.output.warning(msg);
    }

    fn on_debug(&mut self, msg: &str) {
        self.output.debug(msg);
    }

    fn pause_spinner(&mut self) {
        self.output.pause_spinner();
    }

    fn resume_spinner(&mut self) {
        self.output.resume_spinner();
    }
}

/// Interactive consolidation prompt, shared by `CommandBridge` and
/// `TimelineBridge`. The prompts fire during *validation* — before any
/// timeline region materializes — so plain terminal IO is safe in both.
fn prompt_refined(output: &mut dyn Output, req: &ConsolidationRequest) -> ConsolidationChoice {
    // The summary must be visible above the prompt, so suspend any
    // running spinner for the duration (same contract as run_hook).
    output.pause_spinner();
    output.info(&format!(
        "Worktree '{}' has refined daft files not in {}:",
        req.branch, req.target_display
    ));
    for file in &req.files {
        if file.whole_file {
            output.info(&format!(
                "  {} — no seed provenance; consolidating overlays the whole file",
                file.filename
            ));
            continue;
        }
        if !file.adopt_keys.is_empty() {
            output.info(&format!(
                "  {} — would adopt: {}",
                file.filename,
                file.adopt_keys.join(", ")
            ));
        }
        if !file.conflict_keys.is_empty() {
            output.info(&format!(
                "  {} — conflicting keys: {}",
                file.filename,
                file.conflict_keys.join(", ")
            ));
        }
    }
    eprint!(
        "Consolidate into {}, discard, or abort? [c/d/A] ",
        req.target_display
    );
    let result = single_key_select(&PromptConfig {
        options: vec![
            PromptOption {
                key: 'c',
                label: "consolidate",
                is_default: false,
            },
            PromptOption {
                key: 'd',
                label: "discard",
                is_default: false,
            },
            PromptOption {
                key: 'a',
                label: "abort",
                is_default: true,
            },
        ],
        cancel_message: Some("Aborted.".to_string()),
    });
    eprintln!();
    output.resume_spinner();
    match result {
        PromptResult::Selected('c') => ConsolidationChoice::Consolidate,
        PromptResult::Selected('d') => ConsolidationChoice::Discard,
        _ => ConsolidationChoice::Abort,
    }
}

/// Interactive conflict-side prompt, shared by both bridges (see
/// [`prompt_refined`]).
fn prompt_conflict_side(output: &mut dyn Output, filename: &str, keys: &[String]) -> ConflictSide {
    output.pause_spinner();
    eprint!(
        "{}: keep the target's version or take the removed worktree's for {}? [s/t/A] ",
        filename,
        keys.join(", ")
    );
    let result = single_key_select(&PromptConfig {
        options: vec![
            PromptOption {
                key: 's',
                label: "source",
                is_default: false,
            },
            PromptOption {
                key: 't',
                label: "target",
                is_default: false,
            },
            PromptOption {
                key: 'a',
                label: "abort",
                is_default: true,
            },
        ],
        cancel_message: Some("Aborted.".to_string()),
    });
    eprintln!();
    output.resume_spinner();
    match result {
        PromptResult::Selected('s') => ConflictSide::Source,
        PromptResult::Selected('t') => ConflictSide::Target,
        _ => ConflictSide::Abort,
    }
}

impl ConsolidationPrompter for CommandBridge<'_> {
    fn on_refined(&mut self, req: &ConsolidationRequest) -> ConsolidationChoice {
        prompt_refined(self.output, req)
    }

    fn on_conflicts(&mut self, filename: &str, keys: &[String]) -> ConflictSide {
        prompt_conflict_side(self.output, filename, keys)
    }
}

impl HookRunner for CommandBridge<'_> {
    fn run_hook(&mut self, ctx: &crate::hooks::HookContext) -> anyhow::Result<HookOutcome> {
        let presenter: Arc<dyn JobPresenter> = CliPresenter::auto(&self.output_config);
        // The hook executor may render its own indicatif MultiProgress, which
        // would fight the outer command spinner for the stderr cursor. Hide
        // the outer spinner across the hook boundary so step-label updates
        // after the hook (e.g. "Removing worktree at <path>") stay visible.
        self.output.pause_spinner();
        let exec_result = self.executor.execute(ctx, self.output, presenter);
        self.output.resume_spinner();
        let result = exec_result?;
        Ok(HookOutcome {
            success: result.success,
            skipped: result.skipped,
            skip_reason: result.skip_reason.clone(),
        })
    }
}

/// `ProgressSink`-only adapter for rail-rendering commands whose cores do
/// not run hooks through the sink (clone's phase functions). Same routing as
/// [`TimelineBridge`]: free text becomes detail under the active row once the
/// region is live, warnings print above the bars, stage events drive rows.
pub struct TimelineSink<'a> {
    output: &'a mut dyn Output,
    timeline: &'a mut crate::output::timeline::Timeline,
}

impl<'a> TimelineSink<'a> {
    pub fn new(
        output: &'a mut dyn Output,
        timeline: &'a mut crate::output::timeline::Timeline,
    ) -> Self {
        Self { output, timeline }
    }
}

impl ProgressSink for TimelineSink<'_> {
    fn on_step(&mut self, msg: &str) {
        if self.timeline.region_live() {
            self.timeline.detail(msg);
        } else {
            self.output.step(msg);
        }
    }

    fn on_warning(&mut self, msg: &str) {
        if self.timeline.region_live() {
            self.timeline
                .println_above(&crate::output::timeline::warning_line(msg));
        } else {
            self.output.warning(msg);
        }
    }

    fn on_debug(&mut self, msg: &str) {
        if self.timeline.region_live() {
            if self.output.is_verbose() {
                self.timeline.detail(msg);
            }
        } else {
            self.output.debug(msg);
        }
    }

    fn on_plan(&mut self, plan: crate::core::stage::PlanCommit) {
        self.output.finish_spinner();
        self.timeline.commit_plan(plan);
    }

    fn on_stage(
        &mut self,
        key: &crate::core::stage::StepKey,
        event: crate::core::stage::StageEvent,
    ) {
        self.timeline.on_stage(key, event);
    }
}

/// Bridge for commands that render the plan-execute rail timeline (#651).
///
/// Behaves exactly like [`CommandBridge`] until the core commits its plan
/// (`on_plan`): free-text steps drive the command's resolve spinner, prompts
/// use plain terminal IO. Once the region is live, steps become dim detail
/// sub-lines, warnings route above the live bars, and hook phases render as
/// embedded blocks inside the rail.
pub struct TimelineBridge<'a> {
    output: &'a mut dyn Output,
    timeline: &'a mut crate::output::timeline::Timeline,
    executor: HookExecutor,
    output_config: HookOutputConfig,
}

impl<'a> TimelineBridge<'a> {
    pub fn new(
        output: &'a mut dyn Output,
        timeline: &'a mut crate::output::timeline::Timeline,
        executor: HookExecutor,
        output_config: HookOutputConfig,
    ) -> Self {
        Self {
            output,
            timeline,
            executor,
            output_config,
        }
    }
}

impl ProgressSink for TimelineBridge<'_> {
    fn on_step(&mut self, msg: &str) {
        if self.timeline.region_live() {
            self.timeline.detail(msg);
        } else {
            self.output.step(msg);
        }
    }

    fn on_warning(&mut self, msg: &str) {
        if self.timeline.region_live() {
            self.timeline
                .println_above(&crate::output::timeline::warning_line(msg));
        } else {
            self.output.warning(msg);
        }
    }

    fn on_debug(&mut self, msg: &str) {
        if self.timeline.region_live() {
            if self.output.is_verbose() {
                self.timeline.detail(msg);
            }
        } else {
            self.output.debug(msg);
        }
    }

    fn on_plan(&mut self, plan: crate::core::stage::PlanCommit) {
        // The resolve-phase spinner ends where the plan begins.
        self.output.finish_spinner();
        self.timeline.commit_plan(plan);
    }

    fn on_stage(
        &mut self,
        key: &crate::core::stage::StepKey,
        event: crate::core::stage::StageEvent,
    ) {
        self.timeline.on_stage(key, event);
    }
}

impl HookRunner for TimelineBridge<'_> {
    fn run_hook(&mut self, ctx: &crate::hooks::HookContext) -> anyhow::Result<HookOutcome> {
        use crate::core::stage::StageId;

        // Embedded path: the region is live and the plan has a row for this
        // hook phase (scoped by the context's branch when the plan is
        // multi-branch). The presenter is lazy — the row expands into the
        // hook block only if the executor actually starts the phase.
        let embed_key = StageId::for_hook_type(ctx.hook_type)
            .filter(|_| self.timeline.region_live())
            .and_then(|id| self.timeline.resolve_key(id, Some(&ctx.branch_name)));

        if let Some(key) = embed_key {
            let presenter: Arc<dyn JobPresenter> =
                CliPresenter::embedded(&self.output_config, self.timeline.handle(), key.clone());
            let mut region_output = crate::output::timeline::RegionOutput::new(
                self.timeline.handle(),
                self.output.is_quiet(),
                self.output.is_verbose(),
            );
            let result = self.executor.execute(ctx, &mut region_output, presenter)?;
            self.timeline
                .resolve_hook_step(&key, result.skipped, result.skip_reason.as_deref());
            return Ok(HookOutcome {
                success: result.success,
                skipped: result.skipped,
                skip_reason: result.skip_reason.clone(),
            });
        }

        // Region-less (Plain/Hidden, or pre-plan): byte-identical to
        // CommandBridge.
        let presenter: Arc<dyn JobPresenter> = CliPresenter::auto(&self.output_config);
        self.output.pause_spinner();
        let exec_result = self.executor.execute(ctx, self.output, presenter);
        self.output.resume_spinner();
        let result = exec_result?;
        Ok(HookOutcome {
            success: result.success,
            skipped: result.skipped,
            skip_reason: result.skip_reason.clone(),
        })
    }
}

impl ConsolidationPrompter for TimelineBridge<'_> {
    fn on_refined(&mut self, req: &ConsolidationRequest) -> ConsolidationChoice {
        debug_assert!(
            !self.timeline.region_live(),
            "consolidation prompts must precede the plan commit"
        );
        prompt_refined(self.output, req)
    }

    fn on_conflicts(&mut self, filename: &str, keys: &[String]) -> ConflictSide {
        debug_assert!(
            !self.timeline.region_live(),
            "consolidation prompts must precede the plan commit"
        );
        prompt_conflict_side(self.output, filename, keys)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::hooks::{HookContext, HookExecutor, HookType, HooksConfig};
    use crate::output::{OutputEntry, TestOutput};
    use std::path::PathBuf;

    /// Regression test: the outer spinner must be paused around hook execution
    /// so the hook's own progress UI (indicatif MultiProgress) does not clobber
    /// it. Without this, step-label updates like "Removing worktree at <path>"
    /// stay invisible after a pre-remove hook finishes, leaving the user with
    /// an unexplained pause during the filesystem-delete phase.
    #[test]
    fn run_hook_brackets_executor_with_spinner_pause_resume() {
        let mut output = TestOutput::new();
        output.start_spinner("Deleting branches...");

        // Hooks globally disabled so execute() short-circuits without touching
        // the filesystem. The wrapping must happen regardless of the inner
        // outcome — that is the contract under test.
        let hooks_config = HooksConfig {
            enabled: false,
            ..HooksConfig::default()
        };
        let executor = HookExecutor::new(hooks_config).expect("create executor");

        let ctx = HookContext::new(
            HookType::PreRemove,
            "test-remove",
            PathBuf::from("/tmp/project"),
            PathBuf::from("/tmp/project/.git"),
            "origin",
            PathBuf::from("/tmp/project"),
            PathBuf::from("/tmp/project/feature"),
            "feature",
        );

        {
            let mut bridge = CommandBridge::new(&mut output, executor);
            bridge.run_hook(&ctx).expect("run_hook");
        }

        let entries = output.entries();
        let start_idx = entries
            .iter()
            .position(|e| matches!(e, OutputEntry::SpinnerStart(_)))
            .expect("SpinnerStart entry missing");
        let pause_idx = entries
            .iter()
            .position(|e| matches!(e, OutputEntry::SpinnerPause))
            .expect("SpinnerPause entry missing — run_hook must suspend the outer spinner");
        let resume_idx = entries
            .iter()
            .position(|e| matches!(e, OutputEntry::SpinnerResume))
            .expect("SpinnerResume entry missing — run_hook must resume the outer spinner");

        assert!(
            start_idx < pause_idx,
            "spinner must be started before being paused"
        );
        assert!(
            pause_idx < resume_idx,
            "pause must come before resume around hook execution"
        );
    }
}
