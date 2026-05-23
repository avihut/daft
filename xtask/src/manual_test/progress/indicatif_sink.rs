//! Indicatif-backed `ProgressSink` implementation.
//!
//! Renders a pinned multi-row region at the bottom of the terminal: one
//! row per in-flight scenario (carrying scenario name + step counter +
//! step name + elapsed) plus a summary bar
//! (`[done/total] N running ◆ M failed ◆ mm:ss`).
//!
//! Concurrency: every method may be called from any rayon worker thread.
//! `MultiProgress` and `ProgressBar` are internally `Send + Sync` via
//! indicatif's own locking; the `rows` HashMap is wrapped in `Mutex`
//! because indicatif's per-bar API can't be keyed by scenario name
//! externally.
//!
//! Styling follows `reporter/CLAUDE.md` §8: no new color slots — the
//! in-flight area re-uses cyan `[N/M]` (counter), bright_purple step name
//! (identity), default fg scenario name, dim elapsed. `(slow)` annotation
//! in yellow when scenario elapsed > 5s, matching the footer's slow rule.

use std::collections::HashMap;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Mutex;
use std::time::Duration;

use indicatif::{MultiProgress, ProgressBar, ProgressStyle};

use super::super::reporter::ScenarioStatus;
use super::ProgressSink;

/// Threshold above which a scenario row gets a yellow `(slow)` suffix.
/// Matches the footer's slow annotation rule.
const SLOW_THRESHOLD: Duration = Duration::from_secs(5);

/// Spinner tick characters. The trailing space is the "rest" frame
/// indicatif cycles to when nothing's animating.
const SPINNER_TICKS: &str = "⠋⠙⠹⠸⠼⠴⠦⠧⠇⠏ ";

/// How often each bar self-ticks (drives spinner animation and
/// `{elapsed_precise}` updates without external prodding).
const TICK_INTERVAL: Duration = Duration::from_millis(100);

pub struct IndicatifProgressSink {
    multi: MultiProgress,
    summary: ProgressBar,
    rows: Mutex<HashMap<String, ProgressRow>>,
    failed: AtomicUsize,
}

struct ProgressRow {
    bar: ProgressBar,
}

impl IndicatifProgressSink {
    pub fn new() -> Self {
        let multi = MultiProgress::new();
        let summary = multi.add(ProgressBar::new(0));
        summary.set_style(
            // `{pos}/{len}` is the scenario counter; {msg} carries the
            // running / failed segments (rendered as a single string so the
            // failed count can pick up red via console styling when > 0).
            // `{elapsed_precise}` is dim so the structural counters lead.
            ProgressStyle::with_template(
                "  {spinner} [{pos}/{len}] {msg} ◆ {elapsed_precise:.dim}",
            )
            .expect("static template should be valid")
            .tick_chars(SPINNER_TICKS),
        );
        summary.set_message("0 running ◆ 0 failed");
        summary.enable_steady_tick(TICK_INTERVAL);

        Self {
            multi,
            summary,
            rows: Mutex::new(HashMap::new()),
            failed: AtomicUsize::new(0),
        }
    }

    fn update_summary_msg(&self) {
        let running = self.rows.lock().map(|r| r.len()).unwrap_or(0);
        let failed = self.failed.load(Ordering::Relaxed);
        let failed_segment = if failed > 0 {
            // console styling — owo/console respects NO_COLOR via
            // indicatif's own detection.
            format!("\x1b[1;31m{failed} failed\x1b[0m")
        } else {
            format!("{failed} failed")
        };
        self.summary
            .set_message(format!("{running} running ◆ {failed_segment}"));
    }

    /// Rebuild a scenario row's message with its current step + (slow)
    /// annotation. Reuses existing color slots — cyan `[N/M]`, bright
    /// purple step name, default fg scenario name, dim elapsed (in
    /// template), yellow `(slow)` when threshold crossed.
    fn render_row_msg(
        &self,
        scenario_name: &str,
        step_idx: usize,
        step_total: usize,
        step_name: &str,
        elapsed: Duration,
    ) -> String {
        let slow_suffix = if elapsed > SLOW_THRESHOLD {
            "  \x1b[33m(slow)\x1b[0m"
        } else {
            ""
        };
        format!(
            "{scenario}  \x1b[36m[{idx}/{total}]\x1b[0m \x1b[95m{step}\x1b[0m{slow_suffix}",
            scenario = scenario_name,
            idx = step_idx + 1,
            total = step_total,
            step = step_name,
        )
    }
}

impl ProgressSink for IndicatifProgressSink {
    fn run_started(&self, total_scenarios: usize) {
        self.summary.set_length(total_scenarios as u64);
        self.summary.set_position(0);
        self.update_summary_msg();
    }

    fn scenario_started(&self, name: &str, total_steps: usize) {
        let bar = self
            .multi
            .insert_before(&self.summary, ProgressBar::new_spinner());
        bar.set_style(
            // Per-row: dim spinner so the row name leads visually,
            // then the rendered message (which carries its own ANSI codes
            // for [N/M] and step name colors).
            ProgressStyle::with_template("  {spinner:.dim} {msg}")
                .expect("static template should be valid")
                .tick_chars(SPINNER_TICKS),
        );
        bar.set_message(format!(
            "{name}  \x1b[36m[0/{total_steps}]\x1b[0m \x1b[2mstarting…\x1b[0m"
        ));
        bar.enable_steady_tick(TICK_INTERVAL);

        if let Ok(mut rows) = self.rows.lock() {
            rows.insert(name.to_string(), ProgressRow { bar });
        }
        self.update_summary_msg();
    }

    fn step_started(&self, scenario_name: &str, idx: usize, total: usize, step_name: &str) {
        // Read elapsed before re-locking to keep the lock window short.
        let (msg, found) = {
            let Ok(rows) = self.rows.lock() else {
                return;
            };
            match rows.get(scenario_name) {
                Some(row) => {
                    let elapsed = row.bar.elapsed();
                    (
                        Some(self.render_row_msg(scenario_name, idx, total, step_name, elapsed)),
                        true,
                    )
                }
                None => (None, false),
            }
        };
        if !found {
            return;
        }
        if let (Some(msg), Ok(rows)) = (msg, self.rows.lock()) {
            if let Some(row) = rows.get(scenario_name) {
                row.bar.set_message(msg);
            }
        }
    }

    fn scenario_finished(&self, name: &str, status: ScenarioStatus, _duration: Duration) {
        if status == ScenarioStatus::Fail {
            self.failed.fetch_add(1, Ordering::Relaxed);
        }
        if let Ok(mut rows) = self.rows.lock() {
            if let Some(row) = rows.remove(name) {
                row.bar.finish_and_clear();
            }
        }
        self.summary.inc(1);
        self.update_summary_msg();
    }

    fn run_finished(&self) {
        self.summary.finish_and_clear();
        let _ = self.multi.clear();
    }

    fn suspend(&self, f: &mut dyn FnMut()) {
        self.multi.suspend(f);
    }
}

#[cfg(test)]
mod tests {
    //! These tests exercise the public surface of `IndicatifProgressSink`
    //! against indicatif's hidden draw target. They prove the methods
    //! don't panic, the internal counters update correctly, and the
    //! suspend bridge calls back through. They do NOT prove visual
    //! rendering — that's covered by manual smoke at each verbosity tier.
    use super::*;

    /// Construct a sink whose MultiProgress draws to a hidden target so
    /// tests don't print spurious bars when run with `cargo test`.
    fn hidden_sink() -> IndicatifProgressSink {
        let multi = MultiProgress::with_draw_target(indicatif::ProgressDrawTarget::hidden());
        let summary = multi.add(ProgressBar::new(0));
        summary.set_style(
            ProgressStyle::with_template("  {spinner} [{pos}/{len}] {msg}")
                .unwrap()
                .tick_chars(SPINNER_TICKS),
        );
        summary.set_message("0 running ◆ 0 failed");
        IndicatifProgressSink {
            multi,
            summary,
            rows: Mutex::new(HashMap::new()),
            failed: AtomicUsize::new(0),
        }
    }

    #[test]
    fn lifecycle_methods_do_not_panic() {
        let sink = hidden_sink();
        sink.run_started(2);
        sink.scenario_started("alpha", 3);
        sink.step_started("alpha", 0, 3, "first");
        sink.step_started("alpha", 1, 3, "second");
        sink.scenario_finished("alpha", ScenarioStatus::Pass, Duration::from_millis(120));
        sink.scenario_started("beta", 2);
        sink.scenario_finished("beta", ScenarioStatus::Fail, Duration::from_millis(80));
        sink.run_finished();
    }

    #[test]
    fn failed_counter_increments_on_fail_only() {
        let sink = hidden_sink();
        sink.run_started(3);
        sink.scenario_started("a", 1);
        sink.scenario_finished("a", ScenarioStatus::Pass, Duration::ZERO);
        sink.scenario_started("b", 1);
        sink.scenario_finished("b", ScenarioStatus::Fail, Duration::ZERO);
        sink.scenario_started("c", 1);
        sink.scenario_finished("c", ScenarioStatus::Fail, Duration::ZERO);
        assert_eq!(sink.failed.load(Ordering::Relaxed), 2);
    }

    #[test]
    fn rows_clear_on_scenario_finished() {
        let sink = hidden_sink();
        sink.run_started(1);
        sink.scenario_started("x", 1);
        assert_eq!(sink.rows.lock().unwrap().len(), 1);
        sink.scenario_finished("x", ScenarioStatus::Pass, Duration::ZERO);
        assert!(sink.rows.lock().unwrap().is_empty());
    }

    #[test]
    fn suspend_invokes_closure_exactly_once() {
        let sink = hidden_sink();
        let mut count = 0;
        sink.suspend(&mut || count += 1);
        assert_eq!(count, 1);
    }
}
