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
}

impl ConsolidationPrompter for CommandBridge<'_> {
    fn on_refined(&mut self, req: &ConsolidationRequest) -> ConsolidationChoice {
        // The summary must be visible above the prompt, so suspend any
        // running spinner for the duration (same contract as run_hook).
        self.output.pause_spinner();
        self.output.info(&format!(
            "Worktree '{}' has refined daft files not in {}:",
            req.branch, req.target_display
        ));
        for file in &req.files {
            if file.whole_file {
                self.output.info(&format!(
                    "  {} — no seed provenance; consolidating overlays the whole file",
                    file.filename
                ));
                continue;
            }
            if !file.adopt_keys.is_empty() {
                self.output.info(&format!(
                    "  {} — would adopt: {}",
                    file.filename,
                    file.adopt_keys.join(", ")
                ));
            }
            if !file.conflict_keys.is_empty() {
                self.output.info(&format!(
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
        self.output.resume_spinner();
        match result {
            PromptResult::Selected('c') => ConsolidationChoice::Consolidate,
            PromptResult::Selected('d') => ConsolidationChoice::Discard,
            _ => ConsolidationChoice::Abort,
        }
    }

    fn on_conflicts(&mut self, filename: &str, keys: &[String]) -> ConflictSide {
        self.output.pause_spinner();
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
        self.output.resume_spinner();
        match result {
            PromptResult::Selected('s') => ConflictSide::Source,
            PromptResult::Selected('t') => ConflictSide::Target,
            _ => ConflictSide::Abort,
        }
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
