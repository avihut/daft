//! Adapter bridging the core `ProgressSink` trait to the `Output` trait.

use super::ProgressSink;
use crate::output::Output;

/// Adapter that forwards `ProgressSink` calls to an `Output` implementation.
///
/// This bridges the core layer (which knows only `ProgressSink`) with the
/// command layer (which owns the `Output`).
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
