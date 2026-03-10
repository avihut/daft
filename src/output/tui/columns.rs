use super::state::{WorktreeRow, WorktreeStatus};
use crate::output::format::ColumnValues;

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
    pub(super) fn priority(self) -> u8 {
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
    pub(super) fn label(self) -> &'static str {
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
pub(super) const ALL_COLUMNS: &[Column] = &[
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
pub(super) const STATUS_MAX_WIDTH: u16 = 12;

/// Minimum width reserved for the LastCommit column before it switches to Fill.
pub(super) const LAST_COMMIT_MIN: u16 = 10;

/// Compute the visible display width of a status cell.
pub(super) fn status_display_width(status: &WorktreeStatus) -> u16 {
    use super::state::FinalStatus;
    match status {
        WorktreeStatus::Idle => 7, // "waiting"
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
pub(super) fn column_content_width(
    col: Column,
    worktrees: &[WorktreeRow],
    vals: &[ColumnValues],
) -> u16 {
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
            Column::Annotation => 3,
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

#[cfg(test)]
mod tests {
    use super::*;

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
