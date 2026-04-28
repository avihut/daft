//! `daft repo remove` — remove a Git repository and all its worktrees.

use anyhow::{bail, Result};
use clap::Parser;
use std::path::PathBuf;

#[derive(Parser, Debug)]
#[command(name = "git-daft-repo-remove")]
#[command(version = crate::VERSION)]
#[command(about = "Remove a Git repository and all its worktrees")]
#[command(long_about = r#"
Removes a Git repository identified by <path> (or the current directory if no
path is given), including the bare git directory and every checked-out
worktree. For each worktree, the worktree-pre-remove and worktree-post-remove
lifecycle hooks are run when the repository is daft-managed and trusted.

Hook failures do not abort removal; failed hooks are summarized after the
operation completes. The repo is removed regardless.

Refuses to operate on paths that are not inside a Git repository.
"#)]
pub struct Args {
    #[arg(help = "Path to the repo or any directory inside it (default: cwd)")]
    pub path: Option<PathBuf>,

    #[arg(short = 'y', long = "force", help = "Skip the confirmation prompt")]
    pub force: bool,

    #[arg(
        long = "dry-run",
        help = "Print what would be removed without touching anything"
    )]
    pub dry_run: bool,

    #[arg(
        short,
        long,
        action = clap::ArgAction::Count,
        help = "Increase verbosity (-v hook details, -vv full sequential output)"
    )]
    pub verbose: u8,
}

pub fn run() -> Result<()> {
    // Build clap argv: program name + everything after `daft repo remove`.
    // `daft repo` is a subcommand category (like `daft setup shortcuts`), so
    // `crate::get_clap_args` does not recognize it; we rebuild argv manually.
    let argv: Vec<String> = std::iter::once("git-daft-repo-remove".to_string())
        .chain(std::env::args().skip(3))
        .collect();
    let args = Args::parse_from(argv);
    run_with_args(&args)
}

pub(crate) fn run_with_args(args: &Args) -> Result<()> {
    use crate::core::worktree::remove_repo::{enumerate_worktrees, resolve_repo};

    let target = resolve_repo(args.path.as_deref())?;
    let worktrees = enumerate_worktrees(&target)?;

    if args.dry_run {
        print_plan(&target, &worktrees);
        return Ok(());
    }

    if !args.force {
        if !std::io::IsTerminal::is_terminal(&std::io::stdin()) {
            bail!("Refusing to run without --force in non-interactive mode");
        }
        if !confirm_prompt(&target, worktrees.len())? {
            println!("aborted");
            return Ok(());
        }
    }

    let force_sequential =
        args.verbose >= 2 || !std::io::IsTerminal::is_terminal(&std::io::stderr());
    if force_sequential {
        return run_sequential(&target, &worktrees);
    }
    run_tui(&target, &worktrees)
}

fn print_plan(
    target: &crate::core::worktree::remove_repo::RepoTarget,
    worktrees: &[crate::core::worktree::remove_repo::WorktreeEntry],
) {
    println!("Would remove:");
    for w in worktrees {
        let label = w.branch.as_deref().unwrap_or("(detached)");
        println!("  worktree  {}  ({})", w.path.display(), label);
    }
    println!("  bare      {}", target.bare_git_dir.display());
    println!("  trust DB entry for {}", target.bare_git_dir.display());
}

fn confirm_prompt(
    target: &crate::core::worktree::remove_repo::RepoTarget,
    n: usize,
) -> Result<bool> {
    use std::io::{BufRead, Write};
    print!(
        "Remove repo at {}? This will delete {n} worktrees and the bare git dir. [y/N] ",
        target.project_root.display()
    );
    std::io::stdout().flush()?;
    let mut line = String::new();
    std::io::stdin().lock().read_line(&mut line)?;
    Ok(matches!(line.trim(), "y" | "Y"))
}

fn run_sequential(
    target: &crate::core::worktree::remove_repo::RepoTarget,
    worktrees: &[crate::core::worktree::remove_repo::WorktreeEntry],
) -> Result<()> {
    use crate::commands::sync_shared::{execute_remove_bare_task, execute_remove_worktree_task};
    use crate::core::worktree::sync_dag::{DagEvent, TaskMessage, TaskStatus};

    let hooks_config = crate::hooks::HooksConfig::default();
    let (tx, rx) = std::sync::mpsc::channel();

    let mut hook_summaries: Vec<HookSummary> = Vec::new();
    let mut any_failed = false;
    for entry in worktrees {
        let label = entry.branch.clone().unwrap_or_else(|| "(detached)".into());
        let (status, msg) = execute_remove_worktree_task(target, entry, &hooks_config, &tx);
        let line = match &msg {
            TaskMessage::Removed => "removed".to_string(),
            TaskMessage::Failed(e) => {
                any_failed = true;
                e.clone()
            }
            _ => "removed".to_string(),
        };
        println!("  {label}: {line}");
        if matches!(status, TaskStatus::Failed) {
            any_failed = true;
        }
        while let Ok(ev) = rx.try_recv() {
            if let DagEvent::HookCompleted {
                branch_name,
                hook_type,
                success,
                warned,
                duration,
                exit_code,
                output,
            } = ev
            {
                // docs/superpowers/specs/2026-04-28-remove-repo-design.md L178:
                // exit code reflects unwarned hook failures. Worktree filesystem
                // removal proceeds regardless (TaskStatus::Succeeded), so we
                // must mark `any_failed` here when a hook aborts in non-warned
                // mode. Warned-only runs leave `any_failed` untouched.
                // TODO(Bundle G): cover this exit-code path in the YAML
                // scenario `remove-with-hooks.yml` — fail an Abort-mode hook
                // and assert the process exits non-zero.
                if !success && !warned {
                    any_failed = true;
                }
                if !success || warned {
                    hook_summaries.push(HookSummary {
                        branch_name,
                        hook_type,
                        success,
                        warned,
                        duration,
                        exit_code,
                        output,
                    });
                }
            }
        }
    }

    let (bare_status, bare_msg) = execute_remove_bare_task(target);
    let bare_line = match &bare_msg {
        TaskMessage::Removed => "removed".to_string(),
        TaskMessage::Failed(e) => e.clone(),
        _ => "removed".to_string(),
    };
    println!("  (bare): {bare_line}");
    if matches!(bare_status, TaskStatus::Failed) {
        any_failed = true;
    }

    print_hook_summary(&hook_summaries);
    if any_failed {
        bail!("repo removal had failures (see above)");
    }
    Ok(())
}

fn run_tui(
    target: &crate::core::worktree::remove_repo::RepoTarget,
    worktrees: &[crate::core::worktree::remove_repo::WorktreeEntry],
) -> Result<()> {
    // Bundle E will replace this with the OperationTable-driven path.
    run_sequential(target, worktrees)
}

struct HookSummary {
    branch_name: String,
    #[allow(dead_code)] // Held for symmetry; not yet used in summary output.
    success: bool,
    hook_type: crate::hooks::HookType,
    warned: bool,
    duration: std::time::Duration,
    exit_code: Option<i32>,
    output: Option<String>,
}

fn print_hook_summary(entries: &[HookSummary]) {
    if entries.is_empty() {
        return;
    }
    eprintln!();
    eprintln!("Hooks:");
    for h in entries {
        let status_word = if h.warned { "warned" } else { "failed" };
        let exit_str = h
            .exit_code
            .map(|c| format!("exit {c}"))
            .unwrap_or_else(|| "error".into());
        eprintln!(
            "  {}: {} {} ({}, {}ms)",
            h.branch_name,
            h.hook_type.filename(),
            status_word,
            exit_str,
            h.duration.as_millis()
        );
        if let Some(ref out) = h.output {
            for line in out.lines() {
                eprintln!("    {line}");
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::process::Command;

    fn make_repo_with_worktree(tmp: &std::path::Path) -> std::path::PathBuf {
        Command::new("git")
            .arg("init")
            .arg("-q")
            .arg(tmp)
            .status()
            .unwrap();
        std::fs::write(tmp.join("README"), b"hi").unwrap();
        Command::new("git")
            .current_dir(tmp)
            .args(["add", "."])
            .status()
            .unwrap();
        Command::new("git")
            .current_dir(tmp)
            .env("GIT_AUTHOR_NAME", "t")
            .env("GIT_AUTHOR_EMAIL", "t@t.com")
            .env("GIT_COMMITTER_NAME", "t")
            .env("GIT_COMMITTER_EMAIL", "t@t.com")
            .args(["commit", "-q", "-m", "init"])
            .status()
            .unwrap();
        let wt = tmp.join("wt-feat");
        Command::new("git")
            .current_dir(tmp)
            .args(["worktree", "add", wt.to_str().unwrap(), "-b", "feat"])
            .status()
            .unwrap();
        wt
    }

    #[test]
    fn dry_run_does_not_touch_filesystem() {
        let tmp = tempfile::tempdir().unwrap();
        let wt = make_repo_with_worktree(tmp.path());

        let args = Args {
            path: Some(tmp.path().to_path_buf()),
            force: false,
            dry_run: true,
            verbose: 0,
        };
        run_with_args(&args).unwrap();

        assert!(tmp.path().join(".git").exists(), "bare git dir must remain");
        assert!(wt.exists(), "worktree must remain");
    }

    #[test]
    fn run_force_removes_repo_end_to_end() {
        let tmp = tempfile::tempdir().unwrap();
        let wt = make_repo_with_worktree(tmp.path());

        let args = Args {
            path: Some(tmp.path().to_path_buf()),
            force: true,
            dry_run: false,
            verbose: 2, // force sequential path
        };
        run_with_args(&args).unwrap();

        assert!(
            !tmp.path().join(".git").exists(),
            "bare git dir must be gone"
        );
        assert!(!wt.exists(), "worktree must be gone");
    }
}
