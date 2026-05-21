//! `-q` reporter: scenario PASS/FAIL footer + final summary only.
//!
//! Per-step lines, expanded commands, and captured output are suppressed —
//! the failed-scenarios block in the summary still carries the full failure
//! detail, so CI green-path / bench runs stay quiet but failures are not
//! silently swallowed.

use std::io::{self, Write};
use std::time::Duration;

use term_styles as styles;

use super::{Reporter, RunSummary, ScenarioStatus, StepReport};
use crate::manual_test::schema::{Scenario, Step};

pub struct QuietReporter;

impl QuietReporter {
    pub fn new() -> Self {
        Self
    }
}

impl Default for QuietReporter {
    fn default() -> Self {
        Self::new()
    }
}

impl Reporter for QuietReporter {
    fn scenario_header(&self, _out: &mut dyn Write, _scenario: &Scenario) -> io::Result<()> {
        Ok(())
    }

    fn step_start(
        &self,
        _out: &mut dyn Write,
        _idx: usize,
        _total: usize,
        _step: &Step,
    ) -> io::Result<()> {
        Ok(())
    }

    fn step_pass(&self, _out: &mut dyn Write, _report: &StepReport<'_>) -> io::Result<()> {
        Ok(())
    }

    fn step_fail(&self, _out: &mut dyn Write, _report: &StepReport<'_>) -> io::Result<()> {
        Ok(())
    }

    fn scenario_footer(
        &self,
        out: &mut dyn Write,
        scenario: &Scenario,
        status: ScenarioStatus,
        duration: Duration,
    ) -> io::Result<()> {
        // §2/§4 (reporter/CLAUDE.md): the whole icon+name span carries the
        // pass/fail semantic — bold + outcome color. Parity with PrettyReporter
        // so `-q` runs read the same way at the scenario boundary.
        let icon_and_name = match status {
            ScenarioStatus::Pass => styles::bold_green(&format!("✓ {}", &scenario.name)),
            ScenarioStatus::Fail => styles::bold_red(&format!("✗ {}", &scenario.name)),
        };
        // Quiet mode still surfaces slow/duration so green-path runs flag
        // outliers without needing a separate verbosity bump.
        let suffix = super::pretty::scenario_duration_suffix(duration);
        writeln!(out, "{}{}", icon_and_name, suffix)
    }

    fn cleanup_note(&self, _out: &mut dyn Write, _msg: &str) -> io::Result<()> {
        Ok(())
    }

    fn run_summary(&self, out: &mut dyn Write, summary: &RunSummary<'_>) -> io::Result<()> {
        // Share the pretty summary block — `-q` still benefits from the same
        // failed-scenarios / reproduce blocks at the bottom.
        super::pretty::PrettyReporter::new(super::Verbosity::Default).run_summary(out, summary)
    }
}
