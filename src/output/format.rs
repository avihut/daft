//! Shared column formatting functions for worktree list output.
//!
//! These formatters produce plain or ANSI-colored strings used by both the
//! `tabled`-based CLI table (`list.rs`) and the ratatui TUI table (`tui.rs`).

use crate::core::worktree::list::{Stat, WorktreeInfo};
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

/// Format head status indicators: `+` staged, `-` unstaged, `?` untracked.
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
    pub size: String,
    pub base: String,
    pub changes: String,
    pub remote: String,
    pub branch_age: String,
    pub last_commit_age: String,
    pub last_commit_subject: String,
    pub owner: String,
    pub hash: String,
    pub is_old_branch: bool,
    pub is_old_commit: bool,
}

/// Context needed to compute column values for a row.
pub struct ColumnContext<'a> {
    pub project_root: &'a Path,
    pub cwd: &'a Path,
    pub now: i64,
    pub stat: Stat,
}

/// Format head status using line-level counts: combined staged+unstaged
/// insertions/deletions plus untracked file count.
pub fn format_head_status_lines(info: &WorktreeInfo) -> String {
    let ins = info.staged_lines_inserted.unwrap_or(0) + info.unstaged_lines_inserted.unwrap_or(0);
    let del = info.staged_lines_deleted.unwrap_or(0) + info.unstaged_lines_deleted.unwrap_or(0);
    let mut parts = Vec::new();
    if ins > 0 {
        parts.push(format!("+{ins}"));
    }
    if del > 0 {
        parts.push(format!("-{del}"));
    }
    if info.untracked > 0 {
        parts.push(format!("?{}", info.untracked));
    }
    parts.join(" ")
}

/// Format a byte count as a human-readable size string.
///
/// Uses binary units (1024-based) with short labels: K, M, G, T.
/// Examples: `<1K`, `42K`, `1.3M`, `2.5G`, `1.0T`.
pub fn format_human_size(bytes: u64) -> String {
    const KIB: u64 = 1024;
    const MIB: u64 = 1024 * 1024;
    const GIB: u64 = 1024 * 1024 * 1024;
    const TIB: u64 = 1024 * 1024 * 1024 * 1024;

    if bytes < KIB {
        "<1K".to_string()
    } else if bytes < MIB {
        format!("{}K", bytes / KIB)
    } else if bytes < GIB {
        format!("{:.1}M", bytes as f64 / MIB as f64)
    } else if bytes < TIB {
        format!("{:.1}G", bytes as f64 / GIB as f64)
    } else {
        format!("{:.1}T", bytes as f64 / TIB as f64)
    }
}

/// Compute plain-text column values for a single `WorktreeInfo`.
///
/// Respects `ctx.stat`: Summary mode uses commit/file counts, Lines mode
/// uses line-level insertion/deletion counts for Base, Changes, and Remote.
pub fn compute_column_values(info: &WorktreeInfo, ctx: &ColumnContext) -> ColumnValues {
    let branch = info.name.clone();

    let path = info
        .path
        .as_ref()
        .map(|p| relative_display_path(p, ctx.project_root, ctx.cwd))
        .unwrap_or_default();

    let size = info.size_bytes.map(format_human_size).unwrap_or_default();

    let (base, changes, remote) = if ctx.stat == Stat::Lines {
        (
            format_ahead_behind(info.base_lines_inserted, info.base_lines_deleted, false),
            format_head_status_lines(info),
            format_ahead_behind(info.remote_lines_inserted, info.remote_lines_deleted, false),
        )
    } else {
        (
            format_ahead_behind(info.ahead, info.behind, false),
            format_head_status(info.staged, info.unstaged, info.untracked, false),
            format_remote_status(info.remote_ahead, info.remote_behind, false),
        )
    };

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

    let owner = info.owner_email.clone().unwrap_or_default();

    let hash = info.last_commit_hash.clone().unwrap_or_default();

    ColumnValues {
        branch,
        path,
        size,
        base,
        changes,
        remote,
        branch_age,
        last_commit_age,
        last_commit_subject,
        owner,
        hash,
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
    fn test_format_human_size_bytes() {
        assert_eq!(format_human_size(0), "<1K");
        assert_eq!(format_human_size(500), "<1K");
        assert_eq!(format_human_size(1023), "<1K");
    }

    #[test]
    fn test_format_human_size_kilobytes() {
        assert_eq!(format_human_size(1024), "1K");
        assert_eq!(format_human_size(500 * 1024), "500K");
        assert_eq!(format_human_size(1024 * 1024 - 1), "1023K");
    }

    #[test]
    fn test_format_human_size_megabytes() {
        assert_eq!(format_human_size(1024 * 1024), "1.0M");
        assert_eq!(format_human_size(1300 * 1024 * 1024 / 1000), "1.3M");
        assert_eq!(format_human_size(500 * 1024 * 1024), "500.0M");
    }

    #[test]
    fn test_format_human_size_gigabytes() {
        assert_eq!(format_human_size(1024 * 1024 * 1024), "1.0G");
        assert_eq!(format_human_size(1300 * 1024 * 1024), "1.3G");
        assert_eq!(
            format_human_size(2u64 * 1024 * 1024 * 1024 + 500 * 1024 * 1024),
            "2.5G"
        );
    }

    #[test]
    fn test_format_human_size_terabytes() {
        assert_eq!(format_human_size(1024u64 * 1024 * 1024 * 1024), "1.0T");
    }

    #[test]
    fn test_is_old_seconds() {
        assert!(!is_old_seconds(0));
        assert!(!is_old_seconds(7 * 86400));
        assert!(is_old_seconds(7 * 86400 + 1));
        assert!(is_old_seconds(30 * 86400));
    }
}
