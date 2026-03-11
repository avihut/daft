//! No-op `Output` implementation that buffers warnings for post-TUI display.
//!
//! Used by `TuiBridge` to satisfy `HookExecutor::execute()`'s `&mut dyn Output`
//! requirement without writing to stderr (which ratatui owns).

use super::Output;
use std::path::Path;

/// An `Output` implementation that captures warnings and discards everything else.
///
/// Designed for TUI mode where stderr is owned by ratatui. Warnings from
/// `HookExecutor` (deprecation notices, trust fingerprint mismatches) are
/// buffered and can be retrieved after the TUI exits for a post-TUI summary.
pub struct BufferingOutput {
    warnings: Vec<String>,
}

impl Default for BufferingOutput {
    fn default() -> Self {
        Self::new()
    }
}

impl BufferingOutput {
    pub fn new() -> Self {
        Self {
            warnings: Vec::new(),
        }
    }

    /// Take all buffered warnings, draining the internal buffer.
    pub fn take_warnings(&mut self) -> Vec<String> {
        std::mem::take(&mut self.warnings)
    }
}

#[allow(deprecated)]
impl Output for BufferingOutput {
    fn info(&mut self, _msg: &str) {}

    fn success(&mut self, _msg: &str) {}

    fn warning(&mut self, msg: &str) {
        self.warnings.push(msg.to_string());
    }

    fn error(&mut self, _msg: &str) {}

    fn debug(&mut self, _msg: &str) {}

    fn step(&mut self, _msg: &str) {}

    fn result(&mut self, _msg: &str) {}

    fn progress(&mut self, _msg: &str) {}

    fn divider(&mut self) {}

    fn detail(&mut self, _key: &str, _value: &str) {}

    fn list_item(&mut self, _item: &str) {}

    fn operation_start(&mut self, _operation: &str) {}

    fn operation_end(&mut self, _operation: &str, _success: bool) {}

    fn start_spinner(&mut self, _msg: &str) {}

    fn finish_spinner(&mut self) {}

    fn cd_path(&mut self, _path: &Path) {}

    fn raw(&mut self, _content: &str) {}

    fn is_quiet(&self) -> bool {
        false
    }

    fn is_verbose(&self) -> bool {
        false
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn buffers_warnings() {
        let mut output = BufferingOutput::new();
        output.warning("first warning");
        output.warning("second warning");
        let warnings = output.take_warnings();
        assert_eq!(warnings, vec!["first warning", "second warning"]);
    }

    #[test]
    fn take_warnings_drains_buffer() {
        let mut output = BufferingOutput::new();
        output.warning("warning");
        let _ = output.take_warnings();
        let warnings = output.take_warnings();
        assert!(warnings.is_empty());
    }

    #[test]
    #[allow(deprecated)]
    fn discards_non_warning_messages() {
        let mut output = BufferingOutput::new();
        output.info("info");
        output.success("success");
        output.error("error");
        output.debug("debug");
        output.step("step");
        output.result("result");
        output.progress("progress");
        output.divider();
        output.detail("key", "value");
        output.list_item("item");
        output.operation_start("op");
        output.operation_end("op", true);
        output.start_spinner("spin");
        output.finish_spinner();
        output.cd_path(Path::new("/tmp"));
        output.raw("raw");
        let warnings = output.take_warnings();
        assert!(warnings.is_empty());
    }
}
