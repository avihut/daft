//! Indicatif-backed `ProgressSink` implementation.
//!
//! Renders a pinned multi-row region at the bottom of the terminal: a
//! totals line on top, then **one fixed row per worker** below it:
//!
//! ```text
//!   ⟨⣿⡇⣿ ⣧ … ⟩  211/581  0:05  9/10 running        <- totals
//!   ▬▬▬▬───────  2/5  1.2s  checkout-basic · Inspect …  <- busy worker
//!   ─────────────  idle                                  <- idle worker
//! ```
//!
//! The totals line leads with a cyan braille **completion-map**: a scattered
//! dissolve that lights `round(done/total × num_dots)` dots in a fixed
//! scattered, gently bottom-left → top-right order, so the field fills evenly
//! ("develops") as the run progresses. It's followed by the `done/total`
//! counter, run elapsed, and the running/failed/cancelled stats. See
//! [`summary_style`] / [`FieldData`] / [`render_field`] for its layout.
//!
//! The worker rows are a **fixed pool of slots** sized to the worker count
//! (capped at the scenario count), created once at [`ProgressSink::run_started`]
//! and never added or removed for the rest of the run — so the region's height
//! is constant and the rows never shift under the reader. A slot is *claimed*
//! when a worker picks up a scenario (rendered as a light medium-rect step bar
//! with a flowing `scenario · step` tail, see [`row_style`]) and *released*
//! back to a quiet `idle` placeholder ([`idle_row_style`]) when the scenario
//! finishes. Because the rayon pool has exactly `total_workers` threads, at
//! most that many scenarios are ever in flight, so a free slot always exists
//! to claim. See `reporter/CLAUDE.md` §8 for the design rationale.
//!
//! Concurrency: every method may be called from any rayon worker thread.
//! `MultiProgress` and `ProgressBar` are internally `Send + Sync` via
//! indicatif's own locking; the slot pool is wrapped in `Mutex` (claim/release
//! must be atomic against concurrent workers) and the completion-map `field`
//! is behind its own `Mutex`. Lock order is `state_lock` outermost; `slots` and
//! `field` are never held simultaneously.
//!
//! Styling reuses `reporter/CLAUDE.md` §1's budget — no new color slots:
//! medium-rect `▬` bar fill (default fg) over a dim `─` track, default-fg
//! counters, dim elapsed, a default-fg scenario name with a dim ` · step`
//! flowing after it, and a yellow `(slow)` suffix once a scenario's elapsed
//! crosses 5 s. The variable text tail rides in `{wide_msg}`, whose
//! ANSI-aware truncation keeps every row exactly one terminal line tall on
//! narrow displays — a hard requirement for indicatif's line accounting,
//! not just cosmetics.

use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
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

/// Width (in chars) of the totals line's braille completion-map. Each char
/// is a 2×4 braille cell = 8 dots, so the field holds `FIELD_WIDTH * 8`
/// dots; the field lights `round(done/total × num_dots)` of them in a fixed
/// scattered order (see [`FieldData`]).
const FIELD_WIDTH: usize = 16;

/// Unicode base of the braille patterns block (`U+2800`). A cell's glyph is
/// `BRAILLE_BASE + <8-bit dot mask>`.
const BRAILLE_BASE: u32 = 0x2800;

/// Dot-bit order within a 2×4 braille cell. `DOT_ORDER[k]` is the braille bit
/// for the k-th dot index within a cell; `DOT_XY[k]` is that dot's position
/// `(col, row)` (col 0 = left, row 0 = top). The two stay parallel — `k` indexes
/// both — so the reveal-order math can place a global dot at its true 2-D spot.
const DOT_ORDER: [u8; 8] = [0x40, 0x04, 0x02, 0x01, 0x80, 0x20, 0x10, 0x08];

/// `(col, row)` of each `DOT_ORDER[k]` within its 2×4 cell (col 0 = left
/// column, row 0 = top). Parallel to [`DOT_ORDER`]: index `k` is the same dot
/// in both. Used to give every global dot a field position for the reveal
/// gradient.
const DOT_XY: [(u32, u32); 8] = [
    (0, 3),
    (0, 2),
    (0, 1),
    (0, 0),
    (1, 3),
    (1, 2),
    (1, 1),
    (1, 0),
];

/// How strongly the dissolve's reveal order follows the bottom-left → top-right
/// gradient vs. scatters. `0.0` = pure scatter (no direction), `1.0` = a hard
/// directional wipe. `0.45` reads as a scattered "developing photo" that still
/// tends to fill from the bottom-left — tuned by eye against real runs.
const REVEAL_DIRECTION_BIAS: f64 = 0.45;

/// Width (in cols) of each in-flight worker row's step bar. Kept modest so
/// the bar + counter + time prefix leaves room for the truncating
/// `scenario · step` tail on narrow terminals.
const RUNNER_BAR_WIDTH: usize = 14;

/// Fixed width (in cols) of a worker row's time counter, so the
/// step-name tail to its right starts at a stable column across rows.
/// Covers the widest [`format_row_elapsed`] output (`999ms`, `12:34`).
const ROW_ELAPSED_WIDTH: usize = 6;

/// Decimal digit count of `n` (`0` → 1). Used to right-pad the `pos`
/// half of a `pos/len` counter so the column to its right (the time
/// counter) sits at a stable position as `pos` grows digits.
///
/// Takes `u64` to match indicatif's `ProgressState::pos`/`len`. There is a
/// `usize` twin in `manual_test::mod.rs` (used for the pre-scan over step
/// counts) — keep the two in sync if the formula ever changes.
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

/// Build the summary (totals) line's style.
///
/// The totals line leads with the braille **completion-map** (a spatial
/// field of finished scenarios, see [`FieldData`] / [`render_field`]) rather
/// than indicatif's count-based `{bar}` — a `{bar}` can only fill
/// left-to-right by count, but the map lights dots *by scenario index*, so
/// it fills scattered as a non-linear scheduler chews the run. The field is
/// pushed into `{prefix}` (recomputed only when a scenario completes, so the
/// steady tick is cheap); the rest follows the `counter → time → rest`
/// motif:
///
/// ```text
///   ⟨⣿⡇⣿ ⣧ … ⟩  211/581  0:05  9/10 running
/// ```
///
/// - `{prefix}` — the cyan, `⟨ ⟩`-framed completion-map (set via
///   `set_prefix`). Its char width is fixed for a given run, so it behaves
///   like a stable left column.
/// - `{scenario_counter}` — a custom key rendering `done/total` with `done`
///   right-padded to `total`'s digit width, so the time column doesn't shift
///   as the count grows digits.
/// - `{elapsed_precise}` — dim run elapsed (scaffolding).
/// - `{wide_msg}` — the running/failed/cancelled segments built in
///   `update_summary_msg`. `wide_msg` truncates (never wraps) to the terminal
///   width, which keeps the line exactly one row tall on narrow terminals —
///   a correctness requirement for indicatif's line accounting, not just
///   cosmetics. Truncation is ANSI-aware, so the inlined red/yellow segments
///   survive a cut.
fn summary_style() -> ProgressStyle {
    ProgressStyle::with_template("{prefix}  {scenario_counter}  {elapsed_precise:.dim}  {wide_msg}")
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

/// Aperiodic hash of a dot index into `[0, 1)` — the scatter component of the
/// reveal order. A plain integer bit-mix (no `rand` dep, deterministic across
/// runs); the point is only that adjacent dots get unrelated values so the
/// dissolve looks organic rather than tiled/periodic.
fn dot_jitter(d: usize) -> f64 {
    let mut x = (d as u64)
        .wrapping_mul(0x9E3779B97F4A7C15)
        .wrapping_add(0x1234_5678);
    x ^= x >> 30;
    x = x.wrapping_mul(0xBF58_476D_1CE4_E5B9);
    x ^= x >> 27;
    // Top 24 bits → [0, 1). Plenty of resolution to break ties among 128 dots.
    ((x >> 40) as f64) / ((1u64 << 24) as f64)
}

/// The reveal-order sort key for global dot `d` over a `num_cells`-wide field.
/// Combines a bottom-left → top-right gradient with [`dot_jitter`]; sorting
/// dots ascending by this key yields the scattered, gently-directional reveal
/// order the dissolve lights in. Lower = lit earlier.
fn reveal_key(d: usize, num_cells: usize) -> f64 {
    let (col, row) = DOT_XY[d % 8];
    let x = (d / 8) as u32 * 2 + col; // global column, 0 = leftmost
    let max_x = (num_cells.max(1) as u32 * 2).saturating_sub(1).max(1) as f64;
    // Gradient 0 at bottom-left (x=0, row=bottom=3) → 1 at top-right.
    let left_to_right = x as f64 / max_x;
    let bottom_to_top = (3 - row) as f64 / 3.0;
    let gradient = (left_to_right + bottom_to_top) / 2.0;
    gradient * REVEAL_DIRECTION_BIAS + dot_jitter(d) * (1.0 - REVEAL_DIRECTION_BIAS)
}

/// The totals line's completion-map state — a **scattered dissolve**.
///
/// The field is `reveal.len()` dots (≤ `FIELD_WIDTH * 8`). It lights exactly
/// `round(done / total × num_dots)` of them, so the lit fraction tracks the
/// completed fraction **linearly** — the field fills evenly over the run rather
/// than staying dark until the end. *Which* dots light is a fixed scattered
/// order ([`reveal`], built once in [`sized`] from [`reveal_key`]): a gently
/// bottom-left → top-right "developing photo," not a left-to-right wipe.
///
/// This is honest about being a **progress** indicator rendered spatially — a
/// lit dot is not a specific finished scenario. The earlier design lit dots by
/// per-index cluster completion, which (with a breadth-first scheduler) left
/// every cluster partway done for most of the run, so the field stayed near-
/// empty until a late burst — misleading, and the reason this was rebuilt.
struct FieldData {
    total: usize,
    done: usize,
    /// Dot indices in reveal order (lit earliest first). Length is the field's
    /// dot count, `min(total, FIELD_WIDTH*8)`.
    reveal: Vec<u16>,
}

impl FieldData {
    /// An unsized field (before `run_started` knows the scenario count).
    fn empty() -> Self {
        Self {
            total: 0,
            done: 0,
            reveal: Vec::new(),
        }
    }

    /// Size the field for a `total`-scenario run and precompute the scattered
    /// reveal order. `num_dots = min(total, FIELD_WIDTH*8)`.
    fn sized(total: usize) -> Self {
        let num_dots = total.min(FIELD_WIDTH * 8);
        let num_cells = num_dots.div_ceil(8);
        let mut reveal: Vec<u16> = (0..num_dots as u16).collect();
        // Stable sort by the reveal key; `total_cmp` because the keys are
        // finite f64s and we want a deterministic, panic-free ordering.
        reveal.sort_by(|&a, &b| {
            reveal_key(a as usize, num_cells).total_cmp(&reveal_key(b as usize, num_cells))
        });
        Self {
            total,
            done: 0,
            reveal,
        }
    }

    /// Record that one scenario reached a terminal state. The dissolve is
    /// count-based, so the scenario's identity/index doesn't matter — only how
    /// many have finished.
    fn complete(&mut self) {
        if self.done < self.total {
            self.done += 1;
        }
    }

    /// How many dots are lit: `round(done / total × num_dots)`. Proportional,
    /// so the field fills linearly with progress and is full exactly at
    /// `done == total`.
    fn lit_count(&self) -> usize {
        let num_dots = self.reveal.len();
        if self.total == 0 || num_dots == 0 {
            return 0;
        }
        // Rounded integer division: (done·num_dots + total/2) / total.
        (self.done * num_dots + self.total / 2) / self.total
    }
}

/// Render the completion-map prefix: a cyan, `⟨ ⟩`-framed braille field with the
/// first `lit_count` dots of the scattered reveal order lit.
///
/// The `⟨ ⟩` frame is dim, the dots cyan — inlined ANSI (the same `NO_COLOR`
/// carve-out as the bar messages; see `reporter/CLAUDE.md` §8) because the field
/// is a hand-built string, not a styled built-in key. An unsized field renders
/// as `FIELD_WIDTH` blank cells so the placeholder reads as an empty gauge.
fn render_field(field: &FieldData) -> String {
    let num_dots = field.reveal.len();
    let cell_count = if num_dots == 0 {
        FIELD_WIDTH
    } else {
        num_dots.div_ceil(8)
    };
    let mut cells = vec![0u8; cell_count];
    for &d in field.reveal.iter().take(field.lit_count()) {
        let d = d as usize;
        cells[d / 8] |= DOT_ORDER[d % 8];
    }
    let body: String = cells
        .iter()
        .map(|&c| char::from_u32(BRAILLE_BASE + c as u32).expect("braille codepoint is valid"))
        .collect();
    format!("\x1b[2m⟨\x1b[0m\x1b[36m{body}\x1b[0m\x1b[2m⟩\x1b[0m")
}

/// Build the line that persists the completion-map into scrollback at
/// end-of-run, so the filled map doesn't vanish when the live region is torn
/// down. It freezes the totals line's left half — the `⟨ ⟩`-framed field plus
/// a `done/total scenarios` caption — and drops the live-only segments
/// (elapsed, running/failed/cancelled): the reporter's summary block lands
/// directly below this line and already owns the precise duration and
/// pass/fail tally, so repeating them here would just duplicate. `field.done`
/// is the completed count (`== total` on a clean run, fewer after a cancel, so
/// the partially-filled map reads as "this is how far we got"). `scenarios` is
/// dim — the field is the data, the caption is a label. Returns `None` for an
/// empty run (nothing to map).
fn final_completion_line(field: &FieldData) -> Option<String> {
    if field.total == 0 {
        return None;
    }
    Some(format!(
        "{}  {}/{} \x1b[2mscenarios\x1b[0m",
        render_field(field),
        field.done,
        field.total,
    ))
}

/// Build a worker row's style — light and quiet, so N stacked rows never
/// read as a solid block wall:
///
/// ```text
///   ▬▬▬▬─────── 2/5  1.2s  checkout-basic · Inspect workspace
/// ```
///
/// - `{bar}` — completed-step progress, fixed [`RUNNER_BAR_WIDTH`], with
///   `progress_chars("▬─")`: a medium-rect `▬` fill (default fg) over a thin
///   `─` track (dim). Heavier than a hairline rule but never the full-cell
///   block — no new color slot.
/// - `{prefix}` — the `done/total` step counter, padded to the run's widest
///   counter (set via `set_prefix`).
/// - `{row_elapsed}` — a custom key with sub-second precision, dim and
///   padded to [`ROW_ELAPSED_WIDTH`] so the tail starts at a stable column.
/// - `{wide_msg}` — `"<scenario> · <current step>"` (set via `set_message`):
///   scenario name default fg, ` · ` and step name dim, flowing naturally
///   with no fixed step column. `wide_msg` truncates (never wraps) to the
///   terminal width, so the row stays exactly one line tall regardless of
///   name length — a correctness requirement for indicatif's line
///   accounting. Truncation is ANSI-aware, so the dim step and yellow
///   `(slow)` survive a cut without bleeding color.
fn row_style() -> ProgressStyle {
    // The `\x1b[2m … \x1b[0m` wrapping `{row_elapsed}` is raw dim SGR, which
    // (unlike the template's `:.dim` modifier on built-in keys) bypasses
    // `NO_COLOR` — bytes inside a custom key are passed through verbatim.
    // There's no alternative: indicatif's style modifiers only apply to
    // built-in keys, and `row_elapsed` is custom (sub-second precision).
    // Same ANSI-inlining carve-out as `update_summary_msg`; see
    // `reporter/CLAUDE.md` §8.
    ProgressStyle::with_template(&format!(
        "{{bar:{RUNNER_BAR_WIDTH}./dim}}  {{prefix}}  \x1b[2m{{row_elapsed}}\x1b[0m  {{wide_msg}}"
    ))
    .expect("static row template should be valid")
    .progress_chars("▬─")
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

/// The style for a slot that currently has no scenario: a dim empty `─` track
/// (so its left edge lines up with the busy rows' bars) and a dim `idle`
/// label. Deliberately omits the `{prefix}` counter and `{row_elapsed}` time
/// of [`row_style`] — there's no scenario to count steps for, and a ticking
/// clock on a worker that isn't running would read as activity where there is
/// none. With no time token, the steady tick redraws an identical line, so it
/// stays idempotent (no flicker) while the slot waits.
///
/// The `\x1b[2m…\x1b[0m` around `idle` is raw dim SGR (same `NO_COLOR`
/// carve-out as [`row_style`]'s `{row_elapsed}` and the summary separators):
/// `idle` is template text, not a styled built-in key, so the dim is inlined.
fn idle_row_style() -> ProgressStyle {
    ProgressStyle::with_template(&format!(
        "{{bar:{RUNNER_BAR_WIDTH}./dim}}  \x1b[2midle\x1b[0m"
    ))
    .expect("static idle row template should be valid")
    .progress_chars("▬─")
}

/// State the SIGINT hard-exit handler needs to collapse the live region from the
/// `ctrlc` crate's own thread (it has no reference to the sink): the
/// `MultiProgress`, a clone of every bar in it, and the totals "end screen" lines
/// to print where the live totals line was.
///
/// This is the one case the soft-cancel teardown can't cover: when every worker
/// is wedged in a slow subprocess, no `complete_scenario` / `notify_cancelling`
/// fires to collapse the region, so a second Ctrl+C reaches the hard-exit path
/// with the region still drawn.
///
/// Tearing it down must **remove every bar**, not merely `clear()` the drawn
/// lines: each bar carries a 200 ms steady tick, and after a bare `clear()`
/// (even with the draw target hidden) an in-flight tick repaints the region —
/// which is exactly what stranded the live totals line above the forced-exit
/// notice while the `rm -rf` cleanup loop ran for ~120 ms+. Removing the bars
/// stops the ticks at the source, mirroring
/// [`IndicatifProgressSink::tear_down_region`]. The handles are `ProgressBar`
/// clones (an `Arc` inside), so removing via these removes the same underlying
/// bars the sink holds.
struct ExitHandle {
    multi: MultiProgress,
    /// summary + trailer + every slot bar. Empty until `run_started` builds the
    /// slot pool; a hard exit before then has nothing drawn to strand.
    bars: Vec<ProgressBar>,
    /// The totals end-screen lines (completion-map + a passed/failed/cancelled
    /// breakdown), shared with the sink, which refreshes them as scenarios
    /// finish. Carries no live-only `running` count or ticking elapsed — both
    /// meaningless once the run has stopped.
    summary: Arc<Mutex<Vec<String>>>,
}

/// Process-global force-quit handle. `None` whenever there is no live region
/// (non-TTY runs, or after teardown), so the handler's call is a safe no-op.
static EXIT_HANDLE: Mutex<Option<ExitHandle>> = Mutex::new(None);

/// Publish the force-quit handle when the live sink is built. The `summary` Arc
/// is shared with the sink so its later refreshes are visible here without
/// re-registering. Bars are filled by [`set_exit_handle_bars`] once
/// `run_started` builds the pool. Idempotent; overwrites any prior handle (only
/// one run/sink is live at a time).
fn register_exit_handle(multi: &MultiProgress, summary: Arc<Mutex<Vec<String>>>) {
    let mut g = EXIT_HANDLE
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    *g = Some(ExitHandle {
        multi: multi.clone(),
        bars: Vec::new(),
        summary,
    });
}

/// Record the live region's bars on the registered handle so a force-quit can
/// remove them. No-op when no handle is registered — the `hidden_sink` test
/// fixture builds the sink struct directly (bypassing `new`), so unit tests
/// never touch this global.
fn set_exit_handle_bars(bars: Vec<ProgressBar>) {
    let mut g = EXIT_HANDLE
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    if let Some(handle) = g.as_mut() {
        handle.bars = bars;
    }
}

/// Drop the force-quit handle once the region is gone, so a later stray signal
/// doesn't tear down a dead region or reprint a stale summary.
fn unregister_exit_handle() {
    let mut g = EXIT_HANDLE
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    *g = None;
}

/// Collapse the live region from the SIGINT hard-exit handler and print the
/// totals end screen where the stranded live line was. A no-op when no region is
/// registered (non-TTY, or already torn down).
///
/// The erase is driven by `multi.println`, NOT `multi.clear()`. `remove` only
/// hides each bar's own draw target and unlinks it (`multi.rs::remove`) — it
/// does **not** redraw, so the drawn region stays on screen until the next draw
/// op. `clear()` then erases only `last_line_count` lines, and that count
/// desyncs under load: during a high-completion run the summary's steady tick
/// redraws concurrently with our teardown, so `clear()` wipes fewer lines than
/// were drawn and the live totals frame (`…N/M running`) is left stranded above
/// the forced-exit notice. `println`'s redraw path explicitly clears stale lines
/// instead — this is daft's proven hook-progress teardown
/// (`src/output/hook_progress/interactive.rs::remove_job_bars`: "the next
/// `mp.println` does an atomic redraw that cleanly clears the old bar lines").
///
/// Order matters: disable every steady tick first (so no concurrent tick can
/// desync the line count or repaint mid-teardown), then unlink the bars, then
/// `println` the end screen (a leading blank separates it from the streamed
/// footers above), then hide the target so nothing repaints before
/// `process::exit`.
pub fn finalize_region_for_exit() {
    let guard = EXIT_HANDLE
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    let Some(handle) = guard.as_ref() else {
        return;
    };
    for bar in &handle.bars {
        bar.disable_steady_tick();
    }
    for bar in &handle.bars {
        handle.multi.remove(bar);
    }
    let lines = handle.summary.lock().map(|s| s.clone()).unwrap_or_default();
    // Leading blank + the end-screen lines, all via `println` so the first one's
    // atomic redraw erases the just-removed region. Always print the blank even
    // when there are no end-screen lines, so the region is still erased.
    let _ = handle.multi.println("");
    for line in &lines {
        let _ = handle.multi.println(line);
    }
    handle.multi.set_draw_target(ProgressDrawTarget::hidden());
}

/// Build the totals "end screen" a force-quit prints in place of the stranded
/// live totals line: the completion-map line (cyan field + `done/total
/// scenarios`, the same line `run_finished` persists) plus a one-line
/// passed/failed/cancelled/not-run breakdown of what actually finished. Drops
/// the live-only `running` count and ticking elapsed — meaningless once the run
/// has stopped. Empty for a run that never sized (nothing to show).
fn forced_exit_lines(field: &FieldData, failed: usize, cancelled: usize) -> Vec<String> {
    let Some(map) = final_completion_line(field) else {
        return Vec::new();
    };
    let done = field.done;
    // `done` counts every scenario that reached a terminal state; the remainder
    // are passes. `saturating_sub` guards the (impossible) over-count from a
    // torn counter rather than panicking inside a signal-driven teardown.
    let passed = done.saturating_sub(failed + cancelled);
    let not_run = field.total.saturating_sub(done);
    // Dim-anchored, mirroring the live summary's palette: failed bold-red,
    // cancelled yellow, the structural words dim. Inlined SGR (same NO_COLOR
    // carve-out as the rest of the region; see `reporter/CLAUDE.md` §8).
    let mut stats = format!(
        "\x1b[2mstopped at\x1b[0m {done}/{} \x1b[2m·\x1b[0m {passed} passed",
        field.total
    );
    if failed > 0 {
        stats.push_str(&format!(
            " \x1b[2m·\x1b[0m \x1b[1;31m{failed} failed\x1b[0m"
        ));
    }
    if cancelled > 0 {
        stats.push_str(&format!(
            " \x1b[2m·\x1b[0m \x1b[33m{cancelled} cancelled\x1b[0m"
        ));
    }
    if not_run > 0 {
        stats.push_str(&format!(" \x1b[2m· {not_run} not run\x1b[0m"));
    }
    vec![map, stats]
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
    /// The fixed pool of worker rows. Created once in `run_started` (one slot
    /// per worker, capped at the scenario count) and never resized, so the
    /// region's height — and every row's screen position — is stable for the
    /// whole run. Each slot is idle until a worker claims it in
    /// `scenario_started` and returns to idle in `complete_scenario`. A scenario
    /// is matched to its slot by the slot's `occupant` index (scenario *names*
    /// aren't unique, so a name match could collide two in-flight scenarios).
    /// Behind a `Mutex` because claim/release must be atomic against concurrent
    /// workers, and the bar's style swap (busy ↔ idle) must not interleave with
    /// another worker re-claiming the same freed slot.
    slots: Mutex<Vec<Slot>>,
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
    /// Pre-computed widest `done/total` step counter across the run, in
    /// chars. Used to right-pad each worker row's counter so the time
    /// counter to its right lands at a stable position across rows.
    step_counter_width: usize,
    /// Size of the rayon worker pool (resolved `jobs`). Rendered on the
    /// summary as `R/A running` (`R` in-flight, `A` = this) so the reader
    /// can see how saturated the pool is.
    total_workers: usize,
    /// The totals line's completion-map state. Sized in `run_started` and
    /// advanced in `complete_scenario`; rendered into the summary's `{prefix}`.
    /// Behind its own `Mutex` (not `state_lock`) so advancing it doesn't
    /// contend with the row add/remove ordering lock.
    field: Mutex<FieldData>,
    /// Whether the pinned live region is still on screen. Starts `true`; flips
    /// `false` exactly once when the region is torn down — either normally in
    /// `run_finished`, or early via [`enter_cancelled_mode`] on the first
    /// SIGINT. Once `false`, `complete_scenario` stops touching the bars and
    /// just streams scenario buffers as plain lines. This is what stops the
    /// cancel wind-down from ghosting: the old design kept redrawing the region
    /// as a burst of bailing workers streamed footers, and the rapid
    /// `println`-plus-redraw churn desynced indicatif's line accounting,
    /// stranding frame after frame in scrollback. Guarded by `state_lock` (the
    /// atomic is only for `Sync`; every read/write happens under the lock).
    active: AtomicBool,
    /// The totals "end screen" lines a force-quit prints in place of the
    /// stranded live totals line — refreshed on `run_started` and each
    /// `complete_scenario` from [`forced_exit_lines`]. Held behind an `Arc` and
    /// registered on the process-global [`ExitHandle`] so the SIGINT hard-exit
    /// handler (which has no reference to this sink) can read the latest totals
    /// after tearing the region down.
    exit_summary: Arc<Mutex<Vec<String>>>,
}

/// One row in the fixed worker pool: a persistent bar plus the scenario that
/// currently occupies it (or `None` when idle).
struct Slot {
    bar: ProgressBar,
    occupant: Option<SlotOccupant>,
}

struct SlotOccupant {
    /// The scenario's stable index — how `step_started` / `complete_scenario`
    /// re-find this slot (find-by-occupant, never a cached slot position,
    /// since a slot can be released and re-claimed between two calls).
    index: usize,
    /// The scenario's display name, stored so `step_started` can render the
    /// row tail without the caller re-passing it.
    name: String,
}

impl IndicatifProgressSink {
    pub fn new(step_counter_width: usize, total_workers: usize, interrupt: InterruptFlag) -> Self {
        // Cap the overall draw rate so steady ticks don't pile up faster
        // than the terminal can flush them. See `MAX_DRAW_HZ` for the
        // rationale.
        let multi =
            MultiProgress::with_draw_target(ProgressDrawTarget::stderr_with_hz(MAX_DRAW_HZ));
        // Publish the region for the SIGINT hard-exit handler to tear down if a
        // stuck run can't reach the cooperative teardown. The shared
        // `exit_summary` lets that handler print the totals end screen after the
        // teardown; bars are attached once `run_started` builds the pool.
        // Cleared in `tear_down_region`.
        let exit_summary: Arc<Mutex<Vec<String>>> = Arc::new(Mutex::new(Vec::new()));
        register_exit_handle(&multi, Arc::clone(&exit_summary));
        let summary = multi.add(ProgressBar::new(0));
        // Totals line leads with the completion-map at column 0 (so it anchors
        // at the same column as the scrollback `✓ name` / `✗ name` footers)
        // followed by the scenario counter, run elapsed, and the
        // running/failed/cancelled segments. See [`summary_style`] for the
        // layout rationale.
        summary.set_style(summary_style());
        // Deliberately no `set_message` / `set_prefix` / `enable_steady_tick`
        // here: those would draw the summary the instant the sink is built,
        // which is *before* the run banner is written to raw stderr — and the
        // banner write then strands that first frame above it. `run_started`
        // (called after the banner) seeds the real content and starts the
        // steady tick, so the region's first draw lands cleanly below the
        // banner.

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
            slots: Mutex::new(Vec::new()),
            failed: AtomicUsize::new(0),
            cancelled: AtomicUsize::new(0),
            interrupt,
            field: Mutex::new(FieldData::empty()),
            step_counter_width,
            total_workers,
            active: AtomicBool::new(true),
            exit_summary,
        }
    }

    fn update_summary_msg(&self) {
        // `running` = claimed slots, not the slot-pool size — an idle slot is a
        // waiting worker, not a running one. Must not be called while holding
        // `slots` (this re-locks it); every caller drops `slots` first.
        let running = self
            .slots
            .lock()
            .map(|s| s.iter().filter(|slot| slot.occupant.is_some()).count())
            .unwrap_or(0);
        let failed = self.failed.load(Ordering::Relaxed);
        let cancelled = self.cancelled.load(Ordering::Relaxed);
        let interrupted = self.interrupt.is_set();

        // Dim diamond between segments — decoration, so it stays dim and
        // never takes a color slot. Inlined SGR: like the segment colors
        // below, bytes inside the bar message bypass `NO_COLOR` (the bar
        // *template* honors it, but `{msg}` is passed through verbatim);
        // see `reporter/CLAUDE.md` §8's ANSI-inlining carve-out.
        const SEP: &str = " \x1b[2m◆\x1b[0m ";

        let mut msg = format!("{running}/{} running", self.total_workers);
        // Pass-quiet (§4): only surface `failed` once there's a failure —
        // a ticking `0 failed` on every green run is chrome. Bold red when
        // it appears (live equivalent of the fail-loud footer).
        if failed > 0 {
            msg.push_str(&format!("{SEP}\x1b[1;31m{failed} failed\x1b[0m"));
        }
        // Cancelled lives in the yellow slot ("attention without alarm").
        // Surfaces once there's a cancellation or the run is being
        // interrupted, so the user watches the count grow as in-flight
        // workers wind down.
        if cancelled > 0 || interrupted {
            msg.push_str(&format!("{SEP}\x1b[33m{cancelled} cancelled\x1b[0m"));
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

    /// Build the `{wide_msg}` tail of a worker row: the scenario name with
    /// the current step flowing right after it, plus a `(slow)` suffix once
    /// the scenario crosses [`SLOW_THRESHOLD`].
    ///
    /// ```text
    ///   checkout-basic · Inspect workspace
    /// ```
    ///
    /// The scenario name carries the row's identity at default fg (the
    /// scannable anchor); the ` · ` separator and the step name are **dim**
    /// — the step changes every step, so keeping it quiet stops it from
    /// flicker-grabbing the eye, and reserves color for the totals line. No
    /// fixed step column: the step sits directly after the name so it never
    /// feels detached. The `(slow)` suffix is yellow (attention without
    /// alarm). The whole tail rides in `{wide_msg}`, whose ANSI-aware
    /// truncation cuts the step (then the name) on narrow terminals without
    /// wrapping the row or letting a color bleed past the cut.
    fn render_row_tail(&self, scenario_name: &str, step_name: &str, elapsed: Duration) -> String {
        let slow_suffix = if elapsed > SLOW_THRESHOLD {
            "  \x1b[33m(slow)\x1b[0m"
        } else {
            ""
        };
        format!("{scenario_name}\x1b[2m · {step_name}\x1b[0m{slow_suffix}")
    }

    /// Build the `{prefix}` step counter for a worker row: `done/total`
    /// padded to the run's widest counter so the time column to its right
    /// stays put across rows. Plain text (default fg) — the bar to its left
    /// is the visual anchor.
    fn render_row_counter(&self, done: usize, total: usize) -> String {
        Self::pad_to(&format!("{done}/{total}"), self.step_counter_width)
    }

    /// Dress a freshly-claimed slot's bar for an active scenario: the full
    /// [`row_style`], its step bar reset to empty, and the scenario name in the
    /// tail. `reset()` restarts the bar's elapsed timer (so a reused slot's
    /// `{row_elapsed}` counts from this scenario, not the prior occupant) and
    /// zeroes the position; it leaves the steady tick running and doesn't touch
    /// the length we set next. Caller holds the `slots` lock.
    fn dress_busy(&self, bar: &ProgressBar, name: &str, total_steps: usize) {
        bar.set_style(row_style());
        bar.reset();
        bar.set_length(total_steps as u64);
        bar.set_prefix(self.render_row_counter(0, total_steps));
        bar.set_message(name.to_string());
    }

    /// Return a slot's bar to the quiet idle placeholder: an empty `─` track.
    /// `set_length(1)` + `set_position(0)` renders `0/1` → empty; without a
    /// non-zero length a never-claimed slot keeps `ProgressBar::new`'s length 0,
    /// and indicatif draws `0/0` as a *full* bar (the bug where idle rows showed
    /// solid `▬▬▬` before any scenario claimed them). Caller holds the `slots`
    /// lock.
    fn dress_idle(bar: &ProgressBar) {
        bar.set_style(idle_row_style());
        bar.set_length(1);
        bar.set_position(0);
    }

    /// Remove every bar from the multi and wipe the drawn region, leaving the
    /// terminal clean for plain `println`s to follow. Flips `active` to `false`
    /// so `complete_scenario` stops touching the (now-gone) bars. Idempotent:
    /// a no-op once `active` is already `false`. Caller holds `state_lock`.
    fn tear_down_region(&self) {
        if !self.active.swap(false, Ordering::Relaxed) {
            return;
        }
        // Remove the slot bars first, then the framing bars, so nothing is left
        // for a steady tick to redraw, then clear the drawn lines. After this
        // the multi owns no bars and `multi.println` writes plainly.
        if let Ok(slots) = self.slots.lock() {
            for slot in slots.iter() {
                self.multi.remove(&slot.bar);
            }
        }
        self.multi.remove(&self.summary);
        self.multi.remove(&self._trailer);
        let _ = self.multi.clear();
        // The region is gone now — drop the force-quit handle so a later stray
        // signal can't try to tear down a dead region or reprint stale totals.
        unregister_exit_handle();
    }

    /// Collapse the live region on cancellation. Tears the region down once and
    /// prints a one-line notice explaining why scenario footers keep streaming
    /// (the in-flight workers are finishing their current step before bailing).
    /// From here on the run streams plain footers, then `run_finished` persists
    /// the partial completion-map above the summary. Idempotent — called from
    /// both the main thread (`notify_cancelling`) and worker threads
    /// (`complete_scenario`), whichever observes the interrupt first. Caller
    /// holds `state_lock`.
    fn enter_cancelled_mode(&self) {
        if !self.active.load(Ordering::Relaxed) {
            return;
        }
        self.tear_down_region();
        // Dim notice (inlined SGR, same NO_COLOR carve-out as the rest of the
        // region). Printed once, on a now-clean canvas, so the streaming
        // footers below read as deliberate wind-down rather than ghosting.
        let _ = self.multi.println(
            "\x1b[2mInterrupted — finishing in-flight scenarios (Ctrl+C again to force-quit)…\x1b[0m",
        );
    }
}

impl ProgressSink for IndicatifProgressSink {
    fn run_started(&self, total_scenarios: usize) {
        self.summary.set_length(total_scenarios as u64);
        self.summary.set_position(0);
        // Size the completion-map for this run and paint the empty field.
        if let Ok(mut field) = self.field.lock() {
            *field = FieldData::sized(total_scenarios);
            self.summary.set_prefix(render_field(&field));
            // Seed the force-quit snapshot so a hard exit before the first
            // completion still shows a (zero-progress) totals end screen rather
            // than a stranded live line.
            if let Ok(mut snap) = self.exit_summary.lock() {
                *snap = forced_exit_lines(&field, 0, 0);
            }
        }

        // Build the fixed worker-row pool. One slot per worker, but never more
        // than the scenario count (a 2-scenario run can't keep 10 workers busy,
        // so 10 idle rows would be a wall of nothing). Each slot starts idle and
        // is inserted just above the trailer so the order is
        // summary → slots → trailer; the trailer staying last is what keeps
        // indicatif's line accounting stable (see the `_trailer` field doc).
        // Hold `state_lock` for the whole insert sweep so a worker that races
        // ahead into `scenario_started` can't observe a half-built pool.
        let _state = self.state_lock.lock().unwrap_or_else(|e| e.into_inner());
        let num_slots = self.total_workers.min(total_scenarios);
        // The full bar set a force-quit must remove to stop the steady ticks
        // that would otherwise repaint over its `clear()` (see `ExitHandle`):
        // summary + trailer + every slot bar.
        let mut exit_bars = vec![self.summary.clone(), self._trailer.clone()];
        if let Ok(mut slots) = self.slots.lock() {
            if slots.is_empty() {
                for _ in 0..num_slots {
                    let bar = self
                        .multi
                        .insert_before(&self._trailer, ProgressBar::new(0));
                    Self::dress_idle(&bar);
                    // One steady tick per slot, enabled once and left on: the
                    // busy style's `{row_elapsed}` needs it to advance, and the
                    // idle style has no time token so the same tick is a no-op
                    // redraw while the slot waits — no per-claim toggling.
                    bar.enable_steady_tick(TICK_INTERVAL);
                    exit_bars.push(bar.clone());
                    slots.push(Slot {
                        bar,
                        occupant: None,
                    });
                }
            } else {
                exit_bars.extend(slots.iter().map(|slot| slot.bar.clone()));
            }
        }
        // Hand the bars to the force-quit handle (no-op if none registered, e.g.
        // unit tests that bypass `new`).
        set_exit_handle_bars(exit_bars);
        drop(_state);

        self.update_summary_msg();
        // Start the summary's steady tick now — after the banner has been
        // written and the region built — so its first draw lands cleanly below
        // the banner instead of stranding a `0/0` frame above it. The tick
        // refreshes `{elapsed_precise}` and the completion-map between
        // completions.
        self.summary.enable_steady_tick(TICK_INTERVAL);
    }

    fn scenario_started(&self, index: usize, name: &str, total_steps: usize) {
        // Claim the lowest free slot for this scenario. `state_lock` stays held
        // across the claim+dress so the buffer prints in `complete_scenario`
        // (which also takes it) can't interleave a redraw mid-style-swap; the
        // dress itself happens under the `slots` lock so a concurrent
        // `complete_scenario` releasing this same slot can't race the busy ↔
        // idle style swap. No row is added — the pool is fixed — so unlike the
        // old design there's no add/remove for a concurrent `println` to
        // mis-count against. Drop both locks before `update_summary_msg`, which
        // re-locks `slots`.
        let _state = self.state_lock.lock().unwrap_or_else(|e| e.into_inner());
        if let Ok(mut slots) = self.slots.lock() {
            if let Some(slot) = slots.iter_mut().find(|s| s.occupant.is_none()) {
                // No `starting…` placeholder — the bar-at-0 + `0/N` counter
                // already say "just started," and an ellipsis label is the §5
                // microcopy anti-pattern.
                self.dress_busy(&slot.bar, name, total_steps);
                slot.occupant = Some(SlotOccupant {
                    index,
                    name: name.to_string(),
                });
            }
            // No free slot is unreachable in practice (the rayon pool has
            // exactly `total_workers` threads and the pool is sized to match),
            // but if it ever happens the scenario simply runs without a live
            // row — its buffer still prints and its completion-map dot still
            // lights on `complete_scenario`.
        }
        drop(_state);
        self.update_summary_msg();
    }

    fn step_started(&self, index: usize, step_idx: usize, total_steps: usize, step_name: &str) {
        // `step_idx` is the 0-based index of the step now starting, so
        // `step_idx` steps are already complete — that's both the bar
        // position and the `done` half of the `done/total` counter.
        //
        // Two-phase lock: read *only* the scenario name + elapsed under the
        // lock, drop it, then format the tail/counter (pure-functional over
        // immutable `self` fields — no shared state), then re-acquire briefly
        // to push the update onto the bar. Keeping the formatting outside the
        // lock means concurrent workers' `step_started` calls don't serialize
        // on each other's string building.
        //
        // Both phases re-resolve the slot by `occupant.index == index` rather
        // than caching a slot position across the gap: between the two locks
        // `complete_scenario` can release this scenario's slot and another
        // worker can re-claim it, so a cached position could stamp this step
        // onto a *different* scenario's row. If the occupant is gone by phase
        // two, skip the update — visually equivalent to a `set_message` that
        // landed a frame before the slot was released.
        let (scenario_name, elapsed) = {
            let Ok(slots) = self.slots.lock() else {
                return;
            };
            match slots
                .iter()
                .find(|s| s.occupant.as_ref().is_some_and(|o| o.index == index))
            {
                Some(slot) => (
                    slot.occupant
                        .as_ref()
                        .map(|o| o.name.clone())
                        .unwrap_or_default(),
                    slot.bar.elapsed(),
                ),
                None => return,
            }
        };
        let tail = self.render_row_tail(&scenario_name, step_name, elapsed);
        let counter = self.render_row_counter(step_idx, total_steps);
        if let Ok(slots) = self.slots.lock() {
            if let Some(slot) = slots
                .iter()
                .find(|s| s.occupant.as_ref().is_some_and(|o| o.index == index))
            {
                slot.bar.set_position(step_idx as u64);
                slot.bar.set_prefix(counter);
                slot.bar.set_message(tail);
            }
        }
    }

    fn complete_scenario(
        &self,
        index: usize,
        status: ScenarioStatus,
        _duration: Duration,
        buf: &[u8],
    ) {
        // `state_lock` is held for the whole sequence. The fixed slot pool
        // means the region's line count never changes here (we release a slot
        // to idle, we don't remove a row), so the add/remove vs `println`
        // miscount that the old design fought — and indicatif issue #474's
        // stranded-bar class — can't arise: there's no row coming or going for
        // a concurrent redraw to mis-count against. What the lock still buys is
        // serialization of the buffer prints: two rayon workers' multi-line
        // `multi.println` output interleaving on stderr would garble both
        // scenarios' scrollback. (The trailer + draw-rate cap remain as
        // belt-and-suspenders for the steady-tick redraw path.)
        let _state = self.state_lock.lock().unwrap_or_else(|e| e.into_inner());

        // First worker to observe the interrupt collapses the live region (the
        // main thread also does this via `notify_cancelling`, whichever wins).
        // After this, the region is gone and the rest of this call just streams
        // the footer as a plain line — no bar redraws to strand during the
        // wind-down burst.
        if self.interrupt.is_set() {
            self.enter_cancelled_mode();
        }
        let region_live = self.active.load(Ordering::Relaxed);

        // While the region is live, release this scenario's slot back to idle
        // *before* printing its buffer, so by the time the scenario's `✓ name`
        // footer scrolls into place above the region the live row already reads
        // `idle` — no frame where a just-finished scenario still shows as
        // running. The dress happens under the `slots` lock so it can't
        // interleave with a concurrent `scenario_started` re-claiming the slot.
        // Once cancelled (region torn down) there are no bars to touch.
        if region_live {
            if let Ok(mut slots) = self.slots.lock() {
                if let Some(slot) = slots
                    .iter_mut()
                    .find(|s| s.occupant.as_ref().is_some_and(|o| o.index == index))
                {
                    slot.occupant = None;
                    Self::dress_idle(&slot.bar);
                }
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
        // Advance the completion-map by one — always, even after cancellation,
        // so the partial map `run_finished` persists reflects everything that
        // actually finished. Only the *redraw* into the (now-gone) summary bar
        // is gated on the region still being live.
        if let Ok(mut field) = self.field.lock() {
            field.complete();
            if region_live {
                self.summary.set_prefix(render_field(&field));
            }
            // Refresh the force-quit snapshot from the same locked field and the
            // counters bumped just above, so a hard exit (even after the region
            // was torn down by a soft cancel) prints accurate totals. Runs
            // regardless of `region_live` — the snapshot is read at exit, not
            // drawn now.
            if let Ok(mut snap) = self.exit_summary.lock() {
                *snap = forced_exit_lines(
                    &field,
                    self.failed.load(Ordering::Relaxed),
                    self.cancelled.load(Ordering::Relaxed),
                );
            }
        }
        if region_live {
            self.update_summary_msg();
        }
    }

    fn run_finished(&self) {
        // Hold `state_lock` so any in-flight `complete_scenario` or
        // `scenario_started` finishes before we wipe the region.
        // Otherwise the `multi.clear` could land between another
        // thread's println and slot release, leaving partial frame content.
        let _state = self.state_lock.lock().unwrap_or_else(|e| e.into_inner());

        // Tear the live region down first (a no-op if a cancel already did it),
        // so the persisted map and the reporter's summary land on a clean
        // canvas with no bars left to redraw underneath them.
        self.tear_down_region();

        // Persist the finished completion-map into scrollback — otherwise the
        // map the reader watched fill in just vanishes. Plain `multi.println`
        // (the region is gone), landing directly above the reporter's summary
        // block. `field.done` is the completed count (== total on a clean run,
        // fewer after a cancel, so the partial map reads "this far we got").
        let persisted = self
            .field
            .lock()
            .ok()
            .and_then(|field| final_completion_line(&field));
        if let Some(line) = persisted {
            let _ = self.multi.println(line);
        }
    }

    fn notify_cancelling(&self) {
        // Main-thread entry point for cancellation: collapse the live region
        // promptly so the wind-down streams cleanly. Idempotent with the
        // worker-thread trigger in `complete_scenario` — whichever observes the
        // interrupt first tears the region down; the other is a no-op.
        let _state = self.state_lock.lock().unwrap_or_else(|e| e.into_inner());
        self.enter_cancelled_mode();
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
        summary.set_message("0/0 running");
        let trailer = multi.add(ProgressBar::new_spinner());
        trailer.set_style(ProgressStyle::with_template(" ").unwrap());
        IndicatifProgressSink {
            multi,
            state_lock: Mutex::new(()),
            summary,
            _trailer: trailer,
            slots: Mutex::new(Vec::new()),
            failed: AtomicUsize::new(0),
            cancelled: AtomicUsize::new(0),
            interrupt: InterruptFlag::new(),
            field: Mutex::new(FieldData::empty()),
            step_counter_width: 0,
            total_workers: 4,
            active: AtomicBool::new(true),
            // Bypasses `new`, so this sink is not registered on the process-global
            // `EXIT_HANDLE`; tests read `exit_summary` directly.
            exit_summary: Arc::new(Mutex::new(Vec::new())),
        }
    }

    #[test]
    fn lifecycle_methods_do_not_panic() {
        let sink = hidden_sink();
        sink.run_started(2);
        sink.scenario_started(0, "alpha", 3);
        sink.step_started(0, 0, 3, "first");
        sink.step_started(0, 1, 3, "second");
        sink.complete_scenario(0, ScenarioStatus::Pass, Duration::from_millis(120), b"");
        sink.scenario_started(1, "beta", 2);
        sink.complete_scenario(1, ScenarioStatus::Fail, Duration::from_millis(80), b"");
        sink.run_finished();
    }

    #[test]
    fn failed_counter_increments_on_fail_only() {
        let sink = hidden_sink();
        sink.run_started(3);
        sink.scenario_started(0, "a", 1);
        sink.complete_scenario(0, ScenarioStatus::Pass, Duration::ZERO, b"");
        sink.scenario_started(1, "b", 1);
        sink.complete_scenario(1, ScenarioStatus::Fail, Duration::ZERO, b"");
        sink.scenario_started(2, "c", 1);
        sink.complete_scenario(2, ScenarioStatus::Fail, Duration::ZERO, b"");
        assert_eq!(sink.failed.load(Ordering::Relaxed), 2);
        assert_eq!(sink.cancelled.load(Ordering::Relaxed), 0);
    }

    #[test]
    fn notify_cancelling_tears_down_the_region() {
        // On cancellation the orchestrator pokes the sink via
        // notify_cancelling, which collapses the live region exactly once so
        // the wind-down streams cleanly instead of the old design's redraw
        // burst stranding ghost frames. notify_cancelling is idempotent and
        // safe to call before any interrupt too.
        let sink = hidden_sink();
        sink.run_started(2);
        sink.scenario_started(0, "a", 1);
        assert!(sink.active.load(Ordering::Relaxed));

        // First cancel notification flips the region off…
        sink.notify_cancelling();
        assert!(!sink.active.load(Ordering::Relaxed));
        // …and a second is a harmless no-op (no double teardown / panic).
        sink.notify_cancelling();
        assert!(!sink.active.load(Ordering::Relaxed));

        // A scenario completing after cancellation still bumps the counters
        // and the completion-map (so the persisted map is accurate) without
        // touching the torn-down bars.
        sink.complete_scenario(0, ScenarioStatus::Cancelled, Duration::ZERO, b"");
        assert_eq!(sink.cancelled.load(Ordering::Relaxed), 1);
        assert_eq!(sink.field.lock().unwrap().done, 1);
    }

    #[test]
    fn complete_scenario_enters_cancelled_mode_on_interrupt() {
        // A worker observing the interrupt in complete_scenario is the other
        // teardown trigger (when the main thread's notify_cancelling hasn't run
        // yet). The region collapses on that first post-interrupt completion.
        let interrupt = InterruptFlag::new();
        let sink = {
            let mut s = hidden_sink();
            s.interrupt = interrupt.clone();
            s
        };
        sink.run_started(3);
        sink.scenario_started(0, "a", 1);
        sink.scenario_started(1, "b", 1);
        assert!(sink.active.load(Ordering::Relaxed));
        interrupt.set();
        sink.complete_scenario(0, ScenarioStatus::Cancelled, Duration::ZERO, b"");
        assert!(!sink.active.load(Ordering::Relaxed));
    }

    #[test]
    fn cancelled_counter_increments_separately_from_failed() {
        // Regression guard for the SIGINT bug: when a scenario is
        // cancelled mid-run, the bar must not bump the failed counter.
        // Polluting `failed` with cancelled work was the bug this whole
        // path was introduced to fix.
        let sink = hidden_sink();
        sink.run_started(4);
        sink.scenario_started(0, "a", 1);
        sink.complete_scenario(0, ScenarioStatus::Pass, Duration::ZERO, b"");
        sink.scenario_started(1, "b", 1);
        sink.complete_scenario(1, ScenarioStatus::Fail, Duration::ZERO, b"");
        sink.scenario_started(2, "c", 1);
        sink.complete_scenario(2, ScenarioStatus::Cancelled, Duration::ZERO, b"");
        sink.scenario_started(3, "d", 1);
        sink.complete_scenario(3, ScenarioStatus::Cancelled, Duration::ZERO, b"");
        assert_eq!(sink.failed.load(Ordering::Relaxed), 1);
        assert_eq!(sink.cancelled.load(Ordering::Relaxed), 2);
    }

    #[test]
    fn summary_hides_failed_until_a_failure() {
        // Pass-quiet: `failed` is absent from the totals message while the
        // count is 0, and appears bold-red once a scenario fails.
        let sink = hidden_sink();
        sink.run_started(2);
        sink.scenario_started(0, "a", 1);
        sink.complete_scenario(0, ScenarioStatus::Pass, Duration::ZERO, b"");
        assert!(!sink.summary.message().contains("failed"));
        sink.scenario_started(1, "b", 1);
        sink.complete_scenario(1, ScenarioStatus::Fail, Duration::ZERO, b"");
        let msg = sink.summary.message();
        assert!(msg.contains("\x1b[1;31m1 failed\x1b[0m"));
        // The segment separator is a dim diamond, never a colored one.
        assert!(msg.contains("\x1b[2m◆\x1b[0m"));
    }

    /// Count the slots currently occupied by a scenario.
    fn busy_slots(sink: &IndicatifProgressSink) -> usize {
        sink.slots
            .lock()
            .unwrap()
            .iter()
            .filter(|s| s.occupant.is_some())
            .count()
    }

    #[test]
    fn run_started_sizes_slot_pool_to_min_of_workers_and_scenarios() {
        // hidden_sink has total_workers = 4. A 2-scenario run needs only 2
        // rows — 4 idle rows for 2 scenarios would be a wall of nothing.
        let sink = hidden_sink();
        sink.run_started(2);
        assert_eq!(sink.slots.lock().unwrap().len(), 2);
        // A run with more scenarios than workers caps at the worker count.
        let sink = hidden_sink();
        sink.run_started(50);
        assert_eq!(sink.slots.lock().unwrap().len(), 4);
    }

    #[test]
    fn slot_is_claimed_then_released_to_idle() {
        let sink = hidden_sink();
        sink.run_started(3);
        assert_eq!(busy_slots(&sink), 0);
        sink.scenario_started(0, "alpha", 2);
        assert_eq!(busy_slots(&sink), 1);
        sink.scenario_started(1, "beta", 2);
        assert_eq!(busy_slots(&sink), 2);
        // Completing a scenario frees its slot (back to idle), but the pool
        // size — and so the region height — is unchanged.
        sink.complete_scenario(0, ScenarioStatus::Pass, Duration::ZERO, b"");
        assert_eq!(busy_slots(&sink), 1);
        assert_eq!(sink.slots.lock().unwrap().len(), 3);
        sink.complete_scenario(1, ScenarioStatus::Pass, Duration::ZERO, b"");
        assert_eq!(busy_slots(&sink), 0);
        assert_eq!(sink.slots.lock().unwrap().len(), 3);
    }

    #[test]
    fn freed_slot_is_reused_by_the_next_scenario() {
        // The pool is fixed: a third scenario after two finished must reuse a
        // freed slot, never grow the pool past its sized count.
        let sink = hidden_sink();
        sink.run_started(3);
        sink.scenario_started(0, "a", 1);
        sink.scenario_started(1, "b", 1);
        sink.complete_scenario(0, ScenarioStatus::Pass, Duration::ZERO, b"");
        sink.scenario_started(2, "c", 1);
        assert_eq!(busy_slots(&sink), 2);
        assert_eq!(sink.slots.lock().unwrap().len(), 3);
        // The reused slot now belongs to scenario 2, and a step lands on it.
        sink.step_started(2, 0, 1, "only step");
        let occupied: Vec<usize> = sink
            .slots
            .lock()
            .unwrap()
            .iter()
            .filter_map(|s| s.occupant.as_ref().map(|o| o.index))
            .collect();
        assert!(occupied.contains(&2));
        assert!(occupied.contains(&1));
    }

    #[test]
    fn running_count_reflects_busy_slots() {
        let sink = hidden_sink();
        sink.run_started(4);
        sink.scenario_started(0, "a", 1);
        sink.scenario_started(1, "b", 1);
        // total_workers is 4, two claimed → "2/4 running".
        assert!(sink.summary.message().starts_with("2/4 running"));
        sink.complete_scenario(0, ScenarioStatus::Pass, Duration::ZERO, b"");
        assert!(sink.summary.message().starts_with("1/4 running"));
    }

    #[test]
    fn freed_slot_bar_renders_empty() {
        // A released slot's bar must read empty (position 0), not stay full
        // from the scenario that just completed all its steps.
        let sink = hidden_sink();
        sink.run_started(1);
        sink.scenario_started(0, "a", 4);
        sink.step_started(0, 4, 4, "done"); // bar at full
        sink.complete_scenario(0, ScenarioStatus::Pass, Duration::ZERO, b"");
        let pos = sink.slots.lock().unwrap()[0].bar.position();
        assert_eq!(pos, 0);
    }

    #[test]
    fn summary_style_template_parses() {
        // `summary_style()` is only reached through `new()` at runtime —
        // `hidden_sink()` builds its own style — so the `.expect()` on the
        // template parse is otherwise never exercised by `cargo test`. This
        // locks the `{prefix} {scenario_counter} {elapsed} {wide_msg}`
        // template valid.
        let _ = summary_style();
    }

    #[test]
    fn idle_row_style_template_parses() {
        // `idle_row_style()` is only reached through `run_started` /
        // `complete_scenario` at runtime, so its template `.expect()` is
        // otherwise unexercised by `cargo test`. Lock it valid.
        let _ = idle_row_style();
    }

    #[test]
    fn final_line_freezes_field_with_caption() {
        // A full 8-scenario run: the persisted line carries the fully-lit
        // single-cell field (⣿) and an `8/8 scenarios` caption (dim caption,
        // default-fg count).
        let mut f = FieldData::sized(8);
        for _ in 0..8 {
            f.complete();
        }
        let line = final_completion_line(&f).expect("non-empty run yields a line");
        let visible = strip_ansi(&line);
        assert_eq!(visible, "⟨⣿⟩  8/8 scenarios");
    }

    #[test]
    fn final_line_shows_partial_coverage_after_cancel() {
        // Cancelled mid-run: done < total, so the caption reads how far we got
        // and the field is only partly lit.
        let mut f = FieldData::sized(8);
        for _ in 0..3 {
            f.complete();
        }
        let line = final_completion_line(&f).expect("non-empty run yields a line");
        assert!(strip_ansi(&line).ends_with("3/8 scenarios"));
    }

    #[test]
    fn final_line_is_none_for_empty_run() {
        // Nothing ran → nothing to map.
        assert!(final_completion_line(&FieldData::empty()).is_none());
        assert!(final_completion_line(&FieldData::sized(0)).is_none());
    }

    #[test]
    fn forced_exit_lines_break_down_done_into_pass_fail_cancel() {
        // A force-quit at 33/581 finished — 2 failed, 1 cancelled → 30 passed,
        // 548 not run. Line 1 is the persisted completion-map (identical to what
        // `run_finished` writes); line 2 breaks down `done` with no live count.
        let mut f = FieldData::sized(581);
        for _ in 0..33 {
            f.complete();
        }
        let lines = forced_exit_lines(&f, 2, 1);
        assert_eq!(lines.len(), 2);
        assert_eq!(
            strip_ansi(&lines[0]),
            strip_ansi(&final_completion_line(&f).unwrap())
        );
        assert!(strip_ansi(&lines[0]).ends_with("33/581 scenarios"));
        let stats = strip_ansi(&lines[1]);
        assert_eq!(
            stats,
            "stopped at 33/581 · 30 passed · 2 failed · 1 cancelled · 548 not run"
        );
        // The live-only "running" count is gone — it's meaningless once stopped.
        assert!(!stats.contains("running"));
        // Same palette as the live summary: failed bold-red, cancelled yellow.
        assert!(lines[1].contains("\x1b[1;31m2 failed\x1b[0m"));
        assert!(lines[1].contains("\x1b[33m1 cancelled\x1b[0m"));
    }

    #[test]
    fn forced_exit_lines_omit_zero_segments() {
        // A force-quit on an all-green run: no failed/cancelled/not-run noise,
        // just the pass count.
        let mut f = FieldData::sized(8);
        for _ in 0..8 {
            f.complete();
        }
        let stats = strip_ansi(&forced_exit_lines(&f, 0, 0)[1]);
        assert_eq!(stats, "stopped at 8/8 · 8 passed");
        assert!(!stats.contains("failed"));
        assert!(!stats.contains("cancelled"));
        assert!(!stats.contains("not run"));
    }

    #[test]
    fn forced_exit_lines_empty_for_unsized_run() {
        // Nothing sized/ran → nothing to show (parallels final_completion_line).
        assert!(forced_exit_lines(&FieldData::empty(), 0, 0).is_empty());
        assert!(forced_exit_lines(&FieldData::sized(0), 0, 0).is_empty());
    }

    #[test]
    fn complete_scenario_refreshes_force_quit_snapshot() {
        // The snapshot the SIGINT hard-exit handler reads is kept current as
        // scenarios finish. Seeded zero-progress at `run_started`, then after a
        // pass and a fail it reads 2 done / 1 passed / 1 failed — with no live
        // `running` count.
        let sink = hidden_sink();
        sink.run_started(4);
        assert!(strip_ansi(&sink.exit_summary.lock().unwrap()[1]).starts_with("stopped at 0/4"));
        sink.scenario_started(0, "a", 1);
        sink.complete_scenario(0, ScenarioStatus::Pass, Duration::ZERO, b"");
        sink.scenario_started(1, "b", 1);
        sink.complete_scenario(1, ScenarioStatus::Fail, Duration::ZERO, b"");
        let stats = strip_ansi(&sink.exit_summary.lock().unwrap()[1]);
        assert_eq!(stats, "stopped at 2/4 · 1 passed · 1 failed · 2 not run");
        assert!(!stats.contains("running"));
    }

    #[test]
    fn finalize_region_for_exit_is_safe_with_no_region() {
        // Non-TTY / already-torn-down: with no handle registered (unit tests
        // never register the process-global), it's a no-op and never panics
        // (so the hard-exit handler can call it unconditionally).
        finalize_region_for_exit();
    }

    #[test]
    fn field_sizes_dots_and_reveal_order() {
        // 640 scenarios cap at the 128-dot field; the reveal order is a
        // permutation of every dot (each lit exactly once over the run).
        let f = FieldData::sized(640);
        assert_eq!(f.reveal.len(), 128);
        let mut seen = f.reveal.clone();
        seen.sort_unstable();
        assert_eq!(seen, (0..128u16).collect::<Vec<_>>());
        // Small run: fewer scenarios than dots → one dot per scenario.
        assert_eq!(FieldData::sized(8).reveal.len(), 8);
    }

    #[test]
    fn field_fills_proportionally_to_progress() {
        // The lit count tracks the completed fraction linearly — the whole
        // point of the dissolve (the old per-cluster scheme stayed near-empty
        // until a late burst). 64 of 128 dots at the halfway mark.
        let mut f = FieldData::sized(581);
        let num_dots = f.reveal.len();
        for _ in 0..(581 / 2) {
            f.complete();
        }
        let lit = f.lit_count();
        let expected = (290 * num_dots + 581 / 2) / 581;
        assert_eq!(lit, expected);
        assert!(
            (60..=68).contains(&lit),
            "≈half the 128 dots lit at halfway, got {lit}"
        );
    }

    #[test]
    fn field_reveal_is_scattered_not_a_left_to_right_prefix() {
        // The first dots to light must NOT be the contiguous prefix 0,1,2,…
        // (that would be a left-to-right wipe). The scattered reveal order
        // means the early-lit set jumps around the field.
        let f = FieldData::sized(128);
        let first16: Vec<u16> = f.reveal.iter().copied().take(16).collect();
        let contiguous: Vec<u16> = (0..16u16).collect();
        assert_ne!(first16, contiguous);
        // Sanity: the reveal does still pull toward the bottom-left early —
        // dot 0 (cell 0, bottom-left corner) is lit within the first quarter.
        let pos0 = f.reveal.iter().position(|&d| d == 0).unwrap();
        assert!(pos0 < 32, "bottom-left corner lights early, pos {pos0}");
    }

    #[test]
    fn field_full_run_lights_every_dot() {
        // 8 scenarios = 8 dots = one full cell. Completing all → ⣿ (0xFF).
        let mut f = FieldData::sized(8);
        for _ in 0..8 {
            f.complete();
        }
        let body = strip_ansi(&render_field(&f));
        assert_eq!(body.chars().nth(1).unwrap() as u32, BRAILLE_BASE + 0xFF);
    }

    #[test]
    fn field_empty_renders_blank_framed_cells() {
        // An unsized field is the pre-`run_started` placeholder: FIELD_WIDTH
        // blank braille cells inside the ⟨ ⟩ frame.
        let body = strip_ansi(&render_field(&FieldData::empty()));
        assert_eq!(body.chars().next().unwrap(), '⟨');
        assert_eq!(body.chars().last().unwrap(), '⟩');
        assert_eq!(body.chars().count(), FIELD_WIDTH + 2); // frame + cells
        assert!(body.chars().skip(1).take(FIELD_WIDTH).all(|c| c == '⠀'));
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
    fn row_tail_flows_step_after_name() {
        // The step flows right after the name with a ` · ` separator (no
        // fixed column). Scenario name is plain; the separator + step are
        // dim; there is no bright-purple step any more.
        let sink = hidden_sink();
        let tail = sink.render_row_tail("checkout-basic", "Inspect workspace", Duration::ZERO);
        let visible = strip_ansi(&tail);
        assert_eq!(visible, "checkout-basic · Inspect workspace");
        // Separator + step are dim (one span); scenario name carries no
        // styling; no purple; no `(slow)` suffix below the threshold.
        assert!(tail.contains("\x1b[2m · Inspect workspace\x1b[0m"));
        assert!(!tail.contains("\x1b[95m"));
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
