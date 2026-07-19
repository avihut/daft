//! Shared column formatting functions for worktree list output.
//!
//! These formatters produce plain or ANSI-colored strings used by both the
//! `tabled`-based CLI table (`list.rs`) and the ratatui TUI table (`tui.rs`).

use crate::core::worktree::forge_ref::{ForgePrLookup, PrDecoration, PrStatus};
use crate::core::worktree::list::{Stat, WorktreeInfo};
use crate::styles;
use pathdiff::diff_paths;
use std::path::Path;

/// Format ahead/behind counts as `+N -N`, with optional color.
pub fn format_ahead_behind(ahead: Option<usize>, behind: Option<usize>, use_color: bool) -> String {
    let mut parts = Vec::new();

    if let Some(a) = ahead
        && a > 0
    {
        let text = format!("+{a}");
        if use_color {
            parts.push(styles::green(&text));
        } else {
            parts.push(text);
        }
    }

    if let Some(b) = behind
        && b > 0
    {
        let text = format!("-{b}");
        if use_color {
            parts.push(styles::red(&text));
        } else {
            parts.push(text);
        }
    }

    parts.join(" ")
}

/// Format head status indicators: `+` staged, `-` unstaged, `?` untracked.
pub fn format_head_status(
    conflicted: usize,
    staged: usize,
    unstaged: usize,
    untracked: usize,
    use_color: bool,
) -> String {
    let mut parts = Vec::new();

    // Conflicts lead: they are the one state in this cell that blocks the
    // user rather than merely describing work, and bold red separates them
    // from the plain red of unstaged changes.
    if conflicted > 0 {
        let text = format!("!{conflicted}");
        if use_color {
            parts.push(styles::bold_red(&text));
        } else {
            parts.push(text);
        }
    }

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

    if let Some(a) = ahead
        && a > 0
    {
        let text = format!("\u{21E1}{a}");
        if use_color {
            parts.push(styles::green(&text));
        } else {
            parts.push(text);
        }
    }

    if let Some(b) = behind
        && b > 0
    {
        let text = format!("\u{21E3}{b}");
        if use_color {
            parts.push(styles::red(&text));
        } else {
            parts.push(text);
        }
    }

    parts.join(" ")
}

/// State machine shared by [`strip_ansi`] / [`visible_width`]: feed chars,
/// get back whether each is visible. Handles CSI-style sequences (`ESC [ … m`,
/// terminated by an ASCII letter) and OSC sequences (`ESC ] … BEL` or
/// `ESC ] … ESC \`) — the latter is what OSC 8 terminal hyperlinks use, whose
/// URL payload would otherwise leak into width math at the first letter.
#[derive(Default)]
struct AnsiScanner {
    state: AnsiState,
}

#[derive(Default, PartialEq)]
enum AnsiState {
    #[default]
    Text,
    /// Saw ESC; the next char picks the sequence family.
    Escape,
    /// Inside a CSI-style sequence; ends at an ASCII letter.
    Csi,
    /// Inside an OSC payload; ends at BEL or the ST (`ESC \`) terminator.
    Osc,
    /// Saw ESC inside an OSC payload; the next char completes the ST.
    OscEscape,
}

impl AnsiScanner {
    /// Advance over `c`; `true` means the char is visible text.
    fn visible(&mut self, c: char) -> bool {
        use AnsiState::*;
        match self.state {
            Text => {
                if c == '\x1b' {
                    self.state = Escape;
                    false
                } else {
                    true
                }
            }
            Escape => {
                self.state = if c == ']' { Osc } else { Csi };
                false
            }
            Csi => {
                if c.is_ascii_alphabetic() {
                    self.state = Text;
                }
                false
            }
            Osc => {
                match c {
                    '\x07' => self.state = Text,
                    '\x1b' => self.state = OscEscape,
                    _ => {}
                }
                false
            }
            OscEscape => {
                self.state = Text;
                false
            }
        }
    }
}

/// Strip ANSI escape sequences (CSI styling and OSC payloads, including OSC 8
/// hyperlinks) from a string.
///
/// Used for measuring the *visible* width of a styled string — width-based
/// layout code must not count escape bytes.
pub fn strip_ansi(s: &str) -> String {
    let mut result = String::with_capacity(s.len());
    let mut scanner = AnsiScanner::default();
    for c in s.chars() {
        if scanner.visible(c) {
            result.push(c);
        }
    }
    result
}

/// Count the *visible* width of a string in chars, ignoring ANSI escapes
/// (CSI styling and OSC payloads, including OSC 8 hyperlinks).
///
/// Equivalent to `strip_ansi(s).chars().count()` but walks the string in a
/// single pass without allocating.
pub fn visible_width(s: &str) -> usize {
    let mut count = 0usize;
    let mut scanner = AnsiScanner::default();
    for c in s.chars() {
        if scanner.visible(c) {
            count += 1;
        }
    }
    count
}

/// Pad `s` with trailing spaces so its *visible* width reaches `target`.
/// If `s` already meets or exceeds `target`, returns it unchanged.
///
/// "Visible width" is the char count after `strip_ansi`. ANSI escape bytes
/// are not counted.
pub fn pad_to_visible_width(s: &str, target: usize) -> String {
    let visible = visible_width(s);
    if visible >= target {
        s.to_string()
    } else {
        let mut out = String::with_capacity(s.len() + (target - visible));
        out.push_str(s);
        for _ in 0..(target - visible) {
            out.push(' ');
        }
        out
    }
}

/// Convert seconds elapsed into a compact shorthand string.
///
/// Examples: `<1m`, `5m`, `3h`, `2d`, `3w`, `5mo`, `2y`.
pub fn shorthand_from_seconds(secs: i64) -> String {
    if secs < 0 {
        // Negative inputs are clock skew; clamp to "just now".
        return "0s".to_string();
    }
    let minutes = secs / 60;
    let hours = secs / 3600;
    let days = secs / 86400;
    let weeks = days / 7;
    let months = days / 30;
    let years = days / 365;

    if minutes < 1 {
        format!("{secs}s")
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

/// Display form of a machine-wide path (catalog repos, their worktrees):
/// relative to the cwd (same `relative_display_path` as `daft list`) when
/// that form is no longer than the `~`-abbreviated absolute one, which stays
/// the fallback — a global catalog can list repos far from the cwd, where
/// pure relativization degenerates into `../../..` chains. Structured output
/// keeps raw paths. Callers pass a canonicalized cwd: catalog rows store
/// canonical paths, so a symlinked cwd (macOS `/tmp`) would never relativize.
pub fn display_path(path: &str, cwd: Option<&Path>) -> String {
    let tilde = tilde_path(path);
    let Some(cwd) = cwd else {
        return tilde;
    };
    let relative = relative_display_path(Path::new(path), cwd, cwd);
    if relative.chars().count() <= tilde.chars().count() {
        relative
    } else {
        tilde
    }
}

/// Abbreviate `$HOME` to `~` for display. Structured output keeps raw paths.
pub fn tilde_path(path: &str) -> String {
    if let Some(home) = dirs::home_dir()
        && let Ok(rest) = Path::new(path).strip_prefix(&home)
    {
        return if rest.as_os_str().is_empty() {
            "~".to_string()
        } else {
            format!("~/{}", rest.display())
        };
    }
    path.to_string()
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
    pub pr: String,
    /// Status behind the `pr` cell — color-capable renderers color the number
    /// by it (the glyph is already in `pr` when the context has no colors).
    pub pr_status: Option<PrStatus>,
    /// Web URL for the cell's PR — plain-print renderers wrap the cell in an
    /// OSC 8 terminal hyperlink.
    pub pr_url: Option<String>,
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
    /// Forge-PR cache decorations for the PR column (outbound PR numbers +
    /// statuses). `None` when the column isn't selected or no cache exists —
    /// the cell then falls back to config-recorded refs only.
    pub forge_prs: Option<&'a ForgePrLookup>,
    /// Whether the consuming renderer applies color. Colored renderers carry
    /// the PR status in the number's color alone; colorless ones get the
    /// status glyph appended to the cell text (`#723 ✓`) so the signal
    /// survives `NO_COLOR` and pipes.
    pub colors: bool,
}

/// Format head status using line-level counts: combined staged+unstaged
/// insertions/deletions plus untracked file count.
pub fn format_head_status_lines(info: &WorktreeInfo) -> String {
    let ins = info.staged_lines_inserted.unwrap_or(0) + info.unstaged_lines_inserted.unwrap_or(0);
    let del = info.staged_lines_deleted.unwrap_or(0) + info.unstaged_lines_deleted.unwrap_or(0);
    let mut parts = Vec::new();
    // A conflict count is a file count, not a line count — but it stays in
    // this cell in `--stat lines` too. Choosing a stat mode should not make
    // "these files need a decision" disappear.
    if info.conflicted > 0 {
        parts.push(format!("!{}", info.conflicted));
    }
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
            format_head_status(
                info.conflicted,
                info.staged,
                info.unstaged,
                info.untracked,
                false,
            ),
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

    let hash = info.last_commit_hash.clone().unwrap_or_default();

    // PR cell: config-recorded ref (inbound checkout), else an outbound match
    // from the forge cache. In a colored context the number alone is the text
    // (color carries the status); colorless contexts get the status glyph
    // appended (`#723 ✓`) so the signal never exists as color alone.
    let decoration = match ctx.forge_prs {
        Some(lookup) => lookup.decorate(&info.name, info.forge_ref),
        None => info.forge_ref.map(PrDecoration::bare),
    };

    // Owner cell: branch and synthesized rows with a PR show the PR's author
    // — the forge's answer to "whose PR" beats the branch-history heuristic,
    // and it's present at seed (identity) where the deduced owner streams in
    // later. Worktree rows are exempt (mirroring `pr_rows::apply_pr_owners`):
    // they describe the local checkout, whose deduced identity is the richer
    // answer. Undecorated rows keep the deduced owner.
    let owner = (info.kind != crate::core::worktree::list::EntryKind::Worktree)
        .then(|| decoration.as_ref().and_then(|d| d.author.clone()))
        .flatten()
        .or_else(|| info.owner.as_ref().map(|o| o.name.clone()))
        .unwrap_or_default();
    let (pr, pr_status, pr_url) = match decoration {
        Some(d) => {
            let text = match d.status.map(PrStatus::glyph) {
                Some(glyph) if !ctx.colors && !glyph.is_empty() => {
                    format!("{} {}", d.r.short(), glyph)
                }
                _ => d.r.short(),
            };
            (text, d.status, d.url)
        }
        None => (String::new(), None, None),
    };

    ColumnValues {
        branch,
        path,
        size,
        base,
        changes,
        remote,
        pr,
        pr_status,
        pr_url,
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
    fn pr_column_value_reflects_the_tracked_ref() {
        use crate::core::worktree::forge_ref::{ForgeBranchRef, ForgeRefKind};
        use crate::core::worktree::list::WorktreeInfo;
        let ctx = ColumnContext {
            project_root: Path::new("/"),
            cwd: Path::new("/"),
            now: 0,
            stat: Stat::Summary,
            forge_prs: None,
            colors: false,
        };
        let mut info = WorktreeInfo::empty("feat");

        info.forge_ref = Some(ForgeBranchRef::new(ForgeRefKind::GithubPr, 123));
        assert_eq!(compute_column_values(&info, &ctx).pr, "#123");

        info.forge_ref = Some(ForgeBranchRef::new(ForgeRefKind::GitlabMr, 45));
        assert_eq!(compute_column_values(&info, &ctx).pr, "!45");

        info.forge_ref = None;
        assert_eq!(compute_column_values(&info, &ctx).pr, "");
    }

    fn forge_test_lookup() -> ForgePrLookup {
        use crate::core::worktree::forge_ref::{CiStatus, ForgeBranchRef, ForgeRefKind};

        let inbound = ForgeBranchRef::new(ForgeRefKind::GithubPr, 7);
        let outbound = ForgeBranchRef::new(ForgeRefKind::GithubPr, 723);
        let mut lookup = ForgePrLookup::default();
        lookup.by_ref.insert(
            inbound,
            PrDecoration {
                r: inbound,
                status: Some(PrStatus::Ci(CiStatus::Fail)),
                url: Some("https://github.com/acme/widget/pull/7".into()),
                author: None,
            },
        );
        lookup.by_branch.insert(
            "daft-127/feat".into(),
            PrDecoration {
                r: outbound,
                status: Some(PrStatus::Ci(CiStatus::Pass)),
                url: Some("https://github.com/acme/widget/pull/723".into()),
                author: None,
            },
        );
        lookup.by_branch.insert(
            "feat/done".into(),
            PrDecoration {
                r: ForgeBranchRef::new(ForgeRefKind::GithubPr, 6),
                status: Some(PrStatus::Merged),
                url: None,
                author: None,
            },
        );
        lookup
    }

    #[test]
    fn pr_column_appends_glyphs_when_colorless() {
        use crate::core::worktree::forge_ref::{CiStatus, ForgeBranchRef, ForgeRefKind};
        use crate::core::worktree::list::WorktreeInfo;

        let lookup = forge_test_lookup();
        let ctx = ColumnContext {
            project_root: Path::new("/"),
            cwd: Path::new("/"),
            now: 0,
            stat: Stat::Summary,
            forge_prs: Some(&lookup),
            colors: false,
        };

        // Outbound: a plain local branch gains its open PR + CI glyph.
        let info = WorktreeInfo::empty("daft-127/feat");
        let vals = compute_column_values(&info, &ctx);
        assert_eq!(vals.pr, "#723 \u{2713}");
        assert_eq!(vals.pr_status, Some(PrStatus::Ci(CiStatus::Pass)));
        assert_eq!(
            vals.pr_url.as_deref(),
            Some("https://github.com/acme/widget/pull/723")
        );

        // A merged PR marks its branch with the merged glyph.
        let info = WorktreeInfo::empty("feat/done");
        let vals = compute_column_values(&info, &ctx);
        assert_eq!(vals.pr, "#6 \u{25c6}");
        assert_eq!(vals.pr_status, Some(PrStatus::Merged));

        // Inbound: the config ref is authoritative, cache only adds status.
        let mut info = WorktreeInfo::empty("contributor-feature");
        info.forge_ref = Some(ForgeBranchRef::new(ForgeRefKind::GithubPr, 7));
        let vals = compute_column_values(&info, &ctx);
        assert_eq!(vals.pr, "#7 \u{2717}");
        assert_eq!(vals.pr_status, Some(PrStatus::Ci(CiStatus::Fail)));

        // No match anywhere: empty cell.
        let info = WorktreeInfo::empty("plain-branch");
        assert_eq!(compute_column_values(&info, &ctx).pr, "");
    }

    #[test]
    fn owner_cell_prefers_pr_author_except_for_worktree_rows() {
        use crate::core::ownership::BranchOwner;
        use crate::core::worktree::forge_ref::{ForgeBranchRef, ForgeRefKind};
        use crate::core::worktree::list::{EntryKind, WorktreeInfo};

        let mut lookup = ForgePrLookup::default();
        let r = ForgeBranchRef::new(ForgeRefKind::GithubPr, 5);
        lookup.by_branch.insert(
            "feature-x".into(),
            PrDecoration {
                author: Some("dan".into()),
                ..PrDecoration::bare(r)
            },
        );
        let ctx = ColumnContext {
            project_root: Path::new("/"),
            cwd: Path::new("/"),
            now: 0,
            stat: Stat::Summary,
            forge_prs: Some(&lookup),
            colors: true,
        };
        let deduced = BranchOwner {
            name: "History Name".into(),
            email: "h@x".into(),
            is_current_user: true,
        };

        // A branch row with a PR shows the PR's author.
        let mut branch_row = WorktreeInfo::empty("feature-x");
        branch_row.kind = EntryKind::LocalBranch;
        branch_row.owner = Some(deduced.clone());
        assert_eq!(compute_column_values(&branch_row, &ctx).owner, "dan");

        // The same PR on a worktree row decorates the pr cell only — the
        // local checkout keeps its deduced identity (mirrors
        // `pr_rows::apply_pr_owners`).
        let mut worktree_row = WorktreeInfo::empty("feature-x");
        worktree_row.owner = Some(deduced);
        let vals = compute_column_values(&worktree_row, &ctx);
        assert_eq!(vals.owner, "History Name");
        assert_eq!(vals.pr, "#5", "the pr cell still decorates");
    }

    #[test]
    fn pr_column_is_bare_number_when_colored() {
        use crate::core::worktree::forge_ref::{CiStatus, PrStatus};
        use crate::core::worktree::list::WorktreeInfo;

        let lookup = forge_test_lookup();
        let ctx = ColumnContext {
            project_root: Path::new("/"),
            cwd: Path::new("/"),
            now: 0,
            stat: Stat::Summary,
            forge_prs: Some(&lookup),
            colors: true,
        };

        // Color carries the status: the text is the number alone, the status
        // rides in `pr_status` for the renderer's ink.
        let vals = compute_column_values(&WorktreeInfo::empty("daft-127/feat"), &ctx);
        assert_eq!(vals.pr, "#723");
        assert_eq!(vals.pr_status, Some(PrStatus::Ci(CiStatus::Pass)));

        let vals = compute_column_values(&WorktreeInfo::empty("feat/done"), &ctx);
        assert_eq!(vals.pr, "#6");
        assert_eq!(vals.pr_status, Some(PrStatus::Merged));
    }

    /// Regression: repo list showed absolute paths where `daft list`
    /// relativizes. Display paths prefer the cwd-relative form (same helper
    /// as `daft list`) unless the tilde-absolute form is shorter — a global
    /// catalog lists repos far from the cwd, where relativization
    /// degenerates into ../-chains.
    #[test]
    fn display_path_prefers_the_shorter_of_relative_and_tilde() {
        let cwd = Path::new("/tmp/sandbox/test");
        assert_eq!(display_path("/tmp/sandbox/test/api", Some(cwd)), "api");
        assert_eq!(
            display_path("/tmp/sandbox/test/api/main", Some(cwd)),
            "api/main"
        );
        assert_eq!(display_path("/tmp/sandbox/test", Some(cwd)), ".");
        assert_eq!(
            display_path(
                "/tmp/sandbox/test",
                Some(Path::new("/tmp/sandbox/test/api"))
            ),
            ".."
        );
        assert_eq!(
            display_path("/opt/elsewhere", None),
            "/opt/elsewhere",
            "no cwd falls back to the tilde/absolute form"
        );
        if let Some(home) = dirs::home_dir() {
            let repo = format!("{}/src/api", home.display());
            assert_eq!(
                display_path(&repo, Some(Path::new("/tmp/sandbox/deeply/nested/dir"))),
                "~/src/api",
                "far from the cwd the tilde form is shorter than a ../-chain"
            );
        }
    }

    #[test]
    fn tilde_path_abbreviates_home_and_passes_others_through() {
        if let Some(home) = dirs::home_dir() {
            let inside = format!("{}/src/thing", home.display());
            assert_eq!(tilde_path(&inside), "~/src/thing");
            assert_eq!(tilde_path(&home.display().to_string()), "~");
        }
        assert_eq!(tilde_path("/opt/elsewhere"), "/opt/elsewhere");
    }

    #[test]
    fn test_shorthand_from_seconds_sub_minute() {
        assert_eq!(shorthand_from_seconds(0), "0s");
        assert_eq!(shorthand_from_seconds(1), "1s");
        assert_eq!(shorthand_from_seconds(30), "30s");
        assert_eq!(shorthand_from_seconds(59), "59s");
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
    fn test_shorthand_from_seconds_negative_clamps_to_zero() {
        assert_eq!(shorthand_from_seconds(-1), "0s");
        assert_eq!(shorthand_from_seconds(-100), "0s");
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

    #[test]
    fn strip_ansi_removes_csi_sequences() {
        assert_eq!(strip_ansi("\x1b[2mhello\x1b[0m"), "hello");
        assert_eq!(strip_ansi("\x1b[38;5;208mwarn\x1b[0m"), "warn");
        assert_eq!(strip_ansi("plain"), "plain");
        assert_eq!(strip_ansi(""), "");
    }

    #[test]
    fn strip_ansi_removes_osc8_hyperlinks() {
        // ST-terminated (ESC \) — what term_styles::hyperlink emits. The URL
        // payload is full of letters that must not leak into the visible text.
        let linked = "\x1b]8;;https://github.com/acme/widget/pull/723\x1b\\#723\x1b]8;;\x1b\\";
        assert_eq!(strip_ansi(linked), "#723");
        assert_eq!(visible_width(linked), 4);

        // BEL-terminated variant.
        let bel = "\x1b]8;;https://a.com\x07link\x1b]8;;\x07";
        assert_eq!(strip_ansi(bel), "link");
        assert_eq!(visible_width(bel), 4);

        // A styled link: color inside the hyperlink wrapper.
        let styled = "\x1b]8;;https://a.com\x1b\\\x1b[32m#5\x1b[0m\x1b]8;;\x1b\\";
        assert_eq!(strip_ansi(styled), "#5");
        assert_eq!(visible_width(styled), 2);
    }

    #[test]
    fn strip_ansi_preserves_unicode_glyphs() {
        // Box-drawing and arrows must survive — these are core to the timeline
        // display.
        assert_eq!(strip_ansi("\x1b[2m│\x1b[0m"), "│");
        assert_eq!(
            strip_ansi("\x1b[38;5;208m\u{2192}\x1b[0m install"),
            "\u{2192} install",
        );
    }

    #[test]
    fn visible_width_strips_csi_sequences() {
        assert_eq!(visible_width("\x1b[2mhello\x1b[0m"), 5);
        assert_eq!(visible_width("\x1b[38;5;208mwarn\x1b[0m"), 4);
        assert_eq!(visible_width("plain"), 5);
        assert_eq!(visible_width(""), 0);
    }

    #[test]
    fn visible_width_preserves_unicode_glyphs() {
        // Box-drawing and arrows must count as visible chars.
        assert_eq!(visible_width("\x1b[2m│\x1b[0m"), 1);
        assert_eq!(
            visible_width("\x1b[38;5;208m\u{2192}\x1b[0m install"),
            "\u{2192} install".chars().count(),
        );
    }

    #[test]
    fn visible_width_matches_strip_ansi_chars_count() {
        let cases = [
            "",
            "plain",
            "\x1b[31mfailed!\x1b[0m",
            "\x1b[2m│\x1b[0m   row",
            "\x1b[38;5;208m\u{27f3} running (stale)\x1b[0m",
        ];
        for c in cases {
            assert_eq!(
                visible_width(c),
                strip_ansi(c).chars().count(),
                "mismatch for input: {c:?}",
            );
        }
    }

    #[test]
    fn pad_to_visible_width_no_pad_when_already_at_or_above_target() {
        assert_eq!(pad_to_visible_width("abc", 3), "abc");
        assert_eq!(pad_to_visible_width("abcd", 3), "abcd");
    }

    #[test]
    fn pad_to_visible_width_appends_trailing_spaces_to_reach_target() {
        assert_eq!(pad_to_visible_width("ab", 5), "ab   ");
    }

    #[test]
    fn pad_to_visible_width_counts_visible_chars_not_ansi_bytes() {
        // Cell with red wrapping reports raw len 14 but visible len 7.
        let cell = "\x1b[31mfailed!\x1b[0m";
        let padded = pad_to_visible_width(cell, 10);
        // Visible width must be exactly 10 after padding.
        assert_eq!(strip_ansi(&padded).chars().count(), 10);
        // ANSI bytes preserved at the start; trailing spaces appended after RESET.
        assert!(padded.starts_with("\x1b[31mfailed!\x1b[0m"));
        assert!(padded.ends_with("   "));
    }
}
