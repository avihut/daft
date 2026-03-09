//! Inline TUI renderer for sync and prune operations.
//!
//! Uses ratatui with Viewport::Inline to render an operation header
//! and worktree status table that update in-place as tasks execute.

use crate::core::worktree::list::WorktreeInfo;
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
        let columns = select_columns(area.width);

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

        let now = chrono::Utc::now().timestamp();
        let ctx = ColumnContext {
            project_root: &self.project_root,
            cwd: &self.cwd,
            now,
        };

        let rows: Vec<Row> = self
            .worktrees
            .iter()
            .map(|wt| {
                let vals = format::compute_column_values(&wt.info, &ctx);
                let cells: Vec<Cell> = columns
                    .iter()
                    .map(|col| render_cell(col, wt, &vals, self.tick))
                    .collect();
                Row::new(cells)
            })
            .collect();

        let constraints: Vec<Constraint> = columns.iter().map(|col| col.constraint()).collect();

        let table = Table::new(rows, &constraints).header(header_row);

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
            Self::Remote => 5,
            Self::Changes => 6,
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
            Self::Remote => "Remote",
            Self::Changes => "Changes",
            Self::Age => "Age",
            Self::LastCommit => "Last Commit",
        }
    }

    /// Layout constraint for this column.
    fn constraint(self) -> Constraint {
        match self {
            Self::Status => Constraint::Length(14),
            Self::Annotation => Constraint::Length(4),
            Self::Branch => Constraint::Fill(2),
            Self::Path => Constraint::Fill(2),
            Self::Base => Constraint::Length(8),
            Self::Remote => Constraint::Length(8),
            Self::Changes => Constraint::Length(8),
            Self::Age => Constraint::Length(4),
            Self::LastCommit => Constraint::Fill(3),
        }
    }

    /// Minimum width this column needs to be useful.
    fn min_width(self) -> u16 {
        match self {
            Self::Status => 14,
            Self::Annotation => 4,
            Self::Branch => 8,
            Self::Path => 8,
            Self::Base => 8,
            Self::Remote => 8,
            Self::Changes => 8,
            Self::Age => 4,
            Self::LastCommit => 10,
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
    Column::Remote,
    Column::Changes,
    Column::Age,
    Column::LastCommit,
];

/// Select which columns fit in the given terminal width.
///
/// Always keeps columns with priority <= 2 (Status, Annotation, Branch).
/// Drops lowest-priority columns first when the terminal is too narrow.
pub fn select_columns(width: u16) -> Vec<Column> {
    // Start with all columns, then drop from the end (lowest priority) until they fit.
    let mut cols: Vec<Column> = ALL_COLUMNS.to_vec();

    loop {
        let total_min: u16 = cols.iter().map(|c| c.min_width()).sum();
        if total_min <= width {
            break;
        }
        // Drop the lowest-priority column (highest priority number) that isn't essential.
        if let Some(pos) = cols.iter().rposition(|c| c.priority() > 2) {
            cols.remove(pos);
        } else {
            // Only essential columns left, can't drop any more.
            break;
        }
    }

    cols
}

/// Render a single cell for the given column and worktree row.
fn render_cell(col: &Column, wt: &WorktreeRow, vals: &ColumnValues, tick: usize) -> Cell<'static> {
    match col {
        Column::Status => render_status_cell(&wt.status, tick),
        Column::Annotation => render_annotation_cell(&wt.info),
        Column::Branch => Cell::from(vals.branch.clone()),
        Column::Path => Cell::from(vals.path.clone()),
        Column::Base => Cell::from(vals.base.clone()),
        Column::Remote => Cell::from(vals.remote.clone()),
        Column::Changes => Cell::from(vals.changes.clone()),
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

    // ─────────────────────────────────────────────────────────────────────────
    // Column selection tests
    // ─────────────────────────────────────────────────────────────────────────

    #[test]
    fn column_selection_wide_terminal() {
        let cols = select_columns(120);
        assert_eq!(cols.len(), 9);
    }

    #[test]
    fn column_selection_narrow_drops_last_commit() {
        let cols = select_columns(60);
        assert!(!cols.iter().any(|c| matches!(c, Column::LastCommit)));
    }

    #[test]
    fn column_selection_very_narrow_keeps_essentials() {
        let cols = select_columns(30);
        assert!(cols.iter().any(|c| matches!(c, Column::Status)));
        assert!(cols.iter().any(|c| matches!(c, Column::Branch)));
    }
}
