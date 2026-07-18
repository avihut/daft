use super::columns::{
    ALL_COLUMNS, Column, column_content_width, fit_widths_to_available, truncate_with_ellipsis,
};
use super::state::{FinalStatus, PhaseStatus, TuiState, WorktreeStatus};
use crate::core::sort::SortSpec;
use crate::core::worktree::forge_ref::{PrStatus, PrStatusColor};
use crate::core::worktree::info_field::FieldSet;
use crate::core::worktree::list::{EntryKind, Stat, WorktreeInfo};
use crate::output::format::{self, ColumnContext, ColumnValues, format_human_size};
use crate::styles;
use ratatui::{
    Frame,
    layout::{Constraint, Rect},
    style::{Color, Modifier, Style},
    text::{Line, Span},
    widgets::{Cell, Paragraph, Row, Table},
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
    let mut text = format!(" inflight: {inflight} \u{00B7} elapsed: {elapsed_secs:.1}s");
    if state.live.cancelled {
        text.push_str(" \u{00B7} cancelled");
    }
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
        forge_prs: state.live.cfg.forge_prs.as_ref(),
        // The TUI always styles cells, so the PR status rides in color and
        // the cell text stays the bare number.
        colors: true,
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
        // Phased commands: render the user's columns (already resolved by
        // ColumnSelection::parse — replace mode is the user's list verbatim,
        // modifier mode is defaults +/- the user's adjustments) or fall back
        // to ALL_COLUMNS. No width-based dropping: fit_widths_to_available
        // shrinks Branch/Path/LastCommit for narrow terminals, then accepts
        // overflow rather than removing data columns. See #494.
        //
        // Asymmetry preserved: the no-flag fallback is ALL_COLUMNS (includes
        // Hash), while modifier mode's base set comes from
        // ListColumn::tui_defaults() (no Hash). A follow-up may reconcile.
        let columns = state
            .live
            .cfg
            .columns
            .clone()
            .unwrap_or_else(|| ALL_COLUMNS.to_vec());
        // Status is always prepended for TUI commands with phases.
        if columns.contains(&Column::Status) {
            columns
        } else {
            let mut with_status = vec![Column::Status];
            with_status.extend(columns);
            with_status
        }
    };

    // Pre-compute the Size column's formatted TOTAL once and reuse for both
    // (a) the natural-width hint passed to `column_content_width` (so the
    // column is sized to fit the summary cell that's appended later) and
    // (b) the summary footer row build below. The summary row otherwise
    // doesn't participate in width sizing — regression: #501.
    // If a future column also gets a summary row, thread its width through
    // the same way; `column_content_width`'s `extra_width` is single-valued.
    let total_size: Option<String> = if columns.contains(&Column::Size) {
        let total_bytes: u64 = state
            .live
            .rows
            .iter()
            .filter(|wt| wt.info.kind == EntryKind::Worktree)
            .filter(|wt| !matches!(wt.status, WorktreeStatus::Done(FinalStatus::Pruned)))
            .filter_map(|wt| wt.info.size_bytes)
            .sum();
        Some(format_human_size(total_bytes))
    } else {
        None
    };
    let size_total_width: u16 = total_size
        .as_ref()
        .map(|s| s.chars().count() as u16)
        .unwrap_or(0);

    // Compute natural column widths, then shrink Branch/Path so the table fits
    // in the available area. Without this step a single long path or branch
    // name in `live.rows` (often off-screen) blows out those columns and
    // squeezes `LastCommit` (Fill(1)) down to nearly zero width.
    let natural_widths: Vec<u16> = columns
        .iter()
        .map(|col| {
            let extra = if matches!(col, Column::Size) {
                size_total_width
            } else {
                0
            };
            column_content_width(*col, &state.live.rows, &row_vals, sort_ref, extra)
        })
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
                            |fs| state.live.is_cell_unloaded(row_idx, fs),
                            |fs| state.live.is_cell_stale(row_idx, fs),
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
                        |fs| state.live.is_cell_unloaded(row_idx, fs),
                        |fs| state.live.is_cell_stale(row_idx, fs),
                    )
                })
                .collect()
        };
        let mut row = Row::new(main_cells);
        if wt.info.is_current {
            row =
                row.style(Style::default().bg(Color::Indexed(crate::styles::CURRENT_ROW_BG_INDEX)));
        }
        all_rows.push(row);
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

    // Summary footer row for the Size column. `total_size` is computed once
    // above so the natural-width hint and the rendered cell stay in lockstep.
    if let Some(total_size) = &total_size {
        // Empty separator row
        let empty_cells: Vec<Cell> = (0..num_columns).map(|_| Cell::from("")).collect();
        all_rows.push(Row::new(empty_cells));

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
    if let Some(a) = ahead
        && a > 0
    {
        spans.push(Span::styled(
            format!("+{a}"),
            Style::default().fg(Color::Green),
        ));
    }
    if let Some(b) = behind
        && b > 0
    {
        if !spans.is_empty() {
            spans.push(Span::raw(" "));
        }
        spans.push(Span::styled(
            format!("-{b}"),
            Style::default().fg(Color::Red),
        ));
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
        if let Some(a) = info.remote_ahead
            && a > 0
        {
            spans.push(Span::styled(
                format!("\u{21E1}{a}"),
                Style::default().fg(Color::Green),
            ));
        }
        if let Some(b) = info.remote_behind
            && b > 0
        {
            if !spans.is_empty() {
                spans.push(Span::raw(" "));
            }
            spans.push(Span::styled(
                format!("\u{21E3}{b}"),
                Style::default().fg(Color::Red),
            ));
        }
        Cell::from(Line::from(spans))
    }
}

/// Frames in one full breath (dim → bright → dim). At the driver's 80ms tick
/// rate, 16 frames = ~1.3s full cycle. Halve for a snappier pulse, double
/// for a slower one.
const SKELETON_BREATH_FRAMES: usize = 16;

/// xterm 256-color grayscale ramp endpoints. Indices 232 (near-black) through
/// 255 (white) form a 24-step ramp — supported by every terminal that does
/// 256 colors (effectively all modern terminals).
/// Index 239 on the xterm grayscale ramp ≈ 30% luminance (RGB 78,78,78).
const SKELETON_GRAY_DARKEST: u8 = 239;
/// Index 252 on the xterm grayscale ramp ≈ 80% luminance (RGB 208,208,208).
const SKELETON_GRAY_BRIGHTEST: u8 = 252;

/// Render a skeleton placeholder for an unfilled cell — a row of `▬`
/// (BLACK RECTANGLE U+25AC) characters sized to the column's assigned
/// width, breathing uniformly along the xterm 256-color grayscale ramp
/// via a triangle wave. The rectangle char is centered vertically in the
/// cell and shorter than `█`, giving the bar a soft low-profile feel
/// without any height-mismatch caps.
pub(super) fn loading_shimmer_cell(width: u16, tick: usize) -> Cell<'static> {
    if width == 0 {
        return Cell::from("");
    }
    const BAR_CHAR: &str = "\u{25AC}"; // ▬
    let bar: String = BAR_CHAR.repeat(width as usize);
    Cell::from(Span::styled(
        bar,
        Style::default().fg(Color::Indexed(skeleton_pulse_color(tick))),
    ))
}

/// Triangle-wave brightness selector. Returns a 256-color palette index that
/// ramps from `SKELETON_GRAY_DARKEST` up to `SKELETON_GRAY_BRIGHTEST` and
/// back, completing one full breath every `SKELETON_BREATH_FRAMES` ticks.
fn skeleton_pulse_color(tick: usize) -> u8 {
    let half = SKELETON_BREATH_FRAMES / 2;
    let phase = tick % SKELETON_BREATH_FRAMES;
    let t = if phase < half {
        phase
    } else {
        SKELETON_BREATH_FRAMES - phase
    };
    let span = (SKELETON_GRAY_BRIGHTEST - SKELETON_GRAY_DARKEST) as usize;
    let offset = (t * span) / half;
    SKELETON_GRAY_DARKEST + offset as u8
}

/// Render a "data didn't load" placeholder for a cell whose patch was not
/// received before the user cancelled (Ctrl-C). Single em-dash (U+2014) in
/// `Color::Gray`, centered within the column's assigned `width` via leading
/// spaces. Distinct from the loading shimmer (full-width bar of U+25AC) and
/// from a legitimately-empty cell (a blank).
pub(super) fn not_loaded_cell(width: u16) -> Cell<'static> {
    if width == 0 {
        return Cell::from("");
    }
    let left_pad = (width as usize).saturating_sub(1) / 2;
    let padded: String = " ".repeat(left_pad) + "\u{2014}";
    Cell::from(Span::styled(padded, Style::default().fg(Color::Gray)))
}

/// Render a persisted (stale) cell value — a last-known figure shown while a
/// fresh walk runs in the background. Styled `DIM` and nothing else: never
/// colored, never the loading shimmer, so it reads as "real but not yet
/// refreshed" and supersedes cleanly (to the plain, full-brightness value)
/// the moment the fresh patch lands. Shared by the worktree and catalog
/// tables so the stale convention lives in one place.
pub(super) fn stale_cell(value: &str) -> Cell<'static> {
    Cell::from(Span::styled(
        value.to_string(),
        Style::default().add_modifier(Modifier::DIM),
    ))
}

/// Render a single cell for the given column and worktree row.
///
/// `width` is the column's assigned width — used to size shimmer bars when
/// the cell is in a loading state.
/// `is_cell_unloaded` returns true when the user cancelled before the cell's
/// patch arrived; takes precedence over `is_cell_loading`.
/// `is_cell_stale` returns true when the cell holds a persisted value awaiting
/// a fresh walk; applies only to cells that already have a value to show.
#[allow(clippy::too_many_arguments)]
fn render_cell(
    col: &Column,
    wt: &super::state::WorktreeRow,
    vals: &ColumnValues,
    tick: usize,
    stat: Stat,
    width: u16,
    is_cell_loading: impl Fn(FieldSet) -> bool,
    is_cell_unloaded: impl Fn(FieldSet) -> bool,
    is_cell_stale: impl Fn(FieldSet) -> bool,
) -> Cell<'static> {
    match col {
        Column::Status => render_status_cell(wt, tick),
        Column::Annotation => render_annotation_cell(&wt.info),
        Column::Branch => Cell::from(vals.branch.clone()),
        Column::Path => Cell::from(vals.path.clone()),
        Column::Size => {
            if vals.size.is_empty() {
                if is_cell_unloaded(FieldSet::SIZE) {
                    not_loaded_cell(width)
                } else if is_cell_loading(FieldSet::SIZE) {
                    loading_shimmer_cell(width, tick)
                } else {
                    Cell::from(vals.size.clone())
                }
            } else if is_cell_stale(FieldSet::SIZE) {
                stale_cell(&vals.size)
            } else {
                Cell::from(vals.size.clone())
            }
        }
        Column::Base => {
            let unfilled = wt.info.ahead.is_none() && wt.info.behind.is_none();
            if unfilled && is_cell_unloaded(FieldSet::BASE_AHEAD_BEHIND) {
                not_loaded_cell(width)
            } else if unfilled && is_cell_loading(FieldSet::BASE_AHEAD_BEHIND) {
                loading_shimmer_cell(width, tick)
            } else {
                render_base_cell(&wt.info, stat)
            }
        }
        Column::Changes => {
            let unfilled = wt.info.staged + wt.info.unstaged + wt.info.untracked == 0;
            if unfilled && is_cell_unloaded(FieldSet::CHANGES) {
                not_loaded_cell(width)
            } else if unfilled && is_cell_loading(FieldSet::CHANGES) {
                loading_shimmer_cell(width, tick)
            } else {
                render_changes_cell(&wt.info, stat)
            }
        }
        Column::Remote => {
            let unfilled = wt.info.remote_ahead.is_none() && wt.info.remote_behind.is_none();
            if unfilled && is_cell_unloaded(FieldSet::REMOTE_AHEAD_BEHIND) {
                not_loaded_cell(width)
            } else if unfilled && is_cell_loading(FieldSet::REMOTE_AHEAD_BEHIND) {
                loading_shimmer_cell(width, tick)
            } else {
                render_remote_cell(&wt.info, stat)
            }
        }
        Column::Age => {
            if vals.branch_age.is_empty() {
                if is_cell_unloaded(FieldSet::BRANCH_AGE) {
                    not_loaded_cell(width)
                } else if is_cell_loading(FieldSet::BRANCH_AGE) {
                    loading_shimmer_cell(width, tick)
                } else {
                    Cell::from(vals.branch_age.clone())
                }
            } else {
                let cell = Cell::from(vals.branch_age.clone());
                if vals.is_old_branch {
                    cell.style(Style::default().add_modifier(Modifier::DIM))
                } else {
                    cell
                }
            }
        }
        Column::Pr => {
            if vals.pr.is_empty() {
                if is_cell_unloaded(FieldSet::FORGE_REF) {
                    not_loaded_cell(width)
                } else if is_cell_loading(FieldSet::FORGE_REF) {
                    loading_shimmer_cell(width, tick)
                } else {
                    Cell::from(vals.pr.clone())
                }
            } else {
                // Color carries the status here (a ratatui buffer can't hold
                // the colorless glyph fallback's escape-free sibling — plain
                // renderers append `✓`/`✗`/`●`/`◆`/`○` instead). LightMagenta
                // matches the ✦ default-branch purple. The status→slot mapping
                // is shared with the blocking renderer via `semantic_color`.
                let cell = Cell::from(vals.pr.clone());
                match vals.pr_status.and_then(PrStatus::semantic_color) {
                    Some(PrStatusColor::Pass) => cell.style(Style::default().fg(Color::Green)),
                    Some(PrStatusColor::Fail) => cell.style(Style::default().fg(Color::Red)),
                    Some(PrStatusColor::Pending) => cell.style(Style::default().fg(Color::Yellow)),
                    Some(PrStatusColor::Merged) => {
                        cell.style(Style::default().fg(Color::LightMagenta))
                    }
                    Some(PrStatusColor::Closed) => {
                        cell.style(Style::default().add_modifier(Modifier::DIM))
                    }
                    None => cell,
                }
            }
        }
        Column::Owner => {
            if vals.owner.is_empty() {
                if is_cell_unloaded(FieldSet::OWNER) {
                    not_loaded_cell(width)
                } else if is_cell_loading(FieldSet::OWNER) {
                    loading_shimmer_cell(width, tick)
                } else {
                    Cell::from(vals.owner.clone())
                }
            } else {
                Cell::from(vals.owner.clone())
            }
        }
        Column::Hash => {
            if vals.hash.is_empty() {
                if is_cell_unloaded(FieldSet::LAST_COMMIT) {
                    not_loaded_cell(width)
                } else if is_cell_loading(FieldSet::LAST_COMMIT) {
                    loading_shimmer_cell(width, tick)
                } else {
                    Cell::from(vals.hash.clone())
                }
            } else {
                Cell::from(vals.hash.clone())
            }
        }
        Column::LastCommit => {
            if vals.last_commit_age.is_empty() && vals.last_commit_subject.is_empty() {
                if is_cell_unloaded(FieldSet::LAST_COMMIT) {
                    not_loaded_cell(width)
                } else if is_cell_loading(FieldSet::LAST_COMMIT) {
                    loading_shimmer_cell(width, tick)
                } else {
                    Cell::from("")
                }
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
        // Governor-held (#678): dim like Idle — deliberately not running,
        // nothing is wrong.
        WorktreeStatus::Throttled(label) => Cell::from(Line::from(Span::styled(
            label.clone(),
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

    let name = sub.hook_type.hook_name();
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
    use ratatui::Terminal;
    use ratatui::backend::TestBackend;
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
            crate::core::worktree::info_field::FieldSet::EMPTY,
        )
    }

    #[test]
    fn loading_shimmer_cell_zero_width_returns_empty() {
        // Width-0 columns shouldn't paint anything.
        let backend = TestBackend::new(1, 1);
        let mut terminal = Terminal::new(backend).unwrap();
        terminal
            .draw(|frame| {
                let cell = loading_shimmer_cell(0, 0);
                let table = Table::new(vec![Row::new(vec![cell])], &[Constraint::Length(0)]);
                frame.render_widget(table, frame.area());
            })
            .unwrap();
        let buffer = terminal.backend().buffer();
        assert_eq!(buffer[(0, 0)].symbol(), " ");
    }

    #[test]
    fn loading_shimmer_cell_fills_column_with_rectangle_chars() {
        // Render a skeleton cell and confirm every cell in the bar is the
        // BLACK RECTANGLE U+25AC glyph across the full assigned width.
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
            row.chars().all(|c| c == '\u{25AC}'),
            "skeleton bar should be all ▬, got {row:?}"
        );
    }

    #[test]
    fn loading_shimmer_cell_pulses_uniformly_across_phases() {
        // The whole bar should share one foreground color at any given tick,
        // and that color should differ across pulse phases.
        let render_at = |tick: usize| -> ratatui::style::Color {
            let backend = TestBackend::new(20, 1);
            let mut terminal = Terminal::new(backend).unwrap();
            terminal
                .draw(|frame| {
                    let cell = loading_shimmer_cell(20, tick);
                    let table = Table::new(vec![Row::new(vec![cell])], &[Constraint::Length(20)]);
                    frame.render_widget(table, frame.area());
                })
                .unwrap();
            let buffer = terminal.backend().buffer().clone();
            // Confirm uniform fg across the bar.
            let first_fg = buffer[(0, 0)].fg;
            for x in 1..20 {
                assert_eq!(
                    buffer[(x, 0)].fg,
                    first_fg,
                    "skeleton bar should be uniform in color at x={x} (tick={tick})"
                );
            }
            first_fg
        };

        // Tick 0 (darkest) vs tick at the bright peak — different colors.
        let dark = render_at(0);
        let bright = render_at(SKELETON_BREATH_FRAMES / 2);
        assert_ne!(
            dark, bright,
            "skeleton bar should pulse across the breath (dark={dark:?}, bright={bright:?})"
        );
    }

    #[test]
    fn skeleton_pulse_color_traces_a_triangle_wave() {
        // At tick 0 we're at the darkest stop.
        assert_eq!(skeleton_pulse_color(0), SKELETON_GRAY_DARKEST);
        // At the half-cycle we're at the brightest stop.
        assert_eq!(
            skeleton_pulse_color(SKELETON_BREATH_FRAMES / 2),
            SKELETON_GRAY_BRIGHTEST
        );
        // At the end of the cycle we're back at darkest (modular).
        assert_eq!(
            skeleton_pulse_color(SKELETON_BREATH_FRAMES),
            SKELETON_GRAY_DARKEST
        );
        // Symmetry: ascending and descending halves visit the same brightness.
        let quarter = SKELETON_BREATH_FRAMES / 4;
        let three_quarter = SKELETON_BREATH_FRAMES * 3 / 4;
        assert_eq!(
            skeleton_pulse_color(quarter),
            skeleton_pulse_color(three_quarter),
            "triangle wave should be symmetric around the peak"
        );
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

    #[test]
    fn not_loaded_cell_renders_centered_em_dash_in_gray() {
        // The "didn't load" cell should be a single em-dash (U+2014) in
        // Color::Gray, centered within the column's assigned width via
        // leading spaces. Distinct from the breathing skeleton bar (full
        // width of U+25AC).
        let backend = TestBackend::new(5, 1);
        let mut terminal = Terminal::new(backend).unwrap();
        terminal
            .draw(|frame| {
                let cell = not_loaded_cell(5);
                let table = Table::new(vec![Row::new(vec![cell])], &[Constraint::Length(5)]);
                frame.render_widget(table, frame.area());
            })
            .unwrap();
        let buffer = terminal.backend().buffer();
        let row: String = (0..5)
            .map(|x| buffer[(x, 0)].symbol().to_string())
            .collect();
        assert!(
            row.contains("\u{2014}"),
            "expected em-dash in row; got {row:?}"
        );
        // Em-dash should sit at index 2 (center of 5: left_pad = (5-1)/2 = 2).
        assert_eq!(
            buffer[(2, 0)].symbol(),
            "\u{2014}",
            "em-dash should be centered at index 2 for width 5; row was {row:?}"
        );
        assert_eq!(
            buffer[(2, 0)].fg,
            ratatui::style::Color::Gray,
            "em-dash should be Color::Gray for visibility"
        );
    }

    #[test]
    fn not_loaded_cell_zero_width_returns_empty() {
        // Width 0 must not panic and should render nothing.
        let backend = TestBackend::new(1, 1);
        let mut terminal = Terminal::new(backend).unwrap();
        terminal
            .draw(|frame| {
                let cell = not_loaded_cell(0);
                let table = Table::new(vec![Row::new(vec![cell])], &[Constraint::Length(0)]);
                frame.render_widget(table, frame.area());
            })
            .unwrap();
        let buffer = terminal.backend().buffer();
        assert_eq!(buffer[(0, 0)].symbol(), " ");
    }

    #[test]
    fn render_cell_uses_not_loaded_when_cancelled_and_unfilled() {
        // For each loadable column, with is_cell_unloaded returning true,
        // the cell should render the dim em-dash, not the shimmer bar and
        // not an empty cell.
        use crate::core::worktree::info_field::FieldSet;
        use crate::output::format::{ColumnContext, compute_column_values};
        use crate::output::tui::state::WorktreeRow;

        let info = WorktreeInfo::empty("a");
        let wt = WorktreeRow::idle(info.clone());
        let ctx = ColumnContext {
            project_root: &PathBuf::from("/tmp"),
            cwd: &PathBuf::from("/tmp"),
            now: 0,
            stat: Stat::Lines,
            forge_prs: None,
            colors: true,
        };
        let vals = compute_column_values(&info, &ctx);

        let columns = [
            (Column::Size, FieldSet::SIZE),
            (Column::Base, FieldSet::BASE_AHEAD_BEHIND),
            (Column::Changes, FieldSet::CHANGES),
            (Column::Remote, FieldSet::REMOTE_AHEAD_BEHIND),
            (Column::Age, FieldSet::BRANCH_AGE),
            (Column::Owner, FieldSet::OWNER),
            (Column::Hash, FieldSet::LAST_COMMIT),
            (Column::LastCommit, FieldSet::LAST_COMMIT),
        ];

        for (col, _field) in columns {
            let backend = TestBackend::new(10, 1);
            let mut terminal = Terminal::new(backend).unwrap();
            terminal
                .draw(|frame| {
                    let cell = render_cell(
                        &col,
                        &wt,
                        &vals,
                        0,
                        Stat::Lines,
                        10,
                        |_fs| false, // not loading (cancelled implies collection_complete)
                        |_fs| true,  // is_cell_unloaded → true
                        |_fs| false, // not stale
                    );
                    let table = Table::new(vec![Row::new(vec![cell])], &[Constraint::Length(10)]);
                    frame.render_widget(table, frame.area());
                })
                .unwrap();
            let buffer = terminal.backend().buffer();
            let row: String = (0..10)
                .map(|x| buffer[(x, 0)].symbol().to_string())
                .collect();
            assert!(
                row.contains("\u{2014}"),
                "column {col:?} should render em-dash when cancelled and unfilled; row was {row:?}"
            );
        }
    }

    #[test]
    fn render_cell_uses_value_when_received_even_if_cancelled() {
        // If the cell value is non-empty (received), is_cell_unloaded should
        // be false and the value should render. Guards against rendering
        // "—" over real data.
        use crate::output::format::{ColumnContext, compute_column_values};
        use crate::output::tui::state::WorktreeRow;

        let mut info = WorktreeInfo::empty("a");
        info.size_bytes = Some(1024);
        let wt = WorktreeRow::idle(info.clone());
        let ctx = ColumnContext {
            project_root: &PathBuf::from("/tmp"),
            cwd: &PathBuf::from("/tmp"),
            now: 0,
            stat: Stat::Lines,
            forge_prs: None,
            colors: true,
        };
        let vals = compute_column_values(&info, &ctx);

        let backend = TestBackend::new(10, 1);
        let mut terminal = Terminal::new(backend).unwrap();
        terminal
            .draw(|frame| {
                let cell = render_cell(
                    &Column::Size,
                    &wt,
                    &vals,
                    0,
                    Stat::Lines,
                    10,
                    |_fs| false, // not loading
                    |_fs| false, // not unloaded — received
                    |_fs| false, // not stale
                );
                let table = Table::new(vec![Row::new(vec![cell])], &[Constraint::Length(10)]);
                frame.render_widget(table, frame.area());
            })
            .unwrap();
        let buffer = terminal.backend().buffer();
        let row: String = (0..10)
            .map(|x| buffer[(x, 0)].symbol().to_string())
            .collect();
        assert!(
            !row.contains("\u{2014}"),
            "received cell should render value, not em-dash; got {row:?}"
        );
        assert!(
            row.trim_end()
                .chars()
                .any(|c| c.is_ascii_digit() || c == 'B' || c == 'K'),
            "received Size cell should render numeric/unit value; got {row:?}"
        );
    }

    #[test]
    fn render_cell_size_stale_renders_value_dimmed() {
        // A stale (persisted, not-yet-refreshed) Size cell renders its value
        // DIM — visible but muted — never the shimmer, never an em-dash.
        use crate::output::format::{ColumnContext, compute_column_values};
        use crate::output::tui::state::WorktreeRow;

        let mut info = WorktreeInfo::empty("a");
        info.size_bytes = Some(1024);
        let wt = WorktreeRow::idle(info.clone());
        let ctx = ColumnContext {
            project_root: &PathBuf::from("/tmp"),
            cwd: &PathBuf::from("/tmp"),
            now: 0,
            stat: Stat::Summary,
            forge_prs: None,
            colors: true,
        };
        let vals = compute_column_values(&info, &ctx);

        let backend = TestBackend::new(10, 1);
        let mut terminal = Terminal::new(backend).unwrap();
        terminal
            .draw(|frame| {
                let cell = render_cell(
                    &Column::Size,
                    &wt,
                    &vals,
                    0,
                    Stat::Summary,
                    10,
                    |_fs| false, // not loading
                    |_fs| false, // not unloaded
                    |_fs| true,  // stale → dim
                );
                let table = Table::new(vec![Row::new(vec![cell])], &[Constraint::Length(10)]);
                frame.render_widget(table, frame.area());
            })
            .unwrap();
        let buffer = terminal.backend().buffer();
        let row: String = (0..10)
            .map(|x| buffer[(x, 0)].symbol().to_string())
            .collect();
        // Value is shown (not shimmer/em-dash) …
        assert!(
            !row.contains('\u{25AC}') && !row.contains('\u{2014}'),
            "stale cell must render the value, not a placeholder; got {row:?}"
        );
        assert!(
            row.trim_end()
                .chars()
                .any(|c| c.is_ascii_digit() || c == 'B' || c == 'K'),
            "stale Size cell should render the numeric/unit value; got {row:?}"
        );
        // … and it is dimmed. Find the first glyph cell and assert DIM.
        let first_glyph = (0..10)
            .find(|&x| buffer[(x, 0)].symbol().trim() != "")
            .expect("stale cell should paint at least one glyph");
        assert!(
            buffer[(first_glyph, 0)].modifier.contains(Modifier::DIM),
            "stale Size value should carry the DIM modifier"
        );
    }

    #[test]
    fn render_footer_appends_cancelled_when_live_cancelled() {
        let mut state = make_test_state(1);
        state.render_start_elapsed = std::time::Duration::from_millis(1234);
        state.live.mark_cancelled();
        let backend = TestBackend::new(80, 1);
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
        assert!(row.contains("cancelled"), "row was: {row:?}");
        assert!(row.contains("inflight:"), "row was: {row:?}");
    }

    #[test]
    fn render_footer_no_cancelled_suffix_when_not_cancelled() {
        let mut state = make_test_state(1);
        state.render_start_elapsed = std::time::Duration::from_millis(1234);
        // NOT calling mark_cancelled.
        let backend = TestBackend::new(80, 1);
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
        assert!(!row.contains("cancelled"), "row was: {row:?}");
    }

    /// Regression for #494: phased commands (sync/clone/prune/repo-remove)
    /// must render the full default column set at any terminal width that
    /// admits the minimum shrunk widths. A long branch name pushes the
    /// natural total over the constrained width — pre-fix, `select_columns`
    /// would have dropped LastCommit / Hash / Owner here; post-fix,
    /// `fit_widths_to_available` shrinks Branch instead and every column
    /// header is visible.
    ///
    /// At width=100 with all 11 columns at their minimum widths the table
    /// technically overflows by a few characters. That is the spec
    /// ("accept overflow rather than dropping data"). We verify header
    /// *presence* via substring match, not pixel-level fit.
    #[test]
    fn render_table_phased_keeps_all_default_columns_at_constrained_width() {
        let state = TuiState::new(
            vec![OperationPhase::Fetch],
            vec![WorktreeInfo::empty(
                "feature/long-branch-name-to-force-shrinking",
            )],
            PathBuf::from("/tmp/test"),
            PathBuf::from("/tmp/test"),
            Stat::Summary,
            0,
            None,
            false,
            None,
            None::<SortSpec>,
            true,
            false,
            crate::core::worktree::info_field::FieldSet::EMPTY,
        );
        let backend = TestBackend::new(100, 5);
        let mut terminal = Terminal::new(backend).unwrap();
        terminal
            .draw(|frame| {
                render_table(&state, frame, frame.area());
            })
            .unwrap();
        let buffer = terminal.backend().buffer();
        let header: String = (0..buffer.area.width)
            .map(|x| buffer[(x, 0)].symbol().to_string())
            .collect();
        for label in [
            "Status", "Branch", "Path", "Base", "Changes", "Remote", "Age", "Owner", "Hash",
            "Commit",
        ] {
            assert!(
                header.contains(label),
                "header at width 100 should contain {label:?}; got: {header:?}"
            );
        }
    }

    /// Contract for #494: phaseless (`daft list`) was never affected by
    /// `select_columns`, but pin the behavior. With `columns: None` the
    /// `ListColumn::list_defaults()` fallback should yield every default
    /// header — Branch through Commit — at a constrained width.
    #[test]
    fn render_table_phaseless_keeps_all_default_columns_at_constrained_width() {
        let state = TuiState::new(
            Vec::<OperationPhase>::new(),
            vec![WorktreeInfo::empty(
                "feature/long-branch-name-to-force-shrinking",
            )],
            PathBuf::from("/tmp/test"),
            PathBuf::from("/tmp/test"),
            Stat::Summary,
            0,
            None,
            false,
            None,
            None::<SortSpec>,
            true,
            false,
            crate::core::worktree::info_field::FieldSet::EMPTY,
        );
        let backend = TestBackend::new(100, 5);
        let mut terminal = Terminal::new(backend).unwrap();
        terminal
            .draw(|frame| {
                render_table(&state, frame, frame.area());
            })
            .unwrap();
        let buffer = terminal.backend().buffer();
        let header: String = (0..buffer.area.width)
            .map(|x| buffer[(x, 0)].symbol().to_string())
            .collect();
        for label in [
            "Branch", "Path", "Base", "Changes", "Remote", "Age", "Owner", "Commit",
        ] {
            assert!(
                header.contains(label),
                "header at width 100 should contain {label:?}; got: {header:?}"
            );
        }
    }

    /// Regression for #501. Three worktrees of 5 GiB each give a TOTAL of
    /// 15 GiB → "15.0G" (5 chars), wider than each per-row "5.0G" (4 chars).
    /// Pre-fix, `column_content_width` only saw data rows, so the column was
    /// allocated 4 chars and the TOTAL summary cell rendered as "15.0" — the
    /// unit suffix was silently truncated. This test pins that the full
    /// "15.0G" string appears in the rendered buffer.
    #[test]
    fn render_table_size_column_total_fits_when_wider_than_data() {
        let mut wts: Vec<WorktreeInfo> = (0..3)
            .map(|i| {
                let mut info = WorktreeInfo::empty(&format!("feat/branch{i}"));
                info.size_bytes = Some(5 * 1024 * 1024 * 1024); // 5 GiB
                info
            })
            .collect();
        // First worktree is the current one (so it gets a path).
        wts[0].path = Some(PathBuf::from("/tmp/test/feat/branch0"));

        let state = TuiState::new(
            Vec::<OperationPhase>::new(),
            wts,
            PathBuf::from("/tmp/test"),
            PathBuf::from("/tmp/test"),
            Stat::Summary,
            0,
            Some(vec![Column::Branch, Column::Path, Column::Size]),
            true,
            None,
            None::<SortSpec>,
            true,
            false,
            crate::core::worktree::info_field::FieldSet::EMPTY,
        );

        // 80 cols is plenty wide — no shrinking forced. The bug being tested
        // is about natural-width sizing, not about narrow-terminal behavior.
        let backend = TestBackend::new(80, 8);
        let mut terminal = Terminal::new(backend).unwrap();
        terminal
            .draw(|frame| {
                render_table(&state, frame, frame.area());
            })
            .unwrap();
        let buffer = terminal.backend().buffer();

        // Sweep all rows for "15.0G" — the summary row position depends on
        // header + 3 data rows + separator, but pinning the row index would
        // be brittle. The full string only ever appears in the TOTAL cell.
        let full_buffer: String = (0..buffer.area.height)
            .flat_map(|y| {
                (0..buffer.area.width)
                    .map(move |x| buffer[(x, y)].symbol().to_string())
                    .chain(std::iter::once("\n".to_string()))
            })
            .collect();
        assert!(
            full_buffer.contains("15.0G"),
            "TOTAL summary should render full \"15.0G\" — got buffer:\n{full_buffer}"
        );
        assert!(
            !full_buffer.contains("15.0 ") && !full_buffer.contains("15.0\n"),
            "TOTAL must not render as truncated \"15.0\" — got buffer:\n{full_buffer}"
        );
    }
}
