//! Reporter port: presentation layer for the manual test runner.
//!
//! `Reporter` abstracts the runner's emit surface so that:
//! - parallel workers (each owning a `Vec<u8>` buffer) and the serial/interactive
//!   runner (writing to stderr) share the same formatting code,
//! - verbosity is a presenter pick (`Verbosity::Quiet | Default | Verbose | VeryVerbose`)
//!   rather than `if verbose` branches scattered through the runner,
//! - the final summary (failed-scenarios + reproduce blocks) is owned by the
//!   presenter, not glued inline at the orchestrator.
//!
//! Mirrors the `CommandExecutor` port style established by #516.

pub mod pretty;
pub mod quiet;

#[cfg(test)]
mod tests;

use std::io::{self, Write};
use std::path::Path;
use std::time::Duration;

use super::runner::AssertionResult;
use super::schema::{Scenario, Step};

/// User-facing verbosity ladder. Resolved once at the CLI boundary.
#[derive(Copy, Clone, Eq, PartialEq, Debug)]
pub enum Verbosity {
    /// `-q` / `--quiet`: scenario PASS/FAIL footer + final summary only.
    Quiet,
    /// (no flag): scenario header, per-step lines, summary, captured output on fail.
    Default,
    /// `-v`: like `Default` plus per-check icons + captured output on pass.
    Verbose,
    /// `-vv`: like `Verbose` plus expanded commands + full untruncated captured output.
    VeryVerbose,
}

impl Verbosity {
    /// Resolve clap output into a single level. `quiet` wins if both passed.
    pub fn from_flags(verbose: u8, quiet: bool) -> Self {
        if quiet {
            return Verbosity::Quiet;
        }
        match verbose {
            0 => Verbosity::Default,
            1 => Verbosity::Verbose,
            _ => Verbosity::VeryVerbose,
        }
    }
}

/// Per-step snapshot passed to the reporter for `step_pass` / `step_fail`.
///
/// The per-step prefix (`[N/M] name ... `) is owned by `Reporter::step_start`,
/// so `StepReport` only carries the bits the pass/fail continuation needs.
pub struct StepReport<'a> {
    /// Sandbox-expanded `run` command. Populated by the runner when available
    /// (so the reporter doesn't need to know about `Sandbox::expand_vars`).
    /// Shown at `Verbose` and above as `$ <expanded>`.
    pub expanded_command: Option<&'a str>,
    /// All assertion outcomes for this step (in declaration order).
    pub assertions: &'a [AssertionResult],
    /// Captured stdout from the step's command, if quiet capture was on.
    pub stdout: Option<&'a str>,
    /// Captured stderr from the step's command, if quiet capture was on.
    pub stderr: Option<&'a str>,
}

/// Scenario PASS/FAIL outcome.
pub enum ScenarioStatus {
    Pass,
    Fail,
}

/// First failing step's full detail, captured for the summary block.
pub struct FailingStep {
    /// Zero-based step index.
    pub index: usize,
    /// Total step count in the scenario.
    pub total: usize,
    /// Step name (cloned to detach lifetimes from per-scenario allocations).
    pub step_name: String,
    /// 1-indexed source line where the step begins in the scenario YAML.
    /// `None` for synthetic/test scenarios that aren't loaded from a file.
    /// Rendered as `at file.yml:N` next to the failing-step line.
    pub line: Option<usize>,
    /// Failed assertions only.
    pub failed_assertions: Vec<AssertionResult>,
    /// Captured stdout at the time of failure (trimmed of trailing whitespace).
    /// Rendered as a separate labeled block in the summary so the reader can
    /// distinguish the program's normal output from its error stream.
    pub captured_stdout: String,
    /// Captured stderr at the time of failure (trimmed of trailing whitespace).
    pub captured_stderr: String,
}

/// One row in the failed-scenarios block of the summary.
pub struct FailedScenarioRecord<'a> {
    pub name: &'a str,
    /// Full source path (absolute, canonicalized). Kept for downstream uses
    /// that need the canonical location — e.g., the failing-step citation
    /// uses just the basename, but a future terminal-click feature might
    /// want the absolute path.
    pub source: &'a Path,
    /// Path string rendered in the failed-scenarios block, relative to
    /// `tests/manual/scenarios/`. Shorter than the canonical source path so
    /// the scenario name (primary content) outweighs the path metadata
    /// (tertiary) per §2.
    pub display_path: String,
    pub reproduce_token: String,
    /// Wall-clock duration of the scenario's step phase. Rendered next to the
    /// source path so the reader can spot slow failures at a glance.
    pub duration: Duration,
    pub failing_step: Option<&'a FailingStep>,
}

/// One row in the errors block of the summary (scenario hit a fatal error
/// before completing, e.g. failed to parse YAML, panicked, etc.).
pub struct ScenarioErrorRecord<'a> {
    pub name: &'a str,
    pub error: String,
}

/// Aggregated end-of-run summary fed to `Reporter::run_summary`.
pub struct RunSummary<'a> {
    pub scenarios_total: usize,
    pub scenarios_passed: usize,
    pub scenarios_failed: usize,
    pub steps_total: usize,
    pub steps_passed: usize,
    pub steps_failed: usize,
    pub duration: Duration,
    pub parallel_jobs: Option<usize>,
    pub failed: Vec<FailedScenarioRecord<'a>>,
    pub errors: Vec<ScenarioErrorRecord<'a>>,
}

/// Output port for the runner's presentation layer.
///
/// Methods take `&self` plus an injected `&mut dyn Write`, so one reporter
/// instance can be shared across parallel workers (each writing to its own
/// `Vec<u8>` buffer) and reused for the final summary on stderr without any
/// shared mutable state.
pub trait Reporter: Send + Sync {
    /// Called once at the start of each scenario, before any steps.
    fn scenario_header(&self, out: &mut dyn Write, scenario: &Scenario) -> io::Result<()>;

    /// Called at the start of each step, before execution.
    fn step_start(
        &self,
        out: &mut dyn Write,
        idx: usize,
        total: usize,
        step: &Step,
    ) -> io::Result<()>;

    /// Called when a step's assertions all pass.
    fn step_pass(&self, out: &mut dyn Write, report: &StepReport<'_>) -> io::Result<()>;

    /// Called when a step had at least one failed assertion.
    fn step_fail(&self, out: &mut dyn Write, report: &StepReport<'_>) -> io::Result<()>;

    /// Called once at the end of each scenario.
    ///
    /// `duration` is the wall-clock time spent in the scenario's step phase.
    /// Passing `Duration::ZERO` suppresses the duration suffix — used by the
    /// interactive runner, where wall-clock includes time the user spent at
    /// the prompt and would lie about scenario performance.
    fn scenario_footer(
        &self,
        out: &mut dyn Write,
        scenario: &Scenario,
        status: ScenarioStatus,
        duration: Duration,
    ) -> io::Result<()>;

    /// Called for ancillary cleanup notes (kept env, cleanup warning, etc.).
    fn cleanup_note(&self, out: &mut dyn Write, msg: &str) -> io::Result<()>;

    /// Called once at the end of a run with the full summary block.
    fn run_summary(&self, out: &mut dyn Write, summary: &RunSummary<'_>) -> io::Result<()>;
}

/// Pick a reporter for the requested verbosity.
pub fn reporter_for(verbosity: Verbosity) -> Box<dyn Reporter> {
    match verbosity {
        Verbosity::Quiet => Box::new(quiet::QuietReporter::new()),
        _ => Box::new(pretty::PrettyReporter::new(verbosity)),
    }
}

/// Derive the runner-addressable token for a scenario's source path.
///
/// Inverse of `resolve_scenario_paths`. Examples:
/// - `tests/manual/scenarios/hooks/silent-job-logs.yml` → `hooks:silent-job-logs`
/// - `tests/manual/scenarios/clone-basic.yml` → `clone-basic`
///
/// Used to print copy-pasteable `mise run test:manual -- --ci <token>` lines in
/// the reproduce block.
pub fn reproduce_token(source: &Path, scenarios_dir: &Path) -> String {
    let relative = source.strip_prefix(scenarios_dir).unwrap_or(source);
    let stripped = relative.with_extension("");
    stripped
        .to_string_lossy()
        .replace(std::path::MAIN_SEPARATOR, ":")
}
