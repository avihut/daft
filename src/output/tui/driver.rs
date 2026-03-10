use super::render;
use super::state::TuiState;
use crate::core::worktree::sync_dag::DagEvent;
use ratatui::{
    layout::{Constraint, Layout, Position},
    Terminal, TerminalOptions, Viewport,
};
use std::sync::mpsc;
use std::time::{Duration, Instant};

/// Drives the inline TUI render loop, consuming `DagEvent`s and updating the
/// ratatui terminal until all tasks complete.
pub struct TuiRenderer {
    state: TuiState,
    receiver: mpsc::Receiver<DagEvent>,
    /// Extra rows to reserve in the viewport for dynamically discovered branches
    /// (e.g., gone branches found after fetch).
    extra_rows: u16,
}

impl TuiRenderer {
    pub fn new(state: TuiState, receiver: mpsc::Receiver<DagEvent>) -> Self {
        Self {
            state,
            receiver,
            extra_rows: 0,
        }
    }

    /// Reserve extra rows in the viewport for branches that may be discovered
    /// after the TUI starts (e.g., gone branches found after fetch completes).
    pub fn with_extra_rows(mut self, rows: u16) -> Self {
        self.extra_rows = rows;
        self
    }

    /// Compute total rendered worktree rows including hook sub-rows.
    ///
    /// When `show_hook_sub_rows` is true (verbose >= 1), each hook sub-row adds
    /// an extra rendered row beneath its parent worktree row.
    fn total_rendered_rows(&self) -> u16 {
        let base = self.state.worktrees.len() as u16;
        if self.state.show_hook_sub_rows {
            base + self
                .state
                .worktrees
                .iter()
                .map(|wt| wt.hook_sub_rows.len() as u16)
                .sum::<u16>()
        } else {
            base
        }
    }

    /// Run the render loop until all tasks complete.
    /// Returns the final `TuiState` for post-render summary.
    pub fn run(mut self) -> anyhow::Result<TuiState> {
        let header_height = self.state.phases.len() as u16 + 1;
        let table_height = self.state.worktrees.len() as u16 + 2 + self.extra_rows;
        let viewport_height = header_height + table_height;

        let backend = ratatui::backend::CrosstermBackend::new(std::io::stderr());
        let mut terminal = Terminal::with_options(
            backend,
            TerminalOptions {
                viewport: Viewport::Inline(viewport_height),
            },
        )?;

        let tick_rate = Duration::from_millis(80);
        let mut last_tick = Instant::now();

        loop {
            // Render current state.
            terminal.draw(|frame| {
                let area = frame.area();
                let chunks =
                    Layout::vertical([Constraint::Length(header_height), Constraint::Fill(1)])
                        .split(area);

                render::render_header(&self.state, frame, chunks[0]);
                render::render_table(&self.state, frame, chunks[1]);
            })?;

            // Process all pending events.
            loop {
                match self.receiver.try_recv() {
                    Ok(event) => {
                        let is_done = matches!(event, DagEvent::AllDone);
                        self.state.apply_event(&event);
                        if is_done {
                            // Final render — position cursor past all content so
                            // the shell prompt won't overwrite the table.
                            let total_rows = self.total_rendered_rows();
                            terminal.draw(|frame| {
                                let area = frame.area();
                                let chunks = Layout::vertical([
                                    Constraint::Length(header_height),
                                    Constraint::Fill(1),
                                ])
                                .split(area);
                                render::render_header(&self.state, frame, chunks[0]);
                                render::render_table(&self.state, frame, chunks[1]);

                                // table header (1 row) + data rows (including hook sub-rows)
                                let content_bottom = area.y + header_height + 1 + total_rows;
                                frame.set_cursor_position(Position {
                                    x: 0,
                                    y: content_bottom,
                                });
                            })?;

                            drop(terminal);
                            return Ok(self.state);
                        }
                    }
                    Err(mpsc::TryRecvError::Empty) => break,
                    Err(mpsc::TryRecvError::Disconnected) => {
                        let total_rows = self.total_rendered_rows();
                        terminal.draw(|frame| {
                            let area = frame.area();
                            let chunks = Layout::vertical([
                                Constraint::Length(header_height),
                                Constraint::Fill(1),
                            ])
                            .split(area);
                            render::render_header(&self.state, frame, chunks[0]);
                            render::render_table(&self.state, frame, chunks[1]);

                            // table header (1 row) + data rows (including hook sub-rows)
                            let content_bottom = area.y + header_height + 1 + total_rows;
                            frame.set_cursor_position(Position {
                                x: 0,
                                y: content_bottom,
                            });
                        })?;
                        drop(terminal);
                        return Ok(self.state);
                    }
                }
            }

            // Tick spinner animation.
            if last_tick.elapsed() >= tick_rate {
                self.state.tick();
                last_tick = Instant::now();
            }

            std::thread::sleep(Duration::from_millis(16));
        }
    }
}
