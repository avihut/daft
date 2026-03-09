//! Inline TUI renderer for sync and prune operations.
//!
//! Uses ratatui with Viewport::Inline to render an operation header
//! and worktree status table that update in-place as tasks execute.

use crate::core::worktree::list::{Stat, WorktreeInfo};
use crate::core::worktree::sync_dag::{DagEvent, OperationPhase, TaskStatus};
use crate::output::format::{self, ColumnContext, ColumnValues};
use crate::styles;

use ratatui::{
    layout::{Constraint, Layout, Position, Rect},
    style::{Color, Modifier, Style},
    text::{Line, Span},
    widgets::{Cell, Paragraph, Row, Table},
    Frame, Terminal, TerminalOptions, Viewport,
};
use std::path::PathBuf;
use std::sync::mpsc;
use std::time::{Duration, Instant};

const SPINNER_FRAMES: &[&str] = &[
    "\u{280b}", "\u{2819}", "\u{2839}", "\u{2838}", "\u{283c}", "\u{2834}", "\u{2826}", "\u{2827}",
    "\u{2807}", "\u{280f}",
];
const CHECKMARK: &str = "\u{2713}";
const CROSS: &str = "\u{2717}";
const SKIP: &str = "\u{2298}";
const DASH: &str = "\u{2014}";

/// Status of a high-level operation phase in the header.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PhaseStatus {
    Pending,
    Active,
    Completed,
}

/// State of a single operation phase for the header display.
#[derive(Debug, Clone)]
pub struct PhaseState {
    pub phase: OperationPhase,
    pub status: PhaseStatus,
}

/// Display status for a single worktree row in the table.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum WorktreeStatus {
    Idle,
    Active(String), // e.g. "updating", "rebasing"
    Done(FinalStatus),
}

/// Final status after an operation completes for a worktree.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum FinalStatus {
    Updated,
    UpToDate,
    Rebased,
    Conflict,
    Skipped,
    Pruned,
    Failed,
}

/// Complete TUI state, rebuilt from DagEvents.
pub struct TuiState {
    pub phases: Vec<PhaseState>,
    pub worktrees: Vec<WorktreeRow>,
    pub done: bool,
    pub tick: usize,
    pub project_root: PathBuf,
    pub cwd: PathBuf,
    pub stat: Stat,
}

/// A single row in the worktree table.
pub struct WorktreeRow {
    pub info: WorktreeInfo,
    pub status: WorktreeStatus,
}

impl TuiState {
    pub fn new(
        phases: Vec<OperationPhase>,
        worktree_infos: Vec<WorktreeInfo>,
        project_root: PathBuf,
        cwd: PathBuf,
        stat: Stat,
    ) -> Self {
        Self {
            phases: phases
                .into_iter()
                .map(|phase| PhaseState {
                    phase,
                    status: PhaseStatus::Pending,
                })
                .collect(),
            worktrees: worktree_infos
                .into_iter()
                .map(|info| WorktreeRow {
                    info,
                    status: WorktreeStatus::Idle,
                })
                .collect(),
            done: false,
            tick: 0,
            project_root,
            cwd,
            stat,
        }
    }

    pub fn apply_event(&mut self, event: &DagEvent) {
        match event {
            DagEvent::TaskStarted { phase, branch_name } => {
                self.activate_phase(phase);
                let active_label = match phase {
                    OperationPhase::Fetch => "fetching",
                    OperationPhase::Prune => "pruning",
                    OperationPhase::Update => "updating",
                    OperationPhase::Rebase(_) => "rebasing",
                };
                // Auto-create row for newly discovered branches (e.g., gone branches
                // found after fetch completes while TUI is already running).
                if !branch_name.is_empty() && self.find_row_mut(branch_name).is_none() {
                    self.worktrees.push(WorktreeRow {
                        info: WorktreeInfo::empty(branch_name),
                        status: WorktreeStatus::Idle,
                    });
                }
                if let Some(row) = self.find_row_mut(branch_name) {
                    row.status = WorktreeStatus::Active(active_label.into());
                }
            }
            DagEvent::TaskCompleted {
                phase,
                branch_name,
                status,
                message,
            } => {
                let final_status = Self::map_final_status(phase, *status, message);
                if let Some(row) = self.find_row_mut(branch_name) {
                    row.status = WorktreeStatus::Done(final_status);
                }
                self.check_phase_completion(phase);
            }
            DagEvent::AllDone => {
                for phase in &mut self.phases {
                    if phase.status != PhaseStatus::Completed {
                        phase.status = PhaseStatus::Completed;
                    }
                }
                // Mark any worktrees that were never touched as up-to-date.
                // This covers prune (where only gone branches get events) and
                // any other scenario where some rows stay idle.
                for wt in &mut self.worktrees {
                    if wt.status == WorktreeStatus::Idle {
                        wt.status = WorktreeStatus::Done(FinalStatus::UpToDate);
                    }
                }
                self.done = true;
            }
        }
    }

    pub fn tick(&mut self) {
        self.tick += 1;
    }

    fn activate_phase(&mut self, phase: &OperationPhase) {
        if let Some(ps) = self.phases.iter_mut().find(|ps| &ps.phase == phase) {
            if ps.status == PhaseStatus::Pending {
                ps.status = PhaseStatus::Active;
            }
        }
    }

    fn check_phase_completion(&mut self, phase: &OperationPhase) {
        // A phase is complete when all tasks belonging to it have a terminal status
        // in the worktree rows, and no rows show an Active label for this phase.
        let phase_active_label = match phase {
            OperationPhase::Fetch => "fetching",
            OperationPhase::Prune => "pruning",
            OperationPhase::Update => "updating",
            OperationPhase::Rebase(_) => "rebasing",
        };
        let any_active = self.worktrees.iter().any(
            |w| matches!(&w.status, WorktreeStatus::Active(label) if label == phase_active_label),
        );
        if !any_active {
            if let Some(ps) = self.phases.iter_mut().find(|ps| &ps.phase == phase) {
                if ps.status == PhaseStatus::Active {
                    ps.status = PhaseStatus::Completed;
                }
            }
        }
    }

    fn find_row_mut(&mut self, branch_name: &str) -> Option<&mut WorktreeRow> {
        self.worktrees
            .iter_mut()
            .find(|w| w.info.name == branch_name)
    }

    fn map_final_status(phase: &OperationPhase, status: TaskStatus, message: &str) -> FinalStatus {
        match status {
            TaskStatus::Failed => FinalStatus::Failed,
            TaskStatus::DepFailed => FinalStatus::Skipped,
            TaskStatus::Skipped => FinalStatus::Skipped,
            TaskStatus::Succeeded => match phase {
                OperationPhase::Prune => FinalStatus::Pruned,
                OperationPhase::Update => {
                    if message.contains("up to date") || message.contains("Already up to date") {
                        FinalStatus::UpToDate
                    } else {
                        FinalStatus::Updated
                    }
                }
                OperationPhase::Rebase(_) => {
                    if message.contains("conflict") {
                        FinalStatus::Conflict
                    } else {
                        FinalStatus::Rebased
                    }
                }
                OperationPhase::Fetch => FinalStatus::Updated,
            },
            TaskStatus::Pending | TaskStatus::Running => FinalStatus::Failed,
        }
    }

    // ─────────────────────────────────────────────────────────────────────────
    // Rendering
    // ─────────────────────────────────────────────────────────────────────────

    /// Render the operation header showing phase progress.
    pub fn render_header(&self, frame: &mut Frame, area: Rect) {
        let lines: Vec<Line> = self
            .phases
            .iter()
            .map(|ps| match ps.status {
                PhaseStatus::Pending => Line::from(Span::styled(
                    format!("  {}", ps.phase.label()),
                    Style::default().add_modifier(Modifier::DIM),
                )),
                PhaseStatus::Active => {
                    let spinner = SPINNER_FRAMES[self.tick % SPINNER_FRAMES.len()];
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
    pub fn render_table(&self, frame: &mut Frame, area: Rect) {
        let now = chrono::Utc::now().timestamp();
        let ctx = ColumnContext {
            project_root: &self.project_root,
            cwd: &self.cwd,
            now,
            stat: self.stat,
        };

        // Pre-compute all column values for sizing and reuse.
        let row_vals: Vec<ColumnValues> = self
            .worktrees
            .iter()
            .map(|wt| format::compute_column_values(&wt.info, &ctx))
            .collect();

        // Select columns and compute dynamic constraints from content widths.
        let columns = select_columns(area.width, &self.worktrees, &row_vals);

        let constraints: Vec<Constraint> = columns
            .iter()
            .map(|col| {
                if matches!(col, Column::LastCommit) {
                    Constraint::Fill(1)
                } else {
                    Constraint::Length(column_content_width(*col, &self.worktrees, &row_vals))
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

        let rows: Vec<Row> = self
            .worktrees
            .iter()
            .zip(row_vals.iter())
            .map(|(wt, vals)| {
                let cells: Vec<Cell> = columns
                    .iter()
                    .map(|col| render_cell(col, wt, vals, self.tick, self.stat))
                    .collect();
                Row::new(cells)
            })
            .collect();

        let table = Table::new(rows, &constraints)
            .header(header_row)
            .column_spacing(2);

        frame.render_widget(table, area);
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// Column priority system
// ─────────────────────────────────────────────────────────────────────────────

/// Columns available in the worktree table, ordered by display priority.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Column {
    /// Sync/prune status indicator. Priority 0 (always shown).
    Status,
    /// Current/default branch annotation. Priority 1 (always shown).
    Annotation,
    /// Branch name. Priority 2 (always shown).
    Branch,
    /// Worktree path. Priority 3.
    Path,
    /// Commits ahead/behind base branch. Priority 4.
    Base,
    /// Commits ahead/behind remote. Priority 5.
    Remote,
    /// Local changes (staged/unstaged/untracked). Priority 6.
    Changes,
    /// Branch age. Priority 7.
    Age,
    /// Last commit subject. Priority 8.
    LastCommit,
}

impl Column {
    /// Display priority (lower = higher priority, always shown first).
    fn priority(self) -> u8 {
        match self {
            Self::Status => 0,
            Self::Annotation => 1,
            Self::Branch => 2,
            Self::Path => 3,
            Self::Base => 4,
            Self::Changes => 5,
            Self::Remote => 6,
            Self::Age => 7,
            Self::LastCommit => 8,
        }
    }

    /// Column header label.
    fn label(self) -> &'static str {
        match self {
            Self::Status => "Status",
            Self::Annotation => "",
            Self::Branch => "Branch",
            Self::Path => "Path",
            Self::Base => "Base",
            Self::Changes => "Changes",
            Self::Remote => "Remote",
            Self::Age => "Age",
            Self::LastCommit => "Last Commit",
        }
    }
}

/// All columns in display order.
const ALL_COLUMNS: &[Column] = &[
    Column::Status,
    Column::Annotation,
    Column::Branch,
    Column::Path,
    Column::Base,
    Column::Changes,
    Column::Remote,
    Column::Age,
    Column::LastCommit,
];

// ─────────────────────────────────────────────────────────────────────────────
// Dynamic column sizing
// ─────────────────────────────────────────────────────────────────────────────

/// Widest possible status text: "✓ up to date" = 12 visible chars.
/// Used to prevent layout jumps as statuses change during the TUI loop.
const STATUS_MAX_WIDTH: u16 = 12;

/// Minimum width reserved for the LastCommit column before it switches to Fill.
const LAST_COMMIT_MIN: u16 = 10;

/// Compute the visible display width of a status cell.
fn status_display_width(status: &WorktreeStatus) -> u16 {
    match status {
        WorktreeStatus::Idle => 0,
        WorktreeStatus::Active(label) => (2 + label.len()) as u16,
        WorktreeStatus::Done(fs) => match fs {
            FinalStatus::Updated => 9,   // "✓ updated"
            FinalStatus::UpToDate => 12, // "✓ up to date"
            FinalStatus::Rebased => 9,   // "✓ rebased"
            FinalStatus::Conflict => 10, // "✗ conflict"
            FinalStatus::Skipped => 9,   // "⊘ skipped"
            FinalStatus::Pruned => 8,    // "— pruned"
            FinalStatus::Failed => 8,    // "✗ failed"
        },
    }
}

/// Compute the maximum content width a column needs across all rows.
fn column_content_width(col: Column, worktrees: &[WorktreeRow], vals: &[ColumnValues]) -> u16 {
    let header_width = col.label().len() as u16;
    if worktrees.is_empty() {
        return match col {
            Column::Status => header_width.max(STATUS_MAX_WIDTH),
            _ => header_width,
        };
    }
    let max_data = worktrees
        .iter()
        .zip(vals.iter())
        .map(|(wt, v)| match col {
            // Pre-allocate for the longest possible status to avoid layout jumps.
            Column::Status => status_display_width(&wt.status).max(STATUS_MAX_WIDTH),
            Column::Annotation => 4,
            Column::Branch => v.branch.len() as u16,
            Column::Path => v.path.len() as u16,
            Column::Base => v.base.len() as u16,
            Column::Changes => v.changes.len() as u16,
            Column::Remote => v.remote.len() as u16,
            Column::Age => v.branch_age.len() as u16,
            Column::LastCommit => LAST_COMMIT_MIN,
        })
        .max()
        .unwrap_or(0);
    header_width.max(max_data)
}

/// Select which columns fit in the given terminal width using content-based widths.
///
/// Always keeps columns with priority <= 2 (Status, Annotation, Branch).
/// Drops lowest-priority columns first when the terminal is too narrow.
pub fn select_columns(width: u16, worktrees: &[WorktreeRow], vals: &[ColumnValues]) -> Vec<Column> {
    let mut cols: Vec<Column> = ALL_COLUMNS.to_vec();

    loop {
        // Total = sum of content widths + inter-column spacing (1 char each gap).
        let content: u16 = cols
            .iter()
            .map(|c| column_content_width(*c, worktrees, vals))
            .sum();
        let spacing = cols.len().saturating_sub(1) as u16 * 2;
        if content + spacing <= width {
            break;
        }
        if let Some(pos) = cols.iter().rposition(|c| c.priority() > 2) {
            cols.remove(pos);
        } else {
            break;
        }
    }

    cols
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
    wt: &WorktreeRow,
    vals: &ColumnValues,
    tick: usize,
    stat: Stat,
) -> Cell<'static> {
    match col {
        Column::Status => render_status_cell(&wt.status, tick),
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
fn render_status_cell(status: &WorktreeStatus, tick: usize) -> Cell<'static> {
    match status {
        WorktreeStatus::Idle => Cell::from(""),
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
            FinalStatus::Pruned => Cell::from(Line::from(Span::styled(
                format!("{DASH} pruned"),
                Style::default().fg(Color::Red),
            ))),
            FinalStatus::Failed => Cell::from(Line::from(Span::styled(
                format!("{CROSS} failed"),
                Style::default().fg(Color::Red),
            ))),
        },
    }
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

    // Trailing space to match `list` column padding
    spans.push(Span::raw(" "));

    Cell::from(Line::from(spans))
}

// ─────────────────────────────────────────────────────────────────────────────
// TUI Renderer (main loop)
// ─────────────────────────────────────────────────────────────────────────────

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

                self.state.render_header(frame, chunks[0]);
                self.state.render_table(frame, chunks[1]);
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
                            let worktree_rows = self.state.worktrees.len() as u16;
                            terminal.draw(|frame| {
                                let area = frame.area();
                                let chunks = Layout::vertical([
                                    Constraint::Length(header_height),
                                    Constraint::Fill(1),
                                ])
                                .split(area);
                                self.state.render_header(frame, chunks[0]);
                                self.state.render_table(frame, chunks[1]);

                                // table header (1 row) + data rows
                                let content_bottom = area.y + header_height + 1 + worktree_rows;
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
                        let worktree_rows = self.state.worktrees.len() as u16;
                        terminal.draw(|frame| {
                            let area = frame.area();
                            let chunks = Layout::vertical([
                                Constraint::Length(header_height),
                                Constraint::Fill(1),
                            ])
                            .split(area);
                            self.state.render_header(frame, chunks[0]);
                            self.state.render_table(frame, chunks[1]);

                            let content_bottom = area.y + header_height + 1 + worktree_rows;
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::core::worktree::sync_dag::*;

    fn make_test_state() -> TuiState {
        let phases = vec![
            OperationPhase::Fetch,
            OperationPhase::Prune,
            OperationPhase::Update,
        ];

        let worktree_infos = vec![
            WorktreeInfo::empty("master"),
            WorktreeInfo::empty("feat/a"),
            WorktreeInfo::empty("feat/old"),
        ];

        TuiState::new(
            phases,
            worktree_infos,
            PathBuf::from("/tmp/test"),
            PathBuf::from("/tmp/test"),
            Stat::Summary,
        )
    }

    #[test]
    fn initial_state_all_pending() {
        let state = make_test_state();
        assert!(state
            .phases
            .iter()
            .all(|p| p.status == PhaseStatus::Pending));
        assert!(state
            .worktrees
            .iter()
            .all(|w| w.status == WorktreeStatus::Idle));
        assert!(!state.done);
    }

    #[test]
    fn task_started_activates_phase_and_row() {
        let mut state = make_test_state();
        state.apply_event(&DagEvent::TaskStarted {
            phase: OperationPhase::Fetch,
            branch_name: String::new(),
        });
        assert_eq!(state.phases[0].status, PhaseStatus::Active);
    }

    #[test]
    fn task_completed_updates_row_status() {
        let mut state = make_test_state();

        state.apply_event(&DagEvent::TaskStarted {
            phase: OperationPhase::Update,
            branch_name: "master".into(),
        });
        state.apply_event(&DagEvent::TaskCompleted {
            phase: OperationPhase::Update,
            branch_name: "master".into(),
            status: TaskStatus::Succeeded,
            message: "Already up to date".into(),
        });

        let row = state
            .worktrees
            .iter()
            .find(|w| w.info.name == "master")
            .unwrap();
        assert_eq!(row.status, WorktreeStatus::Done(FinalStatus::UpToDate));
    }

    #[test]
    fn all_done_sets_done_flag() {
        let mut state = make_test_state();
        state.apply_event(&DagEvent::AllDone);
        assert!(state.done);
    }

    #[test]
    fn prune_task_sets_pruned_status() {
        let mut state = make_test_state();

        state.apply_event(&DagEvent::TaskStarted {
            phase: OperationPhase::Prune,
            branch_name: "feat/old".into(),
        });
        let row = state
            .worktrees
            .iter()
            .find(|w| w.info.name == "feat/old")
            .unwrap();
        assert_eq!(row.status, WorktreeStatus::Active("pruning".into()));

        state.apply_event(&DagEvent::TaskCompleted {
            phase: OperationPhase::Prune,
            branch_name: "feat/old".into(),
            status: TaskStatus::Succeeded,
            message: "removed".into(),
        });

        let row = state
            .worktrees
            .iter()
            .find(|w| w.info.name == "feat/old")
            .unwrap();
        assert_eq!(row.status, WorktreeStatus::Done(FinalStatus::Pruned));
    }

    #[test]
    fn failed_task_sets_failed_status() {
        let mut state = make_test_state();

        state.apply_event(&DagEvent::TaskStarted {
            phase: OperationPhase::Update,
            branch_name: "master".into(),
        });
        state.apply_event(&DagEvent::TaskCompleted {
            phase: OperationPhase::Update,
            branch_name: "master".into(),
            status: TaskStatus::Failed,
            message: "pull failed".into(),
        });

        let row = state
            .worktrees
            .iter()
            .find(|w| w.info.name == "master")
            .unwrap();
        assert_eq!(row.status, WorktreeStatus::Done(FinalStatus::Failed));
    }

    #[test]
    fn dep_failed_sets_skipped_status() {
        let mut state = make_test_state();

        state.apply_event(&DagEvent::TaskCompleted {
            phase: OperationPhase::Update,
            branch_name: "master".into(),
            status: TaskStatus::DepFailed,
            message: "dependency failed".into(),
        });

        let row = state
            .worktrees
            .iter()
            .find(|w| w.info.name == "master")
            .unwrap();
        assert_eq!(row.status, WorktreeStatus::Done(FinalStatus::Skipped));
    }

    #[test]
    fn tick_increments_counter() {
        let mut state = make_test_state();
        assert_eq!(state.tick, 0);
        state.tick();
        assert_eq!(state.tick, 1);
        state.tick();
        assert_eq!(state.tick, 2);
    }

    #[test]
    fn phase_completes_when_no_active_rows() {
        let mut state = make_test_state();
        // Start and complete the fetch task
        state.apply_event(&DagEvent::TaskStarted {
            phase: OperationPhase::Fetch,
            branch_name: String::new(),
        });
        assert_eq!(state.phases[0].status, PhaseStatus::Active);

        state.apply_event(&DagEvent::TaskCompleted {
            phase: OperationPhase::Fetch,
            branch_name: String::new(),
            status: TaskStatus::Succeeded,
            message: "fetched".into(),
        });
        // Fetch phase should now be completed (fetch task has empty branch_name,
        // so no row was Active for it -- but the phase was Active and now no
        // rows show "fetching")
        assert_eq!(state.phases[0].status, PhaseStatus::Completed);
    }

    #[test]
    fn all_done_completes_remaining_phases() {
        let mut state = make_test_state();
        // All phases start as Pending
        assert!(state
            .phases
            .iter()
            .all(|p| p.status == PhaseStatus::Pending));

        state.apply_event(&DagEvent::AllDone);

        // AllDone should mark all phases as Completed
        assert!(state
            .phases
            .iter()
            .all(|p| p.status == PhaseStatus::Completed));
    }

    #[test]
    fn update_with_changes_sets_updated_status() {
        let mut state = make_test_state();

        state.apply_event(&DagEvent::TaskStarted {
            phase: OperationPhase::Update,
            branch_name: "master".into(),
        });
        state.apply_event(&DagEvent::TaskCompleted {
            phase: OperationPhase::Update,
            branch_name: "master".into(),
            status: TaskStatus::Succeeded,
            message: "Fast-forward".into(),
        });

        let row = state
            .worktrees
            .iter()
            .find(|w| w.info.name == "master")
            .unwrap();
        assert_eq!(row.status, WorktreeStatus::Done(FinalStatus::Updated));
    }

    #[test]
    fn auto_creates_row_for_unknown_branch() {
        let mut state = make_test_state();
        assert_eq!(state.worktrees.len(), 3);

        // A TaskStarted for an unknown branch should auto-create a row
        state.apply_event(&DagEvent::TaskStarted {
            phase: OperationPhase::Prune,
            branch_name: "feat/discovered".into(),
        });

        assert_eq!(state.worktrees.len(), 4);
        let row = state
            .worktrees
            .iter()
            .find(|w| w.info.name == "feat/discovered")
            .unwrap();
        assert_eq!(row.status, WorktreeStatus::Active("pruning".into()));
    }

    #[test]
    fn all_done_marks_idle_rows_as_up_to_date() {
        let mut state = make_test_state();

        // Only one branch gets a prune event; the others stay Idle.
        state.apply_event(&DagEvent::TaskStarted {
            phase: OperationPhase::Prune,
            branch_name: "feat/old".into(),
        });
        state.apply_event(&DagEvent::TaskCompleted {
            phase: OperationPhase::Prune,
            branch_name: "feat/old".into(),
            status: TaskStatus::Succeeded,
            message: "removed".into(),
        });

        state.apply_event(&DagEvent::AllDone);

        // feat/old was pruned
        let pruned = state
            .worktrees
            .iter()
            .find(|w| w.info.name == "feat/old")
            .unwrap();
        assert_eq!(pruned.status, WorktreeStatus::Done(FinalStatus::Pruned));

        // The remaining idle rows should now be up-to-date
        let master = state
            .worktrees
            .iter()
            .find(|w| w.info.name == "master")
            .unwrap();
        assert_eq!(master.status, WorktreeStatus::Done(FinalStatus::UpToDate));

        let feat_a = state
            .worktrees
            .iter()
            .find(|w| w.info.name == "feat/a")
            .unwrap();
        assert_eq!(feat_a.status, WorktreeStatus::Done(FinalStatus::UpToDate));
    }

    // ─────────────────────────────────────────────────────────────────────────
    // Column selection tests
    // ─────────────────────────────────────────────────────────────────────────

    #[test]
    fn column_selection_wide_terminal() {
        let cols = select_columns(120, &[], &[]);
        assert_eq!(cols.len(), 9);
    }

    #[test]
    fn column_selection_narrow_drops_last_commit() {
        let cols = select_columns(60, &[], &[]);
        assert!(!cols.iter().any(|c| matches!(c, Column::LastCommit)));
    }

    #[test]
    fn column_selection_very_narrow_keeps_essentials() {
        let cols = select_columns(30, &[], &[]);
        assert!(cols.iter().any(|c| matches!(c, Column::Status)));
        assert!(cols.iter().any(|c| matches!(c, Column::Branch)));
    }
}
