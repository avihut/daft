use super::state::{WorktreeRow, WorktreeStatus};
use crate::core::columns::ListColumn;
use crate::core::sort::SortSpec;
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
    /// Disk size of worktree. Priority 4.
    Size,
    /// Commits ahead/behind base branch. Priority 5.
    Base,
    /// Commits ahead/behind remote. Priority 5.
    Remote,
    /// Local changes (staged/unstaged/untracked). Priority 6.
    Changes,
    /// Branch age. Priority 7.
    Age,
    /// Branch owner (from git author email). Priority 8.
    Owner,
    /// Abbreviated commit hash (7 chars). Priority 9.
    Hash,
    /// Last commit subject. Priority 10.
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
            Self::Size => 4,
            Self::Base => 5,
            Self::Changes => 6,
            Self::Remote => 7,
            Self::Age => 8,
            Self::Owner => 9,
            Self::Hash => 10,
            Self::LastCommit => 11,
        }
    }

    /// Column header label.
    pub(super) fn label(self) -> &'static str {
        match self {
            Self::Status => "Status",
            Self::Annotation => "",
            Self::Branch => "Branch",
            Self::Path => "Path",
            Self::Size => "Size",
            Self::Base => "Base",
            Self::Changes => "Changes",
            Self::Remote => "Remote",
            Self::Age => "Age",
            Self::Owner => "Owner",
            Self::Hash => "Hash",
            Self::LastCommit => "Commit",
        }
    }

    /// Convert from a user-facing `ListColumn` to the TUI `Column`.
    pub fn from_list_column(lc: ListColumn) -> Self {
        match lc {
            ListColumn::Annotation => Column::Annotation,
            ListColumn::Branch => Column::Branch,
            ListColumn::Path => Column::Path,
            ListColumn::Size => Column::Size,
            ListColumn::Base => Column::Base,
            ListColumn::Changes => Column::Changes,
            ListColumn::Remote => Column::Remote,
            ListColumn::Age => Column::Age,
            ListColumn::Owner => Column::Owner,
            ListColumn::Hash => Column::Hash,
            ListColumn::LastCommit => Column::LastCommit,
        }
    }

    /// Convert to the corresponding `ListColumn`, if one exists.
    ///
    /// Returns `None` for `Status` (TUI-only column with no `ListColumn` equivalent).
    pub fn to_list_column(self) -> Option<ListColumn> {
        match self {
            Self::Status => None,
            Self::Annotation => Some(ListColumn::Annotation),
            Self::Branch => Some(ListColumn::Branch),
            Self::Path => Some(ListColumn::Path),
            Self::Size => Some(ListColumn::Size),
            Self::Base => Some(ListColumn::Base),
            Self::Changes => Some(ListColumn::Changes),
            Self::Remote => Some(ListColumn::Remote),
            Self::Age => Some(ListColumn::Age),
            Self::Owner => Some(ListColumn::Owner),
            Self::Hash => Some(ListColumn::Hash),
            Self::LastCommit => Some(ListColumn::LastCommit),
        }
    }
}

/// Default columns in display order.
///
/// Size is excluded because it requires an expensive filesystem walk and should
/// only appear when the user explicitly requests it via `--columns +size`.
pub(super) const ALL_COLUMNS: &[Column] = &[
    Column::Status,
    Column::Annotation,
    Column::Branch,
    Column::Path,
    Column::Base,
    Column::Changes,
    Column::Remote,
    Column::Age,
    Column::Owner,
    Column::Hash,
    Column::LastCommit,
];

// ─────────────────────────────────────────────────────────────────────────────
// Dynamic column sizing
// ─────────────────────────────────────────────────────────────────────────────

/// Widest possible status text: "⠧ post-remove" = 13 visible chars.
/// Used to prevent layout jumps as statuses change during the TUI loop.
pub(super) const STATUS_MAX_WIDTH: u16 = 13;

/// Minimum width LastCommit can be shrunk to before we accept overflow.
pub(super) const LAST_COMMIT_MIN: u16 = 16;

/// Compute the visible display width of a status cell.
pub(super) fn status_display_width(status: &WorktreeStatus) -> u16 {
    use super::state::FinalStatus;
    match status {
        WorktreeStatus::Idle => 7, // "waiting"
        WorktreeStatus::Active(label) => (2 + label.len()) as u16,
        WorktreeStatus::Done(fs) => match fs {
            FinalStatus::Updated => 9,         // "✓ updated"
            FinalStatus::UpToDate => 12,       // "✓ up to date"
            FinalStatus::Rebased => 9,         // "✓ rebased"
            FinalStatus::Conflict => 10,       // "✗ conflict"
            FinalStatus::Diverged => 10,       // "⊘ diverged"
            FinalStatus::Skipped => 9,         // "⊘ skipped"
            FinalStatus::Pushed => 8,          // "✓ pushed"
            FinalStatus::NoPushUpstream => 11, // "⊘ no remote"
            FinalStatus::Pruned => 8,          // "— pruned"
            FinalStatus::Dirty => 7,           // "⊘ dirty"
            FinalStatus::Failed => 8,          // "✗ failed"
        },
    }
}

/// Compute the maximum content width a column needs across all rows.
pub(super) fn column_content_width(
    col: Column,
    worktrees: &[WorktreeRow],
    vals: &[ColumnValues],
    sort_spec: Option<&SortSpec>,
) -> u16 {
    // Account for sort indicator (" ↓" / " ↑" = 2 visible chars) in header width.
    let sort_extra: u16 = col
        .to_list_column()
        .and_then(|lc| sort_spec.and_then(|s| s.direction_indicator(lc)))
        .map(|_| 2)
        .unwrap_or(0);
    let header_width = col.label().len() as u16 + sort_extra;
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
            Column::Size => v.size.len() as u16,
            Column::Base => v.base.len() as u16,
            Column::Changes => v.changes.len() as u16,
            Column::Remote => v.remote.len() as u16,
            Column::Age => v.branch_age.len() as u16,
            Column::Owner => v.owner.len() as u16,
            Column::Hash => 7,
            // Use the actual rendered width — "<age> <subject>" — so the
            // fit-to-width algorithm has a real natural width to shrink from.
            // Falls back to LAST_COMMIT_MIN when both are empty so the column
            // doesn't disappear during early streaming.
            Column::LastCommit => {
                let age_len = v.last_commit_age.chars().count() as u16;
                let subj_len = v.last_commit_subject.chars().count() as u16;
                let combined = if age_len == 0 || subj_len == 0 {
                    age_len + subj_len
                } else {
                    age_len + 1 + subj_len // " " between age and subject
                };
                combined.max(LAST_COMMIT_MIN)
            }
        })
        .max()
        .unwrap_or(0);
    header_width.max(max_data)
}

/// Select which columns fit in the given terminal width using content-based widths.
///
/// Always keeps columns with priority <= 2 (Status, Annotation, Branch).
/// Drops lowest-priority columns first when the terminal is too narrow.
pub fn select_columns(
    width: u16,
    worktrees: &[WorktreeRow],
    vals: &[ColumnValues],
    sort_spec: Option<&SortSpec>,
) -> Vec<Column> {
    let mut cols: Vec<Column> = ALL_COLUMNS.to_vec();

    loop {
        // Total = sum of content widths + inter-column spacing (1 char each gap).
        let content: u16 = cols
            .iter()
            .map(|c| column_content_width(*c, worktrees, vals, sort_spec))
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

/// Minimum widths for shrinkable columns. Below these the column would lose
/// most of its meaning, so we stop shrinking and accept overflow instead.
const BRANCH_MIN_WIDTH: u16 = 12;
const PATH_MIN_WIDTH: u16 = 8;

/// Inter-column spacing in the TUI table (must match `Table::column_spacing`).
const COLUMN_SPACING: u16 = 2;

/// Adjust per-column natural widths so the table fits in `available` width.
///
/// Shrinks the widest of {Branch, Path, LastCommit} down to per-column
/// minimums until the table fits, mirroring `tabled::Width::truncate(...)
/// .priority(Priority::max(true))` from the blocking renderer. Returns the
/// natural widths unchanged when they already fit.
pub(super) fn fit_widths_to_available(
    columns: &[Column],
    natural_widths: &[u16],
    available: u16,
) -> Vec<u16> {
    let mut widths = natural_widths.to_vec();
    if columns.is_empty() {
        return widths;
    }

    let spacing = (columns.len().saturating_sub(1)) as u16 * COLUMN_SPACING;
    let total_natural: u32 = widths.iter().map(|w| *w as u32).sum::<u32>() + spacing as u32;
    if total_natural <= available as u32 {
        return widths;
    }

    let shrink_min = |c: Column| -> Option<u16> {
        match c {
            Column::Branch => Some(BRANCH_MIN_WIDTH),
            Column::Path => Some(PATH_MIN_WIDTH),
            Column::LastCommit => Some(LAST_COMMIT_MIN),
            _ => None,
        }
    };

    let mut current = total_natural;
    while current > available as u32 {
        let candidate = columns
            .iter()
            .enumerate()
            .filter_map(|(i, c)| {
                let min = shrink_min(*c)?;
                (widths[i] > min).then_some((i, widths[i]))
            })
            .max_by_key(|(_, w)| *w);
        match candidate {
            Some((i, _)) => {
                widths[i] -= 1;
                current -= 1;
            }
            None => break,
        }
    }

    widths
}

/// Truncate `s` to fit `width` columns, appending an ellipsis when shortened.
/// Falls back to a hard cut for very small widths where an ellipsis wouldn't
/// leave room for any content.
pub(super) fn truncate_with_ellipsis(s: &str, width: u16) -> String {
    let w = width as usize;
    if s.chars().count() <= w {
        return s.to_string();
    }
    if w < 4 {
        return s.chars().take(w).collect();
    }
    let prefix: String = s.chars().take(w - 3).collect();
    format!("{prefix}...")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn column_selection_wide_terminal() {
        let cols = select_columns(200, &[], &[], None);
        assert_eq!(cols.len(), ALL_COLUMNS.len());
    }

    #[test]
    fn column_selection_narrow_drops_last_commit() {
        let cols = select_columns(60, &[], &[], None);
        assert!(!cols.iter().any(|c| matches!(c, Column::LastCommit)));
    }

    #[test]
    fn column_selection_very_narrow_keeps_essentials() {
        let cols = select_columns(30, &[], &[], None);
        assert!(cols.iter().any(|c| matches!(c, Column::Status)));
        assert!(cols.iter().any(|c| matches!(c, Column::Branch)));
    }

    /// Regression: Size must never appear in the default responsive set.
    /// It is an opt-in column that requires `--columns +size`.
    #[test]
    fn size_excluded_from_default_responsive_columns() {
        let cols = select_columns(500, &[], &[], None);
        assert!(
            !cols.iter().any(|c| matches!(c, Column::Size)),
            "Size should not appear in responsive defaults even on a wide terminal"
        );
    }

    #[test]
    fn fit_widths_passthrough_when_total_fits() {
        let cols = vec![Column::Branch, Column::Path, Column::Age];
        let natural = vec![20, 30, 4];
        let out = fit_widths_to_available(&cols, &natural, 200);
        assert_eq!(out, natural);
    }

    #[test]
    fn fit_widths_shrinks_widest_first() {
        // Branch=80, Path=60, total+spacing = 142. Available = 100.
        // Path is wider, should be shrunk first; Branch shrinks once Path
        // catches it.
        let cols = vec![Column::Branch, Column::Path];
        let natural = vec![80, 60];
        let out = fit_widths_to_available(&cols, &natural, 100);
        let total: u16 = out.iter().sum::<u16>() + 2;
        assert!(total <= 100, "fit widths exceed available: {out:?}");
        assert!(out[0] >= BRANCH_MIN_WIDTH);
        assert!(out[1] >= PATH_MIN_WIDTH);
    }

    #[test]
    fn fit_widths_shrinks_lastcommit_too() {
        // LastCommit is the widest, so it should be shrunk first when the
        // table doesn't fit.
        let cols = vec![Column::Branch, Column::Path, Column::LastCommit];
        let natural = vec![20, 20, 200];
        let out = fit_widths_to_available(&cols, &natural, 100);
        let total: u16 = out.iter().sum::<u16>() + 4; // 2 gaps = 4
        assert!(total <= 100, "fit widths exceed available: {out:?}");
        // Branch and Path should still be at natural since LastCommit had room
        // to spare on its own.
        assert_eq!(out[0], 20);
        assert_eq!(out[1], 20);
        assert!(out[2] >= LAST_COMMIT_MIN);
    }

    #[test]
    fn fit_widths_lastcommit_keeps_room_when_branch_path_huge() {
        // Branch=200, Path=200, LastCommit=80. Total way over. The widest-first
        // policy should shrink Branch and Path before touching LastCommit.
        let cols = vec![Column::Branch, Column::Path, Column::LastCommit];
        let natural = vec![200, 200, 80];
        let out = fit_widths_to_available(&cols, &natural, 160);
        let total: u16 = out.iter().sum::<u16>() + 4;
        assert!(total <= 160, "fit widths exceed available: {out:?}");
        assert!(
            out[2] >= 40,
            "LastCommit kept most of its width: branch={}, path={}, lc={}",
            out[0],
            out[1],
            out[2]
        );
    }

    #[test]
    fn fit_widths_stops_at_minimums() {
        // Even an absurdly narrow terminal shouldn't shrink Branch/Path below
        // their minimum widths.
        let cols = vec![Column::Branch, Column::Path];
        let natural = vec![100, 100];
        let out = fit_widths_to_available(&cols, &natural, 10);
        assert_eq!(out[0], BRANCH_MIN_WIDTH);
        assert_eq!(out[1], PATH_MIN_WIDTH);
    }

    #[test]
    fn truncate_with_ellipsis_shorter_than_width() {
        assert_eq!(truncate_with_ellipsis("hello", 10), "hello");
    }

    #[test]
    fn truncate_with_ellipsis_appends_dots() {
        assert_eq!(truncate_with_ellipsis("hello world", 8), "hello...");
    }

    #[test]
    fn truncate_with_ellipsis_hard_cut_when_no_room_for_dots() {
        assert_eq!(truncate_with_ellipsis("hello", 3), "hel");
    }

    #[test]
    fn truncate_with_ellipsis_handles_unicode() {
        // Each emoji is 1 char (not 1 byte), so truncating to 5 keeps 5 emoji.
        let s = "🦀🦀🦀🦀🦀🦀🦀";
        assert_eq!(truncate_with_ellipsis(s, 5).chars().count(), 5);
    }
}
