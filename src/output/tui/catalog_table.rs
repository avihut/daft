//! Live inline table for `daft repo list` — the catalog analogue of the
//! worktree live table (`live_table.rs` + `render.rs`).
//!
//! Every cheap cell (name, worktree count, branch, path, remote) is seeded
//! synchronously before the TUI starts; only the per-repo disk-size walk
//! streams in, shimmer-loading its Size cell until each lands. Which columns
//! render — and in what order — comes from the same resolved `--columns`
//! selection the blocking table uses. The screen implements [`LiveScreen`],
//! so the shared [`super::TuiRenderer`] drives the terminal mechanics
//! (inline viewport, tick cadence, Ctrl-C/raw-mode, final-frame cursor
//! parking) — this file is pure state + drawing.

use super::columns::truncate_with_ellipsis;
use super::driver::LiveScreen;
use super::render::{loading_shimmer_cell, not_loaded_cell};
use crate::core::columns::RepoListColumn;
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
    /// Recorded default branch, `None` when unknown.
    pub branch: Option<String>,
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
/// then Remote, then Branch.
const PATH_MIN_WIDTH: u16 = 16;
const REMOTE_MIN_WIDTH: u16 = 12;
const BRANCH_MIN_WIDTH: u16 = 10;
/// Natural width floor for the Size column while walks are in flight, so the
/// shimmer bar has presence before any value is known ("999.9M" = 6).
const SIZE_LOADING_WIDTH: u16 = 6;

pub struct CatalogTable {
    rows: Vec<CatalogRepoCells>,
    columns: Vec<RepoListColumn>,
    sizes: Vec<SizeCell>,
    complete: bool,
    cancelled: bool,
    tick: usize,
}

impl CatalogTable {
    pub fn new(rows: Vec<CatalogRepoCells>, columns: Vec<RepoListColumn>) -> Self {
        let sizes = vec![SizeCell::Loading; rows.len()];
        Self {
            rows,
            columns,
            sizes,
            complete: false,
            cancelled: false,
            tick: 0,
        }
    }

    /// The selected columns minus Annotation, which renders as a separate
    /// unlabeled marker column (and collapses when no row is current).
    fn data_columns(&self) -> Vec<RepoListColumn> {
        self.columns
            .iter()
            .copied()
            .filter(|c| *c != RepoListColumn::Annotation)
            .collect()
    }

    fn has_annotation(&self) -> bool {
        self.columns.contains(&RepoListColumn::Annotation) && self.rows.iter().any(|r| r.current)
    }

    fn has_size(&self) -> bool {
        self.columns.contains(&RepoListColumn::Size)
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

    /// Assigned widths for `data_columns` after fitting to `available`
    /// (which excludes the annotation column): natural widths, then Path,
    /// Remote, and Branch shrink to their floors, in that order.
    fn fit_widths(
        &self,
        data_columns: &[RepoListColumn],
        total_size: &str,
        available: u16,
    ) -> Vec<u16> {
        let char_w = |s: &str| s.chars().count() as u16;
        let mut widths: Vec<u16> = data_columns
            .iter()
            .map(|col| {
                let mut w = char_w(col.header_label());
                for (row, size) in self.rows.iter().zip(self.sizes.iter()) {
                    w = w.max(match col {
                        RepoListColumn::Annotation => 0,
                        RepoListColumn::Name => char_w(&row.name),
                        RepoListColumn::Worktrees => char_w(
                            &row.worktrees
                                .map(|n| n.to_string())
                                .unwrap_or_else(|| "-".to_string()),
                        ),
                        RepoListColumn::Branch => char_w(row.branch.as_deref().unwrap_or("-")),
                        RepoListColumn::Path => char_w(&row.path),
                        RepoListColumn::Remote => char_w(row.remote.as_deref().unwrap_or("-")),
                        RepoListColumn::Size => match size {
                            SizeCell::Loading => SIZE_LOADING_WIDTH,
                            SizeCell::Loaded(Some(bytes)) => char_w(&format_human_size(*bytes)),
                            SizeCell::Loaded(None) => 1,
                        },
                    });
                }
                if *col == RepoListColumn::Size {
                    // Sized to hold the TOTAL footer cell, not just the data.
                    w = w.max(char_w(total_size));
                }
                w
            })
            .collect();

        let spacing = 2 * (widths.len().saturating_sub(1)) as u16;
        let natural: u16 = widths.iter().sum::<u16>() + spacing;
        let mut overflow = natural.saturating_sub(available);
        for (col, min) in [
            (RepoListColumn::Path, PATH_MIN_WIDTH),
            (RepoListColumn::Remote, REMOTE_MIN_WIDTH),
            (RepoListColumn::Branch, BRANCH_MIN_WIDTH),
        ] {
            if overflow == 0 {
                break;
            }
            if let Some(idx) = data_columns.iter().position(|c| *c == col) {
                let give = widths[idx].saturating_sub(min).min(overflow);
                widths[idx] -= give;
                overflow -= give;
            }
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
        // Header row + data rows + TOTAL footer (separator + total, present
        // only when a size column streams) + one row for cursor parking.
        let footer: u16 = if self.has_size() { 2 } else { 0 };
        1 + self.rows.len() as u16 + footer + 1 + extra_rows
    }

    fn render(&self, frame: &mut Frame<'_>, final_frame: bool) {
        let area = frame.area();
        let annotation = self.has_annotation();
        let data_columns = self.data_columns();
        let ann_w: u16 = if annotation { 1 } else { 0 };
        let ann_spacing: u16 = if annotation { 2 } else { 0 };
        let total_size = format_human_size(self.total_loaded_bytes());
        let widths = self.fit_widths(
            &data_columns,
            &total_size,
            area.width.saturating_sub(ann_w + ann_spacing),
        );

        let dim_underline = Style::default()
            .add_modifier(Modifier::DIM)
            .add_modifier(Modifier::UNDERLINED);
        let mut header_cells: Vec<Cell> = Vec::new();
        if annotation {
            header_cells.push(Cell::from(""));
        }
        header_cells.extend(
            data_columns
                .iter()
                .map(|c| Cell::from(Span::styled(c.header_label(), dim_underline))),
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
            for (col, width) in data_columns.iter().zip(widths.iter()) {
                cells.push(match col {
                    RepoListColumn::Annotation => unreachable!("filtered by data_columns"),
                    RepoListColumn::Name => Cell::from(Span::styled(row.name.clone(), base)),
                    RepoListColumn::Worktrees => Cell::from(Span::styled(
                        row.worktrees
                            .map(|n| n.to_string())
                            .unwrap_or_else(|| "-".to_string()),
                        base,
                    )),
                    RepoListColumn::Branch => Cell::from(Span::styled(
                        truncate_with_ellipsis(row.branch.as_deref().unwrap_or("-"), *width),
                        base,
                    )),
                    RepoListColumn::Path => Cell::from(Span::styled(
                        truncate_with_ellipsis(&row.path, *width),
                        base,
                    )),
                    // Remote is reference info, not the primary signal — always dim.
                    RepoListColumn::Remote => Cell::from(Span::styled(
                        truncate_with_ellipsis(row.remote.as_deref().unwrap_or("-"), *width),
                        Style::default().add_modifier(Modifier::DIM),
                    )),
                    RepoListColumn::Size => self.size_cell(idx, *width, dim),
                });
            }
            all_rows.push(Row::new(cells));
        }

        // TOTAL footer: blank separator + dim total under the Size column.
        let footer_rows: u16 = if let Some(size_pos) =
            data_columns.iter().position(|c| *c == RepoListColumn::Size)
        {
            let num_columns = data_columns.len() + usize::from(annotation);
            all_rows.push(Row::new(
                (0..num_columns).map(|_| Cell::from("")).collect::<Vec<_>>(),
            ));
            let mut summary_cells: Vec<Cell> = (0..num_columns).map(|_| Cell::from("")).collect();
            summary_cells[size_pos + usize::from(annotation)] = Cell::from(Span::styled(
                total_size,
                Style::default().add_modifier(Modifier::DIM),
            ));
            all_rows.push(Row::new(summary_cells));
            2
        } else {
            0
        };

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
            // Header + data rows + any footer rows.
            let content_bottom = area.y + 1 + self.rows.len() as u16 + footer_rows;
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
        self.complete
            || !self.has_size()
            || self.sizes.iter().all(|s| matches!(s, SizeCell::Loaded(_)))
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
    use crate::core::columns::RepoColumnSelection;
    use ratatui::Terminal;
    use ratatui::backend::TestBackend;

    fn cells(name: &str, current: bool, removed: bool) -> CatalogRepoCells {
        CatalogRepoCells {
            current,
            removed,
            name: name.to_string(),
            worktrees: Some(2),
            branch: Some("main".to_string()),
            path: format!("~/src/{name}"),
            remote: Some(format!("git@example.com:acme/{name}.git")),
        }
    }

    /// The live table only engages when the size column is selected, so the
    /// canonical test column set is defaults + size.
    fn columns() -> Vec<RepoListColumn> {
        RepoColumnSelection::parse("+size").unwrap().columns
    }

    fn buffer_row(terminal: &Terminal<TestBackend>, y: u16, width: u16) -> String {
        let buffer = terminal.backend().buffer();
        (0..width)
            .map(|x| buffer[(x, y)].symbol().to_string())
            .collect()
    }

    #[test]
    fn viewport_height_covers_header_rows_footer_and_parking() {
        let table = CatalogTable::new(
            vec![cells("a", false, false), cells("b", false, false)],
            columns(),
        );
        // 1 header + 2 rows + 2 footer + 1 parking.
        assert_eq!(table.viewport_height(0), 6);
        assert_eq!(table.viewport_height(3), 9);
    }

    #[test]
    fn render_emits_headers_and_one_line_per_repo() {
        let table = CatalogTable::new(
            vec![cells("alpha", true, false), cells("beta", false, false)],
            columns(),
        );
        let backend = TestBackend::new(100, 8);
        let mut terminal = Terminal::new(backend).unwrap();
        terminal.draw(|f| table.render(f, false)).unwrap();

        let header = buffer_row(&terminal, 0, 100);
        for label in ["Name", "Worktrees", "Path", "Size", "Remote"] {
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
    fn columns_follow_the_selection() {
        let selection = RepoColumnSelection::parse("name,size").unwrap().columns;
        let table = CatalogTable::new(vec![cells("alpha", false, false)], selection);
        let backend = TestBackend::new(100, 6);
        let mut terminal = Terminal::new(backend).unwrap();
        terminal.draw(|f| table.render(f, false)).unwrap();
        let header = buffer_row(&terminal, 0, 100);
        assert!(header.contains("Name") && header.contains("Size"));
        assert!(
            !header.contains("Remote") && !header.contains("Path"),
            "unselected columns must not render: {header:?}"
        );
    }

    #[test]
    fn annotation_column_absent_when_no_repo_is_current() {
        let table = CatalogTable::new(vec![cells("alpha", false, false)], columns());
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
        let mut table = CatalogTable::new(vec![cells("alpha", false, false)], columns());
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
        let mut table = CatalogTable::new(
            vec![cells("alpha", false, false), cells("beta", false, false)],
            columns(),
        );
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
        let mut table = CatalogTable::new(vec![cells("alpha", false, false)], columns());
        assert!(!table.is_complete());
        table.apply_event(&CatalogEvent::Size {
            index: 0,
            bytes: None,
        });
        assert!(
            table.is_complete(),
            "all cells loaded (even failed) completes"
        );

        let mut table = CatalogTable::new(vec![cells("alpha", false, false)], columns());
        table.apply_event(&CatalogEvent::Done);
        assert!(table.is_complete(), "Done sentinel completes");
    }

    #[test]
    fn cancelled_pending_cells_render_the_not_loaded_dash() {
        let mut table = CatalogTable::new(vec![cells("alpha", false, false)], columns());
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
        let table = CatalogTable::new(vec![cells("alpha", false, true)], columns());
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
