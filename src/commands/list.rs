use crate::{
    core::{
        repo::{get_current_worktree_path, get_git_common_dir, get_project_root},
        worktree::list::{collect_worktree_info, Stat},
    },
    git::GitCommand,
    is_git_repository,
    logging::init_logging,
    output::{CliOutput, Output, OutputConfig},
    remote::get_default_branch_local,
    settings::DaftSettings,
    styles,
};
use anyhow::Result;
use chrono::Utc;
use clap::Parser;
use pathdiff::diff_paths;
use tabled::{
    builder::Builder,
    settings::{object::Columns, peaker::Priority, Padding, Style, Width},
};
use terminal_size::{terminal_size, Width as TermWidth};

#[derive(Parser)]
#[command(name = "git-worktree-list")]
#[command(version = crate::VERSION)]
#[command(about = "List all worktrees with status information")]
#[command(long_about = r#"
Lists all worktrees in the current project with enriched status information
including uncommitted changes, ahead/behind counts vs. both the base branch
and the remote tracking branch, branch age, and last commit details.

Each worktree is shown with:
  - A `>` marker for the current worktree
  - Branch name, with `◉` for the default branch
  - Relative path from the current directory
  - Ahead/behind counts vs. the base branch (e.g. +3 -1)
  - File status: +N staged, -N unstaged, ?N untracked
  - Remote tracking status: ⇡N unpushed, ⇣N unpulled
  - Branch age since creation (e.g. 3d, 2w, 5mo)
  - Last commit: shorthand age + subject (e.g. 1h fix login bug)

Ages use shorthand notation: <1m, Xm, Xh, Xd, Xw, Xmo, Xy.

Use --stat lines to show line-level change counts (insertions and deletions)
instead of the default summary (commit counts for base/remote, file counts for
changes). This is slower as it requires computing diffs for each worktree.

Use --json for machine-readable output suitable for scripting.
"#)]
pub struct Args {
    #[arg(long, help = "Output in JSON format")]
    json: bool,

    #[arg(short, long, help = "Be verbose; show detailed progress")]
    verbose: bool,

    #[arg(
        long,
        value_enum,
        default_value_t = Stat::Summary,
        help = "Statistics mode: summary (default) or lines"
    )]
    stat: Stat,
}

/// A row in the worktree list table.
struct TableRow {
    /// Annotation column: current marker (">") and/or default branch indicator ("◉").
    annotation: String,
    /// Branch name.
    name: String,
    /// Relative path from current directory.
    path: String,
    /// Ahead/behind base branch (e.g. "+3 -1").
    base: String,
    /// Worktrunk-style status indicators (e.g. "+3 -2 ?1").
    head: String,
    /// Ahead/behind remote tracking branch (e.g. "⇡1 ⇣2").
    remote: String,
    /// Branch age since creation (shorthand).
    branch_age: String,
    /// Last commit: shorthand age + subject combined.
    last_commit: String,
}

pub fn run() -> Result<()> {
    let args = Args::parse_from(crate::get_clap_args("git-worktree-list"));

    init_logging(args.verbose);

    if !is_git_repository()? {
        anyhow::bail!("Not inside a Git repository");
    }

    let settings = DaftSettings::load()?;
    let git = GitCommand::new(false).with_gitoxide(settings.use_gitoxide);
    let git_common_dir = get_git_common_dir()?;
    let base_branch = get_default_branch_local(&git_common_dir, "origin", settings.use_gitoxide)
        .unwrap_or_else(|_| "master".to_string());
    let current_path = get_current_worktree_path()
        .ok()
        .and_then(|p| p.canonicalize().ok());
    let project_root = get_project_root()?;

    let cwd = std::env::current_dir().unwrap_or_else(|_| project_root.clone());
    let infos = if args.stat == Stat::Lines {
        let mut output = CliOutput::new(OutputConfig::new(false, args.verbose));
        output.start_spinner("Computing line statistics...");
        let result = collect_worktree_info(&git, &base_branch, current_path.as_deref(), args.stat)?;
        output.finish_spinner();
        result
    } else {
        collect_worktree_info(&git, &base_branch, current_path.as_deref(), args.stat)?
    };

    if args.json {
        return print_json(&infos, &project_root, &cwd, args.stat);
    }

    print_table(&infos, &project_root, &cwd, args.stat);
    Ok(())
}

fn print_json(
    infos: &[crate::core::worktree::list::WorktreeInfo],
    project_root: &std::path::Path,
    cwd: &std::path::Path,
    stat: Stat,
) -> Result<()> {
    let now = Utc::now().timestamp();
    let entries: Vec<serde_json::Value> = infos
        .iter()
        .map(|info| {
            let rel_path = relative_display_path(&info.path, project_root, cwd);
            let last_commit_age = info
                .last_commit_timestamp
                .map(|ts| shorthand_from_seconds(now - ts))
                .unwrap_or_default();
            let branch_age = info
                .branch_creation_timestamp
                .map(|ts| shorthand_from_seconds(now - ts))
                .unwrap_or_default();
            let mut entry = serde_json::json!({
                "name": info.name,
                "path": rel_path,
                "is_current": info.is_current,
                "is_default_branch": info.is_default_branch,
                "ahead": info.ahead,
                "behind": info.behind,
                "staged": info.staged,
                "unstaged": info.unstaged,
                "untracked": info.untracked,
                "remote_ahead": info.remote_ahead,
                "remote_behind": info.remote_behind,
                "last_commit_age": last_commit_age,
                "last_commit_subject": info.last_commit_subject,
                "branch_age": branch_age,
            });
            if stat == Stat::Lines {
                let obj = entry.as_object_mut().unwrap();
                obj.insert(
                    "base_lines_inserted".into(),
                    serde_json::json!(info.base_lines_inserted),
                );
                obj.insert(
                    "base_lines_deleted".into(),
                    serde_json::json!(info.base_lines_deleted),
                );
                obj.insert(
                    "staged_lines_inserted".into(),
                    serde_json::json!(info.staged_lines_inserted),
                );
                obj.insert(
                    "staged_lines_deleted".into(),
                    serde_json::json!(info.staged_lines_deleted),
                );
                obj.insert(
                    "unstaged_lines_inserted".into(),
                    serde_json::json!(info.unstaged_lines_inserted),
                );
                obj.insert(
                    "unstaged_lines_deleted".into(),
                    serde_json::json!(info.unstaged_lines_deleted),
                );
                obj.insert(
                    "remote_lines_inserted".into(),
                    serde_json::json!(info.remote_lines_inserted),
                );
                obj.insert(
                    "remote_lines_deleted".into(),
                    serde_json::json!(info.remote_lines_deleted),
                );
            }
            entry
        })
        .collect();

    println!("{}", serde_json::to_string_pretty(&entries)?);
    Ok(())
}

fn print_table(
    infos: &[crate::core::worktree::list::WorktreeInfo],
    project_root: &std::path::Path,
    cwd: &std::path::Path,
    stat: Stat,
) {
    if infos.is_empty() {
        return;
    }

    let use_color = styles::colors_enabled();
    let now = Utc::now().timestamp();

    // Determine which annotation types exist across all rows
    let has_any_current = infos.iter().any(|i| i.is_current);
    let has_any_default = infos.iter().any(|i| i.is_default_branch);

    // Pre-compute max visible width of commit ages to align subjects
    let max_commit_age_width = infos
        .iter()
        .filter_map(|info| {
            info.last_commit_timestamp
                .map(|ts| shorthand_from_seconds(now - ts).len())
        })
        .max()
        .unwrap_or(0);

    let rows: Vec<TableRow> = infos
        .iter()
        .map(|info| {
            // Build annotation: ">" first (cyan), then "◉" (dark gray)
            let mut annotation = String::new();
            if has_any_current {
                if info.is_current {
                    if use_color {
                        annotation.push_str(&styles::cyan(">"));
                    } else {
                        annotation.push('>');
                    }
                } else {
                    annotation.push(' ');
                }
                if has_any_default {
                    annotation.push(' ');
                }
            }
            if has_any_default {
                if info.is_default_branch {
                    if use_color {
                        annotation.push_str(&styles::dark_gray("\u{25C9}"));
                    } else {
                        annotation.push('\u{25C9}');
                    }
                } else {
                    annotation.push(' ');
                }
            }

            let name = info.name.clone();

            let rel_path = relative_display_path(&info.path, project_root, cwd);

            let base = if stat == Stat::Lines {
                format_ahead_behind(info.base_lines_inserted, info.base_lines_deleted, use_color)
            } else {
                format_ahead_behind(info.ahead, info.behind, use_color)
            };

            let head = if stat == Stat::Lines {
                let ins = info.staged_lines_inserted.unwrap_or(0)
                    + info.unstaged_lines_inserted.unwrap_or(0);
                let del = info.staged_lines_deleted.unwrap_or(0)
                    + info.unstaged_lines_deleted.unwrap_or(0);
                let mut parts = Vec::new();
                if ins > 0 {
                    let text = format!("+{ins}");
                    if use_color {
                        parts.push(styles::green(&text));
                    } else {
                        parts.push(text);
                    }
                }
                if del > 0 {
                    let text = format!("-{del}");
                    if use_color {
                        parts.push(styles::red(&text));
                    } else {
                        parts.push(text);
                    }
                }
                if info.untracked > 0 {
                    let text = format!("?{}", info.untracked);
                    if use_color {
                        parts.push(styles::dim(&text));
                    } else {
                        parts.push(text);
                    }
                }
                parts.join(" ")
            } else {
                format_head_status(info.staged, info.unstaged, info.untracked, use_color)
            };

            let remote = if stat == Stat::Lines {
                format_ahead_behind(
                    info.remote_lines_inserted,
                    info.remote_lines_deleted,
                    use_color,
                )
            } else {
                format_remote_status(info.remote_ahead, info.remote_behind, use_color)
            };

            let branch_age = format_shorthand_age(info.branch_creation_timestamp, now, use_color);

            // Combine last commit age + subject into one column, with age right-padded for alignment
            let commit_age = format_shorthand_age(info.last_commit_timestamp, now, use_color);
            let commit_age_visible_len = info
                .last_commit_timestamp
                .map(|ts| shorthand_from_seconds(now - ts).len())
                .unwrap_or(0);
            let last_commit = if commit_age.is_empty() {
                info.last_commit_subject.clone()
            } else if info.last_commit_subject.is_empty() {
                commit_age
            } else {
                let pad = " ".repeat(max_commit_age_width - commit_age_visible_len);
                format!("{commit_age}{pad} {}", info.last_commit_subject)
            };

            TableRow {
                annotation,
                name,
                path: rel_path,
                base,
                head,
                remote,
                branch_age,
                last_commit,
            }
        })
        .collect();

    let has_annotations = has_any_current || has_any_default;

    let mut builder = Builder::new();
    let data_headers = [
        "Branch",
        "Path",
        "Base",
        "Changes",
        "Remote",
        "Age",
        "Last Commit",
    ];
    let header: Vec<String> = if has_annotations {
        std::iter::once("".to_string())
            .chain(data_headers.iter().map(|h| {
                if use_color {
                    styles::dim(h)
                } else {
                    h.to_string()
                }
            }))
            .collect()
    } else {
        data_headers
            .iter()
            .map(|h| {
                if use_color {
                    styles::dim(h)
                } else {
                    h.to_string()
                }
            })
            .collect()
    };
    builder.push_record(header);
    for row in &rows {
        let data_cols: Vec<&str> = vec![
            &row.name,
            &row.path,
            &row.base,
            &row.head,
            &row.remote,
            &row.branch_age,
            &row.last_commit,
        ];
        if has_annotations {
            let mut record = vec![row.annotation.as_str()];
            record.extend(data_cols);
            builder.push_record(record);
        } else {
            builder.push_record(data_cols);
        }
    }

    let mut table = builder.build();
    table.with(Style::blank());
    table.modify(Columns::first(), Padding::new(1, 0, 0, 0));

    if let Some((TermWidth(width), _)) = terminal_size() {
        table.with(
            Width::truncate(width as usize)
                .suffix("...")
                .priority(Priority::max(true)),
        );
    }

    println!("{table}");
}

/// Compute a display path relative to cwd, falling back to project-root-relative.
fn relative_display_path(
    abs_path: &std::path::Path,
    project_root: &std::path::Path,
    cwd: &std::path::Path,
) -> String {
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

fn format_ahead_behind(ahead: Option<usize>, behind: Option<usize>, use_color: bool) -> String {
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
fn format_head_status(staged: usize, unstaged: usize, untracked: usize, use_color: bool) -> String {
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

/// Format remote status using ⇡/⇣ arrows for upstream ahead/behind.
fn format_remote_status(ahead: Option<usize>, behind: Option<usize>, use_color: bool) -> String {
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
fn shorthand_from_seconds(secs: i64) -> String {
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
fn format_shorthand_age(timestamp: Option<i64>, now: i64, use_color: bool) -> String {
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
fn is_old_seconds(secs: i64) -> bool {
    secs > 7 * 86400
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
