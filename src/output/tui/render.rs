use super::columns::{column_content_width, select_columns, Column};
use super::state::{FinalStatus, PhaseStatus, TuiState, WorktreeStatus};
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

/// Render the worktree status table.
pub fn render_table(state: &TuiState, frame: &mut Frame, area: Rect) {
    let now = chrono::Utc::now().timestamp();
    let ctx = ColumnContext {
        project_root: &state.project_root,
        cwd: &state.cwd,
        now,
        stat: state.stat,
    };

    // Pre-compute all column values for sizing and reuse.
    let row_vals: Vec<ColumnValues> = state
        .worktrees
        .iter()
        .map(|wt| format::compute_column_values(&wt.info, &ctx))
        .collect();

    // Select columns and compute dynamic constraints from content widths.
    let columns = match (&state.columns, state.columns_explicit) {
        // Replace mode: user explicitly chose columns, don't responsively drop.
        (Some(user_cols), true) => user_cols.clone(),
        // Modifier mode: user tweaked defaults, responsive dropping still applies.
        (Some(user_cols), false) => {
            let responsive = select_columns(area.width, &state.worktrees, &row_vals);
            responsive
                .into_iter()
                .filter(|c| matches!(c, Column::Status) || user_cols.contains(c))
                .collect()
        }
        // No column selection: fully responsive.
        (None, _) => select_columns(area.width, &state.worktrees, &row_vals),
    };
    // Status is always prepended for TUI commands.
    let columns = if !columns.contains(&Column::Status) {
        let mut with_status = vec![Column::Status];
        with_status.extend(columns);
        with_status
    } else {
        columns
    };

    let constraints: Vec<Constraint> = columns
        .iter()
        .map(|col| {
            if matches!(col, Column::LastCommit) {
                Constraint::Fill(1)
            } else {
                Constraint::Length(column_content_width(*col, &state.worktrees, &row_vals))
            }
        })
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
            Cell::from(Span::styled(
                col.label(),
                Style::default()
                    .add_modifier(Modifier::DIM)
                    .add_modifier(Modifier::UNDERLINED),
            ))
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

    for (wt_idx, (wt, vals)) in state.worktrees.iter().zip(row_vals.iter()).enumerate() {
        // Insert a placeholder row for the section divider between owned and
        // unowned worktrees.  The actual divider content is overlaid later.
        if state.unowned_start_index == Some(wt_idx) {
            let empty_cells: Vec<Cell> = (0..num_columns).map(|_| Cell::from("")).collect();
            all_rows.push(Row::new(empty_cells));
            divider_row_offset = Some(row_count);
            row_count += 1;
        }

        let is_pruned = matches!(wt.status, WorktreeStatus::Done(FinalStatus::Pruned));
        let main_cells: Vec<Cell> = if is_pruned {
            // Status and Annotation keep their normal cells; other columns are
            // left empty because their content is overlaid with a single
            // continuous strikethrough line.
            columns
                .iter()
                .map(|col| {
                    if matches!(col, Column::Status | Column::Annotation) {
                        render_cell(col, wt, vals, state.tick, state.stat)
                    } else {
                        Cell::from("")
                    }
                })
                .collect()
        } else {
            columns
                .iter()
                .map(|col| render_cell(col, wt, vals, state.tick, state.stat))
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
            .worktrees
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

    frame.render_widget(table, area);

    // The header row occupies 1 line, so data rows start at area.y + 1.
    let data_start_y = area.y + 1;

    // Overlay section divider between owned and unowned worktrees.
    if let Some(offset) = divider_row_offset {
        let y = data_start_y + offset;
        if y < area.y + area.height {
            let divider_line = Line::from(Span::styled(
                "Not included",
                Style::default().add_modifier(Modifier::DIM | Modifier::ITALIC),
            ));
            let divider_area = Rect::new(area.x, y, area.width, 1);
            frame.render_widget(Paragraph::new(divider_line), divider_area);
        }
    }

    // Overlay hook lines on placeholder rows (full terminal width, no column constraints).
    for (row_offset, line) in hook_overlays {
        let y = data_start_y + row_offset;
        if y < area.y + area.height {
            let hook_area = Rect::new(area.x, y, area.width, 1);
            frame.render_widget(Paragraph::new(line), hook_area);
        }
    }

    // Overlay pruned row content with continuous strikethrough from the Branch
    // column onwards, bridging column separator gaps.
    for (row_offset, line) in pruned_overlays {
        let y = data_start_y + row_offset;
        if y < area.y + area.height {
            let remaining = area.width.saturating_sub(pruned_x_offset);
            let pruned_area = Rect::new(area.x + pruned_x_offset, y, remaining, 1);
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

/// Render a single cell for the given column and worktree row.
fn render_cell(
    col: &Column,
    wt: &super::state::WorktreeRow,
    vals: &ColumnValues,
    tick: usize,
    stat: Stat,
) -> Cell<'static> {
    match col {
        Column::Status => render_status_cell(wt, tick),
        Column::Annotation => render_annotation_cell(&wt.info),
        Column::Branch => Cell::from(vals.branch.clone()),
        Column::Path => Cell::from(vals.path.clone()),
        Column::Size => Cell::from(vals.size.clone()),
        Column::Base => render_base_cell(&wt.info, stat),
        Column::Changes => render_changes_cell(&wt.info, stat),
        Column::Remote => render_remote_cell(&wt.info, stat),
        Column::Age => {
            let cell = Cell::from(vals.branch_age.clone());
            if vals.is_old_branch {
                cell.style(Style::default().add_modifier(Modifier::DIM))
            } else {
                cell
            }
        }
        Column::Owner => Cell::from(vals.owner.clone()),
        Column::LastCommit => {
            if vals.last_commit_age.is_empty() {
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

    // Sub-position 2: default branch marker (bright purple, matching `list`)
    if info.is_default_branch {
        spans.push(Span::styled(
            styles::DEFAULT_BRANCH_SYMBOL,
            Style::default().fg(Color::LightMagenta),
        ));
    } else {
        spans.push(Span::raw(" "));
    }

    Cell::from(Line::from(spans))
}
