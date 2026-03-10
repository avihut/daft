use super::columns::{column_content_width, select_columns, Column};
use super::state::{FinalStatus, PhaseStatus, TuiState, WorktreeStatus};
use crate::core::worktree::list::{Stat, WorktreeInfo};
use crate::output::format::{self, ColumnContext, ColumnValues};
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
    let columns = select_columns(area.width, &state.worktrees, &row_vals);

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

    let header_cells: Vec<Cell> = columns
        .iter()
        .map(|col| {
            Cell::from(Span::styled(
                col.label(),
                Style::default().add_modifier(Modifier::DIM),
            ))
        })
        .collect();
    let header_row = Row::new(header_cells);

    let rows: Vec<Row> = state
        .worktrees
        .iter()
        .zip(row_vals.iter())
        .flat_map(|(wt, vals)| {
            let main_cells: Vec<Cell> = columns
                .iter()
                .map(|col| render_cell(col, wt, vals, state.tick, state.stat))
                .collect();
            let mut result = vec![Row::new(main_cells)];

            // Add hook sub-rows if present
            if state.show_hook_sub_rows && !wt.hook_sub_rows.is_empty() {
                for (i, sub) in wt.hook_sub_rows.iter().enumerate() {
                    let is_last = i == wt.hook_sub_rows.len() - 1;
                    let prefix = if is_last { "\u{2514}" } else { "\u{251C}" };
                    let sub_row = render_hook_sub_row(sub, prefix, state.tick);
                    result.push(sub_row);
                }
            }

            result
        })
        .collect();

    let table = Table::new(rows, &constraints)
        .header(header_row)
        .column_spacing(2);

    frame.render_widget(table, area);
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
            FinalStatus::Skipped => Cell::from(Line::from(Span::styled(
                format!("{SKIP} skipped"),
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
            FinalStatus::Failed => Cell::from(Line::from(Span::styled(
                format!("{CROSS} failed"),
                Style::default().fg(Color::Red),
            ))),
        },
    }
}

/// Render a hook sub-row showing individual hook status and timing.
fn render_hook_sub_row(sub: &super::state::HookSubRow, prefix: &str, tick: usize) -> Row<'static> {
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

    let line = Line::from(vec![
        Span::styled(
            format!("  {prefix} "),
            Style::default().add_modifier(Modifier::DIM),
        ),
        Span::styled(
            format!("{name} "),
            Style::default().add_modifier(Modifier::DIM),
        ),
        status_span,
    ]);

    // Sub-rows span the status column; other columns are empty
    Row::new(vec![Cell::from(line)])
}

/// Render the annotation cell (current worktree indicator and default branch marker).
///
/// Matches `list` column layout: two fixed sub-positions `[> ][◉]` so that
/// the `>` and `◉` markers stay in separate visual columns.
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

    // Sub-position 2: default branch marker (dark gray, matching `list`)
    if info.is_default_branch {
        spans.push(Span::styled(
            styles::DEFAULT_BRANCH_SYMBOL,
            Style::default().fg(Color::DarkGray),
        ));
    } else {
        spans.push(Span::raw(" "));
    }

    Cell::from(Line::from(spans))
}
