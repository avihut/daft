use super::columns::Column;
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

    /// Compute total rendered worktree rows including hook sub-rows and divider.
    ///
    /// When `show_hook_sub_rows` is true (verbose >= 1), each hook sub-row and
    /// its nested job sub-rows add extra rendered rows beneath the parent worktree row.
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
        let table_height = self.state.live.rows.len() as u16 + 2 + self.extra_rows + divider_row;
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

        loop {
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
                            // Final render — position cursor past all content so
                            // the shell prompt won't overwrite the table.
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

                                // table header (1 row) + data rows (including hook sub-rows)
                                let content_bottom =
                                    area.y + header_height + 1 + total_rows + footer_height;
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
                                Constraint::Length(footer_height),
                            ])
                            .split(area);
                            render::render_header(&self.state, frame, chunks[0]);
                            render::render_table(&self.state, frame, chunks[1]);
                            render::render_footer(&self.state, frame, chunks[2]);

                            // table header (1 row) + data rows (including hook sub-rows)
                            let content_bottom =
                                area.y + header_height + 1 + total_rows + footer_height;
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
                self.state.render_start_elapsed = render_start.elapsed();
                self.state.tick();
                last_tick = Instant::now();
            }

            std::thread::sleep(Duration::from_millis(16));
        }
    }
}
