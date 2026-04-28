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
        run_sequential(&target, &worktrees)?;
    } else {
        run_tui(&target, &worktrees)?;
    }
    maybe_redirect_cwd(&target);
    Ok(())
}

/// If the current working directory is inside the removed repo, hand the
/// shell wrapper a safe path to `cd` into via `DAFT_CD_FILE`. Without this,
/// the user's shell would be left sitting in a now-deleted directory.
///
/// Picks `project_root.parent()` first (the natural sibling), then falls back
/// to `dirs::data_dir()`, then `dirs::home_dir()`, then `/`.
///
/// TODO(Bundle G): exercise this in the YAML scenario `remove-from-inside.yml`
/// — the spec-aligned integration coverage. The unit-test layer cannot
/// reliably exercise the cwd-mutation path because Rust unit tests share
/// process-wide cwd / env state and run in parallel, which makes the test
/// inherently racy with the rest of the suite.
fn maybe_redirect_cwd(target: &crate::core::worktree::remove_repo::RepoTarget) {
    let cwd = match std::env::current_dir() {
        Ok(c) => c,
        Err(_) => return,
    };
    if !cwd.starts_with(&target.project_root) {
        return;
    }
    let safe_target = target
        .project_root
        .parent()
        .map(|p| p.to_path_buf())
        .or_else(dirs::data_dir)
        .or_else(dirs::home_dir)
        .unwrap_or_else(|| std::path::PathBuf::from("/"));
    if let Ok(cd_file) = std::env::var(crate::CD_FILE_ENV) {
        let _ = std::fs::write(&cd_file, format!("{}\n", safe_target.display()));
    } else {
        eprintln!(
            "Run `cd {}` (your previous working directory was removed)",
            safe_target.display()
        );
    }
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
    use crate::commands::sync_shared::{
        check_tui_failures, execute_remove_bare_task, execute_remove_worktree_task,
    };
    use crate::core::worktree::list::{Stat, WorktreeInfo};
    use crate::core::worktree::sync_dag::{
        DagExecutor, OperationPhase, SyncDag, SyncTask, TaskId, TaskMessage, TaskOutcome,
        TaskStatus,
    };
    use crate::output::tui::operation_table::{OperationTable, TableConfig};
    use std::collections::HashSet;
    use std::path::PathBuf;
    use std::sync::Arc;

    let phases = vec![OperationPhase::RemoveRepo];

    // Seed one TUI row per worktree (keyed by branch label so events emitted
    // from `build_remove_repo` resolve via `find_row_mut(branch_name)`), plus
    // one synthetic "(bare)" row for the terminal task.
    let mut worktree_infos: Vec<WorktreeInfo> = worktrees
        .iter()
        .map(|w| {
            let label = w.branch.as_deref().unwrap_or("(detached)");
            WorktreeInfo::local_branch_stub(label, None)
        })
        .collect();
    worktree_infos.push(WorktreeInfo::local_branch_stub("(bare)", None));

    let dag = SyncDag::build_remove_repo(
        worktrees
            .iter()
            .map(|w| {
                (
                    w.branch.clone().unwrap_or_else(|| "(detached)".into()),
                    w.path.clone(),
                )
            })
            .collect(),
        target.bare_git_dir.clone(),
    );

    let (tx, rx) = std::sync::mpsc::channel();

    // Shared state for the orchestrator thread. We share entries via Arc and
    // look up by path inside the executor closure.
    let target_arc = Arc::new(target.clone());
    let entries_arc: Arc<Vec<crate::core::worktree::remove_repo::WorktreeEntry>> =
        Arc::new(worktrees.to_vec());
    let hooks_arc = Arc::new(crate::hooks::HooksConfig::default());

    let tx_for_tasks = tx.clone();
    let target_for_tasks = Arc::clone(&target_arc);
    let entries_for_tasks = Arc::clone(&entries_arc);
    let hooks_for_tasks = Arc::clone(&hooks_arc);

    let orchestrator = std::thread::spawn(move || {
        let executor = DagExecutor::new(dag, tx);
        executor.run(
            move |task: &SyncTask,
                  outcomes: &HashSet<TaskOutcome>|
                  -> (TaskStatus, TaskMessage, HashSet<TaskOutcome>) {
                match &task.id {
                    TaskId::RemoveWorktree(path) => {
                        let entry = entries_for_tasks
                            .iter()
                            .find(|e| &e.path == path)
                            .expect("entry for task path must exist");
                        let (s, m) = execute_remove_worktree_task(
                            &target_for_tasks,
                            entry,
                            &hooks_for_tasks,
                            &tx_for_tasks,
                        );
                        (s, m, outcomes.clone())
                    }
                    TaskId::RemoveBare => {
                        let (s, m) = execute_remove_bare_task(&target_for_tasks);
                        (s, m, outcomes.clone())
                    }
                    _ => (
                        TaskStatus::Skipped,
                        TaskMessage::Ok("not applicable".into()),
                        outcomes.clone(),
                    ),
                }
            },
        );
    });

    let cwd = std::env::current_dir().unwrap_or_else(|_| PathBuf::from("."));
    // Reserve viewport rows for hook sub-rows: at most 2 hooks per worktree
    // (pre-remove + post-remove), and we may want a couple of job sub-rows
    // each. Over-allocate since the inline ratatui viewport cannot grow.
    let extra_rows = 5 + (worktrees.len() as u16) * 4;
    let table = OperationTable::new(
        phases,
        worktree_infos,
        target.project_root.clone(),
        cwd,
        Stat::Summary,
        rx,
        TableConfig {
            columns: None,
            columns_explicit: false,
            sort_spec: None,
            extra_rows,
            verbosity: 0,
            pin_default_branch: false,
            partition_by_owner: false,
        },
        None,
    );
    let completed = table.run()?;

    orchestrator
        .join()
        .map_err(|_| anyhow::anyhow!("DAG orchestrator thread panicked"))?;

    if !completed.hook_summaries.is_empty() {
        eprintln!();
        eprintln!("Hooks:");
        for entry in &completed.hook_summaries {
            let status_word = if entry.warned { "warned" } else { "failed" };
            let exit_str = entry
                .exit_code
                .map(|c| format!("exit {c}"))
                .unwrap_or_else(|| "error".into());
            eprintln!(
                "  {}: {} {} ({}, {}ms)",
                entry.branch_name,
                entry.hook_type.filename(),
                status_word,
                exit_str,
                entry.duration.as_millis()
            );
            if let Some(ref out) = entry.output {
                for line in out.lines() {
                    eprintln!("    {line}");
                }
            }
        }
    }

    check_tui_failures(&completed.rows)?;
    Ok(())
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
