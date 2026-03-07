//! CLI output implementation.
//!
//! This implementation preserves the exact current output behavior of the codebase,
//! ensuring backward compatibility during the migration.

use super::{Output, OutputConfig};
use crate::styles::{self, colors_enabled, colors_enabled_stderr};
use crate::CD_FILE_ENV;
use indicatif::{ProgressBar, ProgressStyle};
use std::env;
use std::path::Path;
use std::time::Duration;

/// CLI output implementation that writes directly to stdout/stderr.
///
/// Git-like output format:
/// - `step()` → verbose only, no prefix
/// - `result()` → primary output, always shown (unless quiet)
/// - `warning()` → `eprintln!("warning: {msg}")`
/// - `error()` → `eprintln!("error: {msg}")`
/// - `progress()` → deprecated, delegates to `step()`
/// - `divider()` → deprecated, no-op
pub struct CliOutput {
    config: OutputConfig,
    spinner: Option<ProgressBar>,
}

impl CliOutput {
    /// Create a new CLI output with the given configuration.
    pub fn new(config: OutputConfig) -> Self {
        Self {
            config,
            spinner: None,
        }
    }

    /// Create a CLI output with default (non-quiet, non-verbose) settings.
    pub fn default_output() -> Self {
        Self::new(OutputConfig::default())
    }

    /// Create a CLI output in quiet mode.
    pub fn quiet() -> Self {
        Self::new(OutputConfig::new(true, false))
    }

    /// Create a CLI output in verbose mode.
    pub fn verbose() -> Self {
        Self::new(OutputConfig::new(false, true))
    }
}

impl CliOutput {
    /// Print a line to stdout, suspending any active spinner first.
    fn stdout_line(&self, line: &str) {
        if let Some(ref spinner) = self.spinner {
            spinner.suspend(|| println!("{line}"));
        } else {
            println!("{line}");
        }
    }

    /// Print a line to stderr, printing above any active spinner.
    fn stderr_line(&self, line: &str) {
        if let Some(ref spinner) = self.spinner {
            // println() prints above the spinner and redraws it below,
            // keeping indicatif's cursor tracking correct.
            spinner.println(line);
        } else {
            eprintln!("{line}");
        }
    }
}

impl Output for CliOutput {
    fn info(&mut self, msg: &str) {
        if !self.config.quiet {
            self.stdout_line(msg);
        }
    }

    fn success(&mut self, msg: &str) {
        if !self.config.quiet {
            if colors_enabled() {
                self.stdout_line(&format!("{}{msg}{}", styles::GREEN, styles::RESET));
            } else {
                self.stdout_line(msg);
            }
        }
    }

    fn warning(&mut self, msg: &str) {
        // Warnings are always shown (not affected by quiet mode)
        // Git-like format: lowercase prefix
        if colors_enabled_stderr() {
            self.stderr_line(&format!(
                "{}warning:{} {msg}",
                styles::YELLOW,
                styles::RESET
            ));
        } else {
            self.stderr_line(&format!("warning: {msg}"));
        }
    }

    fn error(&mut self, msg: &str) {
        // Errors are always shown (not affected by quiet mode)
        // Git-like format: lowercase prefix
        if colors_enabled_stderr() {
            self.stderr_line(&format!("{}error:{} {msg}", styles::RED, styles::RESET));
        } else {
            self.stderr_line(&format!("error: {msg}"));
        }
    }

    fn debug(&mut self, msg: &str) {
        if self.config.verbose {
            if colors_enabled() {
                self.stdout_line(&format!("{}debug: {msg}{}", styles::DIM, styles::RESET));
            } else {
                self.stdout_line(&format!("debug: {msg}"));
            }
        }
    }

    fn step(&mut self, msg: &str) {
        // If a spinner is active, update its message text
        if let Some(ref spinner) = self.spinner {
            spinner.set_message(msg.to_string());
            return;
        }
        // Steps are only shown in verbose mode
        if self.config.verbose && !self.config.quiet {
            if colors_enabled() {
                self.stdout_line(&format!("{}{msg}{}", styles::DIM, styles::RESET));
            } else {
                self.stdout_line(msg);
            }
        }
    }

    fn result(&mut self, msg: &str) {
        if !self.config.quiet {
            if colors_enabled() {
                self.stdout_line(&format!("{}{msg}{}", styles::BOLD, styles::RESET));
            } else {
                self.stdout_line(msg);
            }
        }
    }

    #[allow(deprecated)]
    fn progress(&mut self, msg: &str) {
        // Legacy: now delegates to step() for verbose-only output
        self.step(msg);
    }

    #[allow(deprecated)]
    fn divider(&mut self) {
        // No-op: dividers are no longer used in git-like output
    }

    fn detail(&mut self, key: &str, value: &str) {
        if !self.config.quiet {
            if colors_enabled() {
                self.stdout_line(&format!(
                    "  {}{key}:{} {value}",
                    styles::BOLD,
                    styles::RESET
                ));
            } else {
                self.stdout_line(&format!("  {key}: {value}"));
            }
        }
    }

    fn list_item(&mut self, item: &str) {
        if !self.config.quiet {
            self.stdout_line(&format!(" - {item}"));
        }
    }

    fn operation_start(&mut self, operation: &str) {
        // In CLI mode, just print a step message (verbose only)
        self.step(operation);
    }

    fn operation_end(&mut self, operation: &str, success: bool) {
        if self.config.verbose && !self.config.quiet {
            if success {
                self.stdout_line(&format!("{operation} completed"));
            } else {
                self.stderr_line(&format!("{operation} failed"));
            }
        }
    }

    fn start_spinner(&mut self, msg: &str) {
        if self.config.quiet {
            return;
        }
        if cfg!(test) || env::var("DAFT_TESTING").is_ok() {
            return;
        }
        if !colors_enabled_stderr() {
            // Non-TTY stderr: skip spinner
            return;
        }

        let style = ProgressStyle::with_template("{spinner:.cyan} {msg}")
            .unwrap()
            .tick_chars(
                "\u{2807}\u{2819}\u{2839}\u{2838}\u{283c}\u{2834}\u{2826}\u{2827}\u{2807}\u{280f}",
            );

        // ProgressBar::new_spinner() defaults to ProgressDrawTarget::stderr().
        // Important: do NOT call set_draw_target() after creation, as that
        // replaces the target with a fresh instance that has lost track of
        // any lines already drawn, causing finish_and_clear() to fail.
        let spinner = ProgressBar::new_spinner();
        spinner.set_style(style);
        spinner.set_message(msg.to_string());
        // Force an immediate draw so the spinner is visible right away.
        // enable_steady_tick() only schedules future ticks — without this
        // explicit tick(), fast operations finish before the first draw.
        spinner.tick();
        spinner.enable_steady_tick(Duration::from_millis(80));

        self.spinner = Some(spinner);
    }

    fn finish_spinner(&mut self) {
        if let Some(spinner) = self.spinner.take() {
            spinner.finish_and_clear();
            // Belt-and-suspenders: ensure the spinner line is fully erased.
            // In some edge cases (e.g., interleaved stdout/stderr writes),
            // finish_and_clear() may not fully clear the line.
            use std::io::Write;
            let _ = std::io::stderr().write_all(b"\x1b[2K\r");
            let _ = std::io::stderr().flush();
        }
    }

    fn cd_path(&mut self, path: &Path) {
        if self.config.autocd {
            if let Ok(cd_file) = env::var(CD_FILE_ENV) {
                if let Err(e) = std::fs::write(&cd_file, path.display().to_string()) {
                    self.stderr_line(&format!(
                        "warning: failed to write cd path to {cd_file}: {e}"
                    ));
                }
            }
        }
    }

    fn raw(&mut self, content: &str) {
        // Raw output is not affected by quiet mode - it's explicit content
        if let Some(ref spinner) = self.spinner {
            spinner.suspend(|| print!("{content}"));
        } else {
            print!("{content}");
        }
    }

    fn is_quiet(&self) -> bool {
        self.config.quiet
    }

    fn is_verbose(&self) -> bool {
        self.config.verbose
    }
}

impl Drop for CliOutput {
    fn drop(&mut self) {
        self.finish_spinner();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_cli_output_default() {
        let output = CliOutput::default_output();
        assert!(!output.is_quiet());
        assert!(!output.is_verbose());
    }

    #[test]
    fn test_cli_output_quiet() {
        let output = CliOutput::quiet();
        assert!(output.is_quiet());
        assert!(!output.is_verbose());
    }

    #[test]
    fn test_cli_output_verbose() {
        let output = CliOutput::verbose();
        assert!(!output.is_quiet());
        assert!(output.is_verbose());
    }

    #[test]
    fn test_cli_output_config() {
        let config = OutputConfig::new(true, true);
        let output = CliOutput::new(config);
        assert!(output.is_quiet());
        assert!(output.is_verbose());
    }
}
