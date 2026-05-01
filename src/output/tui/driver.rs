use super::columns::Column;
use super::render;
use super::state::TuiState;
use crate::core::worktree::sync_dag::DagEvent;
use crossterm::event::{self, Event, KeyCode, KeyModifiers};
use ratatui::{
    layout::{Constraint, Layout, Position},
    Terminal, TerminalOptions, Viewport,
};
use std::io::Write;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc;
use std::sync::Arc;
use std::time::{Duration, Instant};

/// Drives the inline TUI render loop, consuming `DagEvent`s and updating the
/// ratatui terminal until all tasks complete.
pub struct TuiRenderer {
    pub(crate) state: TuiState,
    receiver: mpsc::Receiver<DagEvent>,
    /// Extra rows to reserve in the viewport for dynamically discovered branches
    /// (e.g., gone branches found after fetch).
    extra_rows: u16,
    /// Optional cooperative-cancellation flag. When set externally OR by a
    /// Ctrl-C key event observed during the render loop, the renderer flips
    /// the signal and exits cleanly after one final draw.
    pub(crate) cancel_signal: Option<Arc<AtomicBool>>,
}

impl TuiRenderer {
    pub fn new(state: TuiState, receiver: mpsc::Receiver<DagEvent>) -> Self {
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

    /// Whether `render_table` will emit a "Sorted by …" summary line above the
    /// table (true sort spec set AND its keys aren't all already shown as
    /// column-header arrows). Always 2 rows when present: summary + spacer.
    fn sort_summary_rows(&self) -> u16 {
        let Some(spec) = self.state.live.cfg.sort_spec.as_ref() else {
            return 0;
        };
        let displayed: Vec<crate::core::columns::ListColumn> = self
            .state
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
        let base = self.state.live.rows.len() as u16;
        let divider = if self.state.live.unowned_start_index.is_some() {
            1
        } else {
            0
        };
        // Summary footer: 2 rows (separator + total) when Size column is present
        let summary = if self
            .state
            .live
            .cfg
            .columns
            .as_ref()
            .is_some_and(|cols| cols.contains(&Column::Size))
        {
            2
        } else {
            0
        };
        if self.state.show_hook_sub_rows {
            base + divider
                + summary
                + self
                    .state
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

    /// Run the render loop until all tasks complete.
    /// Returns the final `TuiState` for post-render summary.
    pub fn run(mut self) -> anyhow::Result<TuiState> {
        let render_start = Instant::now();
        // `+1` is the phase header label row when phases exist; zero phases =
        // no header at all (daft list).
        let header_height = if self.state.phases.is_empty() {
            0
        } else {
            self.state.phases.len() as u16 + 1
        };
        let divider_row = if self.state.live.unowned_start_index.is_some() {
            1
        } else {
            0
        };
        // Size summary footer: 2 rows (blank separator + total) when present.
        let size_summary_rows: u16 = if self
            .state
            .live
            .cfg
            .columns
            .as_ref()
            .is_some_and(|cols| cols.contains(&Column::Size))
        {
            2
        } else {
            0
        };
        // table_height = sort summary (rendered inside chunks[1] when present)
        // + table header row + 1 trailing row for cursor parking + data rows
        // + extra room for late-arriving rows + divider + size summary footer.
        let sort_rows = self.sort_summary_rows();
        let table_height = sort_rows
            + self.state.live.rows.len() as u16
            + 2
            + self.extra_rows
            + divider_row
            + size_summary_rows;
        let footer_height: u16 = if self.state.show_hook_sub_rows { 1 } else { 0 };
        let viewport_height = header_height + table_height + footer_height;

        let backend = ratatui::backend::CrosstermBackend::new(std::io::stderr());
        let mut terminal = Terminal::with_options(
            backend,
            TerminalOptions {
                viewport: Viewport::Inline(viewport_height),
            },
        )?;

        let tick_rate = Duration::from_millis(80);
        let mut last_tick = Instant::now();

        // Helper: emit one last draw with the cursor placed past the table so
        // the shell prompt does not overwrite content. Used by all exit paths
        // (completion, channel disconnect, Ctrl-C).
        macro_rules! final_draw_and_return {
            () => {{
                let total_rows = self.total_rendered_rows();
                terminal.draw(|frame| {
                    let area = frame.area();
                    let chunks = Layout::vertical([
                        Constraint::Length(header_height),
                        Constraint::Fill(1),
                        Constraint::Length(footer_height),
                    ])
                    .split(area);
                    render::render_header(&self.state, frame, chunks[0]);
                    render::render_table(&self.state, frame, chunks[1]);
                    render::render_footer(&self.state, frame, chunks[2]);

                    // sort summary rows (when present) + table header (1 row)
                    // + data rows (including hook sub-rows / size summary).
                    let content_bottom =
                        area.y + header_height + sort_rows + 1 + total_rows + footer_height;
                    frame.set_cursor_position(Position {
                        x: 0,
                        y: content_bottom,
                    });
                })?;
                drop(terminal);
                if self.state.live.cancelled {
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
            if let Some(sig) = &self.cancel_signal {
                if sig.load(Ordering::Relaxed) {
                    self.state.live.mark_cancelled();
                    self.state.done = true;
                    final_draw_and_return!();
                }
            }

            // Render current state.
            terminal.draw(|frame| {
                let area = frame.area();
                let chunks = Layout::vertical([
                    Constraint::Length(header_height),
                    Constraint::Fill(1),
                    Constraint::Length(footer_height),
                ])
                .split(area);

                render::render_header(&self.state, frame, chunks[0]);
                render::render_table(&self.state, frame, chunks[1]);
                render::render_footer(&self.state, frame, chunks[2]);
            })?;

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
            // exits cooperatively, mark state done and live as cancelled,
            // and emit a final draw.
            if event::poll(Duration::from_millis(0)).unwrap_or(false) {
                if let Ok(Event::Key(key)) = event::read() {
                    if key.code == KeyCode::Char('c')
                        && key.modifiers.contains(KeyModifiers::CONTROL)
                    {
                        if let Some(sig) = &self.cancel_signal {
                            sig.store(true, Ordering::Relaxed);
                        }
                        self.state.live.mark_cancelled();
                        self.state.done = true;
                        final_draw_and_return!();
                    }
                }
            }

            // Tick spinner animation.
            if last_tick.elapsed() >= tick_rate {
                self.state.render_start_elapsed = render_start.elapsed();
                self.state.tick();
                last_tick = Instant::now();
            }

            std::thread::sleep(Duration::from_millis(16));
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::core::worktree::list::Stat;
    use std::path::PathBuf;
    use std::sync::atomic::{AtomicBool, Ordering};
    use std::sync::Arc;

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
        assert!(renderer
            .cancel_signal
            .as_ref()
            .unwrap()
            .load(Ordering::Relaxed));
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
    fn mark_cancelled_via_state_flips_live_cancelled() {
        // Direct unit test for the post-Ctrl-C state mutation that the
        // driver's Ctrl-C arm performs. We can't easily synthesize a
        // crossterm Event in a unit test, so we exercise the same
        // mutation path the arm performs.
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

        state.live.mark_cancelled();
        state.done = true;

        assert!(state.live.cancelled);
        assert!(state.live.collection_complete);
        assert!(state.done);
    }
}
