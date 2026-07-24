//! Shared per-row output-thread machinery for rail renderers (#533).
//!
//! A "threaded job" is a live spinner row with, hanging from its glyph column,
//! an optional `❯ <command>` provenance line and a rolling window of the job's
//! latest output — and, on resolution, a persisted receipt of the job's full
//! log (`│  │    <line>`). The [`RailHookRenderer`](super::rail_hook) grew this
//! first for hook jobs; `daft exec` reuses the exact same grammar for its
//! per-worktree rows, so the subtle indicatif accounting (window rotation, the
//! never-`finish_and_clear` removal discipline, the wall-clock promoter, the
//! anti-fusion trailer) lives here once.
//!
//! What stays with each *caller* — deliberately, so this type serves both:
//! - the **main spinner row bar** (hooks own a name-keyed job bar; the region
//!   owns a plan-row bar), passed in as the `insert_after` anchor and the
//!   promoter's target;
//! - the **receipt row face** (hooks render `hook_job_row`, the region renders
//!   `final_row`) — [`ThreadedJob::compose_log`] emits only the thread lines;
//! - **how the latest line shows live** (hooks pad `{name}  {annotation}`, the
//!   region composes `active_message`) — [`ThreadedJob::record`] only buffers
//!   and flips the promoter atomics; the caller repaints its own bar.

use super::render;
use crate::output::hook_progress::format_duration;
use crate::output::palette::{DARK_GREY, GREY};
use indicatif::{MultiProgress, ProgressBar, ProgressStyle};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant};

/// One-line rendering of a job's command for the `❯` provenance line:
/// block-scalar `run:` commands carry embedded newlines that would tear the
/// thread apart (a println line must stay a line), so everything past the
/// first non-empty line collapses into `…`.
pub(super) fn command_line(cmd: &str) -> String {
    let mut lines = cmd.lines().filter(|l| !l.trim().is_empty());
    let first = lines.next().unwrap_or("").trim_end();
    match lines.next() {
        Some(_) => format!("{first} \u{2026}"),
        None => first.to_string(),
    }
}

/// The sub-row (thread) styles plus the rendering context they need. The main
/// spinner row's own style is the caller's (a hook job bar, or the region's
/// active-step bar) — only the thread lines live here.
///
/// `depth` is how many `│  ` rail tiers precede the thread's own inner `│` —
/// i.e. the gutter depth of the row the thread hangs under. A hook job row
/// sits inside its `├─` section, so its thread is depth 1 (2 in a group span);
/// a top-level `daft exec` row is the rail itself, so its thread is depth 0
/// (1 inside a worktree group). Getting this right is what keeps the inner `│`
/// under the row's glyph column in both worlds.
pub(super) struct ThreadStyles {
    /// `<depth × "│  ">│    {wide_msg}`.
    thread: ProgressStyle,
    /// The live block's bottom spacer: `{msg}` poked once at creation (a bar
    /// that never receives a state poke never draws — the `add_line_bar`
    /// pattern).
    trailer: ProgressStyle,
    use_color: bool,
    depth: usize,
}

impl ThreadStyles {
    pub(super) fn new(use_color: bool, depth: usize) -> Self {
        let inner = render::paint(DARK_GREY, "\u{2502}", use_color);
        let thread = ProgressStyle::with_template(&gutter_tiers(
            &format!("{inner}    {{wide_msg}}"),
            depth,
            use_color,
        ))
        .expect("thread template is valid");
        let trailer = ProgressStyle::with_template("{msg}").expect("trailer template is valid");
        Self {
            thread,
            trailer,
            use_color,
            depth,
        }
    }

    /// The thread's empty closing line: `<depth tiers>│` — the air stays inside
    /// the row's own thread, never spending the rail's lone-`│` boundary glyph
    /// on intra-thread spacing.
    pub(super) fn thread_air(&self) -> String {
        let inner = render::paint(DARK_GREY, "\u{2502}", self.use_color);
        gutter_tiers(&inner, self.depth, self.use_color)
    }

    /// One thread line: `<depth tiers>│    <text>` — the inner `│` hangs from
    /// the row's glyph column.
    fn thread_line(&self, text: &str) -> String {
        let inner = render::paint(DARK_GREY, "\u{2502}", self.use_color);
        gutter_tiers(&format!("{inner}    {text}"), self.depth, self.use_color)
    }
}

/// Prefix `line` with `depth` rail gutter tiers (`│  `).
fn gutter_tiers(line: &str, depth: usize, use_color: bool) -> String {
    let mut out = line.to_string();
    for _ in 0..depth {
        out = render::gutter(&out, use_color);
    }
    out
}

/// The live thread hanging under a spinner row, plus the buffered log for the
/// receipt. Owns everything *below* the row bar; the row bar itself is the
/// caller's.
pub(super) struct ThreadedJob {
    /// The `❯ <command>` provenance thread bar (verbose, preview known).
    cmd_bar: Option<ProgressBar>,
    /// Rolling live-tail thread bars (verbose; capped at `tail_lines`).
    tail_bars: Vec<ProgressBar>,
    /// Blank spacer bar at the bottom of the live block (verbose) so parallel
    /// blocks don't fuse — the live twin of the receipt's trailing air line.
    trailer: Option<ProgressBar>,
    /// Everything the job printed, buffered for the receipt log (verbose) or
    /// the deferred failure dump (succinct). Bounded by `cap` when set.
    output: Vec<String>,
    /// Running byte size of `output` (line lengths + one newline each), for
    /// the byte-budget cap.
    output_bytes: usize,
    /// Byte budget for `output`: oldest lines drop when exceeded. `None` keeps
    /// the whole log (hooks); `daft exec` caps a chatty worker's tail.
    cap: Option<usize>,
    /// The command preview, kept for the receipt log's `❯` line (`None` for
    /// callers that name the command elsewhere, e.g. exec's row/header).
    command_preview: Option<String>,
    /// Set on the first buffered output line; the promoter ticker exits once
    /// it's up (output un-promotes the elapsed counter).
    output_seen: Arc<AtomicBool>,
    /// Set when the job's bars leave the region; stops a pending promoter.
    resolved: Arc<AtomicBool>,
    /// Whether the elapsed counter currently occupies the annotation slot —
    /// set by the promoter ticker, cleared when output arrives.
    promoted: Arc<AtomicBool>,
    /// Set when a recognized manager's block for this job flushes (#753): the
    /// job finished running, verdict still pending. The promoter reads it so a
    /// straggler tick composes the grey `✓` done face, never the elapsed
    /// counter — the row can't flicker back to a timer once it's done.
    done_pending: Arc<AtomicBool>,
}

impl ThreadedJob {
    pub(super) fn new(cap: Option<usize>, command_preview: Option<String>) -> Self {
        Self {
            cmd_bar: None,
            tail_bars: Vec::new(),
            trailer: None,
            output: Vec::new(),
            output_bytes: 0,
            cap,
            command_preview,
            output_seen: Arc::new(AtomicBool::new(false)),
            resolved: Arc::new(AtomicBool::new(false)),
            promoted: Arc::new(AtomicBool::new(false)),
            done_pending: Arc::new(AtomicBool::new(false)),
        }
    }

    /// Open the live thread under `row_bar`: the `❯ <command>` provenance line
    /// (when a preview is known) and the bottom air spacer. Callers gate this
    /// on verbose && !quiet — succinct rows carry their liveness in the row's
    /// own annotation, not a thread.
    pub(super) fn open(
        &mut self,
        mp: &MultiProgress,
        row_bar: &ProgressBar,
        styles: &ThreadStyles,
    ) {
        // Idempotent: a density toggle can ask an already-open thread to
        // open again, and re-running the body would orphan the live bars it
        // overwrites (they stay in the `MultiProgress`, drawn forever) and
        // stack a second trailer.
        if self.is_open() {
            return;
        }
        self.cmd_bar = self.command_preview.as_deref().map(|cmd| {
            let pb = mp.insert_after(row_bar, ProgressBar::new_spinner());
            pb.set_style(styles.thread.clone());
            pb.set_message(render::paint(
                DARK_GREY,
                &format!("\u{276f} {}", command_line(cmd)),
                styles.use_color,
            ));
            pb
        });
        // The trailer sits below the cmd bar; tails insert above it
        // (`insert_after` the last tail / cmd / row bar places them before it).
        let anchor = self.cmd_bar.as_ref().unwrap_or(row_bar);
        let pb = mp.insert_after(anchor, ProgressBar::new_spinner());
        pb.set_style(styles.trailer.clone());
        pb.set_message(styles.thread_air());
        self.trailer = Some(pb);
    }

    /// Buffer one output line *raw* into the receipt log, enforcing the byte
    /// cap. The live view (annotation or rolling window) is repainted by the
    /// caller / [`Self::roll_window`]; this only records.
    ///
    /// The buffer is the source of truth and holds the unmodified child line,
    /// so the permanent surfaces that read it back — [`Self::compose_log`] for
    /// receipts and the caller's deferred failure dump — keep full fidelity
    /// (color, emoji, alignment). Width normalization for the live region
    /// ([`crate::output::live_line`], #751) happens only where a line becomes a
    /// bar message: [`Self::roll_window`] and [`Self::live_tail`]. A live view
    /// must be composed through one of those, never from a raw buffer line.
    pub(super) fn record(&mut self, line: &str) {
        self.output_bytes += line.len() + 1;
        self.output.push(line.to_string());
        if let Some(cap) = self.cap {
            let mut drop_to = 0;
            while self.output_bytes > cap && drop_to + 1 < self.output.len() {
                self.output_bytes -= self.output[drop_to].len() + 1;
                drop_to += 1;
            }
            if drop_to > 0 {
                self.output.drain(0..drop_to);
            }
        }
    }

    /// Note that output has arrived: the elapsed-counter answer to "is this
    /// silent job alive?" retires once real output does the answering. Returns
    /// whether the counter was currently promoted (so the caller repaints the
    /// row's resting message). Both densities now run the ticker for a silent
    /// job (#753), so both un-promote here.
    pub(super) fn mark_output_seen(&self) -> bool {
        self.output_seen.store(true, Ordering::SeqCst);
        self.promoted.swap(false, Ordering::SeqCst)
    }

    /// Mark the job's block as flushed (#753): it finished running, verdict
    /// pending. A live promoter reads this on its next tick and holds the grey
    /// `✓` done face instead of the elapsed counter, so no straggler tick can
    /// repaint a timer over a done row.
    pub(super) fn mark_done_pending(&self) {
        self.done_pending.store(true, Ordering::SeqCst);
    }

    /// The done-pending flag, for a promoter closure to consult each tick.
    pub(super) fn done_pending_flag(&self) -> Arc<AtomicBool> {
        Arc::clone(&self.done_pending)
    }

    /// Whether the job's block has flushed (finished running, verdict pending).
    pub(super) fn is_done_pending(&self) -> bool {
        self.done_pending.load(Ordering::SeqCst)
    }

    /// Grow the live window one bar per line until it caps at `tail_lines`,
    /// then roll the buffer's tail through the fixed bars (grey — a live
    /// window, not the receipt).
    pub(super) fn roll_window(
        &mut self,
        mp: &MultiProgress,
        styles: &ThreadStyles,
        tail_lines: usize,
        row_bar: &ProgressBar,
    ) {
        if self.tail_bars.len() < tail_lines {
            let anchor = self
                .tail_bars
                .last()
                .or(self.cmd_bar.as_ref())
                .unwrap_or(row_bar);
            let pb = mp.insert_after(anchor, ProgressBar::new_spinner());
            pb.set_style(styles.thread.clone());
            self.tail_bars.push(pb);
        }
        let start = self.output.len().saturating_sub(self.tail_bars.len());
        for (i, pb) in self.tail_bars.iter().enumerate() {
            // Live seam: each windowed line is sanitized (#751) so the padded
            // bar row renders exactly as wide as indicatif measures it.
            let text = self
                .output
                .get(start + i)
                .map_or(String::new(), |l| crate::output::live_line::sanitize(l));
            pb.set_message(render::paint(GREY, &text, styles.use_color));
        }
    }

    /// The most recent buffered line, sanitized for a single live annotation
    /// slot (#751). Skips lines that sanitize away to nothing — a control-only
    /// line (e.g. a bare erase) must not blank the row's liveness; the last
    /// line that still carries visible text stays shown. `None` only when no
    /// buffered line has visible content.
    pub(super) fn live_tail(&self) -> Option<String> {
        self.output
            .iter()
            .rev()
            .map(|l| crate::output::live_line::sanitize(l))
            .find(|s| !s.is_empty())
    }

    /// A ticking promoter: a job still silent past `delay` shows a dim elapsed
    /// counter in its row's annotation slot, refreshed once a second, until
    /// output or resolution. Wall-clock driven (long silent jobs never trigger
    /// output-driven swaps). `compose` builds the message from the elapsed
    /// duration — hooks pad `{name}  (elapsed)`, the region composes
    /// `active_message`. Unconditional here; the caller decides when a mode
    /// wants it (hook-verbose and exec always; hook-succinct never).
    pub(super) fn spawn_promoter(
        &self,
        row_bar: ProgressBar,
        delay: Duration,
        compose: impl Fn(Duration) -> String + Send + 'static,
    ) {
        let seen = Arc::clone(&self.output_seen);
        let done = Arc::clone(&self.resolved);
        let flag = Arc::clone(&self.promoted);
        let started = Instant::now();
        std::thread::spawn(move || {
            std::thread::sleep(delay);
            // The message to fall back to if an iteration races a just-arrived
            // first output line: the composition as of promotion time. Load-
            // bearing in verbose (restores the stable description); in succinct
            // the annotation changes per line, so a restored snapshot is at
            // worst one line stale and the next output line repaints it.
            let resting = row_bar.message();
            loop {
                if done.load(Ordering::SeqCst) || seen.load(Ordering::SeqCst) {
                    flag.store(false, Ordering::SeqCst);
                    break;
                }
                flag.store(true, Ordering::SeqCst);
                row_bar.set_message(compose(started.elapsed()));
                // Re-check after writing: if output landed in the gap, this
                // write clobbered a fresher message and nothing else will
                // repaint a silent-again bar — put the resting one back.
                if done.load(Ordering::SeqCst) || seen.load(Ordering::SeqCst) {
                    flag.store(false, Ordering::SeqCst);
                    row_bar.set_message(resting);
                    break;
                }
                std::thread::sleep(Duration::from_secs(1));
            }
        });
    }

    /// A dim, parenthesized elapsed suffix in the rail's duration vocabulary —
    /// the promoter's counter payload.
    pub(super) fn elapsed_suffix(elapsed: Duration, use_color: bool) -> String {
        render::paint(GREY, &format!("({})", format_duration(elapsed)), use_color)
    }

    /// Stop any pending promoter (its next wake observes this and exits). Set
    /// before the row's bars leave, so a straggler tick can't repaint a
    /// detached bar.
    pub(super) fn mark_resolved(&self) {
        self.resolved.store(true, Ordering::SeqCst);
    }

    /// Whether the live thread block is currently up.
    pub(super) fn is_open(&self) -> bool {
        self.trailer.is_some()
    }

    /// The bottom-most live thread bar (the anti-fusion trailer), when the
    /// thread is open. Nested children anchor below it so they render under
    /// the parent's `❯ command`/output tail, not wedged above it (`tail_bars`
    /// always insert above the trailer, so the trailer stays the floor).
    pub(super) fn bottom_bar(&self) -> Option<&ProgressBar> {
        self.trailer.as_ref()
    }

    /// Take the live thread down *and forget its bars* — the inverse of
    /// [`Self::open`], for a row that keeps running at a lower density (the
    /// `v` toggle flipping back to terse).
    ///
    /// Distinct from [`Self::remove_thread_bars`], which detaches the bars on
    /// the way to a receipt and never reopens. Here the handles must go too:
    /// a detached bar still counts toward `tail_bars.len()`, so the next
    /// [`Self::roll_window`] would think its window was already grown and
    /// write every line into bars that draw nothing.
    pub(super) fn close(&mut self, mp: &MultiProgress) {
        self.remove_thread_bars(mp);
        self.cmd_bar = None;
        self.tail_bars.clear();
        self.trailer = None;
    }

    /// Remove the live thread bars (never the row bar — the caller owns that).
    /// `mp.remove`, never `finish_and_clear` (the zombie-line lesson):
    /// receipts replace them via `mp.println`.
    pub(super) fn remove_thread_bars(&self, mp: &MultiProgress) {
        if let Some(pb) = &self.cmd_bar {
            mp.remove(pb);
        }
        for pb in &self.tail_bars {
            mp.remove(pb);
        }
        if let Some(pb) = &self.trailer {
            mp.remove(pb);
        }
    }

    /// The receipt log's lines. `❯ <command>` provenance and the `(no output)`
    /// marker sit on the metadata tier (dark grey); output recedes to the
    /// scaffolding grey under a success (a receipt) and keeps the default ink
    /// under a failure (evidence). Thread lines only — the caller prepends the
    /// receipt row face.
    pub(super) fn compose_log(&self, styles: &ThreadStyles, failed: bool) -> Vec<String> {
        let mut lines = Vec::new();
        if let Some(cmd) = &self.command_preview {
            lines.push(styles.thread_line(&render::paint(
                DARK_GREY,
                &format!("\u{276f} {}", command_line(cmd)),
                styles.use_color,
            )));
        }
        if self.output.is_empty() {
            lines.push(styles.thread_line(&render::paint(
                DARK_GREY,
                "(no output)",
                styles.use_color,
            )));
        } else {
            for line in &self.output {
                let text = if failed {
                    line.clone()
                } else {
                    render::paint(GREY, line, styles.use_color)
                };
                lines.push(styles.thread_line(&text));
            }
        }
        lines
    }

    /// The buffered log (for a succinct caller's deferred failure dump).
    pub(super) fn output(&self) -> &[String] {
        &self.output
    }

    /// Test-only: whether the elapsed counter is currently promoted.
    #[cfg(test)]
    pub(super) fn is_promoted(&self) -> bool {
        self.promoted.load(Ordering::SeqCst)
    }

    /// Test-only: whether the `❯` provenance bar is live.
    #[cfg(test)]
    pub(super) fn has_cmd_bar(&self) -> bool {
        self.cmd_bar.is_some()
    }

    /// Test-only: the `❯` provenance bar's current message.
    #[cfg(test)]
    pub(super) fn cmd_bar_message(&self) -> Option<String> {
        self.cmd_bar.as_ref().map(|b| b.message().to_string())
    }

    /// Test-only: the live window's bar count.
    #[cfg(test)]
    pub(super) fn tail_len(&self) -> usize {
        self.tail_bars.len()
    }

    /// Test-only: the live window's current messages, top to bottom.
    #[cfg(test)]
    pub(super) fn tail_messages(&self) -> Vec<String> {
        self.tail_bars
            .iter()
            .map(|b| b.message().to_string())
            .collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn command_line_flattens_multiline_to_one_line() {
        assert_eq!(
            command_line("echo one\necho two\nexit 7\n"),
            "echo one \u{2026}"
        );
        assert_eq!(command_line("cargo build"), "cargo build");
        assert_eq!(command_line("cargo build\n"), "cargo build");
    }

    #[test]
    fn byte_cap_drops_oldest_lines_keeping_the_tail() {
        // cap of 10 bytes: each "abc\n" is 4 bytes, so at most two lines fit;
        // the oldest drops as new ones arrive, and the newest line always
        // survives even alone-over-budget.
        let mut job = ThreadedJob::new(Some(10), None);
        job.record("abc"); // 4 bytes buffered
        job.record("def"); // 8
        job.record("ghi"); // 12 > 10 → drop "abc" → 8
        assert_eq!(job.output(), &["def".to_string(), "ghi".to_string()]);
        // A single line larger than the cap is never dropped to empty.
        let mut job = ThreadedJob::new(Some(4), None);
        job.record("a-very-long-single-line");
        assert_eq!(job.output(), &["a-very-long-single-line".to_string()]);
    }

    #[test]
    fn closing_forgets_its_bars_so_the_window_can_reopen() {
        // The density toggle's off→on cycle. `remove_thread_bars` alone
        // detaches the bars but keeps the handles, so a reopened window would
        // believe it had already grown to `tail_lines` and write every line
        // into bars that draw nothing.
        let mp = MultiProgress::with_draw_target(indicatif::ProgressDrawTarget::hidden());
        let row = mp.add(ProgressBar::new_spinner());
        let styles = ThreadStyles::new(false, 0);
        let mut job = ThreadedJob::new(None, None);
        job.record("one");
        job.record("two");

        job.open(&mp, &row, &styles);
        job.roll_window(&mp, &styles, 4, &row);
        assert!(job.is_open());
        assert_eq!(job.tail_len(), 1);

        job.close(&mp);
        assert!(!job.is_open(), "close must forget the trailer");
        assert_eq!(job.tail_len(), 0, "close must forget the tail bars");

        // Reopening rebuilds the window from live bars, and it repaints the
        // *buffered* tail — the toggle reveals what the row already printed,
        // not just what it prints next.
        job.open(&mp, &row, &styles);
        job.roll_window(&mp, &styles, 4, &row);
        job.roll_window(&mp, &styles, 4, &row);
        assert_eq!(job.tail_len(), 2);
        assert_eq!(
            job.tail_messages(),
            vec!["one".to_string(), "two".to_string()]
        );
    }

    #[test]
    fn opening_an_already_open_thread_is_a_no_op() {
        let mp = MultiProgress::with_draw_target(indicatif::ProgressDrawTarget::hidden());
        let row = mp.add(ProgressBar::new_spinner());
        let styles = ThreadStyles::new(false, 0);
        let mut job = ThreadedJob::new(None, Some("cargo test".to_string()));
        job.open(&mp, &row, &styles);
        let first = job.cmd_bar_message();
        job.open(&mp, &row, &styles);
        assert!(job.has_cmd_bar());
        assert_eq!(
            job.cmd_bar_message(),
            first,
            "a second open must not orphan the first provenance bar"
        );
    }

    #[test]
    fn uncapped_buffer_keeps_everything() {
        let mut job = ThreadedJob::new(None, None);
        for i in 0..100 {
            job.record(&format!("line {i}"));
        }
        assert_eq!(job.output().len(), 100);
    }

    #[test]
    fn record_keeps_raw_lines_the_live_seam_sanitizes() {
        // #751: `record` buffers the child line unmodified — the buffer is the
        // source of truth. VS16 (`✔️`), ANSI color, and `\r` rewrites all
        // survive verbatim, so the permanent receipt / failure dump keep full
        // fidelity.
        let mut job = ThreadedJob::new(None, None);
        job.record("sync hooks: \u{2714}\u{FE0F} (pre-push)");
        job.record("\x1b[32m40%\x1b[0m\r\x1b[32m100%\x1b[0m");
        assert_eq!(
            job.output(),
            &[
                "sync hooks: \u{2714}\u{FE0F} (pre-push)".to_string(),
                "\x1b[32m40%\x1b[0m\r\x1b[32m100%\x1b[0m".to_string(),
            ]
        );
    }

    #[test]
    fn live_tail_sanitizes_and_skips_lines_that_sanitize_away() {
        // The live annotation seam normalizes on the way to the bar: VS16 drops
        // to text presentation, ANSI/`\r` resolve to the shown segment.
        let mut job = ThreadedJob::new(None, None);
        job.record("\u{2714}\u{FE0F} unit tests (33.07 seconds)");
        assert_eq!(
            job.live_tail().as_deref(),
            Some("\u{2714} unit tests (33.07 seconds)")
        );
        // A trailing control-only line sanitizes to nothing; the annotation
        // keeps the last line with visible text rather than blanking (#751).
        job.record("\x1b[2K");
        assert_eq!(
            job.live_tail().as_deref(),
            Some("\u{2714} unit tests (33.07 seconds)")
        );
        // No visible content anywhere → no annotation.
        let mut blank = ThreadedJob::new(None, None);
        blank.record("\x1b[2K");
        assert_eq!(blank.live_tail(), None);
    }

    #[test]
    fn roll_window_sanitizes_and_compose_log_keeps_raw() {
        // The live window and the permanent receipt read the same raw buffer
        // but diverge at the seam: the window sanitizes (no VS16 reaches a
        // padded bar row), the receipt keeps the raw evidence under a failure.
        let mp = MultiProgress::with_draw_target(indicatif::ProgressDrawTarget::hidden());
        let row = mp.add(ProgressBar::new_spinner());
        let styles = ThreadStyles::new(false, 0);
        let mut job = ThreadedJob::new(None, None);
        job.record("\u{2714}\u{FE0F} done");
        job.open(&mp, &row, &styles);
        job.roll_window(&mp, &styles, 4, &row);
        assert!(
            job.tail_messages().iter().all(|m| !m.contains('\u{FE0F}')),
            "VS16 must not reach a live window bar: {:?}",
            job.tail_messages()
        );
        assert!(
            job.compose_log(&styles, true)
                .iter()
                .any(|l| l.contains('\u{FE0F}')),
            "the failure receipt keeps the raw glyph as evidence"
        );
    }
}
