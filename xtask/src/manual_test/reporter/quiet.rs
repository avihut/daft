//! `-q` reporter: scenario PASS/FAIL footer + final summary only.
//!
//! Per-step lines, expanded commands, and captured output are suppressed —
//! the failed-scenarios block in the summary still carries the full failure
//! detail, so CI green-path / bench runs stay quiet but failures are not
//! silently swallowed.

use std::io::{self, Write};

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
    ) -> io::Result<()> {
        match status {
            ScenarioStatus::Pass => writeln!(out, "{} {}", styles::green("✓"), &scenario.name),
            ScenarioStatus::Fail => writeln!(out, "{} {}", styles::red("✗"), &scenario.name),
        }
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
