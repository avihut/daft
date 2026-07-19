//! `Output` implementation for code that runs while the rail region is live.
//!
//! `HookExecutor::execute` takes a `&mut dyn Output` for its warnings, errors
//! and defensive spinner clears. During an embedded hook block those writes
//! must compose with the region: warnings/errors go above the live bars via
//! `mp.println`; stdout writes happen under `MultiProgress::suspend`; and —
//! critically — the spinner methods are **no-ops**, so the executor's
//! defensive `finish_spinner()` (yaml_executor) cannot tear the rail down.

use super::TimelineHandle;
use crate::output::Output;
use crate::styles;
use std::path::Path;

/// `warning: <msg>` in the CliOutput vocabulary, for printing above the
/// live bars.
pub fn warning_line(msg: &str) -> String {
    if styles::colors_enabled_stderr() {
        format!("{}warning:{} {msg}", styles::YELLOW, styles::RESET)
    } else {
        format!("warning: {msg}")
    }
}

/// `error: <msg>` in the CliOutput vocabulary, for printing above the
/// live bars.
pub fn error_line(msg: &str) -> String {
    if styles::colors_enabled_stderr() {
        format!("{}error:{} {msg}", styles::RED, styles::RESET)
    } else {
        format!("error: {msg}")
    }
}

pub struct RegionOutput {
    handle: TimelineHandle,
    quiet: bool,
    /// `-v` free-text chatter, not the rail's job-log density: this is the
    /// `Output` contract's verbosity, and the live `v` toggle (#729)
    /// deliberately leaves it alone — pressing `v` asks for job output, not
    /// for debug lines.
    verbose: bool,
}

impl RegionOutput {
    pub fn new(handle: TimelineHandle, quiet: bool, verbose: bool) -> Self {
        Self {
            handle,
            quiet,
            verbose,
        }
    }

    fn stdout_line(&self, line: String) {
        self.handle.suspend(|| println!("{line}"));
    }
}

impl Output for RegionOutput {
    fn info(&mut self, msg: &str) {
        if !self.quiet {
            self.stdout_line(msg.to_string());
        }
    }

    fn success(&mut self, msg: &str) {
        if !self.quiet {
            if styles::colors_enabled() {
                self.stdout_line(format!("{}{msg}{}", styles::GREEN, styles::RESET));
            } else {
                self.stdout_line(msg.to_string());
            }
        }
    }

    fn warning(&mut self, msg: &str) {
        self.handle.println_above(&warning_line(msg));
    }

    fn notice(&mut self, msg: &str) {
        // Neutral fact line (no `warning:` prefix, no styling of its own —
        // see `Output::notice`); persists above the live bars like a warning.
        self.handle.println_above(msg);
    }

    fn error(&mut self, msg: &str) {
        self.handle.println_above(&error_line(msg));
    }

    fn debug(&mut self, msg: &str) {
        if self.verbose {
            self.handle.detail(msg);
        }
    }

    fn step(&mut self, msg: &str) {
        // Free-text step detail rides under the active row (`-v` only —
        // `detail` gates internally on the timeline's verbose flag).
        self.handle.detail(msg);
    }

    fn result(&mut self, msg: &str) {
        if !self.quiet {
            if styles::colors_enabled() {
                self.stdout_line(format!("{}{msg}{}", styles::BOLD, styles::RESET));
            } else {
                self.stdout_line(msg.to_string());
            }
        }
    }

    #[allow(deprecated)]
    fn progress(&mut self, msg: &str) {
        self.step(msg);
    }

    #[allow(deprecated)]
    fn divider(&mut self) {}

    fn detail(&mut self, key: &str, value: &str) {
        if !self.quiet {
            self.stdout_line(format!("  {key}: {value}"));
        }
    }

    fn list_item(&mut self, item: &str) {
        if !self.quiet {
            self.stdout_line(format!(" - {item}"));
        }
    }

    fn operation_start(&mut self, operation: &str) {
        self.step(operation);
    }

    fn operation_end(&mut self, _operation: &str, _success: bool) {}

    // The rail is not the legacy spinner: the executor's defensive
    // `finish_spinner()` and the bridge's pause/resume bracketing must not
    // touch the region.
    fn start_spinner(&mut self, _msg: &str) {}
    fn finish_spinner(&mut self) {}
    fn pause_spinner(&mut self) {}
    fn resume_spinner(&mut self) {}

    fn cd_path(&mut self, _path: &Path) {
        // Never called during hook execution; the owning command writes the
        // cd target through its real Output after the timeline finishes.
        debug_assert!(false, "cd_path routed through RegionOutput");
    }

    fn raw(&mut self, content: &str) {
        self.handle.suspend(|| print!("{content}"));
    }

    fn is_quiet(&self) -> bool {
        self.quiet
    }

    fn is_verbose(&self) -> bool {
        self.verbose
    }
}
