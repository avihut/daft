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

use indicatif::{MultiProgress, ProgressBar, ProgressState, ProgressStyle};

use super::super::reporter::ScenarioStatus;
use super::{InterruptFlag, ProgressSink};

/// Threshold above which a scenario row gets a yellow `(slow)` suffix.
/// Matches the footer's slow annotation rule.
const SLOW_THRESHOLD: Duration = Duration::from_secs(5);

/// Spinner tick characters. The trailing space is the "rest" frame
/// indicatif cycles to when nothing's animating.
const SPINNER_TICKS: &str = "⠋⠙⠹⠸⠼⠴⠦⠧⠇⠏ ";

/// How often each bar self-ticks (drives spinner animation and
/// `{elapsed_precise}` updates without external prodding).
const TICK_INTERVAL: Duration = Duration::from_millis(100);

/// Format a scenario-row elapsed: `Xms` while sub-second, `X.Ys` while
/// sub-minute, `M:SS` beyond that. Matches the scrollback footer's
/// `format_duration` rhythm (sub-second precision matters here because
/// most scenarios finish before the second crosses over, so a row that
/// always reads `0s` would be useless).
fn format_row_elapsed(d: Duration) -> String {
    let ms = d.as_millis();
    if ms < 1000 {
        format!("{ms}ms")
    } else if d.as_secs() < 60 {
        format!("{:.1}s", d.as_secs_f64())
    } else {
        let total = d.as_secs();
        format!("{}:{:02}", total / 60, total % 60)
    }
}

pub struct IndicatifProgressSink {
    multi: MultiProgress,
    summary: ProgressBar,
    rows: Mutex<HashMap<String, ProgressRow>>,
    failed: AtomicUsize,
    /// Scenarios that bailed mid-run via SIGINT. Surfaced as a separate
    /// segment on the summary bar (yellow, attention-without-alarm slot)
    /// so the reader can distinguish cancelled work from genuine failures
    /// at a glance.
    cancelled: AtomicUsize,
    /// Cooperative cancellation flag. Set by the SIGINT handler; read here
    /// to color the cancelled segment and (in the orchestrator's bookkeeping)
    /// to gate the run's exit code. Held by `Arc` so a clone is cheap.
    interrupt: InterruptFlag,
    /// Pre-computed widest scenario name across the run, in chars. Used
    /// to right-pad the scenario name column in the row message so the
    /// step label column lands at a stable position across rows.
    name_col_width: usize,
    /// Pre-computed widest `[N/M] step_name` label across the run, in
    /// chars. Used to right-pad the step label column so the elapsed
    /// counter on the right lands at a stable position across rows.
    step_col_width: usize,
}

struct ProgressRow {
    bar: ProgressBar,
}

impl IndicatifProgressSink {
    pub fn new(name_col_width: usize, step_col_width: usize, interrupt: InterruptFlag) -> Self {
        let multi = MultiProgress::new();
        let summary = multi.add(ProgressBar::new(0));
        summary.set_style(
            // Summary leads at column 0 — no leading indent — so the bar
            // anchors at the same column as the scrollback `✓ name` /
            // `✗ name` footers and the eye runs a single column down to
            // count completed scenarios.
            //
            // `{pos}/{len}` is the scenario counter; {msg} carries the
            // running / failed segments (rendered as a single string so the
            // failed count can pick up red via console styling when > 0).
            // `{elapsed_precise}` is dim so the structural counters lead.
            ProgressStyle::with_template("{spinner} [{pos}/{len}] {msg} ◆ {elapsed_precise:.dim}")
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
            cancelled: AtomicUsize::new(0),
            interrupt,
            name_col_width,
            step_col_width,
        }
    }

    fn update_summary_msg(&self) {
        let running = self.rows.lock().map(|r| r.len()).unwrap_or(0);
        let failed = self.failed.load(Ordering::Relaxed);
        let cancelled = self.cancelled.load(Ordering::Relaxed);
        let failed_segment = if failed > 0 {
            // Raw SGR bytes. Indicatif's template DSL (e.g. `{msg:.red}`)
            // doesn't expose a conditional hook for "red only when
            // `failed > 0`", so the styling has to be inlined into the
            // message string. Known limitation: the bar message bypasses
            // `NO_COLOR` — bar templates honor it, but bytes inside
            // `{msg}` are passed through verbatim. See `reporter/CLAUDE.md`
            // §8's ANSI-inlining carve-out.
            format!("\x1b[1;31m{failed} failed\x1b[0m")
        } else {
            format!("{failed} failed")
        };
        // §8 + §1: cancelled lives in the yellow slot ("attention without
        // alarm") and only surfaces when > 0 — printing `0 cancelled` on
        // every green run would add chrome. Once a run is cancelled, the
        // segment always shows so the user can see the count grow as
        // in-flight workers wind down.
        let mut msg = format!("{running} running ◆ {failed_segment}");
        let interrupted = self.interrupt.is_set();
        if cancelled > 0 || interrupted {
            msg.push_str(&format!(" ◆ \x1b[33m{cancelled} cancelled\x1b[0m"));
        }
        // Live feedback that Ctrl+C registered. The handler is deliberately
        // silent (any stderr write pushes the bar into scrollback as ghost
        // rows); the suffix here is how the user sees their cancel landed.
        // Drops away once `running` reaches 0 — no point claiming we're
        // "cancelling" when there's nothing left to cancel.
        if interrupted && running > 0 {
            msg.push_str(" \x1b[33m(cancelling)\x1b[0m");
        }
        self.summary.set_message(msg);
    }

    /// Right-pad a plain string to `width` chars using
    /// `chars().count()`. Plain text only — caller must apply ANSI
    /// styling AFTER padding so escape bytes don't get counted in the
    /// width.
    fn pad_to(text: &str, width: usize) -> String {
        let len = text.chars().count();
        if len < width {
            let mut padded = String::with_capacity(text.len() + (width - len));
            padded.push_str(text);
            for _ in 0..(width - len) {
                padded.push(' ');
            }
            padded
        } else {
            text.to_string()
        }
    }

    /// Rebuild a scenario row's message with its current step. The row
    /// is laid out in three padded columns so multiple in-flight rows
    /// stack into a grid:
    ///
    /// ```text
    ///   <scenario name (col 1, pad to name_col_width)>  <[N/M] step (col 2, pad to step_col_width)>  <elapsed (col 3, template)>
    /// ```
    ///
    /// The `(slow)` annotation, when present, attaches to the end of
    /// column 2 (inside the padding zone) so it doesn't push the
    /// elapsed column out of alignment. Slow rows extend into the pad
    /// budget; only rows whose step label is already at max width can
    /// shift the elapsed column, and at that point only by the (slow)
    /// suffix's own width.
    ///
    /// Reuses existing color slots — default fg scenario name, cyan
    /// `[N/M]`, bright purple step name, yellow `(slow)`.
    fn render_row_msg(
        &self,
        scenario_name: &str,
        step_idx: usize,
        step_total: usize,
        step_name: &str,
        elapsed: Duration,
    ) -> String {
        let name_padded = Self::pad_to(scenario_name, self.name_col_width);

        // Build the step label as plain text first, then pad, then
        // splice the ANSI codes back in over the un-styled segments.
        // This keeps `pad_to` honest about visible width.
        let counter = format!("[{}/{}]", step_idx + 1, step_total);
        let plain_label = format!("{counter} {step_name}");
        let label_padded = Self::pad_to(&plain_label, self.step_col_width);
        // Re-apply styling to the counter and step name, leaving the
        // trailing padding untouched. `replacen(_, _, 1)` matches first
        // occurrence:
        //
        // 1. Counter precedes step_name in plain_label, so the first
        //    replacen targets the structural counter.
        // 2. After splice #1, the counter's bytes still appear contiguously
        //    inside the SGR escape (`\x1b[36m[N/M]\x1b[0m`). The second
        //    replacen relies on step_name NOT being a literal counter-
        //    shaped string — a step named `"[1/3]"` would match inside
        //    the styled counter and corrupt the row.
        //
        // Real step names in `tests/manual/scenarios/` are human-readable
        // prose (verbs/phrases), never counter strings; the debug_assert
        // documents the assumption.
        debug_assert!(
            !step_name.is_empty(),
            "step_name must be non-empty; empty triggers replacen at byte-0 corruption"
        );
        let label_styled = label_padded
            .replacen(&counter, &format!("\x1b[36m{counter}\x1b[0m"), 1)
            .replacen(step_name, &format!("\x1b[95m{step_name}\x1b[0m"), 1);

        let slow_suffix = if elapsed > SLOW_THRESHOLD {
            "  \x1b[33m(slow)\x1b[0m"
        } else {
            ""
        };
        format!("{name_padded}  {label_styled}{slow_suffix}")
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
            // Per-row leads at column 0 so when the scenario completes its
            // `✓ name  duration` footer (also at column 0) drops cleanly
            // into the same column the live row was occupying.
            //
            // `{row_elapsed}` is a custom key (added below via `with_key`)
            // that renders sub-second precision — most scenarios finish
            // before `{elapsed}` would cross 1s and update to "1s".
            ProgressStyle::with_template("{spinner:.dim} {msg} \x1b[2m{row_elapsed}\x1b[0m")
                .expect("static template should be valid")
                .tick_chars(SPINNER_TICKS)
                .with_key(
                    "row_elapsed",
                    |state: &ProgressState, w: &mut dyn std::fmt::Write| {
                        let _ = write!(w, "{}", format_row_elapsed(state.elapsed()));
                    },
                ),
        );
        // Initial message uses the same column layout as render_row_msg
        // (scenario name padded → step label padded → elapsed via
        // template) so the bar doesn't jump on the first step_started.
        // `starting…` is dim because no step has actually fired yet —
        // it's a placeholder, not a real step name.
        let name_padded = Self::pad_to(name, self.name_col_width);
        let counter = format!("[0/{total_steps}]");
        let plain_label = format!("{counter} starting…");
        let label_padded = Self::pad_to(&plain_label, self.step_col_width);
        let label_styled = label_padded
            .replacen(&counter, &format!("\x1b[36m{counter}\x1b[0m"), 1)
            .replacen("starting…", "\x1b[2mstarting…\x1b[0m", 1);
        bar.set_message(format!("{name_padded}  {label_styled}"));
        bar.enable_steady_tick(TICK_INTERVAL);

        if let Ok(mut rows) = self.rows.lock() {
            rows.insert(name.to_string(), ProgressRow { bar });
        }
        self.update_summary_msg();
    }

    fn step_started(&self, scenario_name: &str, idx: usize, total: usize, step_name: &str) {
        // Two-phase lock: read elapsed under the lock, release while
        // `render_row_msg` does ANSI string formatting (no shared state
        // needed), then re-acquire briefly for `set_message`. Holding the
        // mutex across the formatting would serialize every worker's
        // step_started even though the formatting is pure-functional.
        //
        // The race window between the two locks is benign: if
        // `scenario_finished` removes the row between phases, the second
        // `if let Some(row)` simply skips the update — visually equivalent
        // to a `set_message` that landed a frame before the row cleared.
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
        match status {
            ScenarioStatus::Fail => {
                self.failed.fetch_add(1, Ordering::Relaxed);
            }
            ScenarioStatus::Cancelled => {
                // Cancelled doesn't bump the failed counter — it has its
                // own yellow segment in the summary message so the eye
                // doesn't conflate user-cancellation with genuine
                // assertion failures.
                self.cancelled.fetch_add(1, Ordering::Relaxed);
            }
            ScenarioStatus::Pass => {}
        }
        if let Ok(mut rows) = self.rows.lock() {
            if let Some(row) = rows.remove(name) {
                // `multi.remove`, NOT `bar.finish_and_clear`. The latter
                // transitions the bar to `DoneHidden` and interacts with
                // indicatif's zombie-line accounting: on drop, the bar's
                // `mark_zombie` can feed non-zero line counts into
                // `LineAdjust::Keep`, leaving the last-drawn row stuck
                // in scrollback above subsequent `multi.println` calls.
                // `multi.remove` unlinks the bar from the ordering and
                // hides its draw target so the next `multi.println` does
                // an atomic redraw that cleanly clears the row.
                // See `src/output/hook_progress/interactive.rs:remove_job_bars`
                // — the main daft binary hit the exact same bug and
                // landed the same fix.
                self.multi.remove(&row.bar);
            }
        }
        self.summary.inc(1);
        self.update_summary_msg();
    }

    fn run_finished(&self) {
        // Same zombie-line concern as in `scenario_finished`: prefer
        // `multi.remove` over `summary.finish_and_clear` so the summary
        // bar doesn't leave a trailing line above the final summary
        // block. `multi.clear` then wipes any remaining draw-target
        // content for a fully clean end-of-run frame.
        self.multi.remove(&self.summary);
        let _ = self.multi.clear();
    }

    fn notify_cancelling(&self) {
        // Refresh the summary message so the `(cancelling)` suffix lands
        // immediately. Without this call, the suffix wouldn't appear until
        // the first worker bails — which can be seconds on a slow step.
        self.update_summary_msg();
    }

    fn println(&self, line: &str) {
        // multi.println handles the cursor/clear sequencing internally:
        // hide the bar region, write the line above it, then redraw the
        // region below. This is what the previous `suspend + write_all`
        // path tried to do but got wrong — it left bar-row trailing
        // whitespace on the cursor's physical line, so footers landed on
        // the same row as in-flight bar entries ("ghost rows"). Letting
        // indicatif own the sequencing fixes that.
        let _ = self.multi.println(line);
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
            cancelled: AtomicUsize::new(0),
            interrupt: InterruptFlag::new(),
            name_col_width: 0,
            step_col_width: 0,
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
        assert_eq!(sink.cancelled.load(Ordering::Relaxed), 0);
    }

    #[test]
    fn notify_cancelling_appends_cancelling_suffix_to_summary() {
        // The orchestrator pokes the sink via notify_cancelling so the
        // `(cancelling)` suffix appears immediately after Ctrl+C instead
        // of waiting for the first worker to bail. Without the flag being
        // set, the suffix shouldn't appear.
        let interrupt = InterruptFlag::new();
        let multi = MultiProgress::with_draw_target(indicatif::ProgressDrawTarget::hidden());
        let summary = multi.add(ProgressBar::new(0));
        summary.set_style(
            ProgressStyle::with_template("{msg}")
                .unwrap()
                .tick_chars(SPINNER_TICKS),
        );
        let sink = IndicatifProgressSink {
            multi,
            summary,
            rows: Mutex::new(HashMap::new()),
            failed: AtomicUsize::new(0),
            cancelled: AtomicUsize::new(0),
            interrupt: interrupt.clone(),
            name_col_width: 0,
            step_col_width: 0,
        };
        sink.run_started(2);
        sink.scenario_started("a", 1);

        // Before the flag is set, notify_cancelling is a no-op effectively
        // (suffix shouldn't appear).
        sink.notify_cancelling();
        assert!(!sink.summary.message().contains("(cancelling)"));

        // After the flag is set, notify_cancelling refreshes the message
        // so the suffix lands immediately.
        interrupt.set();
        sink.notify_cancelling();
        assert!(sink.summary.message().contains("(cancelling)"));

        // Once running drops to 0 (last worker bailed), the suffix drops
        // away — nothing left to cancel.
        sink.scenario_finished("a", ScenarioStatus::Cancelled, Duration::ZERO);
        assert!(!sink.summary.message().contains("(cancelling)"));
    }

    #[test]
    fn cancelled_counter_increments_separately_from_failed() {
        // Regression guard for the SIGINT bug: when a scenario is
        // cancelled mid-run, the bar must not bump the failed counter.
        // Polluting `failed` with cancelled work was the bug this whole
        // path was introduced to fix.
        let sink = hidden_sink();
        sink.run_started(4);
        sink.scenario_started("a", 1);
        sink.scenario_finished("a", ScenarioStatus::Pass, Duration::ZERO);
        sink.scenario_started("b", 1);
        sink.scenario_finished("b", ScenarioStatus::Fail, Duration::ZERO);
        sink.scenario_started("c", 1);
        sink.scenario_finished("c", ScenarioStatus::Cancelled, Duration::ZERO);
        sink.scenario_started("d", 1);
        sink.scenario_finished("d", ScenarioStatus::Cancelled, Duration::ZERO);
        assert_eq!(sink.failed.load(Ordering::Relaxed), 1);
        assert_eq!(sink.cancelled.load(Ordering::Relaxed), 2);
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
    fn format_row_elapsed_ms_under_one_second() {
        assert_eq!(format_row_elapsed(Duration::from_millis(0)), "0ms");
        assert_eq!(format_row_elapsed(Duration::from_millis(42)), "42ms");
        assert_eq!(format_row_elapsed(Duration::from_millis(999)), "999ms");
    }

    #[test]
    fn format_row_elapsed_seconds_under_one_minute() {
        assert_eq!(format_row_elapsed(Duration::from_millis(1_000)), "1.0s");
        assert_eq!(format_row_elapsed(Duration::from_millis(1_500)), "1.5s");
        assert_eq!(format_row_elapsed(Duration::from_secs(59)), "59.0s");
    }

    #[test]
    fn format_row_elapsed_mm_ss_past_one_minute() {
        assert_eq!(format_row_elapsed(Duration::from_secs(60)), "1:00");
        assert_eq!(format_row_elapsed(Duration::from_secs(65)), "1:05");
        assert_eq!(format_row_elapsed(Duration::from_secs(3_605)), "60:05");
    }

    #[test]
    fn render_row_msg_label_width_matches_step_label_width_formula() {
        // Lock the contract between the orchestrator's pre-scan formula
        // (`step_label_width` in mod.rs) and the live renderer's actual
        // output. If either side's format changes independently,
        // in-flight rows silently misalign — and no other test catches it
        // because the bars draw to a hidden target.
        let sink = hidden_sink();
        let cases = [
            (0_usize, 1_usize, "first"),
            (4, 5, "Inspect workspace"),
            (9, 10, "Long step name with spaces"),
        ];
        for (idx, total, step) in cases {
            let msg = sink.render_row_msg("anything", idx, total, step, Duration::ZERO);
            let visible = strip_ansi(&msg);
            // hidden_sink uses width 0 on both columns, so layout is
            // "<name>  <label>" — name unpadded, two-space separator,
            // unpadded label, no slow suffix at Duration::ZERO.
            let label = visible
                .strip_prefix("anything  ")
                .expect("name + 2-space separator should lead the row");
            assert_eq!(
                label.chars().count(),
                crate::manual_test::step_label_width(idx + 1, total, step),
                "render_row_msg label width must match step_label_width formula",
            );
        }
    }

    /// Strip SGR escape sequences (`ESC [ … m`) so visible width can be
    /// measured by `chars().count()`. Sufficient for the styles
    /// `render_row_msg` emits (cyan/purple/yellow/dim/reset).
    fn strip_ansi(s: &str) -> String {
        let mut out = String::with_capacity(s.len());
        let mut chars = s.chars();
        while let Some(c) = chars.next() {
            if c == '\x1b' {
                for nc in chars.by_ref() {
                    if nc == 'm' {
                        break;
                    }
                }
            } else {
                out.push(c);
            }
        }
        out
    }
}
