use crate::{
    core::{
        repo::{get_current_worktree_path, get_git_common_dir, get_project_root},
        worktree::list::collect_worktree_info,
    },
    git::GitCommand,
    is_git_repository,
    logging::init_logging,
    remote::get_default_branch_local,
    settings::DaftSettings,
    styles,
};
use anyhow::Result;
use clap::Parser;
use tabled::{
    builder::Builder,
    settings::{object::Columns, Modify, Style, Width},
};

#[derive(Parser)]
#[command(name = "git-worktree-list")]
#[command(version = crate::VERSION)]
#[command(about = "List all worktrees with status information")]
#[command(long_about = r#"
Lists all worktrees in the current project with enriched status information
including ahead/behind counts relative to the base branch, dirty status,
and last commit details.

Each worktree is shown with:
  - A `>` marker for the current worktree
  - Branch name (or "(detached)" for detached HEAD)
  - Relative path from the project root
  - Ahead/behind counts vs. the base branch (e.g. +3 -1)
  - A `*` dirty marker if there are uncommitted changes
  - Relative age of the last commit
  - Subject line of the last commit (truncated to 40 chars)

Use --json for machine-readable output suitable for scripting.
"#)]
pub struct Args {
    #[arg(long, help = "Output in JSON format")]
    json: bool,

    #[arg(short, long, help = "Be verbose; show detailed progress")]
    verbose: bool,
}

/// A row in the worktree list table.
struct TableRow {
    /// Current worktree marker ("> " or "  ").
    current: String,
    /// Branch name.
    name: String,
    /// Relative path from project root.
    path: String,
    /// Ahead/behind base branch (e.g. "+3 -1").
    base: String,
    /// Dirty marker ("*" or "").
    dirty: String,
    /// Relative age of last commit.
    age: String,
    /// Last commit subject line.
    subject: String,
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
    let current_path = get_current_worktree_path()?
        .canonicalize()
        .unwrap_or_else(|_| get_current_worktree_path().unwrap_or_default());
    let project_root = get_project_root()?;

    let infos = collect_worktree_info(&git, &base_branch, &current_path)?;

    if args.json {
        return print_json(&infos, &project_root);
    }

    print_table(&infos, &project_root);
    Ok(())
}

fn print_json(
    infos: &[crate::core::worktree::list::WorktreeInfo],
    project_root: &std::path::Path,
) -> Result<()> {
    let entries: Vec<serde_json::Value> = infos
        .iter()
        .map(|info| {
            let rel_path = info
                .path
                .strip_prefix(project_root)
                .unwrap_or(&info.path)
                .display()
                .to_string();
            serde_json::json!({
                "name": info.name,
                "path": rel_path,
                "is_current": info.is_current,
                "ahead": info.ahead,
                "behind": info.behind,
                "is_dirty": info.is_dirty,
                "last_commit_age": info.last_commit_age,
                "last_commit_subject": info.last_commit_subject,
            })
        })
        .collect();

    println!("{}", serde_json::to_string_pretty(&entries)?);
    Ok(())
}

fn print_table(
    infos: &[crate::core::worktree::list::WorktreeInfo],
    project_root: &std::path::Path,
) {
    if infos.is_empty() {
        return;
    }

    let use_color = styles::colors_enabled();

    let rows: Vec<TableRow> = infos
        .iter()
        .map(|info| {
            let current = if info.is_current {
                if use_color {
                    styles::cyan(">")
                } else {
                    ">".to_string()
                }
            } else {
                " ".to_string()
            };

            let rel_path = info
                .path
                .strip_prefix(project_root)
                .unwrap_or(&info.path)
                .display()
                .to_string();

            let base = format_ahead_behind(info.ahead, info.behind, use_color);

            let dirty = if info.is_dirty {
                if use_color {
                    styles::yellow("*")
                } else {
                    "*".to_string()
                }
            } else {
                String::new()
            };

            let age = format_age(&info.last_commit_age, use_color);

            TableRow {
                current,
                name: info.name.clone(),
                path: rel_path,
                base,
                dirty,
                age,
                subject: info.last_commit_subject.clone(),
            }
        })
        .collect();

    // Build table without header row using Builder
    let mut builder = Builder::new();
    for row in &rows {
        builder.push_record([
            &row.current,
            &row.name,
            &row.path,
            &row.base,
            &row.dirty,
            &row.age,
            &row.subject,
        ]);
    }

    let mut table = builder.build();
    table
        .with(Style::blank())
        .with(Modify::new(Columns::last()).with(Width::truncate(40).suffix("...")));

    println!("{table}");
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

fn format_age(age: &str, use_color: bool) -> String {
    if age.is_empty() {
        return String::new();
    }

    if use_color && is_old_age(age) {
        styles::dim(age)
    } else {
        age.to_string()
    }
}

/// Check if a relative age string represents more than 7 days.
fn is_old_age(age: &str) -> bool {
    // Git's relative dates look like "3 days ago", "2 weeks ago", "5 months ago", etc.
    // Anything with "week", "month", or "year" is definitely > 7 days.
    // For "N days ago", check if N > 7.
    if age.contains("week") || age.contains("month") || age.contains("year") {
        return true;
    }
    if age.contains("day") {
        // Extract the number of days
        if let Some(num_str) = age.split_whitespace().next() {
            if let Ok(days) = num_str.parse::<u32>() {
                return days > 7;
            }
        }
    }
    false
}
