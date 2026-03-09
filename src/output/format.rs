//! Shared column formatting functions for worktree list output.
//!
//! These formatters produce plain or ANSI-colored strings used by both the
//! `tabled`-based CLI table (`list.rs`) and the ratatui TUI table (`tui.rs`).

use crate::core::worktree::list::WorktreeInfo;
use crate::styles;
use pathdiff::diff_paths;
use std::path::Path;

/// Format ahead/behind counts as `+N -N`, with optional color.
pub fn format_ahead_behind(ahead: Option<usize>, behind: Option<usize>, use_color: bool) -> String {
    let mut parts = Vec::new();

    if let Some(a) = ahead {
        if a > 0 {
            let text = format!("+{a}");
            if use_color {
                parts.push(styles::green(&text));
            } else {
                parts.push(text);
            }
        }
    }

    if let Some(b) = behind {
        if b > 0 {
            let text = format!("-{b}");
            if use_color {
                parts.push(styles::red(&text));
            } else {
                parts.push(text);
            }
        }
    }

    parts.join(" ")
}

/// Format head status using worktrunk-style indicators: `+` staged, `-` unstaged, `?` untracked.
pub fn format_head_status(
    staged: usize,
    unstaged: usize,
    untracked: usize,
    use_color: bool,
) -> String {
    let mut parts = Vec::new();

    if staged > 0 {
        let text = format!("+{staged}");
        if use_color {
            parts.push(styles::green(&text));
        } else {
            parts.push(text);
        }
    }

    if unstaged > 0 {
        let text = format!("-{unstaged}");
        if use_color {
            parts.push(styles::red(&text));
        } else {
            parts.push(text);
        }
    }

    if untracked > 0 {
        let text = format!("?{untracked}");
        if use_color {
            parts.push(styles::dim(&text));
        } else {
            parts.push(text);
        }
    }

    parts.join(" ")
}

/// Format remote status using arrows for upstream ahead/behind.
pub fn format_remote_status(
    ahead: Option<usize>,
    behind: Option<usize>,
    use_color: bool,
) -> String {
    let mut parts = Vec::new();

    if let Some(a) = ahead {
        if a > 0 {
            let text = format!("\u{21E1}{a}");
            if use_color {
                parts.push(styles::green(&text));
            } else {
                parts.push(text);
            }
        }
    }

    if let Some(b) = behind {
        if b > 0 {
            let text = format!("\u{21E3}{b}");
            if use_color {
                parts.push(styles::red(&text));
            } else {
                parts.push(text);
            }
        }
    }

    parts.join(" ")
}

/// Convert seconds elapsed into a compact shorthand string.
///
/// Examples: `<1m`, `5m`, `3h`, `2d`, `3w`, `5mo`, `2y`.
pub fn shorthand_from_seconds(secs: i64) -> String {
    if secs < 0 {
        return "<1m".to_string();
    }
    let minutes = secs / 60;
    let hours = secs / 3600;
    let days = secs / 86400;
    let weeks = days / 7;
    let months = days / 30;
    let years = days / 365;

    if minutes < 1 {
        "<1m".to_string()
    } else if hours < 1 {
        format!("{minutes}m")
    } else if days < 1 {
        format!("{hours}h")
    } else if days < 7 {
        format!("{days}d")
    } else if days < 30 {
        format!("{weeks}w")
    } else if years < 1 {
        format!("{months}mo")
    } else {
        format!("{years}y")
    }
}

/// Format a Unix timestamp as a shorthand age string, with optional dim styling.
pub fn format_shorthand_age(timestamp: Option<i64>, now: i64, use_color: bool) -> String {
    match timestamp {
        Some(ts) => {
            let secs = now - ts;
            let text = shorthand_from_seconds(secs);
            if use_color && is_old_seconds(secs) {
                styles::dim(&text)
            } else {
                text
            }
        }
        None => String::new(),
    }
}

/// Check if an age in seconds represents more than 7 days.
pub fn is_old_seconds(secs: i64) -> bool {
    secs > 7 * 86400
}

/// Compute a display path relative to cwd, falling back to project-root-relative.
pub fn relative_display_path(abs_path: &Path, project_root: &Path, cwd: &Path) -> String {
    // Try relative to cwd first
    if let Some(rel) = diff_paths(abs_path, cwd) {
        let s = rel.display().to_string();
        if s.is_empty() {
            return ".".to_string();
        }
        return s;
    }
    // Fallback: relative to project root
    abs_path
        .strip_prefix(project_root)
        .unwrap_or(abs_path)
        .display()
        .to_string()
}

// ─────────────────────────────────────────────────────────────────────────────
// Unified column value computation
// ─────────────────────────────────────────────────────────────────────────────

/// Pre-computed plain-text column values for a single worktree row.
/// No ANSI codes — renderers apply their own styling.
pub struct ColumnValues {
    pub branch: String,
    pub path: String,
    pub base: String,
    pub changes: String,
    pub remote: String,
    pub branch_age: String,
    pub last_commit_age: String,
    pub last_commit_subject: String,
    pub is_old_branch: bool,
    pub is_old_commit: bool,
}

/// Context needed to compute column values for a row.
pub struct ColumnContext<'a> {
    pub project_root: &'a Path,
    pub cwd: &'a Path,
    pub now: i64,
}

/// Compute plain-text column values for a single `WorktreeInfo`.
///
/// Returns Summary-mode values only. For `Stat::Lines` mode, callers can
/// override the `base`, `changes`, and `remote` fields after this call.
pub fn compute_column_values(info: &WorktreeInfo, ctx: &ColumnContext) -> ColumnValues {
    let branch = info.name.clone();

    let path = info
        .path
        .as_ref()
        .map(|p| relative_display_path(p, ctx.project_root, ctx.cwd))
        .unwrap_or_default();

    let base = format_ahead_behind(info.ahead, info.behind, false);
    let changes = format_head_status(info.staged, info.unstaged, info.untracked, false);
    let remote = format_remote_status(info.remote_ahead, info.remote_behind, false);

    let branch_age_secs = info.branch_creation_timestamp.map(|ts| ctx.now - ts);
    let branch_age = info
        .branch_creation_timestamp
        .map(|ts| shorthand_from_seconds(ctx.now - ts))
        .unwrap_or_default();
    let is_old_branch = branch_age_secs.is_some_and(is_old_seconds);

    let commit_age_secs = info.last_commit_timestamp.map(|ts| ctx.now - ts);
    let last_commit_age = info
        .last_commit_timestamp
        .map(|ts| shorthand_from_seconds(ctx.now - ts))
        .unwrap_or_default();
    let is_old_commit = commit_age_secs.is_some_and(is_old_seconds);

    let last_commit_subject = info.last_commit_subject.clone();

    ColumnValues {
        branch,
        path,
        base,
        changes,
        remote,
        branch_age,
        last_commit_age,
        last_commit_subject,
        is_old_branch,
        is_old_commit,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_shorthand_from_seconds_sub_minute() {
        assert_eq!(shorthand_from_seconds(0), "<1m");
        assert_eq!(shorthand_from_seconds(30), "<1m");
        assert_eq!(shorthand_from_seconds(59), "<1m");
    }

    #[test]
    fn test_shorthand_from_seconds_minutes() {
        assert_eq!(shorthand_from_seconds(60), "1m");
        assert_eq!(shorthand_from_seconds(300), "5m");
        assert_eq!(shorthand_from_seconds(3599), "59m");
    }

    #[test]
    fn test_shorthand_from_seconds_hours() {
        assert_eq!(shorthand_from_seconds(3600), "1h");
        assert_eq!(shorthand_from_seconds(7200), "2h");
        assert_eq!(shorthand_from_seconds(86399), "23h");
    }

    #[test]
    fn test_shorthand_from_seconds_days() {
        assert_eq!(shorthand_from_seconds(86400), "1d");
        assert_eq!(shorthand_from_seconds(3 * 86400), "3d");
        assert_eq!(shorthand_from_seconds(6 * 86400), "6d");
    }

    #[test]
    fn test_shorthand_from_seconds_weeks() {
        assert_eq!(shorthand_from_seconds(7 * 86400), "1w");
        assert_eq!(shorthand_from_seconds(14 * 86400), "2w");
        assert_eq!(shorthand_from_seconds(28 * 86400), "4w");
        assert_eq!(shorthand_from_seconds(29 * 86400), "4w");
    }

    #[test]
    fn test_shorthand_from_seconds_months() {
        assert_eq!(shorthand_from_seconds(30 * 86400), "1mo");
        assert_eq!(shorthand_from_seconds(90 * 86400), "3mo");
        assert_eq!(shorthand_from_seconds(364 * 86400), "12mo");
    }

    #[test]
    fn test_shorthand_from_seconds_years() {
        assert_eq!(shorthand_from_seconds(365 * 86400), "1y");
        assert_eq!(shorthand_from_seconds(730 * 86400), "2y");
    }

    #[test]
    fn test_shorthand_from_seconds_negative() {
        assert_eq!(shorthand_from_seconds(-100), "<1m");
    }

    #[test]
    fn test_is_old_seconds() {
        assert!(!is_old_seconds(0));
        assert!(!is_old_seconds(7 * 86400));
        assert!(is_old_seconds(7 * 86400 + 1));
        assert!(is_old_seconds(30 * 86400));
    }
}
