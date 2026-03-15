use crate::{
    core::{
        repo::{get_current_worktree_path, get_git_common_dir, get_project_root},
        worktree::list::{collect_branch_info, collect_worktree_info, EntryKind, Stat},
    },
    git::GitCommand,
    is_git_repository,
    logging::init_logging,
    output::{
        format::{
            compute_column_values, format_ahead_behind, format_head_status, format_remote_status,
            format_shorthand_age, relative_display_path, shorthand_from_seconds, ColumnContext,
        },
        CliOutput, Output, OutputConfig,
    },
    remote::get_default_branch_local,
    settings::DaftSettings,
    styles,
};
use anyhow::Result;
use chrono::Utc;
use clap::Parser;
use std::collections::HashSet;
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
  - Branch name, with `✦` for the default branch
  - Relative path from the current directory
  - Ahead/behind counts vs. the base branch (e.g. +3 -1)
  - File status: +N staged, -N unstaged, ?N untracked
  - Remote tracking status: ⇡N unpushed, ⇣N unpulled
  - Branch age since creation (e.g. 3d, 2w, 5mo)
  - Last commit: shorthand age + subject (e.g. 1h fix login bug)

Ages use shorthand notation: <1m, Xm, Xh, Xd, Xw, Xmo, Xy.

Use -b / --branches to also show local branches without a worktree.
Use -r / --remotes to also show remote tracking branches.
Use -a / --all to show both (equivalent to -b -r).

Non-worktree branches are shown with dimmed styling and blank Path/Changes columns.

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
        short = 'b',
        long = "branches",
        help = "Also show local branches without a worktree"
    )]
    branches: bool,

    #[arg(
        short = 'r',
        long = "remotes",
        help = "Also show remote tracking branches"
    )]
    remotes: bool,

    #[arg(
        short = 'a',
        long = "all",
        help = "Show all branches (equivalent to -b -r)"
    )]
    all: bool,

    #[arg(
        long,
        value_enum,
        help = "Statistics mode: summary or lines (default: from git config daft.list.stat, or summary)"
    )]
    stat: Option<Stat>,
}

/// A row in the worktree list table.
struct TableRow {
    /// Annotation column: current marker (">") and/or default branch indicator ("✦").
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
    let stat = args.stat.unwrap_or(settings.list_stat);
    let git = GitCommand::new(false).with_gitoxide(settings.use_gitoxide);
    let git_common_dir = get_git_common_dir()?;
    let base_branch = get_default_branch_local(&git_common_dir, "origin", settings.use_gitoxide)
        .unwrap_or_else(|_| "master".to_string());
    let current_path = get_current_worktree_path()
        .ok()
        .and_then(|p| p.canonicalize().ok());
    let project_root = get_project_root()?;

    let cwd = std::env::current_dir().unwrap_or_else(|_| project_root.clone());
    let show_local = args.branches || args.all;
    let show_remote = args.remotes || args.all;
    let needs_spinner = stat == Stat::Lines || show_local || show_remote;

    let infos = if needs_spinner {
        let mut output = CliOutput::new(OutputConfig::new(false, args.verbose));
        let msg = if stat == Stat::Lines {
            "Computing line statistics..."
        } else {
            "Collecting branch information..."
        };
        output.start_spinner(msg);
        let result = collect_worktree_info(&git, &base_branch, current_path.as_deref(), stat)?;
        if show_local || show_remote {
            let worktree_branches: HashSet<String> =
                result.iter().map(|i| i.name.clone()).collect();
            let branch_infos = collect_branch_info(
                &git,
                &base_branch,
                stat,
                show_local,
                show_remote,
                &worktree_branches,
                &project_root,
            )?;
            let mut merged = result;
            merged.extend(branch_infos);
            merged.sort_by(|a, b| a.name.to_lowercase().cmp(&b.name.to_lowercase()));
            output.finish_spinner();
            merged
        } else {
            output.finish_spinner();
            result
        }
    } else {
        collect_worktree_info(&git, &base_branch, current_path.as_deref(), stat)?
    };

    if args.json {
        return print_json(&infos, &project_root, &cwd, stat);
    }

    print_table(&infos, &project_root, &cwd, stat);
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
            let rel_path = info
                .path
                .as_ref()
                .map(|p| relative_display_path(p, project_root, cwd));
            let last_commit_age = info
                .last_commit_timestamp
                .map(|ts| shorthand_from_seconds(now - ts))
                .unwrap_or_default();
            let branch_age = info
                .branch_creation_timestamp
                .map(|ts| shorthand_from_seconds(now - ts))
                .unwrap_or_default();
            let kind = match info.kind {
                EntryKind::Worktree => "worktree",
                EntryKind::LocalBranch => "branch",
                EntryKind::RemoteBranch => "remote",
            };
            let mut entry = serde_json::json!({
                "kind": kind,
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

    let col_ctx = ColumnContext {
        project_root,
        cwd,
        now,
        stat,
    };

    // Pre-compute plain column values for alignment and reuse
    let col_vals: Vec<_> = infos
        .iter()
        .map(|i| compute_column_values(i, &col_ctx))
        .collect();

    // Max visible width of commit ages (for subject alignment)
    let max_commit_age_width = col_vals
        .iter()
        .map(|v| v.last_commit_age.len())
        .max()
        .unwrap_or(0);

    let rows: Vec<TableRow> = infos
        .iter()
        .zip(col_vals.iter())
        .map(|(info, vals)| {
            // Build annotation: ">" first (cyan), then "✦" (dark gray)
            let mut annotation = String::new();
            if has_any_current {
                if info.is_current {
                    if use_color {
                        annotation.push_str(&styles::cyan(styles::CURRENT_WORKTREE_SYMBOL));
                    } else {
                        annotation.push_str(styles::CURRENT_WORKTREE_SYMBOL);
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
                        annotation.push_str(&styles::bright_purple(styles::DEFAULT_BRANCH_SYMBOL));
                    } else {
                        annotation.push_str(styles::DEFAULT_BRANCH_SYMBOL);
                    }
                } else {
                    annotation.push(' ');
                }
            }

            // Stat::Lines mode overrides base/changes/remote with line-level counts
            let (base, head, remote) = if stat == Stat::Lines {
                let base = format_ahead_behind(
                    info.base_lines_inserted,
                    info.base_lines_deleted,
                    use_color,
                );

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
                let head = parts.join(" ");

                let remote = format_ahead_behind(
                    info.remote_lines_inserted,
                    info.remote_lines_deleted,
                    use_color,
                );

                (base, head, remote)
            } else {
                (
                    format_ahead_behind(info.ahead, info.behind, use_color),
                    format_head_status(info.staged, info.unstaged, info.untracked, use_color),
                    format_remote_status(info.remote_ahead, info.remote_behind, use_color),
                )
            };

            let branch_age = format_shorthand_age(info.branch_creation_timestamp, now, use_color);

            // Combine last commit age + subject, with age right-padded for alignment
            let commit_age = format_shorthand_age(info.last_commit_timestamp, now, use_color);
            let last_commit = if vals.last_commit_age.is_empty() {
                vals.last_commit_subject.clone()
            } else if vals.last_commit_subject.is_empty() {
                commit_age
            } else {
                let pad = " ".repeat(max_commit_age_width - vals.last_commit_age.len());
                format!("{commit_age}{pad} {}", vals.last_commit_subject)
            };

            let is_non_worktree = info.kind != EntryKind::Worktree;
            if use_color && is_non_worktree {
                TableRow {
                    annotation,
                    name: styles::dim(&vals.branch),
                    path: styles::dim(&vals.path),
                    base: if base.is_empty() {
                        base
                    } else {
                        styles::dim(&strip_ansi(&base))
                    },
                    head: if head.is_empty() {
                        head
                    } else {
                        styles::dim(&strip_ansi(&head))
                    },
                    remote: if remote.is_empty() {
                        remote
                    } else {
                        styles::dim(&strip_ansi(&remote))
                    },
                    branch_age: if branch_age.is_empty() {
                        branch_age
                    } else {
                        styles::dim(&strip_ansi(&branch_age))
                    },
                    last_commit: if last_commit.is_empty() {
                        last_commit
                    } else {
                        styles::dim(&strip_ansi(&last_commit))
                    },
                }
            } else {
                TableRow {
                    annotation,
                    name: vals.branch.clone(),
                    path: vals.path.clone(),
                    base,
                    head,
                    remote,
                    branch_age,
                    last_commit,
                }
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

/// Strip ANSI escape codes from a string so it can be re-wrapped with a single style.
fn strip_ansi(s: &str) -> String {
    let mut result = String::with_capacity(s.len());
    let mut in_escape = false;
    for c in s.chars() {
        if in_escape {
            if c.is_ascii_alphabetic() {
                in_escape = false;
            }
        } else if c == '\x1b' {
            in_escape = true;
        } else {
            result.push(c);
        }
    }
    result
}
