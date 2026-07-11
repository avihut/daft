use super::columns::Column;
use super::render;
use super::state::TuiState;
use crate::core::worktree::sync_dag::DagEvent;
use crossterm::event::{self, Event, KeyCode, KeyModifiers};
use ratatui::{
    Frame, Terminal, TerminalOptions, Viewport,
    layout::{Constraint, Layout, Position},
};
use std::io::Write;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc;
use std::time::{Duration, Instant};

/// A screen the inline TUI driver can run: a self-contained model that knows
/// its own viewport geometry, how to draw itself, and how to fold producer
/// events into its state.
///
/// The driver owns everything terminal-shaped — the inline viewport,
/// tick cadence, Ctrl-C/raw-mode interplay, and final-frame cursor parking —
/// precisely because those details are subtle and must not fork per consumer.
/// Screens stay pure state + drawing. [`TuiState`] (worktree operations and
/// `daft list`) is the original implementor; the catalog table
/// (`daft repo list --sizes`) is the second.
pub trait LiveScreen {
    /// Event type produced by this screen's collector.
    type Event;

    /// Total viewport rows to reserve, including `extra_rows` requested by
    /// the caller for rows that may be discovered after the TUI starts.
    fn viewport_height(&self, extra_rows: u16) -> u16;

    /// Draw one frame. When `final_frame` is true this is the last draw
    /// before the terminal is dropped: the screen must also park the cursor
    /// past its content so the shell prompt lands on a fresh line.
    fn render(&self, frame: &mut Frame<'_>, final_frame: bool);

    /// Fold one collector event into the screen state.
    fn apply_event(&mut self, event: &Self::Event);

    /// True when the screen has reached a terminal state and the render loop
    /// should exit after one final draw.
    fn is_complete(&self) -> bool;

    /// Transition into the cancelled terminal state (Ctrl-C or external
    /// cancel signal). Must leave `is_complete()` true.
    fn mark_cancelled(&mut self);

    /// Whether the run ended via `mark_cancelled` — the driver emits a
    /// trailing newline in that case (the cursor can land inside the last
    /// table row when the terminal clamps the inline viewport).
    fn is_cancelled(&self) -> bool;

    /// Advance animations. Called at the driver's tick rate with the elapsed
    /// time since the render loop started.
    fn on_tick(&mut self, render_start_elapsed: Duration);
}

/// Drives the inline TUI render loop, consuming collector events and updating
/// the ratatui terminal until the screen reports completion.
pub struct TuiRenderer<S: LiveScreen> {
    pub(crate) state: S,
    receiver: mpsc::Receiver<S::Event>,
    /// Extra rows to reserve in the viewport for dynamically discovered branches
    /// (e.g., gone branches found after fetch).
    extra_rows: u16,
    /// Optional cooperative-cancellation flag. When set externally OR by a
    /// Ctrl-C key event observed during the render loop, the renderer flips
    /// the signal and exits cleanly after one final draw.
    pub(crate) cancel_signal: Option<Arc<AtomicBool>>,
}

impl<S: LiveScreen> TuiRenderer<S> {
    pub fn new(state: S, receiver: mpsc::Receiver<S::Event>) -> Self {
        Self {
            state,
            receiver,
            extra_rows: 0,
            cancel_signal: None,
        }
    }

    /// Reserve extra rows in the viewport for branches that may be discovered
    /// after the TUI starts (e.g., gone branches found after fetch completes).
    pub fn with_extra_rows(mut self, rows: u16) -> Self {
        self.extra_rows = rows;
        self
    }

    /// Attach a cancel signal shared with an upstream producer (typically the
    /// streaming collector). Ctrl-C in the render loop flips this flag; the
    /// producer observes it between cluster calls and exits cooperatively.
    pub fn with_cancel_signal(mut self, cancel: Arc<AtomicBool>) -> Self {
        self.cancel_signal = Some(cancel);
        self
    }

    /// Run the render loop until the screen completes.
    /// Returns the final screen state for post-render summary.
    pub fn run(mut self) -> anyhow::Result<S> {
        let render_start = Instant::now();
        let viewport_height = self.state.viewport_height(self.extra_rows);

        let backend = ratatui::backend::CrosstermBackend::new(std::io::stderr());
        let mut terminal = Terminal::with_options(
            backend,
            TerminalOptions {
                viewport: Viewport::Inline(viewport_height),
            },
        )?;

        let tick_rate = Duration::from_millis(80);
        let mut last_tick = Instant::now();

        // Helper: emit one last draw with the cursor placed past the content so
        // the shell prompt does not overwrite it. Used by all exit paths
        // (completion, channel disconnect, Ctrl-C).
        macro_rules! final_draw_and_return {
            () => {{
                terminal.draw(|frame| self.state.render(frame, true))?;
                drop(terminal);
                if self.state.is_cancelled() {
                    // After a cancelled run the cursor can land inside the
                    // last table row (terminal-height clamping of the inline
                    // viewport). Emit a newline so the shell prompt starts on
                    // a fresh line. Best-effort.
                    let _ = std::io::stderr().write_all(b"\n");
                }
                return Ok(self.state);
            }};
        }

        loop {
            // Honor an externally-set cancel signal (e.g. flipped by a sibling
            // thread or a SIGINT handler) before we draw, so we exit promptly
            // and the final draw renders the cancelled-state UI.
            if let Some(sig) = &self.cancel_signal
                && sig.load(Ordering::Relaxed)
            {
                self.state.mark_cancelled();
                final_draw_and_return!();
            }

            // Render current state.
            terminal.draw(|frame| self.state.render(frame, false))?;

            // Process all pending events.
            loop {
                match self.receiver.try_recv() {
                    Ok(event) => {
                        self.state.apply_event(&event);
                        if self.state.is_complete() {
                            final_draw_and_return!();
                        }
                    }
                    Err(mpsc::TryRecvError::Empty) => break,
                    Err(mpsc::TryRecvError::Disconnected) => {
                        final_draw_and_return!();
                    }
                }
            }

            // Poll for keyboard events (Ctrl-C). Non-blocking. If a Ctrl-C
            // is observed, flip the optional cancel signal so the producer
            // exits cooperatively, mark the screen cancelled, and emit a
            // final draw.
            if event::poll(Duration::from_millis(0)).unwrap_or(false)
                && let Ok(Event::Key(key)) = event::read()
                && key.code == KeyCode::Char('c')
                && key.modifiers.contains(KeyModifiers::CONTROL)
            {
                if let Some(sig) = &self.cancel_signal {
                    sig.store(true, Ordering::Relaxed);
                }
                self.state.mark_cancelled();
                final_draw_and_return!();
            }

            // Tick spinner animation.
            if last_tick.elapsed() >= tick_rate {
                self.state.on_tick(render_start.elapsed());
                last_tick = Instant::now();
            }

            std::thread::sleep(Duration::from_millis(16));
        }
    }
}

impl TuiState {
    /// Phase header rows: one row per phase plus a label row when phases
    /// exist; zero phases = no header at all (daft list).
    fn header_height(&self) -> u16 {
        if self.phases.is_empty() {
            0
        } else {
            self.phases.len() as u16 + 1
        }
    }

    fn footer_height(&self) -> u16 {
        if self.show_hook_sub_rows { 1 } else { 0 }
    }

    /// Size summary footer: 2 rows (blank separator + total) when the Size
    /// column is present.
    fn size_summary_rows(&self) -> u16 {
        if self
            .live
            .cfg
            .columns
            .as_ref()
            .is_some_and(|cols| cols.contains(&Column::Size))
        {
            2
        } else {
            0
        }
    }

    /// Whether `render_table` will emit a "Sorted by …" summary line above the
    /// table (true sort spec set AND its keys aren't all already shown as
    /// column-header arrows). Always 2 rows when present: summary + spacer.
    fn sort_summary_rows(&self) -> u16 {
        let Some(spec) = self.live.cfg.sort_spec.as_ref() else {
            return 0;
        };
        let displayed: Vec<crate::core::columns::ListColumn> = self
            .live
            .cfg
            .columns
            .as_ref()
            .map(|cols| cols.iter().filter_map(|c| c.to_list_column()).collect())
            .unwrap_or_else(|| crate::core::columns::ListColumn::list_defaults().to_vec());
        if spec.needs_summary_line(&displayed) {
            2
        } else {
            0
        }
    }

    /// Total rendered rows beneath the table header — data rows, divider,
    /// optional Size summary footer, and any hook/job sub-rows expanded in
    /// verbose mode. Used for cursor positioning after the final draw.
    fn total_rendered_rows(&self) -> u16 {
        let base = self.live.rows.len() as u16;
        let divider = if self.live.unowned_start_index.is_some() {
            1
        } else {
            0
        };
        let summary = self.size_summary_rows();
        if self.show_hook_sub_rows {
            base + divider
                + summary
                + self
                    .live
                    .rows
                    .iter()
                    .map(|wt| {
                        let hooks = wt.hook_sub_rows.len() as u16;
                        let jobs: u16 = wt
                            .hook_sub_rows
                            .iter()
                            .map(|h| h.job_sub_rows.len() as u16)
                            .sum();
                        hooks + jobs
                    })
                    .sum::<u16>()
        } else {
            base + divider + summary
        }
    }
}

impl LiveScreen for TuiState {
    type Event = DagEvent;

    fn viewport_height(&self, extra_rows: u16) -> u16 {
        let divider_row = if self.live.unowned_start_index.is_some() {
            1
        } else {
            0
        };
        // table_height = sort summary (rendered inside the table chunk when
        // present) + table header row + 1 trailing row for cursor parking
        // + data rows + extra room for late-arriving rows + divider + size
        // summary footer.
        let table_height = self.sort_summary_rows()
            + self.live.rows.len() as u16
            + 2
            + extra_rows
            + divider_row
            + self.size_summary_rows();
        self.header_height() + table_height + self.footer_height()
    }

    fn render(&self, frame: &mut Frame<'_>, final_frame: bool) {
        let header_height = self.header_height();
        let footer_height = self.footer_height();
        let area = frame.area();
        let chunks = Layout::vertical([
            Constraint::Length(header_height),
            Constraint::Fill(1),
            Constraint::Length(footer_height),
        ])
        .split(area);
        render::render_header(self, frame, chunks[0]);
        render::render_table(self, frame, chunks[1]);
        render::render_footer(self, frame, chunks[2]);

        if final_frame {
            // sort summary rows (when present) + table header (1 row)
            // + data rows (including hook sub-rows / size summary).
            let content_bottom = area.y
                + header_height
                + self.sort_summary_rows()
                + 1
                + self.total_rendered_rows()
                + footer_height;
            frame.set_cursor_position(Position {
                x: 0,
                y: content_bottom,
            });
        }
    }

    fn apply_event(&mut self, event: &DagEvent) {
        // Resolves to the inherent `TuiState::apply_event` (inherent methods
        // take precedence over trait methods in path resolution).
        Self::apply_event(self, event);
    }

    fn is_complete(&self) -> bool {
        Self::is_complete(self)
    }

    fn mark_cancelled(&mut self) {
        self.live.mark_cancelled();
        self.done = true;
    }

    fn is_cancelled(&self) -> bool {
        self.live.cancelled
    }

    fn on_tick(&mut self, render_start_elapsed: Duration) {
        self.render_start_elapsed = render_start_elapsed;
        self.tick();
    }
}

/// RAII guard that enables crossterm raw mode now and restores cooked mode
/// on drop. Best-effort: if `enable_raw_mode` fails (e.g. stdin isn't a
/// terminal), the guard is still returned so its `Drop` is safe to run.
/// Disabling raw mode on a terminal that wasn't in raw mode is a no-op.
///
/// Raw mode is what routes Ctrl+C into the render loop as a key event
/// (ISIG off) instead of a process-wide SIGINT, and stops the terminal
/// driver echoing `^C` mid-render. Callers keep a process-global SIGINT
/// handler installed as the fallback for when raw mode fails to enable —
/// and for the moment this guard drops, when Ctrl+C is a signal again.
pub fn enable_raw_mode_guard() -> RawModeGuard {
    let _ = crossterm::terminal::enable_raw_mode();
    RawModeGuard
}

pub struct RawModeGuard;

impl Drop for RawModeGuard {
    fn drop(&mut self) {
        let _ = crossterm::terminal::disable_raw_mode();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::core::worktree::list::Stat;
    use std::path::PathBuf;
    use std::sync::Arc;
    use std::sync::atomic::{AtomicBool, Ordering};

    #[test]
    fn cancel_signal_can_be_set_externally() {
        let signal = Arc::new(AtomicBool::new(false));
        let state = TuiState::new(
            Vec::new(),
            Vec::new(),
            PathBuf::from("/tmp"),
            PathBuf::from("/tmp"),
            Stat::Summary,
            0,
            None,
            false,
            None,
            None,
            false,
            false,
            crate::core::worktree::info_field::FieldSet::EMPTY,
        );
        let (_tx, rx) = mpsc::channel();
        let renderer = TuiRenderer::new(state, rx).with_cancel_signal(Arc::clone(&signal));

        // Renderer should be holding the signal.
        assert!(renderer.cancel_signal.is_some());

        // Flipping the external clone is observable through the renderer's
        // own clone (single source of truth).
        signal.store(true, Ordering::Relaxed);
        assert!(
            renderer
                .cancel_signal
                .as_ref()
                .unwrap()
                .load(Ordering::Relaxed)
        );
    }

    #[test]
    fn renderer_without_cancel_signal_defaults_to_none() {
        let state = TuiState::new(
            Vec::new(),
            Vec::new(),
            PathBuf::from("/tmp"),
            PathBuf::from("/tmp"),
            Stat::Summary,
            0,
            None,
            false,
            None,
            None,
            false,
            false,
            crate::core::worktree::info_field::FieldSet::EMPTY,
        );
        let (_tx, rx) = mpsc::channel();
        let renderer = TuiRenderer::new(state, rx);
        assert!(renderer.cancel_signal.is_none());
    }

    #[test]
    fn mark_cancelled_flips_live_cancelled_and_done() {
        // Direct unit test for the post-Ctrl-C state mutation the driver's
        // Ctrl-C arm performs via `LiveScreen::mark_cancelled`. We can't
        // easily synthesize a crossterm Event in a unit test, so we exercise
        // the trait method the arm calls.
        let phases = Vec::<crate::core::worktree::sync_dag::OperationPhase>::new();
        let infos = vec![crate::core::worktree::list::WorktreeInfo::empty("a")];
        let mut state = TuiState::new(
            phases,
            infos,
            std::path::PathBuf::from("/tmp"),
            std::path::PathBuf::from("/tmp"),
            crate::core::worktree::list::Stat::Summary,
            0,
            None,
            false,
            None,
            None,
            true,
            false,
            crate::core::worktree::info_field::FieldSet::EMPTY,
        );
        assert!(!state.live.cancelled);
        assert!(!state.live.collection_complete);

        LiveScreen::mark_cancelled(&mut state);

        assert!(state.live.cancelled);
        assert!(state.live.collection_complete);
        assert!(state.done);
        assert!(LiveScreen::is_complete(&state));
        assert!(LiveScreen::is_cancelled(&state));
    }

    #[test]
    fn viewport_height_matches_phaseless_geometry() {
        // daft list geometry: no phases (no header), no hooks footer, no
        // divider, no sort spec, no Size column → rows + header row + cursor
        // parking row + extra.
        let infos = vec![
            crate::core::worktree::list::WorktreeInfo::empty("a"),
            crate::core::worktree::list::WorktreeInfo::empty("b"),
        ];
        let state = TuiState::new(
            Vec::new(),
            infos,
            PathBuf::from("/tmp"),
            PathBuf::from("/tmp"),
            Stat::Summary,
            0,
            None,
            false,
            None,
            None,
            false,
            false,
            crate::core::worktree::info_field::FieldSet::EMPTY,
        );
        assert_eq!(state.viewport_height(0), 2 + 2);
        assert_eq!(state.viewport_height(3), 2 + 2 + 3);
    }
}
