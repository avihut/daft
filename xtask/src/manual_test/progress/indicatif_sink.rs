//! Indicatif-backed `ProgressSink` implementation.
//!
//! Renders a pinned multi-row region at the bottom of the terminal: a
//! summary (totals) bar on top, then one row per in-flight scenario below
//! it. Both lines share a `[bar] → [counter] → [time] → tail` layout so
//! the bars stack into one left column:
//!
//! ```text
//!   [██████░░░░░░░░] 3/8  0:05  2/4 running ◆ 0 failed   <- summary
//!   [████░░░░░░] 2/5  1.2s  checkout-basic   Inspect …    <- worker row
//! ```
//!
//! See [`summary_style`] and [`row_style`] for the per-line layout, and
//! `reporter/CLAUDE.md` §8 for the design rationale.
//!
//! Concurrency: every method may be called from any rayon worker thread.
//! `MultiProgress` and `ProgressBar` are internally `Send + Sync` via
//! indicatif's own locking; the `rows` HashMap is wrapped in `Mutex`
//! because indicatif's per-bar API can't be keyed by scenario name
//! externally.
//!
//! Styling reuses `reporter/CLAUDE.md` §1's budget — no new color slots:
//! bar fill default fg / unfilled dim, default-fg counters, dim elapsed,
//! bright-purple step name (identity), and a yellow `(slow)` suffix once a
//! scenario's elapsed crosses 5 s. The variable text tail rides in
//! `{wide_msg}`, whose ANSI-aware truncation keeps every row exactly one
//! terminal line tall on narrow displays — a hard requirement for
//! indicatif's line accounting, not just cosmetics.

use std::collections::HashMap;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Mutex;
use std::time::Duration;

use indicatif::{MultiProgress, ProgressBar, ProgressDrawTarget, ProgressState, ProgressStyle};

use super::super::reporter::ScenarioStatus;
use super::{InterruptFlag, ProgressSink};

/// Threshold above which a scenario row gets a yellow `(slow)` suffix.
/// Matches the footer's slow annotation rule.
const SLOW_THRESHOLD: Duration = Duration::from_secs(5);

/// How often each bar self-ticks (drives the live `{elapsed}` /
/// `{row_elapsed}` counters forward without external prodding).
///
/// Set deliberately above the multi's draw-target Hz cap below so steady
/// ticks don't pile up faster than the draw target can flush them. Faster
/// ticks (e.g. 100 ms) accumulate draw requests under heavy worker churn,
/// and indicatif's internal line accounting can desync from terminal
/// reality — leaving in-flight bar rows stranded in scrollback above
/// subsequent `multi.println` output. 200 ms keeps the elapsed counters
/// visibly moving (~5 updates/s) while giving line accounting room to
/// settle between concurrent updates.
const TICK_INTERVAL: Duration = Duration::from_millis(200);

/// Cap the multi's overall redraw rate. The bar's internal line-counting
/// is most fragile under draw pressure — capping at 10 Hz halves the
/// observed ghost-row rate without making the spinner feel sluggish.
/// Combine with `TICK_INTERVAL` above (which throttles per-bar ticks)
/// and the trailer in `IndicatifProgressSink::new` (which keeps the line
/// count stable as rows are added/removed) — together they close the
/// race window the daft hook-progress UI also had to address (see
/// `src/output/hook_progress/interactive.rs` trailer comment).
const MAX_DRAW_HZ: u8 = 10;

/// Shared fixed width (in cols) of the progress bars on both the summary
/// line and the per-worker rows, so every bar stacks into one left column.
/// Kept modest so the bar + counter + time prefix leaves room for the
/// truncating text tail on narrow terminals.
const BAR_WIDTH: usize = 14;

/// Fixed width (in cols) of a worker row's time counter, so the
/// step-name tail to its right starts at a stable column across rows.
/// Covers the widest [`format_row_elapsed`] output (`999ms`, `12:34`).
const ROW_ELAPSED_WIDTH: usize = 6;

/// Decimal digit count of `n` (`0` → 1). Used to right-pad the `pos`
/// half of a `pos/len` counter so the column to its right (the time
/// counter) sits at a stable position as `pos` grows digits.
fn digit_count(n: u64) -> usize {
    if n == 0 {
        1
    } else {
        (n.ilog10() as usize) + 1
    }
}

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

/// Build the summary (totals) bar's style.
///
/// Layout follows the live region's `[bar] → [counter] → [time] → rest`
/// motif, shared with the worker rows:
///
/// ```text
///   [████████░░░░░░] 3/8  0:05  2/4 running ◆ 0 failed
/// ```
///
/// - `{bar}` — scenario completion, fixed [`BAR_WIDTH`]; filled default fg,
///   unfilled `dim` (`./dim`) so it reuses existing ink (no new color slot).
/// - `{scenario_counter}` — a custom key rendering `done/total` with `done`
///   right-padded to `total`'s digit width, so the time column doesn't shift
///   as the count grows digits.
/// - `{elapsed_precise}` — dim run elapsed (scaffolding), same data as before.
/// - `{wide_msg}` — the running/failed/cancelled segments built in
///   `update_summary_msg`. `wide_msg` truncates (never wraps) to the terminal
///   width, which keeps the line exactly one row tall on narrow terminals —
///   a correctness requirement for indicatif's line accounting, not just
///   cosmetics. Truncation is ANSI-aware, so the inlined red/yellow segments
///   survive a cut.
fn summary_style() -> ProgressStyle {
    ProgressStyle::with_template(&format!(
        "{{bar:{BAR_WIDTH}./dim}}  {{scenario_counter}}  {{elapsed_precise:.dim}}  {{wide_msg}}"
    ))
    .expect("static summary template should be valid")
    .with_key(
        "scenario_counter",
        |state: &ProgressState, w: &mut dyn std::fmt::Write| {
            let len = state.len().unwrap_or(0);
            let pos = state.pos();
            let width = digit_count(len);
            let _ = write!(w, "{pos:>width$}/{len}");
        },
    )
}

/// Build a worker row's style.
///
/// Mirrors the summary's `[bar] → [counter] → [time] → tail` motif so the
/// bars on both lines stack into one left column:
///
/// ```text
///   [████░░░░░░] 2/5  1.2s  checkout-basic   Inspect workspace
/// ```
///
/// - `{bar}` — completed-step progress for this scenario, fixed
///   [`BAR_WIDTH`]; filled default fg, unfilled `dim` (no new color slot).
/// - `{prefix}` — the `done/total` step counter, padded to the run's
///   widest counter (set via `set_prefix`).
/// - `{row_elapsed}` — a custom key with sub-second precision, dim and
///   padded to [`ROW_ELAPSED_WIDTH`] so the tail starts at a stable column.
/// - `{wide_msg}` — `"<scenario name (padded)>  <current step name>"`,
///   set via `set_message`. `wide_msg` truncates (never wraps) to the
///   terminal width, so the row stays exactly one line tall regardless of
///   how long the names are — a correctness requirement for indicatif's
///   line accounting. Truncation is ANSI-aware, so the bright-purple step
///   name and yellow `(slow)` survive a cut without bleeding color.
fn row_style() -> ProgressStyle {
    ProgressStyle::with_template(&format!(
        "{{bar:{BAR_WIDTH}./dim}}  {{prefix}}  \x1b[2m{{row_elapsed}}\x1b[0m  {{wide_msg}}"
    ))
    .expect("static row template should be valid")
    .with_key(
        "row_elapsed",
        |state: &ProgressState, w: &mut dyn std::fmt::Write| {
            let _ = write!(
                w,
                "{:<ROW_ELAPSED_WIDTH$}",
                format_row_elapsed(state.elapsed())
            );
        },
    )
}

pub struct IndicatifProgressSink {
    multi: MultiProgress,
    /// Global serializer for *all* state-mutating multi operations
    /// (`add`/`remove`/`println`/`insert_before`). Indicatif's internal
    /// `RwLock` makes each individual call thread-safe, but doesn't
    /// prevent two threads' calls from interleaving with each other or
    /// with an in-flight steady-tick redraw. That cross-call interleave
    /// is what leaves in-flight rows stranded in scrollback under load:
    /// e.g. `complete_scenario` does `println(footer); remove(row)` and
    /// another thread's `scenario_started` slips between them, shifting
    /// the row count out from under the remove. Holding this mutex for
    /// the duration of every multi state change forces a total order on
    /// those operations and closes the last race window.
    state_lock: Mutex<()>,
    summary: ProgressBar,
    /// A single-space-template "trailer" bar that lives at the bottom of
    /// the multi's bar set and is never removed. Its job is to keep
    /// indicatif's internal line-count accounting aligned with the actual
    /// terminal: when rows come and go via `multi.add` / `multi.remove`,
    /// the trailer absorbs any boundary jitter so that a concurrent
    /// `multi.println` doesn't undercount the lines it needs to clear
    /// and leave an in-flight row stranded in scrollback.
    ///
    /// Pattern lifted from the main daft binary's hook-progress UI
    /// (`src/output/hook_progress/interactive.rs`), which hit the same
    /// class of bug and landed the same fix.
    _trailer: ProgressBar,
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
    /// to right-pad the scenario name in each worker row's tail so the
    /// step name lands at a stable position across rows.
    name_col_width: usize,
    /// Pre-computed widest `done/total` step counter across the run, in
    /// chars. Used to right-pad each worker row's counter so the time
    /// counter to its right lands at a stable position across rows.
    step_counter_width: usize,
    /// Size of the rayon worker pool (resolved `jobs`). Rendered on the
    /// summary as `R/A running` (`R` in-flight, `A` = this) so the reader
    /// can see how saturated the pool is.
    total_workers: usize,
}

struct ProgressRow {
    bar: ProgressBar,
}

impl IndicatifProgressSink {
    pub fn new(
        name_col_width: usize,
        step_counter_width: usize,
        total_workers: usize,
        interrupt: InterruptFlag,
    ) -> Self {
        // Cap the overall draw rate so steady ticks don't pile up faster
        // than the terminal can flush them. See `MAX_DRAW_HZ` for the
        // rationale.
        let multi =
            MultiProgress::with_draw_target(ProgressDrawTarget::stderr_with_hz(MAX_DRAW_HZ));
        let summary = multi.add(ProgressBar::new(0));
        // Totals line leads with the completion bar at column 0 (so it
        // anchors at the same column as the scrollback `✓ name` / `✗ name`
        // footers) followed by the scenario counter, run elapsed, and the
        // running/failed/cancelled segments. See [`summary_style`] for the
        // layout rationale. The steady tick refreshes `{elapsed_precise}`
        // and the bar even when no scenario completes.
        summary.set_style(summary_style());
        summary.set_message("0/0 running ◆ 0 failed");
        summary.enable_steady_tick(TICK_INTERVAL);

        // Anchor a single-space "trailer" bar at the bottom of the multi.
        // It renders as a single blank line that's always present, which
        // keeps indicatif's internal line-count accounting stable as row
        // bars are added / removed above it. Single space (not empty) is
        // load-bearing — an empty template desyncs the "drawn lines"
        // counter (see the matching comment in
        // `src/output/hook_progress/interactive.rs`). The trailer never
        // finishes / never gets removed; `multi.clear()` at end-of-run
        // wipes it.
        let trailer = multi.add(ProgressBar::new_spinner());
        trailer.set_style(
            ProgressStyle::with_template(" ").expect("trailer template is a single space"),
        );
        trailer.set_message(String::new());

        Self {
            multi,
            state_lock: Mutex::new(()),
            summary,
            _trailer: trailer,
            rows: Mutex::new(HashMap::new()),
            failed: AtomicUsize::new(0),
            cancelled: AtomicUsize::new(0),
            interrupt,
            name_col_width,
            step_counter_width,
            total_workers,
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
        let mut msg = format!(
            "{running}/{} running ◆ {failed_segment}",
            self.total_workers
        );
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

    /// Build the `{wide_msg}` tail of a worker row: the padded scenario
    /// name followed by the current step name, plus a `(slow)` suffix once
    /// the scenario crosses [`SLOW_THRESHOLD`].
    ///
    /// ```text
    ///   checkout-basic   Inspect workspace
    /// ```
    ///
    /// The scenario name is padded to `name_col_width` (default fg, matching
    /// the passing-footer convention) so step names align across rows; the
    /// step name is bright purple (the identity slot) and the `(slow)`
    /// suffix yellow. All three reuse existing color slots. The whole tail
    /// rides in `{wide_msg}`, whose ANSI-aware truncation cuts the step
    /// name (then the scenario name) on narrow terminals without ever
    /// wrapping the row or letting a color bleed past the cut.
    fn render_row_tail(&self, scenario_name: &str, step_name: &str, elapsed: Duration) -> String {
        let name_padded = Self::pad_to(scenario_name, self.name_col_width);
        let slow_suffix = if elapsed > SLOW_THRESHOLD {
            "  \x1b[33m(slow)\x1b[0m"
        } else {
            ""
        };
        format!("{name_padded}  \x1b[95m{step_name}\x1b[0m{slow_suffix}")
    }

    /// Build the `{prefix}` step counter for a worker row: `done/total`
    /// padded to the run's widest counter so the time column to its right
    /// stays put across rows. Plain text (default fg) — the bar to its left
    /// is the visual anchor.
    fn render_row_counter(&self, done: usize, total: usize) -> String {
        Self::pad_to(&format!("{done}/{total}"), self.step_counter_width)
    }
}

impl ProgressSink for IndicatifProgressSink {
    fn run_started(&self, total_scenarios: usize) {
        self.summary.set_length(total_scenarios as u64);
        self.summary.set_position(0);
        self.update_summary_msg();
    }

    fn scenario_started(&self, name: &str, total_steps: usize) {
        // Hold state_lock for the entire add+style+insert sequence so it
        // can't interleave with another worker's `complete_scenario` (or
        // its own `scenario_started`) and shift the row count out from
        // under either side. See the field doc on `state_lock`.
        let _state = self.state_lock.lock().unwrap_or_else(|e| e.into_inner());
        // Insert worker rows *below* the summary (between it and the
        // never-removed trailer) so the totals bar sits on top of the
        // in-flight rows. The multi's order is `summary` (added first,
        // top) → worker rows → `_trailer` (added last, bottom);
        // `insert_before(&self._trailer)` lands each new row just above
        // the trailer. The trailer staying last is what keeps indicatif's
        // line accounting stable as rows come and go — do not insert after
        // it.
        let bar = self
            .multi
            .insert_before(&self._trailer, ProgressBar::new(total_steps as u64));
        // Row leads with a completed-step bar at column 0 (so the scenario's
        // `✓ name  duration` footer drops cleanly into the same column when
        // it completes), then the step counter, the dim elapsed, and the
        // scenario/step tail. See [`row_style`] for the layout.
        bar.set_style(row_style());
        bar.set_position(0);
        // Initial state: 0 steps done, `starting…` as a dim placeholder
        // until the first `step_started` names a real step. Same column
        // layout as a stepped row so the bar doesn't jump on the first step.
        let name_padded = Self::pad_to(name, self.name_col_width);
        bar.set_prefix(self.render_row_counter(0, total_steps));
        bar.set_message(format!("{name_padded}  \x1b[2mstarting…\x1b[0m"));
        bar.enable_steady_tick(TICK_INTERVAL);

        if let Ok(mut rows) = self.rows.lock() {
            rows.insert(name.to_string(), ProgressRow { bar });
        }
        self.update_summary_msg();
    }

    fn step_started(&self, scenario_name: &str, idx: usize, total: usize, step_name: &str) {
        // `idx` is the 0-based index of the step now starting, so `idx`
        // steps are already complete — that's both the bar position and
        // the `done` half of the `done/total` counter.
        //
        // Two-phase lock: read elapsed under the lock, release while
        // `render_row_tail` does ANSI string formatting (no shared state
        // needed), then re-acquire briefly to update the bar. Holding the
        // mutex across the formatting would serialize every worker's
        // step_started even though the formatting is pure-functional.
        //
        // The race window between the two locks is benign: if
        // `scenario_finished` removes the row between phases, the second
        // `if let Some(row)` simply skips the update — visually equivalent
        // to a `set_message` that landed a frame before the row cleared.
        let (update, found) = {
            let Ok(rows) = self.rows.lock() else {
                return;
            };
            match rows.get(scenario_name) {
                Some(row) => {
                    let elapsed = row.bar.elapsed();
                    let tail = self.render_row_tail(scenario_name, step_name, elapsed);
                    let counter = self.render_row_counter(idx, total);
                    (Some((tail, counter)), true)
                }
                None => (None, false),
            }
        };
        if !found {
            return;
        }
        if let (Some((tail, counter)), Ok(rows)) = (update, self.rows.lock()) {
            if let Some(row) = rows.get(scenario_name) {
                row.bar.set_position(idx as u64);
                row.bar.set_prefix(counter);
                row.bar.set_message(tail);
            }
        }
    }

    fn complete_scenario(
        &self,
        name: &str,
        status: ScenarioStatus,
        _duration: Duration,
        buf: &[u8],
    ) {
        // CRITICAL ORDERING: remove FIRST, then println. This matches the
        // production pattern in `src/output/hook_progress/interactive.rs::
        // finish_job`, which removes all job bars before calling
        // `mp.println` for the heading. The reasoning is documented on
        // `remove_job_bars` in that file; the short version follows.
        //
        // `mp.remove` sets the bar's draw_target to hidden and unlinks it
        // from the multi's ordering — it does NOT trigger a redraw
        // (see `MultiProgress::remove` in indicatif 0.18.4 `multi.rs:150`).
        // The next `mp.println` then performs an atomic clear+orphan+redraw
        // against the post-remove bar set, so the previously-occupied row
        // gets covered by whatever shifts up into its slot (or by blank
        // padding from the trailer).
        //
        // The reversed order (println-then-remove) leaves the doomed bar
        // in the set during the println's redraw, so `last_line_count`
        // gets set to N (all bars including the doomed one). The remove
        // then drops the bar set to N-1 without redrawing. The next
        // steady-tick redraw clears N lines but writes only N-1, leaving
        // the bottom line stale — a stranded bar row in scrollback.
        // That was the cause of the ghost-row bug; see indicatif issue
        // #474 for the upstream discussion of the same class of issue.
        //
        // `state_lock` is still held for the whole sequence: hook_progress
        // is single-threaded so it doesn't need one, but rayon workers
        // here can hit `complete_scenario` and `scenario_started`
        // concurrently, and two workers' per-scenario buffer prints
        // interleaving on stderr would be far worse than the original ghost.
        let _state = self.state_lock.lock().unwrap_or_else(|e| e.into_inner());

        if let Ok(mut rows) = self.rows.lock() {
            if let Some(row) = rows.remove(name) {
                // `multi.remove`, NOT `bar.finish_and_clear`. See the
                // hook-progress comment in
                // `src/output/hook_progress/interactive.rs:remove_job_bars`
                // for the longer explanation of the zombie-line hazard.
                self.multi.remove(&row.bar);
            }
        }

        if let Ok(text) = std::str::from_utf8(buf) {
            for line in text.split_inclusive('\n') {
                let trimmed = line.strip_suffix('\n').unwrap_or(line);
                let _ = self.multi.println(trimmed);
            }
        } else {
            // Fallback for non-UTF-8: write directly to stderr. Reaching
            // this is a schema violation (every buffer is built from
            // `write!` on `&str`) but we don't drop bytes silently.
            use std::io::Write;
            let stderr = std::io::stderr();
            let mut lock = stderr.lock();
            let _ = lock.write_all(buf);
        }

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
        self.summary.inc(1);
        self.update_summary_msg();
    }

    fn run_finished(&self) {
        // Hold `state_lock` so any in-flight `complete_scenario` or
        // `scenario_started` finishes before we wipe the region.
        // Otherwise the `multi.clear` could land between another
        // thread's println and remove, leaving partial frame content.
        let _state = self.state_lock.lock().unwrap_or_else(|e| e.into_inner());
        // Same zombie-line concern as in `complete_scenario`: prefer
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
        // Use the real production styles so the lifecycle tests exercise
        // `summary_style()` (and, via `scenario_started`, `row_style()`)
        // rather than ad-hoc templates that could drift from production.
        summary.set_style(summary_style());
        summary.set_message("0/0 running ◆ 0 failed");
        let trailer = multi.add(ProgressBar::new_spinner());
        trailer.set_style(ProgressStyle::with_template(" ").unwrap());
        IndicatifProgressSink {
            multi,
            state_lock: Mutex::new(()),
            summary,
            _trailer: trailer,
            rows: Mutex::new(HashMap::new()),
            failed: AtomicUsize::new(0),
            cancelled: AtomicUsize::new(0),
            interrupt: InterruptFlag::new(),
            name_col_width: 0,
            step_counter_width: 0,
            total_workers: 4,
        }
    }

    #[test]
    fn lifecycle_methods_do_not_panic() {
        let sink = hidden_sink();
        sink.run_started(2);
        sink.scenario_started("alpha", 3);
        sink.step_started("alpha", 0, 3, "first");
        sink.step_started("alpha", 1, 3, "second");
        sink.complete_scenario(
            "alpha",
            ScenarioStatus::Pass,
            Duration::from_millis(120),
            b"",
        );
        sink.scenario_started("beta", 2);
        sink.complete_scenario("beta", ScenarioStatus::Fail, Duration::from_millis(80), b"");
        sink.run_finished();
    }

    #[test]
    fn failed_counter_increments_on_fail_only() {
        let sink = hidden_sink();
        sink.run_started(3);
        sink.scenario_started("a", 1);
        sink.complete_scenario("a", ScenarioStatus::Pass, Duration::ZERO, b"");
        sink.scenario_started("b", 1);
        sink.complete_scenario("b", ScenarioStatus::Fail, Duration::ZERO, b"");
        sink.scenario_started("c", 1);
        sink.complete_scenario("c", ScenarioStatus::Fail, Duration::ZERO, b"");
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
        summary.set_style(summary_style());
        let trailer = multi.add(ProgressBar::new_spinner());
        trailer.set_style(ProgressStyle::with_template(" ").unwrap());
        let sink = IndicatifProgressSink {
            multi,
            state_lock: Mutex::new(()),
            summary,
            _trailer: trailer,
            rows: Mutex::new(HashMap::new()),
            failed: AtomicUsize::new(0),
            cancelled: AtomicUsize::new(0),
            interrupt: interrupt.clone(),
            name_col_width: 0,
            step_counter_width: 0,
            total_workers: 4,
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
        sink.complete_scenario("a", ScenarioStatus::Cancelled, Duration::ZERO, b"");
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
        sink.complete_scenario("a", ScenarioStatus::Pass, Duration::ZERO, b"");
        sink.scenario_started("b", 1);
        sink.complete_scenario("b", ScenarioStatus::Fail, Duration::ZERO, b"");
        sink.scenario_started("c", 1);
        sink.complete_scenario("c", ScenarioStatus::Cancelled, Duration::ZERO, b"");
        sink.scenario_started("d", 1);
        sink.complete_scenario("d", ScenarioStatus::Cancelled, Duration::ZERO, b"");
        assert_eq!(sink.failed.load(Ordering::Relaxed), 1);
        assert_eq!(sink.cancelled.load(Ordering::Relaxed), 2);
    }

    #[test]
    fn rows_clear_on_scenario_finished() {
        let sink = hidden_sink();
        sink.run_started(1);
        sink.scenario_started("x", 1);
        assert_eq!(sink.rows.lock().unwrap().len(), 1);
        sink.complete_scenario("x", ScenarioStatus::Pass, Duration::ZERO, b"");
        assert!(sink.rows.lock().unwrap().is_empty());
    }

    #[test]
    fn summary_style_template_parses() {
        // `summary_style()` is only reached through `new()` at runtime —
        // `hidden_sink()` builds its own style — so the `.expect()` on the
        // template parse is otherwise never exercised by `cargo test`. This
        // locks the template (and the `{bar:N./dim}` style spec) valid.
        let _ = summary_style();
    }

    #[test]
    fn digit_count_matches_decimal_width() {
        assert_eq!(digit_count(0), 1);
        assert_eq!(digit_count(9), 1);
        assert_eq!(digit_count(10), 2);
        assert_eq!(digit_count(580), 3);
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
    fn row_tail_pads_name_and_styles_step() {
        // hidden_sink uses width 0, so the scenario name isn't padded:
        // the tail is "<name>  <ESC>step<ESC>". Lock the column layout
        // (two-space separator) and that the step name carries styling.
        let sink = hidden_sink();
        let tail = sink.render_row_tail("checkout-basic", "Inspect workspace", Duration::ZERO);
        let visible = strip_ansi(&tail);
        assert_eq!(visible, "checkout-basic  Inspect workspace");
        // Style bytes are present around the step name (no styling on the
        // scenario name), and no `(slow)` suffix below the threshold.
        assert!(tail.contains("\x1b[95mInspect workspace\x1b[0m"));
        assert!(!visible.contains("(slow)"));
    }

    #[test]
    fn row_tail_appends_slow_past_threshold() {
        let sink = hidden_sink();
        let tail = sink.render_row_tail("s", "step", SLOW_THRESHOLD + Duration::from_secs(1));
        assert!(strip_ansi(&tail).ends_with("(slow)"));
    }

    #[test]
    fn row_counter_is_done_over_total_padded() {
        // step_counter_width 0 (hidden_sink) → no padding: bare "done/total".
        let sink = hidden_sink();
        assert_eq!(sink.render_row_counter(0, 5), "0/5");
        assert_eq!(sink.render_row_counter(2, 5), "2/5");
    }

    #[test]
    fn row_counter_pads_to_width() {
        let mut sink = hidden_sink();
        sink.step_counter_width = 6;
        // "2/5" is 3 chars, padded with trailing spaces to 6.
        assert_eq!(sink.render_row_counter(2, 5), "2/5   ");
    }

    /// Strip SGR escape sequences (`ESC [ … m`) so visible width can be
    /// measured by `chars().count()`. Sufficient for the styles the row
    /// renderer emits (purple/yellow/dim/reset).
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
