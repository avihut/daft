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

worktree-post-remove fires AFTER the worktree directory has been deleted —
$DAFT_WORKTREE_PATH points at a path that no longer exists. Hook scripts that
need to inspect the worktree must do so in worktree-pre-remove.

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
    //
    // The router in `src/main.rs` only dispatches here when argv[1] == "repo"
    // and argv[2] == "remove", so `skip(3)` is correct. Assert the invariant
    // in debug builds — if a future shortcut alias or alternative entry path
    // dispatches here without that argv shape, we want a loud failure rather
    // than silently dropping or shifting positional arguments.
    let raw_args: Vec<String> = std::env::args().collect();
    debug_assert!(
        raw_args.len() >= 3 && raw_args[1] == "repo" && raw_args[2] == "remove",
        "repo::remove::run() invoked with unexpected argv: {raw_args:?} \
         (expected `daft repo remove ...`)"
    );
    let argv: Vec<String> = std::iter::once("git-daft-repo-remove".to_string())
        .chain(raw_args.into_iter().skip(3))
        .collect();
    let args = Args::parse_from(argv);
    run_with_args(&args)
}

pub(crate) fn run_with_args(args: &Args) -> Result<()> {
    use crate::core::worktree::remove_repo::{enumerate_worktrees, resolve_repo};

    // Load global config only. `daft repo remove` is the one daft command that
    // commonly runs from outside any repo (e.g. `daft repo remove ./old-repo`
    // from a parent directory). `DaftSettings::load()` ultimately calls
    // `git.config_get` which does `gix::discover(&cwd)` and fails outside a
    // repo, so the local-config variant breaks the basic
    // `daft repo remove <path>` invocation. The only setting we read here is
    // `use_gitoxide`, which users typically configure globally anyway.
    let settings = crate::core::settings::DaftSettings::load_global()?;
    let use_gitoxide = settings.use_gitoxide;
    if crate::git::should_show_gitoxide_notice(use_gitoxide) {
        eprintln!("[experimental] Using gitoxide backend for git operations");
    }

    // Honor user-configured hook settings (timeout, output verbosity,
    // per-hook trust defaults). Loading from global mirrors the
    // `load_global()` call above — same rationale, same failure-mode
    // tolerance for cwd-outside-any-repo invocations.
    let hooks_config = crate::core::settings::load_hooks_config_global()?;

    let target = resolve_repo(args.path.as_deref(), use_gitoxide)?;
    let worktrees = enumerate_worktrees(&target, use_gitoxide)?;

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

    // Snapshot cwd BEFORE we delete anything. If the user is running from
    // inside a worktree that's about to be removed, `std::env::current_dir()`
    // after the removal returns ENOENT (the inode is gone) and we lose the
    // signal we need to write `DAFT_CD_FILE`. Captured ahead of time, the
    // path itself is enough — we just need a string to compare against
    // `project_root`. Integration coverage: `remove-from-inside.yml`.
    let original_cwd = std::env::current_dir().ok();

    // The TUI is meaningful only when there are concurrent worktree-removal
    // tasks to track. When the worktree list is empty, the only task is the
    // single bare-removal — sequential output is clearer (and avoids the
    // empty-table-with-headers TUI render that looks like a glitch).
    let force_sequential = worktrees.is_empty()
        || args.verbose >= 2
        || !std::io::IsTerminal::is_terminal(&std::io::stderr());
    // `maybe_redirect_cwd` runs regardless of success/failure: even on partial
    // failure we may have removed the worktree containing the user's cwd, in
    // which case we still need to hand the shell wrapper a safe directory.
    let result = if force_sequential {
        run_sequential(&target, &worktrees, &settings, &hooks_config)
    } else {
        run_tui(&target, &worktrees, args.verbose, &settings, &hooks_config)
    };
    maybe_redirect_cwd(&target, original_cwd.as_deref());
    result
}

/// If the current working directory is inside the removed repo, hand the
/// shell wrapper a safe path to `cd` into via `DAFT_CD_FILE`. Without this,
/// the user's shell would be left sitting in a now-deleted directory.
///
/// Picks `project_root.parent()` first (the natural sibling), then falls back
/// to `dirs::data_dir()`, then `dirs::home_dir()`, then `/`.
///
/// Integration coverage lives in `tests/manual/scenarios/repo/remove-from-inside.yml`
/// — the unit-test layer can't reliably exercise the cwd-mutation path because
/// Rust unit tests share process-wide cwd / env state and run in parallel.
fn maybe_redirect_cwd(
    target: &crate::core::worktree::remove_repo::RepoTarget,
    original_cwd: Option<&std::path::Path>,
) {
    // Prefer the snapshot taken before removal: by the time we get here the
    // user's worktree is gone and `std::env::current_dir()` may fail with
    // ENOENT. Fall back to a fresh lookup only if the caller didn't capture.
    let cwd = match original_cwd {
        Some(c) => c.to_path_buf(),
        None => match std::env::current_dir() {
            Ok(c) => c,
            Err(_) => return,
        },
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
    let suffix = match n {
        0 => "This will delete the bare git dir (no worktrees to remove).".to_string(),
        1 => "This will delete 1 worktree and the bare git dir.".to_string(),
        n => format!("This will delete {n} worktrees and the bare git dir."),
    };
    print!(
        "Remove repo at {}? {suffix} [y/N] ",
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
    settings: &crate::core::settings::DaftSettings,
    hooks_config: &crate::hooks::HooksConfig,
) -> Result<()> {
    use crate::commands::sync_shared::{execute_remove_bare_task, execute_remove_worktree_task};
    use crate::core::worktree::sync_dag::{DagEvent, TaskMessage, TaskStatus};

    let main_worktree_path = main_worktree_path(worktrees);
    let (tx, rx) = std::sync::mpsc::channel();

    let mut hook_summaries: Vec<HookSummary> = Vec::new();
    let mut any_failed = false;
    for entry in worktrees {
        let label = entry.branch.clone().unwrap_or_else(|| "(detached)".into());
        // `execute_remove_worktree_task` always emits the pair
        // `(TaskStatus::Failed, TaskMessage::Failed(_))` together — we set
        // `any_failed` from the message arm only and discard the status.
        let (_status, msg) = execute_remove_worktree_task(
            target,
            entry,
            hooks_config,
            &settings.remote,
            main_worktree_path,
            &tx,
        );
        let line = match &msg {
            TaskMessage::Removed => "removed".to_string(),
            TaskMessage::Failed(e) => {
                any_failed = true;
                e.clone()
            }
            _ => "removed".to_string(),
        };
        println!("  {label}: {line}");
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
                //
                // Integration coverage for the abort-mode exit-code path is
                // deferred until YAML config exposes `fail_mode` for
                // `worktree-pre-remove` (see #446). Today the schema defaults
                // to Warn and provides no override, so neither YAML scenarios
                // nor bats tests can drive this branch.
                if !success && !warned {
                    any_failed = true;
                }
                // Only surface hooks that had a problem. A warn-mode hook
                // that succeeded has nothing to summarize; the "warned"
                // label exists to distinguish a *failed* warn-mode hook
                // from a *failed* abort-mode hook in the printed list.
                if !success {
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

    // Defensive drain: today `execute_remove_bare_task` doesn't fire hooks,
    // so this is a no-op. If a future repo-level post-remove hook lands
    // here it would otherwise be silently dropped.
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
            if !success && !warned {
                any_failed = true;
            }
            if !success {
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

    print_hook_summary(&hook_summaries);
    if any_failed {
        bail!("repo removal had failures (see above)");
    }
    Ok(())
}

/// Build the per-worktree TUI rows for `daft repo remove`.
///
/// One row per `WorktreeEntry`, keyed by the branch label so DAG events
/// emitted from `build_remove_repo` resolve via `find_row_mut(branch_name)`.
/// `path` is populated so `WorktreeInfo::refresh_dynamic_fields` could fill
/// in Path/Base/Changes/etc. cells (we don't call it here because most of
/// those fields require base-branch comparison, status scan, network, or
/// owner resolution — none of which is worth doing for a repo about to be
/// deleted). However, we DO read HEAD commit metadata
/// (`last_commit_{timestamp,hash,subject}` and `branch_creation_timestamp`)
/// synchronously per worktree: it's purely local, cheap, and lets the TUI
/// render real values for the Commit / Hash / Age columns instead of blanks.
///
/// Note: the bare-removal task (`TaskId::RemoveBare`) emits events with an
/// empty `branch_name` so the TUI's auto-create guard skips it; no row is
/// needed for the bare git dir. The OperationPhase indicator at the top of
/// the TUI ("Removing repository") already gives the user feedback that the
/// overall operation is in flight.
fn build_tui_rows(
    worktrees: &[crate::core::worktree::remove_repo::WorktreeEntry],
) -> Vec<crate::core::worktree::list::WorktreeInfo> {
    use crate::core::worktree::list::{
        get_branch_creation_timestamp, get_commit_metadata, WorktreeInfo,
    };

    // Subprocess backend is fine here: this runs once at TUI bootstrap for a
    // typical 1-10 worktrees. Threading `use_gitoxide` through is out of
    // scope; `get_commit_metadata` falls back cleanly when gitoxide is off.
    let git = crate::git::GitCommand::new(true);

    worktrees
        .iter()
        .map(|w| {
            let label = w.branch.as_deref().unwrap_or("(detached)");
            let mut info = WorktreeInfo::empty(label);
            info.path = Some(w.path.clone());

            let (ts, hash, subj) = get_commit_metadata(&w.path, &git);
            info.last_commit_timestamp = ts;
            info.last_commit_hash = hash;
            info.last_commit_subject = subj;

            // `branch_creation_timestamp` (Age column source) needs a branch
            // name; detached worktrees have none, so leave it as `None` and
            // the Age cell renders blank — which is correct, "branch age"
            // doesn't apply without a branch.
            if let Some(branch) = w.branch.as_deref() {
                info.branch_creation_timestamp = get_branch_creation_timestamp(branch, &w.path);
            }

            info
        })
        .collect()
}

/// Pick the main worktree's path from the enumerated list. `git worktree
/// list --porcelain` (and our gix-backed equivalent) emits the main worktree
/// first; we rely on that ordering to identify it. The main worktree is the
/// first non-bare entry that isn't a detached-HEAD sandbox. Returns `None`
/// for bare-only repos with no checked-out worktrees.
///
/// Used to populate `$DAFT_SOURCE_WORKTREE` for `worktree-pre/post-remove`
/// hooks. Setting this to the actual main worktree (rather than the
/// `project_root` parent) means hook scripts that `cd "$DAFT_SOURCE_WORKTREE"`
/// land inside a real git working tree where `git` commands behave normally.
fn main_worktree_path(
    worktrees: &[crate::core::worktree::remove_repo::WorktreeEntry],
) -> Option<&std::path::Path> {
    worktrees
        .iter()
        .find(|w| !w.is_bare && !w.is_detached)
        .map(|w| w.path.as_path())
}

/// Reserve viewport rows for hook sub-rows: at most 2 hooks per worktree
/// (pre-remove + post-remove), with a couple of job sub-rows each. We
/// over-allocate because the inline ratatui viewport cannot grow.
///
/// Always allocates the per-worktree slack regardless of verbosity: hooks
/// during `daft repo remove` may be doing critical teardown (docker, networks,
/// mounts) and the user shouldn't have to pass `-v` to see them run.
///
/// Uses saturating arithmetic so absurdly large worktree counts (>16k) don't
/// overflow `u16` — the viewport just clamps to its max.
fn hook_viewport_budget(worktrees_len: usize) -> u16 {
    let total = worktrees_len.saturating_mul(4).saturating_add(5);
    total.try_into().unwrap_or(u16::MAX)
}

fn run_tui(
    target: &crate::core::worktree::remove_repo::RepoTarget,
    worktrees: &[crate::core::worktree::remove_repo::WorktreeEntry],
    verbose: u8,
    settings: &crate::core::settings::DaftSettings,
    hooks_config: &crate::hooks::HooksConfig,
) -> Result<()> {
    use crate::commands::sync_shared::{
        check_tui_failures_strict, execute_remove_bare_task, execute_remove_worktree_task,
    };
    use crate::core::worktree::list::Stat;
    use crate::core::worktree::sync_dag::{
        DagExecutor, OperationPhase, SyncDag, SyncTask, TaskId, TaskMessage, TaskOutcome,
        TaskStatus,
    };
    use crate::output::tui::operation_table::{OperationTable, TableConfig};
    use std::collections::HashSet;
    use std::path::PathBuf;
    use std::sync::Arc;

    let phases = vec![OperationPhase::RemoveRepo];

    let worktree_infos = build_tui_rows(worktrees);

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
    let hooks_arc = Arc::new(hooks_config.clone());
    let remote_name_arc: Arc<String> = Arc::new(settings.remote.clone());
    let main_worktree_path_arc: Arc<Option<PathBuf>> =
        Arc::new(main_worktree_path(worktrees).map(|p| p.to_path_buf()));

    let tx_for_tasks = tx.clone();
    let target_for_tasks = Arc::clone(&target_arc);
    let entries_for_tasks = Arc::clone(&entries_arc);
    let hooks_for_tasks = Arc::clone(&hooks_arc);
    let remote_name_for_tasks = Arc::clone(&remote_name_arc);
    let main_worktree_path_for_tasks = Arc::clone(&main_worktree_path_arc);

    let orchestrator = std::thread::spawn(move || {
        let executor = DagExecutor::new(dag, tx);
        executor.run(
            move |task: &SyncTask,
                  outcomes: &HashSet<TaskOutcome>|
                  -> (TaskStatus, TaskMessage, HashSet<TaskOutcome>) {
                match &task.id {
                    TaskId::RemoveWorktree(path) => {
                        // Soft-fail rather than panic: a panic here would hang
                        // the worker pool waiting on its cvar. The invariant
                        // (one entry per RemoveWorktree task) is held by
                        // construction, but we still degrade gracefully.
                        let Some(entry) =
                            entries_for_tasks.iter().find(|e| &e.path == path).cloned()
                        else {
                            return (
                                TaskStatus::Failed,
                                TaskMessage::Failed(format!(
                                    "internal: no entry for {}",
                                    path.display()
                                )),
                                outcomes.clone(),
                            );
                        };
                        let (s, m) = execute_remove_worktree_task(
                            &target_for_tasks,
                            &entry,
                            &hooks_for_tasks,
                            &remote_name_for_tasks,
                            main_worktree_path_for_tasks.as_deref(),
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
    let extra_rows = hook_viewport_budget(worktrees.len());
    // Bump verbosity to at least 1 so hook sub-rows render in the TUI. For
    // `daft repo remove`, hooks may be doing critical teardown (docker,
    // networks, mounts) and the user shouldn't have to pass `-v` to see them.
    // The `force_sequential` gate above (`verbose >= 2`) is computed on the
    // raw `args.verbose`, so this bump only affects the TUI path.
    let table_verbosity = verbose.max(1);
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
            verbosity: table_verbosity,
            pin_default_branch: false,
            partition_by_owner: false,
            // Repo removal doesn't stream field updates — `WorktreeInfo` rows
            // are seeded with whatever we know up front (path, branch, kind)
            // and never patched. Mark every field as already-received so the
            // table renders blanks for unset cells instead of perpetual
            // loaders.
            seeded_fields: crate::core::worktree::info_field::FieldSet::ALL,
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

    // repo-remove uses the strict variant: a non-warned hook failure must
    // flip the process exit code even when the row is `Done(Pruned)` because
    // the filesystem-side removal succeeded. See
    // `sync_shared::check_tui_failures_strict` for the rationale; this keeps
    // the TUI path symmetric with `run_sequential`'s `any_failed` handling.
    check_tui_failures_strict(&completed.rows)?;
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

    use crate::core::worktree::list::EntryKind;
    use crate::core::worktree::remove_repo::WorktreeEntry;
    use std::path::PathBuf;

    fn entry(branch: Option<&str>, path: &str) -> WorktreeEntry {
        WorktreeEntry {
            path: PathBuf::from(path),
            branch: branch.map(String::from),
            is_bare: false,
            is_detached: branch.is_none(),
        }
    }

    #[test]
    fn build_tui_rows_omits_synthetic_bare_row() {
        let entries = vec![
            entry(Some("master"), "/repo/master"),
            entry(Some("feature"), "/repo/feature"),
        ];
        let rows = build_tui_rows(&entries);
        assert!(
            rows.iter().all(|r| r.name != "(bare)"),
            "build_tui_rows must not emit a synthetic '(bare)' row; got names: {:?}",
            rows.iter().map(|r| r.name.as_str()).collect::<Vec<_>>()
        );
    }

    #[test]
    fn build_tui_rows_one_per_entry_with_path_and_worktree_kind() {
        let entries = vec![
            entry(Some("master"), "/repo/master"),
            entry(Some("feature"), "/repo/feature"),
        ];
        let rows = build_tui_rows(&entries);
        assert_eq!(
            rows.len(),
            entries.len(),
            "build_tui_rows must return exactly one row per WorktreeEntry"
        );
        for (row, source) in rows.iter().zip(entries.iter()) {
            assert_eq!(
                row.name,
                source.branch.clone().unwrap_or_else(|| "(detached)".into()),
                "row name should mirror the entry's branch label"
            );
            assert_eq!(
                row.path.as_ref(),
                Some(&source.path),
                "row.path must be Some(entry.path) so refresh_dynamic_fields can populate cells"
            );
            assert_eq!(
                row.kind,
                EntryKind::Worktree,
                "row kind must be Worktree (not LocalBranch) so the TUI treats it as a real checkout"
            );
        }
    }

    #[test]
    fn build_tui_rows_handles_detached_worktree() {
        let entries = vec![entry(None, "/repo/sandbox")];
        let rows = build_tui_rows(&entries);
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].name, "(detached)");
        assert_eq!(
            rows[0].path.as_deref(),
            Some(std::path::Path::new("/repo/sandbox"))
        );
    }

    #[test]
    fn build_tui_rows_populates_head_commit_metadata() {
        // Bug 2: Commit / Hash / Age cells were blank because
        // `build_tui_rows` constructed `WorktreeInfo` rows with `path` set
        // but never populated last_commit_{timestamp,hash,subject} or
        // branch_creation_timestamp. These fields are local and cheap to
        // read at row-build time.
        let tmp = tempfile::tempdir().unwrap();
        let wt = make_repo_with_worktree(tmp.path());

        // Capture the actual abbreviated SHA and message for the worktree's
        // HEAD; SHA is unstable across timestamps/authors so we read it back.
        let sha_out = Command::new("git")
            .current_dir(&wt)
            .args(["rev-parse", "--short", "HEAD"])
            .output()
            .unwrap();
        let expected_hash = String::from_utf8(sha_out.stdout)
            .unwrap()
            .trim()
            .to_string();
        assert!(
            !expected_hash.is_empty(),
            "test setup: rev-parse --short HEAD must return a hash"
        );

        let entries = vec![WorktreeEntry {
            path: wt.clone(),
            branch: Some("feat".to_string()),
            is_bare: false,
            is_detached: false,
        }];
        let rows = build_tui_rows(&entries);

        assert_eq!(rows.len(), 1);
        let info = &rows[0];
        assert_eq!(
            info.last_commit_hash.as_deref(),
            Some(expected_hash.as_str()),
            "last_commit_hash must match `git rev-parse --short HEAD`",
        );
        assert!(
            info.last_commit_timestamp.is_some(),
            "last_commit_timestamp must be populated",
        );
        assert_eq!(
            info.last_commit_subject, "init",
            "last_commit_subject must match the commit message",
        );
        assert!(
            info.branch_creation_timestamp.is_some(),
            "branch_creation_timestamp must be populated for non-detached worktrees so the Age column has data",
        );
    }

    #[test]
    fn hook_viewport_budget_nonzero_for_default_verbosity() {
        // Bug 3: hook viewport must be allocated regardless of verbosity, so
        // hook progress is visible at the default `verbose=0`.
        assert!(
            hook_viewport_budget(0) > 0,
            "budget must reserve baseline rows even with no worktrees"
        );
        assert!(
            hook_viewport_budget(2) > hook_viewport_budget(0),
            "budget must scale with worktree count"
        );
        assert_eq!(hook_viewport_budget(2), 5 + 2 * 4);
    }

    #[test]
    fn hook_viewport_budget_saturates_on_huge_input() {
        // PR-review hardening: a wildly large worktree count must clamp to
        // `u16::MAX` rather than overflow. The previous unchecked
        // `worktrees_len as u16 * 4` would panic in debug for >16383 and wrap
        // silently in release.
        assert_eq!(hook_viewport_budget(usize::MAX), u16::MAX);
        assert_eq!(hook_viewport_budget(u16::MAX as usize), u16::MAX);
        // Just below the saturation threshold should still scale linearly.
        assert_eq!(hook_viewport_budget(100), 5 + 100 * 4);
    }

    #[test]
    fn main_worktree_path_picks_first_non_bare_non_detached() {
        use crate::core::worktree::remove_repo::WorktreeEntry;
        use std::path::PathBuf;

        let entries = vec![
            WorktreeEntry {
                path: PathBuf::from("/tmp/repo/main"),
                branch: Some("main".into()),
                is_bare: false,
                is_detached: false,
            },
            WorktreeEntry {
                path: PathBuf::from("/tmp/repo/feature"),
                branch: Some("feature".into()),
                is_bare: false,
                is_detached: false,
            },
        ];
        assert_eq!(
            main_worktree_path(&entries),
            Some(std::path::Path::new("/tmp/repo/main"))
        );
    }

    #[test]
    fn main_worktree_path_skips_detached_sandboxes() {
        use crate::core::worktree::remove_repo::WorktreeEntry;
        use std::path::PathBuf;

        // If a detached-HEAD sandbox happens to land before the main worktree
        // in the porcelain output, we must still pick the real working tree.
        let entries = vec![
            WorktreeEntry {
                path: PathBuf::from("/tmp/repo/sandbox"),
                branch: None,
                is_bare: false,
                is_detached: true,
            },
            WorktreeEntry {
                path: PathBuf::from("/tmp/repo/main"),
                branch: Some("main".into()),
                is_bare: false,
                is_detached: false,
            },
        ];
        assert_eq!(
            main_worktree_path(&entries),
            Some(std::path::Path::new("/tmp/repo/main"))
        );
    }

    #[test]
    fn main_worktree_path_returns_none_for_bare_only() {
        // No checked-out worktrees at all → no source worktree to point hooks
        // at. Caller falls back to `project_root` in that case.
        assert!(main_worktree_path(&[]).is_none());
    }
}
