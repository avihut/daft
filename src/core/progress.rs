//! Adapters bridging core traits to the command layer.

use super::{HookOutcome, HookRunner, ProgressSink};
use crate::hooks::HookExecutor;
use crate::output::Output;

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
}

impl<'a> CommandBridge<'a> {
    pub fn new(output: &'a mut dyn Output, executor: HookExecutor) -> Self {
        Self { output, executor }
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

impl HookRunner for CommandBridge<'_> {
    fn run_hook(&mut self, ctx: &crate::hooks::HookContext) -> anyhow::Result<HookOutcome> {
        let result = self.executor.execute(ctx, self.output)?;
        Ok(HookOutcome {
            success: result.success,
            skipped: result.skipped,
            skip_reason: if result.skipped {
                Some(result.stderr.clone())
            } else {
                None
            },
        })
    }
}
