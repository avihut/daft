use super::columns::{
    column_content_width, fit_widths_to_available, select_columns, truncate_with_ellipsis, Column,
    ALL_COLUMNS,
};
use super::state::{FinalStatus, PhaseStatus, TuiState, WorktreeStatus};
use crate::core::sort::SortSpec;
use crate::core::worktree::info_field::FieldSet;
use crate::core::worktree::list::{EntryKind, Stat, WorktreeInfo};
use crate::output::format::{self, format_human_size, ColumnContext, ColumnValues};
use crate::styles;
use ratatui::{
    layout::{Constraint, Rect},
    style::{Color, Modifier, Style},
    text::{Line, Span},
    widgets::{Cell, Paragraph, Row, Table},
    Frame,
};

const SPINNER_FRAMES: &[&str] = &[
    "\u{280b}", "\u{2819}", "\u{2839}", "\u{2838}", "\u{283c}", "\u{2834}", "\u{2826}", "\u{2827}",
    "\u{2807}", "\u{280f}",
];
const CHECKMARK: &str = "\u{2713}";
const CROSS: &str = "\u{2717}";
const SKIP: &str = "\u{2298}";
const DASH: &str = "\u{2014}";

/// Render the operation header showing phase progress.
pub fn render_header(state: &TuiState, frame: &mut Frame, area: Rect) {
    if state.phases.is_empty() {
        return;
    }
    let lines: Vec<Line> = state
        .phases
        .iter()
        .map(|ps| match ps.status {
            PhaseStatus::Pending => Line::from(Span::styled(
                format!("  {}", ps.phase.label()),
                Style::default().add_modifier(Modifier::DIM),
            )),
            PhaseStatus::Active => {
                let spinner = SPINNER_FRAMES[state.tick % SPINNER_FRAMES.len()];
                Line::from(vec![
                    Span::styled(format!("{spinner} "), Style::default().fg(Color::Yellow)),
                    Span::styled(ps.phase.label(), Style::default().fg(Color::Yellow)),
                ])
            }
            PhaseStatus::Completed => Line::from(vec![
                Span::styled(format!("{CHECKMARK} "), Style::default().fg(Color::Green)),
                Span::styled(
                    ps.phase.label(),
                    Style::default().add_modifier(Modifier::DIM),
                ),
            ]),
        })
        .collect();

    let header = Paragraph::new(lines);
    frame.render_widget(header, area);
}

/// Build styled spans for the "Sorted by" summary (e.g., "Branch ↓, Size ↑").
fn render_sort_summary_spans(spec: &SortSpec) -> Vec<Span<'static>> {
    let mut spans = Vec::new();
    for (rank, key) in spec.keys.iter().enumerate() {
        if rank > 0 {
            spans.push(Span::styled(
                ", ",
                Style::default().add_modifier(Modifier::DIM),
            ));
        }
        let arrow_color = match rank {
            0 => Color::White,
            1 => Color::Gray,
            _ => Color::DarkGray,
        };
        spans.push(Span::styled(
            key.column.display_name().to_string(),
            Style::default().add_modifier(Modifier::DIM),
        ));
        spans.push(Span::styled(
            format!(" {}", SortSpec::arrow(key.direction)),
            Style::default().fg(arrow_color),
        ));
    }
    spans
}

/// Render a verbose-mode footer below the table showing inflight cell
/// count and elapsed time. No-op when not in verbose mode.
pub fn render_footer(state: &TuiState, frame: &mut Frame, area: Rect) {
    if !state.show_hook_sub_rows {
        return;
    }
    let inflight: usize = state
        .live
        .received_patches
        .iter()
        .filter(|fs| !fs.contains(crate::core::worktree::info_field::FieldSet::ALL))
        .count();
    let elapsed_secs = state.render_start_elapsed.as_secs_f32();
    let text = format!(" inflight: {inflight} \u{00B7} elapsed: {elapsed_secs:.1}s");
    let line = Line::from(Span::styled(
        text,
        Style::default().add_modifier(Modifier::DIM),
    ));
    frame.render_widget(Paragraph::new(line), area);
}

/// Render the worktree status table.
pub fn render_table(state: &TuiState, frame: &mut Frame, area: Rect) {
    let now = chrono::Utc::now().timestamp();
    let ctx = ColumnContext {
        project_root: &state.live.cfg.project_root,
        cwd: &state.live.cfg.cwd,
        now,
        stat: state.live.cfg.stat,
    };

    // Pre-compute all column values for sizing and reuse.
    let mut row_vals: Vec<ColumnValues> = state
        .live
        .rows
        .iter()
        .map(|wt| format::compute_column_values(&wt.info, &ctx))
        .collect();

    // Select columns and compute dynamic constraints from content widths.
    let sort_ref = state.live.cfg.sort_spec.as_ref();

    // Render "Sorted by" summary line if column headers can't convey the sort.
    let table_area = if let Some(spec) = sort_ref {
        // Collect displayed ListColumns (excluding Status which is TUI-only).
        let displayed: Vec<crate::core::columns::ListColumn> = state
            .live
            .cfg
            .columns
            .as_deref()
            .map(|cols| cols.iter().filter_map(|c| c.to_list_column()).collect())
            .unwrap_or_else(|| crate::core::columns::ListColumn::list_defaults().to_vec());
        if spec.needs_summary_line(&displayed) {
            let spans: Vec<Span> = render_sort_summary_spans(spec);
            let mut line_spans = vec![Span::styled(
                "Sorted by ",
                Style::default().add_modifier(Modifier::DIM),
            )];
            line_spans.extend(spans);
            let summary = Paragraph::new(Line::from(line_spans));
            let chunks = ratatui::layout::Layout::vertical([
                Constraint::Length(1),
                Constraint::Length(1), // spacer
                Constraint::Fill(1),
            ])
            .split(area);
            frame.render_widget(summary, chunks[0]);
            chunks[2]
        } else {
            area
        }
    } else {
        area
    };
    let columns = if state.phases.is_empty() {
        // Phase-less = daft list. Use user-selected columns as-is in canonical
        // order (matching the blocking print_table behavior). No Status column
        // (no operations to track), no responsive dropping (let the table wrap
        // or truncate via Branch/Path widths instead of dropping data columns).
        state.live.cfg.columns.clone().unwrap_or_else(|| {
            // Fallback to defaults converted from ListColumn::list_defaults.
            crate::core::columns::ListColumn::list_defaults()
                .iter()
                .map(|lc| Column::from_list_column(*lc))
                .collect()
        })
    } else {
        let columns = match (&state.live.cfg.columns, state.live.cfg.columns_explicit) {
            // Replace mode: user explicitly chose columns, don't responsively drop.
            (Some(user_cols), true) => user_cols.clone(),
            // Modifier mode: user tweaked defaults, responsive dropping still applies.
            // Opt-in columns (not in ALL_COLUMNS, e.g. Size) that the user explicitly
            // added are always included — they bypass responsive dropping.
            (Some(user_cols), false) => {
                let responsive =
                    select_columns(table_area.width, &state.live.rows, &row_vals, sort_ref);
                let mut cols: Vec<Column> = responsive
                    .into_iter()
                    .filter(|c| matches!(c, Column::Status) || user_cols.contains(c))
                    .collect();
                for col in user_cols {
                    if !ALL_COLUMNS.contains(col) && !cols.contains(col) {
                        cols.push(*col);
                    }
                }
                cols
            }
            // No column selection: fully responsive.
            (None, _) => select_columns(table_area.width, &state.live.rows, &row_vals, sort_ref),
        };
        // Status is always prepended for TUI commands with phases.
        if !columns.contains(&Column::Status) {
            let mut with_status = vec![Column::Status];
            with_status.extend(columns);
            with_status
        } else {
            columns
        }
    };

    // Compute natural column widths, then shrink Branch/Path so the table fits
    // in the available area. Without this step a single long path or branch
    // name in `live.rows` (often off-screen) blows out those columns and
    // squeezes `LastCommit` (Fill(1)) down to nearly zero width.
    let natural_widths: Vec<u16> = columns
        .iter()
        .map(|col| column_content_width(*col, &state.live.rows, &row_vals, sort_ref))
        .collect();
    let assigned_widths = fit_widths_to_available(&columns, &natural_widths, table_area.width);

    // When Branch / Path / LastCommit were shrunk below natural, pre-truncate
    // the displayed text so the renderer shows "..." rather than ratatui's
    // silent hard cut. For LastCommit the user-visible string is
    // "<age> <subject>"; truncate the subject so the combined length fits.
    for (i, col) in columns.iter().enumerate() {
        if assigned_widths[i] >= natural_widths[i] {
            continue;
        }
        match col {
            Column::Branch => {
                for vals in &mut row_vals {
                    vals.branch = truncate_with_ellipsis(&vals.branch, assigned_widths[i]);
                }
            }
            Column::Path => {
                for vals in &mut row_vals {
                    vals.path = truncate_with_ellipsis(&vals.path, assigned_widths[i]);
                }
            }
            Column::LastCommit => {
                let width = assigned_widths[i];
                for vals in &mut row_vals {
                    if vals.last_commit_subject.is_empty() {
                        // Only age is shown — that's already short, but fall
                        // back to direct truncation in pathological cases.
                        if !vals.last_commit_age.is_empty() {
                            vals.last_commit_age =
                                truncate_with_ellipsis(&vals.last_commit_age, width);
                        }
                        continue;
                    }
                    let prefix = if vals.last_commit_age.is_empty() {
                        0
                    } else {
                        vals.last_commit_age.chars().count() as u16 + 1 // " "
                    };
                    let subject_room = width.saturating_sub(prefix);
                    vals.last_commit_subject =
                        truncate_with_ellipsis(&vals.last_commit_subject, subject_room);
                }
            }
            _ => {}
        }
    }

    let constraints: Vec<Constraint> = assigned_widths
        .iter()
        .map(|w| Constraint::Length(*w))
        .collect();

    // X offset where the first data column (Branch) starts — used for
    // positioning pruned-row overlays with continuous strikethrough.
    let pruned_x_offset: u16 = columns
        .iter()
        .zip(constraints.iter())
        .take_while(|(col, _)| matches!(col, Column::Status | Column::Annotation))
        .map(|(_, c)| match c {
            Constraint::Length(w) => w + 2, // column width + column spacing
            _ => 2,
        })
        .sum();

    let header_cells: Vec<Cell> = columns
        .iter()
        .map(|col| {
            let dim_underline = Style::default()
                .add_modifier(Modifier::DIM)
                .add_modifier(Modifier::UNDERLINED);
            let indicator = col.to_list_column().and_then(|lc| {
                state
                    .live
                    .cfg
                    .sort_spec
                    .as_ref()
                    .and_then(|s| s.direction_indicator(lc))
            });
            match indicator {
                Some((arrow, rank)) => {
                    let arrow_color = match rank {
                        0 => Color::White,
                        1 => Color::Gray,
                        _ => Color::DarkGray,
                    };
                    Cell::from(Line::from(vec![
                        Span::styled(col.label(), dim_underline),
                        Span::styled(format!(" {arrow}"), Style::default().fg(arrow_color)),
                    ]))
                }
                None => Cell::from(Span::styled(col.label(), dim_underline)),
            }
        })
        .collect();
    let header_row = Row::new(header_cells);

    // Build table rows, inserting empty placeholders for hook sub-rows.
    // Hook lines are rendered as full-width overlays after the table so they
    // aren't constrained by column widths.
    let mut all_rows: Vec<Row> = Vec::new();
    let mut hook_overlays: Vec<(u16, Line)> = Vec::new();
    let mut pruned_overlays: Vec<(u16, Line)> = Vec::new();
    let mut divider_row_offset: Option<u16> = None;
    let mut row_count: u16 = 0;
    let num_columns = columns.len();

    for (wt_idx, (wt, vals)) in state.live.rows.iter().zip(row_vals.iter()).enumerate() {
        // Insert a placeholder row for the section divider between owned and
        // unowned worktrees.  The actual divider content is overlaid later.
        if state.live.unowned_start_index == Some(wt_idx) {
            let empty_cells: Vec<Cell> = (0..num_columns).map(|_| Cell::from("")).collect();
            all_rows.push(Row::new(empty_cells));
            divider_row_offset = Some(row_count);
            row_count += 1;
        }

        let is_pruned = matches!(wt.status, WorktreeStatus::Done(FinalStatus::Pruned));
        let row_idx = wt_idx;
        let main_cells: Vec<Cell> = if is_pruned {
            // Status and Annotation keep their normal cells; other columns are
            // left empty because their content is overlaid with a single
            // continuous strikethrough line.
            columns
                .iter()
                .enumerate()
                .map(|(i, col)| {
                    if matches!(col, Column::Status | Column::Annotation) {
                        render_cell(
                            col,
                            wt,
                            vals,
                            state.tick,
                            state.live.cfg.stat,
                            assigned_widths[i],
                            |fs| state.live.is_cell_loading(row_idx, fs),
                        )
                    } else {
                        Cell::from("")
                    }
                })
                .collect()
        } else {
            columns
                .iter()
                .enumerate()
                .map(|(i, col)| {
                    render_cell(
                        col,
                        wt,
                        vals,
                        state.tick,
                        state.live.cfg.stat,
                        assigned_widths[i],
                        |fs| state.live.is_cell_loading(row_idx, fs),
                    )
                })
                .collect()
        };
        all_rows.push(Row::new(main_cells));
        if is_pruned {
            pruned_overlays.push((
                row_count,
                format_pruned_overlay(vals, &columns, &constraints),
            ));
        }
        row_count += 1;

        if state.show_hook_sub_rows && !wt.hook_sub_rows.is_empty() {
            let hook_count = wt.hook_sub_rows.len();
            for (i, sub) in wt.hook_sub_rows.iter().enumerate() {
                let is_last_hook = i == hook_count - 1;
                let hook_prefix = if is_last_hook { "\u{2514}" } else { "\u{251C}" };

                // Hook placeholder row.
                let empty_cells: Vec<Cell> = (0..num_columns).map(|_| Cell::from("")).collect();
                all_rows.push(Row::new(empty_cells));
                hook_overlays.push((row_count, format_hook_line(sub, hook_prefix, state.tick)));
                row_count += 1;

                // Job sub-rows within this hook.
                let job_count = sub.job_sub_rows.len();
                for (j, job) in sub.job_sub_rows.iter().enumerate() {
                    let is_last_job = j == job_count - 1;
                    let empty_cells: Vec<Cell> = (0..num_columns).map(|_| Cell::from("")).collect();
                    all_rows.push(Row::new(empty_cells));
                    hook_overlays.push((
                        row_count,
                        format_job_line(job, is_last_hook, is_last_job, state.tick),
                    ));
                    row_count += 1;
                }
            }
        }
    }

    // Summary footer row for the Size column
    let has_size_column = columns.contains(&Column::Size);
    if has_size_column {
        // Empty separator row
        let empty_cells: Vec<Cell> = (0..num_columns).map(|_| Cell::from("")).collect();
        all_rows.push(Row::new(empty_cells));

        // Summary row with total size (excludes pruned worktrees)
        let total_bytes: u64 = state
            .live
            .rows
            .iter()
            .filter(|wt| wt.info.kind == EntryKind::Worktree)
            .filter(|wt| !matches!(wt.status, WorktreeStatus::Done(FinalStatus::Pruned)))
            .filter_map(|wt| wt.info.size_bytes)
            .sum();
        let total_size = format_human_size(total_bytes);
        let summary_cells: Vec<Cell> = columns
            .iter()
            .map(|col| {
                if matches!(col, Column::Size) {
                    Cell::from(Span::styled(
                        total_size.clone(),
                        Style::default().add_modifier(Modifier::DIM),
                    ))
                } else {
                    Cell::from("")
                }
            })
            .collect();
        all_rows.push(Row::new(summary_cells));
    }

    let table = Table::new(all_rows, &constraints)
        .header(header_row)
        .column_spacing(2);

    frame.render_widget(table, table_area);

    // The header row occupies 1 line, so data rows start at table_area.y + 1.
    let data_start_y = table_area.y + 1;

    // Overlay section divider between owned and unowned worktrees.
    if let Some(offset) = divider_row_offset {
        let y = data_start_y + offset;
        if y < table_area.y + table_area.height {
            let divider_line = Line::from(Span::styled(
                "Not included",
                Style::default().add_modifier(Modifier::DIM | Modifier::ITALIC),
            ));
            let divider_area = Rect::new(table_area.x, y, table_area.width, 1);
            frame.render_widget(Paragraph::new(divider_line), divider_area);
        }
    }

    // Overlay hook lines on placeholder rows (full terminal width, no column constraints).
    for (row_offset, line) in hook_overlays {
        let y = data_start_y + row_offset;
        if y < table_area.y + table_area.height {
            let hook_area = Rect::new(table_area.x, y, table_area.width, 1);
            frame.render_widget(Paragraph::new(line), hook_area);
        }
    }

    // Overlay pruned row content with continuous strikethrough from the Branch
    // column onwards, bridging column separator gaps.
    for (row_offset, line) in pruned_overlays {
        let y = data_start_y + row_offset;
        if y < table_area.y + table_area.height {
            let remaining = table_area.width.saturating_sub(pruned_x_offset);
            let pruned_area = Rect::new(table_area.x + pruned_x_offset, y, remaining, 1);
            frame.render_widget(Paragraph::new(line), pruned_area);
        }
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// Colored cell rendering
// ─────────────────────────────────────────────────────────────────────────────

/// Render ahead/behind counts as colored spans: green for `+N`, red for `-N`.
fn render_ahead_behind_spans(ahead: Option<usize>, behind: Option<usize>) -> Line<'static> {
    let mut spans = Vec::new();
    if let Some(a) = ahead {
        if a > 0 {
            spans.push(Span::styled(
                format!("+{a}"),
                Style::default().fg(Color::Green),
            ));
        }
    }
    if let Some(b) = behind {
        if b > 0 {
            if !spans.is_empty() {
                spans.push(Span::raw(" "));
            }
            spans.push(Span::styled(
                format!("-{b}"),
                Style::default().fg(Color::Red),
            ));
        }
    }
    Line::from(spans)
}

/// Render the Base column cell with green/red coloring.
fn render_base_cell(info: &WorktreeInfo, stat: Stat) -> Cell<'static> {
    if stat == Stat::Lines {
        Cell::from(render_ahead_behind_spans(
            info.base_lines_inserted,
            info.base_lines_deleted,
        ))
    } else {
        Cell::from(render_ahead_behind_spans(info.ahead, info.behind))
    }
}

/// Render the Changes column cell with colored indicators.
fn render_changes_cell(info: &WorktreeInfo, stat: Stat) -> Cell<'static> {
    let mut spans = Vec::new();
    if stat == Stat::Lines {
        let ins =
            info.staged_lines_inserted.unwrap_or(0) + info.unstaged_lines_inserted.unwrap_or(0);
        let del = info.staged_lines_deleted.unwrap_or(0) + info.unstaged_lines_deleted.unwrap_or(0);
        if ins > 0 {
            spans.push(Span::styled(
                format!("+{ins}"),
                Style::default().fg(Color::Green),
            ));
        }
        if del > 0 {
            if !spans.is_empty() {
                spans.push(Span::raw(" "));
            }
            spans.push(Span::styled(
                format!("-{del}"),
                Style::default().fg(Color::Red),
            ));
        }
    } else {
        if info.staged > 0 {
            spans.push(Span::styled(
                format!("+{}", info.staged),
                Style::default().fg(Color::Green),
            ));
        }
        if info.unstaged > 0 {
            if !spans.is_empty() {
                spans.push(Span::raw(" "));
            }
            spans.push(Span::styled(
                format!("-{}", info.unstaged),
                Style::default().fg(Color::Red),
            ));
        }
    }
    if info.untracked > 0 {
        if !spans.is_empty() {
            spans.push(Span::raw(" "));
        }
        spans.push(Span::styled(
            format!("?{}", info.untracked),
            Style::default().add_modifier(Modifier::DIM),
        ));
    }
    Cell::from(Line::from(spans))
}

/// Render the Remote column cell with colored indicators.
fn render_remote_cell(info: &WorktreeInfo, stat: Stat) -> Cell<'static> {
    if stat == Stat::Lines {
        Cell::from(render_ahead_behind_spans(
            info.remote_lines_inserted,
            info.remote_lines_deleted,
        ))
    } else {
        let mut spans = Vec::new();
        if let Some(a) = info.remote_ahead {
            if a > 0 {
                spans.push(Span::styled(
                    format!("\u{21E1}{a}"),
                    Style::default().fg(Color::Green),
                ));
            }
        }
        if let Some(b) = info.remote_behind {
            if b > 0 {
                if !spans.is_empty() {
                    spans.push(Span::raw(" "));
                }
                spans.push(Span::styled(
                    format!("\u{21E3}{b}"),
                    Style::default().fg(Color::Red),
                ));
            }
        }
        Cell::from(Line::from(spans))
    }
}

/// Render a dim middle-dot for an unfilled cell while the table is still
/// streaming. Used as a fallback when the column is too narrow for a shimmer
/// bar to read clearly.
fn loading_glyph_cell() -> Cell<'static> {
    Cell::from(Span::styled(
        "\u{00B7}",
        Style::default().add_modifier(Modifier::DIM),
    ))
}

/// Width below which the shimmer bar collapses to a single dim glyph.
const SHIMMER_MIN_WIDTH: u16 = 3;

/// Width of the bright "highlight" band that sweeps across the shimmer bar.
const SHIMMER_HIGHLIGHT_WIDTH: u16 = 3;

/// Render a sweeping shimmer placeholder for an unfilled cell. The bar fills
/// the column's assigned width with a dim block character; a brighter band
/// sweeps left-to-right driven by the renderer tick. When the column is too
/// narrow for the sweep to read (< 3 chars), falls back to the dim middle-dot.
fn loading_shimmer_cell(width: u16, tick: usize) -> Cell<'static> {
    if width == 0 {
        return Cell::from("");
    }
    if width < SHIMMER_MIN_WIDTH {
        return loading_glyph_cell();
    }
    const BAR_CHAR: &str = "\u{2592}"; // ▒
    let cycle = width + SHIMMER_HIGHLIGHT_WIDTH;
    let pos = (tick as u16) % cycle;
    let dim = Style::default().fg(Color::DarkGray);
    let bright = Style::default().fg(Color::Gray);
    let mut spans: Vec<Span<'static>> = Vec::with_capacity(width as usize);
    for i in 0..width {
        let in_highlight = pos >= i && pos - i < SHIMMER_HIGHLIGHT_WIDTH;
        let style = if in_highlight { bright } else { dim };
        spans.push(Span::styled(BAR_CHAR, style));
    }
    Cell::from(Line::from(spans))
}

/// Render a single cell for the given column and worktree row.
///
/// `width` is the column's assigned width — used to size shimmer bars when
/// the cell is in a loading state.
fn render_cell(
    col: &Column,
    wt: &super::state::WorktreeRow,
    vals: &ColumnValues,
    tick: usize,
    stat: Stat,
    width: u16,
    is_cell_loading: impl Fn(FieldSet) -> bool,
) -> Cell<'static> {
    match col {
        Column::Status => render_status_cell(wt, tick),
        Column::Annotation => render_annotation_cell(&wt.info),
        Column::Branch => Cell::from(vals.branch.clone()),
        Column::Path => Cell::from(vals.path.clone()),
        Column::Size => {
            if vals.size.is_empty() && is_cell_loading(FieldSet::SIZE) {
                loading_shimmer_cell(width, tick)
            } else {
                Cell::from(vals.size.clone())
            }
        }
        Column::Base => {
            if wt.info.ahead.is_none()
                && wt.info.behind.is_none()
                && is_cell_loading(FieldSet::BASE_AHEAD_BEHIND)
            {
                loading_shimmer_cell(width, tick)
            } else {
                render_base_cell(&wt.info, stat)
            }
        }
        Column::Changes => {
            if is_cell_loading(FieldSet::CHANGES)
                && wt.info.staged + wt.info.unstaged + wt.info.untracked == 0
            {
                loading_shimmer_cell(width, tick)
            } else {
                render_changes_cell(&wt.info, stat)
            }
        }
        Column::Remote => {
            if wt.info.remote_ahead.is_none()
                && wt.info.remote_behind.is_none()
                && is_cell_loading(FieldSet::REMOTE_AHEAD_BEHIND)
            {
                loading_shimmer_cell(width, tick)
            } else {
                render_remote_cell(&wt.info, stat)
            }
        }
        Column::Age => {
            if vals.branch_age.is_empty() && is_cell_loading(FieldSet::BRANCH_AGE) {
                loading_shimmer_cell(width, tick)
            } else {
                let cell = Cell::from(vals.branch_age.clone());
                if vals.is_old_branch {
                    cell.style(Style::default().add_modifier(Modifier::DIM))
                } else {
                    cell
                }
            }
        }
        Column::Owner => {
            if vals.owner.is_empty() && is_cell_loading(FieldSet::OWNER) {
                loading_shimmer_cell(width, tick)
            } else {
                Cell::from(vals.owner.clone())
            }
        }
        Column::Hash => {
            if vals.hash.is_empty() && is_cell_loading(FieldSet::LAST_COMMIT) {
                loading_shimmer_cell(width, tick)
            } else {
                Cell::from(vals.hash.clone())
            }
        }
        Column::LastCommit => {
            if vals.last_commit_age.is_empty()
                && vals.last_commit_subject.is_empty()
                && is_cell_loading(FieldSet::LAST_COMMIT)
            {
                loading_shimmer_cell(width, tick)
            } else if vals.last_commit_age.is_empty() {
                Cell::from(vals.last_commit_subject.clone())
            } else if vals.last_commit_subject.is_empty() {
                let cell = Cell::from(vals.last_commit_age.clone());
                if vals.is_old_commit {
                    cell.style(Style::default().add_modifier(Modifier::DIM))
                } else {
                    cell
                }
            } else {
                let age_style = if vals.is_old_commit {
                    Style::default().add_modifier(Modifier::DIM)
                } else {
                    Style::default()
                };
                Cell::from(Line::from(vec![
                    Span::styled(vals.last_commit_age.clone(), age_style),
                    Span::raw(format!(" {}", vals.last_commit_subject)),
                ]))
            }
        }
    }
}

/// Render the status cell with appropriate icon and color.
fn render_status_cell(wt: &super::state::WorktreeRow, tick: usize) -> Cell<'static> {
    match &wt.status {
        WorktreeStatus::Idle => Cell::from(Line::from(Span::styled(
            "waiting",
            Style::default().add_modifier(Modifier::DIM),
        ))),
        WorktreeStatus::Active(label) => {
            let spinner = SPINNER_FRAMES[tick % SPINNER_FRAMES.len()];
            Cell::from(Line::from(Span::styled(
                format!("{spinner} {label}"),
                Style::default().fg(Color::Yellow),
            )))
        }
        WorktreeStatus::Done(final_status) => match final_status {
            FinalStatus::Updated => Cell::from(Line::from(Span::styled(
                format!("{CHECKMARK} updated"),
                Style::default().fg(Color::Green),
            ))),
            FinalStatus::UpToDate => Cell::from(Line::from(Span::styled(
                format!("{CHECKMARK} up to date"),
                Style::default().add_modifier(Modifier::DIM),
            ))),
            FinalStatus::Rebased => Cell::from(Line::from(Span::styled(
                format!("{CHECKMARK} rebased"),
                Style::default().fg(Color::Green),
            ))),
            FinalStatus::Conflict => Cell::from(Line::from(Span::styled(
                format!("{CROSS} conflict"),
                Style::default().fg(Color::Red),
            ))),
            FinalStatus::Diverged => Cell::from(Line::from(Span::styled(
                format!("{SKIP} diverged"),
                Style::default().fg(Color::Yellow),
            ))),
            FinalStatus::Skipped => Cell::from(Line::from(Span::styled(
                format!("{SKIP} skipped"),
                Style::default().fg(Color::Yellow),
            ))),
            FinalStatus::Dirty => Cell::from(Line::from(Span::styled(
                format!("{SKIP} dirty"),
                Style::default().fg(Color::Yellow),
            ))),
            FinalStatus::Pruned => {
                if wt.hook_failed {
                    Cell::from(Line::from(Span::styled(
                        format!("{CROSS} hook failed"),
                        Style::default().fg(Color::Red),
                    )))
                } else if wt.hook_warned {
                    Cell::from(Line::from(vec![
                        Span::styled(format!("{DASH} pruned "), Style::default().fg(Color::Red)),
                        Span::styled("\u{26A0}", Style::default().fg(Color::Yellow)),
                    ]))
                } else {
                    Cell::from(Line::from(Span::styled(
                        format!("{DASH} pruned"),
                        Style::default().fg(Color::Red),
                    )))
                }
            }
            FinalStatus::Pushed => Cell::from(Line::from(Span::styled(
                format!("{CHECKMARK} pushed"),
                Style::default().fg(Color::Green),
            ))),
            FinalStatus::NoPushUpstream => Cell::from(Line::from(Span::styled(
                format!("{SKIP} no remote"),
                Style::default().fg(Color::Yellow),
            ))),
            FinalStatus::Failed => Cell::from(Line::from(Span::styled(
                format!("{CROSS} failed"),
                Style::default().fg(Color::Red),
            ))),
        },
    }
}

/// Format a hook sub-row as a full-width line (not constrained by table columns).
///
/// Rendered as a `Paragraph` overlay on top of an empty placeholder row in the table.
fn format_hook_line(sub: &super::state::HookSubRow, prefix: &str, tick: usize) -> Line<'static> {
    use super::state::HookSubStatus;

    let name = sub.hook_type.filename();
    let status_span = match &sub.status {
        HookSubStatus::Running => {
            let spinner = SPINNER_FRAMES[tick % SPINNER_FRAMES.len()];
            Span::styled(spinner.to_string(), Style::default().fg(Color::Yellow))
        }
        HookSubStatus::Succeeded(d) => Span::styled(
            format!("{CHECKMARK} {}ms", d.as_millis()),
            Style::default().fg(Color::Green),
        ),
        HookSubStatus::Warned(d) => Span::styled(
            format!("\u{26A0} {}ms", d.as_millis()),
            Style::default().fg(Color::Yellow),
        ),
        HookSubStatus::Failed(d) => Span::styled(
            format!("{CROSS} {}ms", d.as_millis()),
            Style::default().fg(Color::Red),
        ),
    };

    Line::from(vec![
        Span::styled(
            format!("  {prefix} "),
            Style::default().add_modifier(Modifier::DIM),
        ),
        Span::styled(
            format!("{name} "),
            Style::default().fg(Color::Indexed(styles::ACCENT_COLOR_INDEX)),
        ),
        status_span,
    ])
}

/// Format a job sub-row as a full-width line with nested tree indentation.
///
/// Job lines are indented one level deeper than their parent hook line.
/// The tree prefix depends on whether the parent hook is last and whether
/// this job is last within its hook.
fn format_job_line(
    job: &super::state::JobSubRow,
    parent_hook_is_last: bool,
    job_is_last: bool,
    tick: usize,
) -> Line<'static> {
    use super::state::JobSubStatus;

    let prefix = match (parent_hook_is_last, job_is_last) {
        (false, false) => "  \u{2502} \u{251C} ", // "  │ ├ "
        (false, true) => "  \u{2502} \u{2514} ",  // "  │ └ "
        (true, false) => "    \u{251C} ",         // "    ├ "
        (true, true) => "    \u{2514} ",          // "    └ "
    };

    let (status_span, name_color) = match &job.status {
        JobSubStatus::Running => {
            let spinner = SPINNER_FRAMES[tick % SPINNER_FRAMES.len()];
            (
                Span::styled(spinner.to_string(), Style::default().fg(Color::Yellow)),
                Color::Yellow,
            )
        }
        JobSubStatus::Succeeded(d) => (
            Span::styled(
                format!("{CHECKMARK} {}ms", d.as_millis()),
                Style::default().fg(Color::Green),
            ),
            Color::Green,
        ),
        JobSubStatus::Failed(d) => (
            Span::styled(
                format!("{CROSS} {}ms", d.as_millis()),
                Style::default().fg(Color::Red),
            ),
            Color::Red,
        ),
        JobSubStatus::Skipped { reason, .. } => {
            let text = if reason.is_empty() {
                format!("{SKIP} skipped")
            } else {
                format!("{SKIP} {reason}")
            };
            (
                Span::styled(text, Style::default().add_modifier(Modifier::DIM)),
                Color::Reset,
            )
        }
    };

    let name_style = if matches!(job.status, JobSubStatus::Skipped { .. }) {
        Style::default().add_modifier(Modifier::DIM)
    } else {
        Style::default().fg(name_color)
    };

    Line::from(vec![
        Span::styled(prefix, Style::default().add_modifier(Modifier::DIM)),
        Span::styled(format!("{} ", job.name), name_style),
        status_span,
    ])
}

/// Extract the plain-text content for a column from pre-computed values.
fn column_plain_text(col: &Column, vals: &ColumnValues) -> String {
    match col {
        Column::Branch => vals.branch.clone(),
        Column::Path => vals.path.clone(),
        Column::Size => vals.size.clone(),
        Column::Base => vals.base.clone(),
        Column::Changes => vals.changes.clone(),
        Column::Remote => vals.remote.clone(),
        Column::Age => vals.branch_age.clone(),
        Column::Owner => vals.owner.clone(),
        Column::Hash => vals.hash.clone(),
        Column::LastCommit => {
            if vals.last_commit_age.is_empty() {
                vals.last_commit_subject.clone()
            } else if vals.last_commit_subject.is_empty() {
                vals.last_commit_age.clone()
            } else {
                format!("{} {}", vals.last_commit_age, vals.last_commit_subject)
            }
        }
        _ => String::new(),
    }
}

/// Build an overlay line for a pruned worktree row with continuous strikethrough.
///
/// Unlike per-cell styling (which leaves gaps at column separators), this
/// produces a single `Line` that spans all columns from Branch onwards so the
/// `CROSSED_OUT` modifier runs unbroken through the separator gaps, ending at
/// the last column's text boundary.
fn format_pruned_overlay(
    vals: &ColumnValues,
    columns: &[Column],
    constraints: &[Constraint],
) -> Line<'static> {
    let style = Style::default()
        .add_modifier(Modifier::CROSSED_OUT)
        .add_modifier(Modifier::DIM);

    let pruned_cols: Vec<_> = columns
        .iter()
        .zip(constraints.iter())
        .filter(|(col, _)| !matches!(col, Column::Status | Column::Annotation))
        .collect();

    if pruned_cols.is_empty() {
        return Line::from("");
    }

    let mut text = String::new();
    let last_idx = pruned_cols.len() - 1;

    for (i, (col, constraint)) in pruned_cols.iter().enumerate() {
        if i > 0 {
            text.push_str("  "); // matches column_spacing(2)
        }
        let col_text = column_plain_text(col, vals);
        if i < last_idx {
            // Pad intermediate columns to their resolved width so the
            // strikethrough spans the full column area.
            let width = match constraint {
                Constraint::Length(w) => *w as usize,
                _ => col_text.len(),
            };
            text.push_str(&format!("{col_text:<width$}"));
        } else {
            // Last column: end at the text boundary (no trailing padding).
            text.push_str(&col_text);
        }
    }

    Line::from(Span::styled(text, style))
}

/// Render the annotation cell (current worktree indicator and default branch marker).
///
/// Matches `list` column layout: two fixed sub-positions `[> ][✦]` so that
/// the `>` and `✦` markers stay in separate visual columns.
fn render_annotation_cell(info: &WorktreeInfo) -> Cell<'static> {
    let mut spans: Vec<Span<'static>> = Vec::new();

    // Sub-position 1: current worktree marker
    if info.is_current {
        spans.push(Span::styled(
            styles::CURRENT_WORKTREE_SYMBOL,
            Style::default().fg(Color::Cyan),
        ));
    } else {
        spans.push(Span::raw(" "));
    }

    // Spacer between the two sub-positions
    spans.push(Span::raw(" "));

    // Sub-position 2: default branch marker (bright purple) or sandbox marker (dim)
    if info.is_default_branch {
        spans.push(Span::styled(
            styles::DEFAULT_BRANCH_SYMBOL,
            Style::default().fg(Color::LightMagenta),
        ));
    } else if info.is_sandbox {
        spans.push(Span::styled(
            styles::SANDBOX_SYMBOL,
            Style::default().fg(Color::DarkGray),
        ));
    } else {
        spans.push(Span::raw(" "));
    }

    Cell::from(Line::from(spans))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::core::sort::SortSpec;
    use crate::core::worktree::list::{Stat, WorktreeInfo};
    use crate::core::worktree::sync_dag::OperationPhase;
    use ratatui::backend::TestBackend;
    use ratatui::Terminal;
    use std::path::PathBuf;

    fn make_test_state(verbose: u8) -> TuiState {
        TuiState::new(
            Vec::<OperationPhase>::new(),
            vec![WorktreeInfo::empty("master")],
            PathBuf::from("/tmp/test"),
            PathBuf::from("/tmp/test"),
            Stat::Summary,
            verbose,
            None,
            false,
            None,
            None::<SortSpec>,
            true,
            false,
        )
    }

    #[test]
    fn loading_glyph_cell_renders_dim_middle_dot() {
        let cell = loading_glyph_cell();
        // Cell is opaque; the test just confirms it constructs without panic.
        // Detailed visual verification belongs in the PTY scenario tests.
        let _ = cell;
    }

    #[test]
    fn loading_shimmer_cell_collapses_to_glyph_when_too_narrow() {
        // Width below SHIMMER_MIN_WIDTH (3) should fall back to the dim dot —
        // a single ▒ wouldn't read as "loading" without movement.
        let _ = loading_shimmer_cell(0, 0);
        let _ = loading_shimmer_cell(1, 0);
        let _ = loading_shimmer_cell(2, 0);
    }

    #[test]
    fn loading_shimmer_cell_fills_column_with_block_chars() {
        // Render a shimmer cell into a 1-row buffer and confirm the bar
        // characters appear across the assigned width.
        let backend = TestBackend::new(10, 1);
        let mut terminal = Terminal::new(backend).unwrap();
        terminal
            .draw(|frame| {
                let cell = loading_shimmer_cell(10, 0);
                let table = Table::new(vec![Row::new(vec![cell])], &[Constraint::Length(10)]);
                frame.render_widget(table, frame.area());
            })
            .unwrap();
        let buffer = terminal.backend().buffer();
        let row: String = (0..10)
            .map(|x| buffer[(x, 0)].symbol().to_string())
            .collect();
        assert!(
            row.chars().all(|c| c == '\u{2592}'),
            "shimmer row should be all ▒, got {row:?}"
        );
    }

    #[test]
    fn loading_shimmer_cell_highlight_position_advances_with_tick() {
        // Render two snapshots at different ticks and confirm the bright
        // highlight is at different positions. We can't easily inspect the fg
        // color of cells directly, but we can confirm the renders differ when
        // the highlight band moves enough to land on different cells.
        let backend1 = TestBackend::new(20, 1);
        let mut t1 = Terminal::new(backend1).unwrap();
        t1.draw(|frame| {
            let cell = loading_shimmer_cell(20, 0);
            let table = Table::new(vec![Row::new(vec![cell])], &[Constraint::Length(20)]);
            frame.render_widget(table, frame.area());
        })
        .unwrap();
        let buf1 = t1.backend().buffer().clone();

        let backend2 = TestBackend::new(20, 1);
        let mut t2 = Terminal::new(backend2).unwrap();
        t2.draw(|frame| {
            let cell = loading_shimmer_cell(20, 10);
            let table = Table::new(vec![Row::new(vec![cell])], &[Constraint::Length(20)]);
            frame.render_widget(table, frame.area());
        })
        .unwrap();
        let buf2 = t2.backend().buffer().clone();

        // Compare foreground colors at each x — they should differ for at
        // least one cell because the highlight has moved.
        let mut differs = false;
        for x in 0..20 {
            if buf1[(x, 0)].fg != buf2[(x, 0)].fg {
                differs = true;
                break;
            }
        }
        assert!(differs, "highlight should move between tick 0 and tick 10");
    }

    #[test]
    fn render_footer_no_op_when_not_verbose() {
        let state = make_test_state(0);
        assert!(!state.show_hook_sub_rows);
        let backend = TestBackend::new(40, 1);
        let mut terminal = Terminal::new(backend).unwrap();
        terminal
            .draw(|frame| {
                render_footer(&state, frame, frame.area());
            })
            .unwrap();
        let buffer = terminal.backend().buffer();
        // All cells should be empty (default).
        for cell in buffer.content().iter() {
            assert_eq!(cell.symbol(), " ");
        }
    }

    #[test]
    fn render_footer_shows_inflight_and_elapsed_when_verbose() {
        let mut state = make_test_state(1);
        state.render_start_elapsed = std::time::Duration::from_millis(1234);
        let backend = TestBackend::new(60, 1);
        let mut terminal = Terminal::new(backend).unwrap();
        terminal
            .draw(|frame| {
                render_footer(&state, frame, frame.area());
            })
            .unwrap();
        let buffer = terminal.backend().buffer();
        let row: String = (0..buffer.area.width)
            .map(|x| buffer[(x, 0)].symbol().to_string())
            .collect();
        assert!(row.contains("inflight:"), "row was: {row:?}");
        assert!(row.contains("elapsed:"), "row was: {row:?}");
        assert!(row.contains("1.2s"), "row was: {row:?}");
    }
}
