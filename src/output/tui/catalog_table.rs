//! Live inline table for `daft repo list` — the catalog analogue of the
//! worktree live table (`live_table.rs` + `render.rs`).
//!
//! Every cheap cell (name, worktree count, path, remote) is seeded
//! synchronously before the TUI starts; only the per-repo disk-size walk
//! streams in, shimmer-loading its Size cell until each lands. The screen
//! implements [`LiveScreen`], so the shared [`super::TuiRenderer`] drives
//! the terminal mechanics (inline viewport, tick cadence, Ctrl-C/raw-mode,
//! final-frame cursor parking) — this file is pure state + drawing.

use super::columns::truncate_with_ellipsis;
use super::driver::LiveScreen;
use super::render::{loading_shimmer_cell, not_loaded_cell};
use crate::output::format::format_human_size;
use ratatui::{
    Frame,
    layout::{Constraint, Position},
    style::{Color, Modifier, Style},
    text::Span,
    widgets::{Cell, Row, Table},
};

/// Events streamed by the size collector.
#[derive(Debug)]
pub enum CatalogEvent {
    /// A repo's disk-size walk finished. `bytes` is `None` when the walk
    /// failed (stale path, permissions) — rendered as "didn't load".
    Size { index: usize, bytes: Option<u64> },
    /// All size workers have finished; the renderer may exit.
    Done,
}

/// Pre-formatted display cells for one catalog repo. The command layer owns
/// the formatting decisions (tilde-abbreviated path, `(removed)` suffix on
/// the name) so the blocking table and this live screen render identically.
#[derive(Debug, Clone)]
pub struct CatalogRepoCells {
    /// The user's cwd is inside this repo — marked with the cyan `>`.
    pub current: bool,
    /// Tombstoned entry (`repo list --all`) — the whole row renders dim.
    pub removed: bool,
    /// Display name, including any ` (removed)` suffix.
    pub name: String,
    /// Worktree count, `None` when the repo couldn't be opened.
    pub worktrees: Option<usize>,
    /// Display path (tilde-abbreviated).
    pub path: String,
    /// Remote URL, `None` for local-only repos.
    pub remote: Option<String>,
}

#[derive(Debug, Clone, Copy)]
enum SizeCell {
    Loading,
    Loaded(Option<u64>),
}

/// Column shrink floors when the table overflows the terminal width. Name
/// and Worktrees never shrink (short by construction); Path gives way first,
/// then Remote.
const PATH_MIN_WIDTH: u16 = 16;
const REMOTE_MIN_WIDTH: u16 = 12;
/// Natural width floor for the Size column while walks are in flight, so the
/// shimmer bar has presence before any value is known ("999.9M" = 6).
const SIZE_LOADING_WIDTH: u16 = 6;

const HEADERS: [&str; 5] = ["Name", "Worktrees", "Path", "Remote", "Size"];

pub struct CatalogTable {
    rows: Vec<CatalogRepoCells>,
    sizes: Vec<SizeCell>,
    complete: bool,
    cancelled: bool,
    tick: usize,
}

impl CatalogTable {
    pub fn new(rows: Vec<CatalogRepoCells>) -> Self {
        let sizes = vec![SizeCell::Loading; rows.len()];
        Self {
            rows,
            sizes,
            complete: false,
            cancelled: false,
            tick: 0,
        }
    }

    fn has_annotation(&self) -> bool {
        self.rows.iter().any(|r| r.current)
    }

    /// Sum of the size walks that have landed so far — the TOTAL footer
    /// grows live as cells fill, matching the worktree live table.
    fn total_loaded_bytes(&self) -> u64 {
        self.sizes
            .iter()
            .filter_map(|s| match s {
                SizeCell::Loaded(bytes) => *bytes,
                SizeCell::Loading => None,
            })
            .sum()
    }

    /// Assigned data-column widths `[name, worktrees, path, remote, size]`
    /// after fitting to `available` (which excludes the annotation column):
    /// natural widths, then Path and Remote shrink to their floors.
    fn fit_widths(&self, total_size: &str, available: u16) -> [u16; 5] {
        let char_w = |s: &str| s.chars().count() as u16;
        let mut widths = [
            char_w(HEADERS[0]),
            char_w(HEADERS[1]),
            char_w(HEADERS[2]),
            char_w(HEADERS[3]),
            char_w(HEADERS[4]).max(char_w(total_size)),
        ];
        for (row, size) in self.rows.iter().zip(self.sizes.iter()) {
            widths[0] = widths[0].max(char_w(&row.name));
            widths[2] = widths[2].max(char_w(&row.path));
            widths[3] = widths[3].max(char_w(row.remote.as_deref().unwrap_or("-")));
            widths[4] = widths[4].max(match size {
                SizeCell::Loading => SIZE_LOADING_WIDTH,
                SizeCell::Loaded(Some(bytes)) => char_w(&format_human_size(*bytes)),
                SizeCell::Loaded(None) => 1,
            });
        }

        let spacing = 2 * (widths.len() as u16 - 1);
        let natural: u16 = widths.iter().sum::<u16>() + spacing;
        let mut overflow = natural.saturating_sub(available);
        for (idx, min) in [(2, PATH_MIN_WIDTH), (3, REMOTE_MIN_WIDTH)] {
            if overflow == 0 {
                break;
            }
            let give = widths[idx].saturating_sub(min).min(overflow);
            widths[idx] -= give;
            overflow -= give;
        }
        widths
    }

    fn size_cell(&self, index: usize, width: u16, dim: bool) -> Cell<'static> {
        match self.sizes[index] {
            SizeCell::Loaded(Some(bytes)) => {
                let style = if dim {
                    Style::default().add_modifier(Modifier::DIM)
                } else {
                    Style::default()
                };
                Cell::from(Span::styled(format_human_size(bytes), style))
            }
            SizeCell::Loaded(None) => not_loaded_cell(width),
            SizeCell::Loading if self.cancelled => not_loaded_cell(width),
            SizeCell::Loading => loading_shimmer_cell(width, self.tick),
        }
    }
}

impl LiveScreen for CatalogTable {
    type Event = CatalogEvent;

    fn viewport_height(&self, extra_rows: u16) -> u16 {
        // Header row + data rows + TOTAL footer (separator + total) + one
        // trailing row for cursor parking.
        1 + self.rows.len() as u16 + 2 + 1 + extra_rows
    }

    fn render(&self, frame: &mut Frame<'_>, final_frame: bool) {
        let area = frame.area();
        let annotation = self.has_annotation();
        let ann_w: u16 = if annotation { 1 } else { 0 };
        let ann_spacing: u16 = if annotation { 2 } else { 0 };
        let total_size = format_human_size(self.total_loaded_bytes());
        let widths = self.fit_widths(&total_size, area.width.saturating_sub(ann_w + ann_spacing));

        let dim_underline = Style::default()
            .add_modifier(Modifier::DIM)
            .add_modifier(Modifier::UNDERLINED);
        let mut header_cells: Vec<Cell> = Vec::new();
        if annotation {
            header_cells.push(Cell::from(""));
        }
        header_cells.extend(
            HEADERS
                .iter()
                .map(|h| Cell::from(Span::styled(*h, dim_underline))),
        );

        let mut all_rows: Vec<Row> = Vec::new();
        for (idx, row) in self.rows.iter().enumerate() {
            let dim = row.removed;
            let base = if dim {
                Style::default().add_modifier(Modifier::DIM)
            } else {
                Style::default()
            };
            let mut cells: Vec<Cell> = Vec::new();
            if annotation {
                cells.push(if row.current {
                    Cell::from(Span::styled(
                        crate::styles::CURRENT_WORKTREE_SYMBOL,
                        Style::default().fg(Color::Cyan),
                    ))
                } else {
                    Cell::from("")
                });
            }
            cells.push(Cell::from(Span::styled(row.name.clone(), base)));
            cells.push(Cell::from(Span::styled(
                row.worktrees
                    .map(|n| n.to_string())
                    .unwrap_or_else(|| "-".to_string()),
                base,
            )));
            cells.push(Cell::from(Span::styled(
                truncate_with_ellipsis(&row.path, widths[2]),
                base,
            )));
            // Remote is reference info, not the primary signal — always dim.
            cells.push(Cell::from(Span::styled(
                truncate_with_ellipsis(row.remote.as_deref().unwrap_or("-"), widths[3]),
                Style::default().add_modifier(Modifier::DIM),
            )));
            cells.push(self.size_cell(idx, widths[4], dim));
            all_rows.push(Row::new(cells));
        }

        // TOTAL footer: blank separator + dim total in the Size column.
        let num_columns = HEADERS.len() + usize::from(annotation);
        all_rows.push(Row::new(
            (0..num_columns).map(|_| Cell::from("")).collect::<Vec<_>>(),
        ));
        let mut summary_cells: Vec<Cell> = (0..num_columns - 1).map(|_| Cell::from("")).collect();
        summary_cells.push(Cell::from(Span::styled(
            total_size,
            Style::default().add_modifier(Modifier::DIM),
        )));
        all_rows.push(Row::new(summary_cells));

        let mut constraints: Vec<Constraint> = Vec::new();
        if annotation {
            constraints.push(Constraint::Length(ann_w));
        }
        constraints.extend(widths.iter().map(|w| Constraint::Length(*w)));

        let table = Table::new(all_rows, &constraints)
            .header(Row::new(header_cells))
            .column_spacing(2);
        frame.render_widget(table, area);

        if final_frame {
            // Header + data rows + footer separator + total row.
            let content_bottom = area.y + 1 + self.rows.len() as u16 + 2;
            frame.set_cursor_position(Position {
                x: 0,
                y: content_bottom,
            });
        }
    }

    fn apply_event(&mut self, event: &CatalogEvent) {
        match event {
            CatalogEvent::Size { index, bytes } => {
                if let Some(cell) = self.sizes.get_mut(*index) {
                    *cell = SizeCell::Loaded(*bytes);
                }
            }
            CatalogEvent::Done => self.complete = true,
        }
    }

    fn is_complete(&self) -> bool {
        self.complete || self.sizes.iter().all(|s| matches!(s, SizeCell::Loaded(_)))
    }

    fn mark_cancelled(&mut self) {
        self.cancelled = true;
        self.complete = true;
    }

    fn is_cancelled(&self) -> bool {
        self.cancelled
    }

    fn on_tick(&mut self, _render_start_elapsed: std::time::Duration) {
        self.tick = self.tick.wrapping_add(1);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use ratatui::Terminal;
    use ratatui::backend::TestBackend;

    fn cells(name: &str, current: bool, removed: bool) -> CatalogRepoCells {
        CatalogRepoCells {
            current,
            removed,
            name: name.to_string(),
            worktrees: Some(2),
            path: format!("~/src/{name}"),
            remote: Some(format!("git@example.com:acme/{name}.git")),
        }
    }

    fn buffer_row(terminal: &Terminal<TestBackend>, y: u16, width: u16) -> String {
        let buffer = terminal.backend().buffer();
        (0..width)
            .map(|x| buffer[(x, y)].symbol().to_string())
            .collect()
    }

    #[test]
    fn viewport_height_covers_header_rows_footer_and_parking() {
        let table = CatalogTable::new(vec![cells("a", false, false), cells("b", false, false)]);
        // 1 header + 2 rows + 2 footer + 1 parking.
        assert_eq!(table.viewport_height(0), 6);
        assert_eq!(table.viewport_height(3), 9);
    }

    #[test]
    fn render_emits_headers_and_one_line_per_repo() {
        let table = CatalogTable::new(vec![
            cells("alpha", true, false),
            cells("beta", false, false),
        ]);
        let backend = TestBackend::new(100, 8);
        let mut terminal = Terminal::new(backend).unwrap();
        terminal.draw(|f| table.render(f, false)).unwrap();

        let header = buffer_row(&terminal, 0, 100);
        for label in ["Name", "Worktrees", "Path", "Remote", "Size"] {
            assert!(header.contains(label), "header missing {label}: {header:?}");
        }
        let row_alpha = buffer_row(&terminal, 1, 100);
        assert!(
            row_alpha.contains("alpha"),
            "row 1 should hold alpha: {row_alpha:?}"
        );
        assert!(
            row_alpha
                .trim_start()
                .starts_with(crate::styles::CURRENT_WORKTREE_SYMBOL),
            "current repo row should carry the marker: {row_alpha:?}"
        );
        let row_beta = buffer_row(&terminal, 2, 100);
        assert!(
            row_beta.contains("beta"),
            "row 2 should hold beta: {row_beta:?}"
        );
    }

    #[test]
    fn annotation_column_absent_when_no_repo_is_current() {
        let table = CatalogTable::new(vec![cells("alpha", false, false)]);
        let backend = TestBackend::new(100, 6);
        let mut terminal = Terminal::new(backend).unwrap();
        terminal.draw(|f| table.render(f, false)).unwrap();
        let header = buffer_row(&terminal, 0, 100);
        assert!(
            header.starts_with("Name"),
            "without a current repo the first column is Name: {header:?}"
        );
    }

    #[test]
    fn size_cells_shimmer_until_their_patch_lands() {
        let mut table = CatalogTable::new(vec![cells("alpha", false, false)]);
        let backend = TestBackend::new(100, 6);
        let mut terminal = Terminal::new(backend).unwrap();
        terminal.draw(|f| table.render(f, false)).unwrap();
        let row = buffer_row(&terminal, 1, 100);
        assert!(
            row.contains('\u{25AC}'),
            "unloaded size cell should shimmer: {row:?}"
        );

        table.apply_event(&CatalogEvent::Size {
            index: 0,
            bytes: Some(2048),
        });
        terminal.draw(|f| table.render(f, false)).unwrap();
        let row = buffer_row(&terminal, 1, 100);
        assert!(
            !row.contains('\u{25AC}'),
            "loaded cell must not shimmer: {row:?}"
        );
        assert!(row.contains("2K"), "loaded cell shows the size: {row:?}");
    }

    #[test]
    fn total_footer_sums_loaded_sizes() {
        let mut table = CatalogTable::new(vec![
            cells("alpha", false, false),
            cells("beta", false, false),
        ]);
        table.apply_event(&CatalogEvent::Size {
            index: 0,
            bytes: Some(1024 * 1024),
        });
        table.apply_event(&CatalogEvent::Size {
            index: 1,
            bytes: Some(1024 * 1024),
        });
        let backend = TestBackend::new(100, 8);
        let mut terminal = Terminal::new(backend).unwrap();
        terminal.draw(|f| table.render(f, false)).unwrap();
        // Rows: 0 header, 1-2 data, 3 separator, 4 total.
        let total_row = buffer_row(&terminal, 4, 100);
        assert!(
            total_row.contains("2.0M"),
            "footer should sum loaded sizes: {total_row:?}"
        );
    }

    #[test]
    fn completes_when_all_sizes_load_or_on_done_sentinel() {
        let mut table = CatalogTable::new(vec![cells("alpha", false, false)]);
        assert!(!table.is_complete());
        table.apply_event(&CatalogEvent::Size {
            index: 0,
            bytes: None,
        });
        assert!(
            table.is_complete(),
            "all cells loaded (even failed) completes"
        );

        let mut table = CatalogTable::new(vec![cells("alpha", false, false)]);
        table.apply_event(&CatalogEvent::Done);
        assert!(table.is_complete(), "Done sentinel completes");
    }

    #[test]
    fn cancelled_pending_cells_render_the_not_loaded_dash() {
        let mut table = CatalogTable::new(vec![cells("alpha", false, false)]);
        table.mark_cancelled();
        assert!(table.is_complete());
        assert!(table.is_cancelled());
        let backend = TestBackend::new(100, 6);
        let mut terminal = Terminal::new(backend).unwrap();
        terminal.draw(|f| table.render(f, false)).unwrap();
        let row = buffer_row(&terminal, 1, 100);
        assert!(
            !row.contains('\u{25AC}'),
            "no shimmer after cancel: {row:?}"
        );
        assert!(
            row.contains('\u{2014}'),
            "pending cell shows em-dash: {row:?}"
        );
    }

    #[test]
    fn removed_rows_render_dim() {
        let table = CatalogTable::new(vec![cells("alpha", false, true)]);
        let backend = TestBackend::new(100, 6);
        let mut terminal = Terminal::new(backend).unwrap();
        terminal.draw(|f| table.render(f, false)).unwrap();
        let buffer = terminal.backend().buffer();
        // Find the first cell of the name and assert the DIM modifier.
        let x = (0..100u16)
            .find(|x| buffer[(*x, 1)].symbol() == "a")
            .expect("name cell rendered");
        assert!(
            buffer[(x, 1)].style().add_modifier.contains(Modifier::DIM),
            "removed row cells should be dim"
        );
    }
}
