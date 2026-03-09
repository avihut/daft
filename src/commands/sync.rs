//! git-sync - Synchronize worktrees with remote
//!
//! Orchestrates pruning stale branches/worktrees and updating all remaining
//! worktrees in a single command.
//!
//! When running in an interactive terminal, uses a DAG-based parallel executor
//! with an inline TUI (ratatui). Falls back to sequential execution when
//! stderr is not a TTY or verbose mode is enabled.

use crate::{
    core::{
        worktree::{
            fetch, list,
            list::Stat,
            prune, rebase,
            sync_dag::{DagEvent, DagExecutor, SyncDag, SyncTask, TaskId, TaskStatus},
        },
        CommandBridge, NullBridge, NullSink, OutputSink,
    },
    get_git_common_dir, get_project_root,
    git::{should_show_gitoxide_notice, GitCommand},
    hooks::{HookExecutor, HooksConfig},
    is_git_repository,
    logging::init_logging,
    output::{
        tui::{TuiRenderer, TuiState},
        CliOutput, Output, OutputConfig,
    },
    remote::get_default_branch_local,
    settings::DaftSettings,
    styles, WorktreeConfig, CD_FILE_ENV,
};
use anyhow::Result;
use clap::Parser;
use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::Arc;

#[derive(Parser)]
#[command(name = "git-sync")]
#[command(version = crate::VERSION)]
#[command(about = "Synchronize worktrees with remote (prune + update all)")]
#[command(long_about = r#"
Synchronizes all worktrees with the remote in a single command.

This is equivalent to running `daft prune` followed by `daft update --all`:

  1. Prune: fetches with --prune, removes worktrees and branches for deleted
     remote branches, executes lifecycle hooks for each removal.
  2. Update: pulls all remaining worktrees from their remote tracking branches.
  3. Rebase (--rebase BRANCH): rebases all remaining worktrees onto BRANCH.
     Best-effort: conflicts are immediately aborted and reported.

If you are currently inside a worktree that gets pruned, the shell is redirected
to a safe location (project root by default, or as configured via
daft.prune.cdTarget).

For fine-grained control over either phase, use `daft prune` and `daft update`
separately.
"#)]
pub struct Args {
    #[arg(short, long, help = "Be verbose; show detailed progress")]
    verbose: bool,

    #[arg(
        short,
        long,
        help = "Force removal of worktrees with uncommitted changes"
    )]
    force: bool,

    #[arg(
        long,
        value_name = "BRANCH",
        help = "Rebase all branches onto BRANCH after updating"
    )]
    rebase: Option<String>,
}

pub fn run() -> Result<()> {
    let args = Args::parse_from(crate::get_clap_args("git-sync"));

    init_logging(args.verbose);

    if !is_git_repository()? {
        anyhow::bail!("Not inside a Git repository");
    }

    let settings = DaftSettings::load()?;

    if std::io::IsTerminal::is_terminal(&std::io::stderr()) && !args.verbose {
        run_tui(args, settings)
    } else {
        run_sequential(args, settings)
    }
}

/// Sequential (non-TTY) execution path — the original sync flow.
fn run_sequential(args: Args, settings: DaftSettings) -> Result<()> {
    let config = OutputConfig::with_autocd(false, args.verbose, settings.autocd);
    let mut output = CliOutput::new(config);

    if should_show_gitoxide_notice(settings.use_gitoxide) {
        output.warning("[experimental] Using gitoxide backend for git operations");
    }

    // Phase 1: Prune stale branches and worktrees
    let prune_result = run_prune_phase(&mut output, &settings, args.force)?;

    // Phase 2: Update all remaining worktrees
    run_update_phase(&mut output, &settings, args.force)?;

    // Phase 3: Rebase all worktrees onto base branch (if requested)
    if let Some(ref base_branch) = args.rebase {
        run_rebase_phase(&mut output, &settings, base_branch, args.force)?;
    }

    // Write the cd target for the shell wrapper (from prune phase)
    if let Some(ref cd_target) = prune_result.cd_target {
        if std::env::var(CD_FILE_ENV).is_ok() {
            output.cd_path(cd_target);
        } else {
            output.result(&format!(
                "Run `cd {}` (your previous working directory was removed)",
                cd_target.display()
            ));
        }
    }

    Ok(())
}

/// Interactive TUI execution path — parallel DAG executor with inline ratatui display.
fn run_tui(args: Args, settings: DaftSettings) -> Result<()> {
    let git = GitCommand::new(false).with_gitoxide(settings.use_gitoxide);
    let project_root = get_project_root()?;

    // ── Phase 0: Pre-TUI work ──────────────────────────────────────────
    // Fetch + identify gone branches before building the DAG, since the DAG
    // needs to know which branches are gone.
    let config = OutputConfig::with_autocd(false, false, settings.autocd);
    let mut output = CliOutput::new(config);

    output.start_spinner("Fetching remote branches...");
    git.fetch(&settings.remote, true)?;
    output.finish_spinner();

    // Determine base branch for worktree info
    let base_branch = get_default_branch_local(
        &get_git_common_dir()?,
        &settings.remote,
        settings.use_gitoxide,
    )
    .unwrap_or_else(|_| "master".to_string());

    let current_path = crate::core::repo::get_current_worktree_path()
        .ok()
        .and_then(|p| p.canonicalize().ok());

    // Collect worktree info for the TUI table
    let worktree_infos =
        list::collect_worktree_info(&git, &base_branch, current_path.as_deref(), Stat::Summary)?;

    // Parse worktree list and identify gone branches
    let worktree_entries = prune::parse_worktree_list(&git)?;
    let is_bare_layout = worktree_entries.first().map(|e| e.is_bare).unwrap_or(false);

    let mut worktree_map: HashMap<String, (PathBuf, bool)> = HashMap::new();
    for (i, entry) in worktree_entries.iter().enumerate() {
        if let Some(ref branch) = entry.branch {
            worktree_map.insert(branch.clone(), (entry.path.clone(), i == 0));
        }
    }

    let hooks_config = HooksConfig::default();
    let executor = HookExecutor::new(hooks_config)?;
    let gone_branches = {
        let mut bridge = CommandBridge::new(&mut output, executor);
        prune::identify_gone_branches(
            &git,
            &worktree_map,
            &settings.remote,
            settings.use_gitoxide,
            &mut bridge,
        )?
    };

    // Get worktree list for DAG (branch name + path pairs)
    let all_worktrees = fetch::get_all_worktrees_with_branches(&git)?;

    // ── Build the DAG (twice: one for executor, one for renderer) ──────
    let dag_for_renderer = SyncDag::build_sync(
        all_worktrees
            .iter()
            .map(|(p, b)| (b.clone(), p.clone()))
            .collect(),
        gone_branches.clone(),
        args.rebase.clone(),
    );
    let dag_for_executor = SyncDag::build_sync(
        all_worktrees
            .iter()
            .map(|(p, b)| (b.clone(), p.clone()))
            .collect(),
        gone_branches.clone(),
        args.rebase.clone(),
    );

    // Add gone branches to worktree_infos for the TUI table (they may not
    // have existing worktrees but should still appear as rows).
    let mut all_infos = worktree_infos;
    for branch in &gone_branches {
        if !all_infos.iter().any(|info| info.name == *branch) {
            all_infos.push(list::WorktreeInfo {
                kind: list::EntryKind::Worktree,
                name: branch.clone(),
                path: worktree_map.get(branch).map(|(p, _)| p.clone()),
                is_current: false,
                is_default_branch: false,
                ahead: None,
                behind: None,
                staged: 0,
                unstaged: 0,
                untracked: 0,
                remote_ahead: None,
                remote_behind: None,
                last_commit_timestamp: None,
                last_commit_subject: String::new(),
                branch_creation_timestamp: None,
                base_lines_inserted: None,
                base_lines_deleted: None,
                staged_lines_inserted: None,
                staged_lines_deleted: None,
                unstaged_lines_inserted: None,
                unstaged_lines_deleted: None,
                remote_lines_inserted: None,
                remote_lines_deleted: None,
            });
        }
    }

    // ── Create TUI state ───────────────────────────────────────────────
    let phases = dag_for_renderer.phases();
    let mut state = TuiState::new(phases, all_infos);

    // Mark fetch phase as already completed (we did it pre-TUI).
    // Synthesize events for the fetch task.
    state.apply_event(&DagEvent::TaskStarted { task_idx: 0 }, &dag_for_renderer);
    state.apply_event(
        &DagEvent::TaskCompleted {
            task_idx: 0,
            status: TaskStatus::Succeeded,
            message: "fetched".into(),
        },
        &dag_for_renderer,
    );

    // ── Create channel and executor ────────────────────────────────────
    let (tx, rx) = std::sync::mpsc::channel();
    let dag_arc = Arc::new(dag_for_renderer);

    // Shared context for workers (must be Send + Sync + 'static)
    let shared_settings = Arc::new(settings.clone());
    let shared_project_root = Arc::new(project_root.clone());
    let shared_worktree_map = Arc::new(worktree_map.clone());
    let shared_current_wt_path = Arc::new(git.get_current_worktree_path().ok());
    let shared_current_branch = Arc::new(git.symbolic_ref_short_head().ok());

    // Build pull arguments (same logic as run_update_phase)
    let config_args: Vec<&str> = settings.update_args.split_whitespace().collect();
    let config_has_rebase = config_args.contains(&"--rebase");
    let config_has_autostash = config_args.contains(&"--autostash");
    let shared_pull_args = Arc::new(fetch::build_pull_args(&fetch::FetchParams {
        targets: vec![],
        all: true,
        force: args.force,
        dry_run: false,
        rebase: config_has_rebase,
        autostash: config_has_autostash,
        ff_only: false,
        no_ff_only: false,
        pull_args: vec![],
        quiet: false,
        remote_name: settings.remote.clone(),
    }));
    let shared_force = args.force;
    let shared_is_bare_layout = is_bare_layout;

    // Pre-compute prune context values
    let git_dir = get_git_common_dir()?;
    let shared_git_dir = Arc::new(git_dir.clone());
    let shared_remote_name = Arc::new(settings.remote.clone());
    let source_worktree = std::env::current_dir()?;
    let shared_source_worktree = Arc::new(source_worktree.clone());
    let shared_rebase_branch: Arc<Option<String>> = Arc::new(args.rebase.clone());

    // Track deferred branch (current worktree) for post-TUI handling.
    let deferred_branch: Arc<std::sync::Mutex<Option<String>>> =
        Arc::new(std::sync::Mutex::new(None));
    let deferred_branch_writer = Arc::clone(&deferred_branch);

    // ── Spawn executor in a background thread ──────────────────────────
    let executor = DagExecutor::new(dag_for_executor, tx);

    let executor_handle = std::thread::spawn(move || {
        executor.run(move |task: &SyncTask| -> (TaskStatus, String) {
            match &task.id {
                TaskId::Fetch => {
                    // Already done pre-TUI
                    (TaskStatus::Succeeded, "fetched".into())
                }
                TaskId::Prune(branch_name) => {
                    let result = execute_prune_task(
                        branch_name,
                        &shared_settings,
                        &shared_project_root,
                        &shared_git_dir,
                        &shared_remote_name,
                        &shared_source_worktree,
                        &shared_worktree_map,
                        shared_is_bare_layout,
                        &shared_current_wt_path,
                        &shared_current_branch,
                        shared_force,
                    );
                    if result.1 == "deferred" {
                        *deferred_branch_writer.lock().unwrap() = Some(branch_name.clone());
                    }
                    result
                }
                TaskId::Update(branch_name) => execute_update_task(
                    branch_name,
                    task.worktree_path.as_ref(),
                    &shared_settings,
                    &shared_project_root,
                    &shared_pull_args,
                    shared_force,
                ),
                TaskId::Rebase(branch_name) => {
                    let base = shared_rebase_branch.as_deref().unwrap_or("master");
                    execute_rebase_task(
                        branch_name,
                        task.worktree_path.as_ref(),
                        base,
                        &shared_project_root,
                        &shared_settings,
                        shared_force,
                    )
                }
            }
        });
    });

    // ── Run TUI renderer on main thread ────────────────────────────────
    let renderer = TuiRenderer::new(state, Arc::clone(&dag_arc), rx);
    let _final_state = renderer.run()?;

    // Wait for executor thread to finish
    executor_handle
        .join()
        .map_err(|_| anyhow::anyhow!("DAG executor thread panicked"))?;

    // ── Post-TUI: handle deferred branch (current worktree) ────────────
    // If the user was inside a worktree that needs pruning, prune_single_branch
    // deferred it. Now that the TUI is done, perform the actual removal.
    let deferred = deferred_branch.lock().unwrap().clone();
    if let Some(ref branch_name) = deferred {
        let git = GitCommand::new(false).with_gitoxide(settings.use_gitoxide);
        let ctx = prune::PruneContext {
            git: &git,
            project_root: project_root.clone(),
            git_dir,
            remote_name: settings.remote.clone(),
            source_worktree,
        };
        let params = prune::PruneParams {
            force: args.force,
            use_gitoxide: settings.use_gitoxide,
            is_quiet: true,
            remote_name: settings.remote.clone(),
            prune_cd_target: settings.prune_cd_target,
        };
        let mut sink = NullBridge;
        let cd_target =
            prune::handle_deferred_prune(&ctx, branch_name, &worktree_map, &params, &mut sink);

        if let Some(ref cd_path) = cd_target {
            let config = OutputConfig::with_autocd(false, false, settings.autocd);
            let mut output = CliOutput::new(config);
            if std::env::var(CD_FILE_ENV).is_ok() {
                output.cd_path(cd_path);
            } else {
                output.result(&format!(
                    "Run `cd {}` (your previous working directory was removed)",
                    cd_path.display()
                ));
            }
        }
    }

    Ok(())
}

// ── DAG task execution functions ───────────────────────────────────────────

/// Execute a single prune task for a DAG worker.
#[allow(clippy::too_many_arguments)]
fn execute_prune_task(
    branch_name: &str,
    settings: &DaftSettings,
    project_root: &std::path::Path,
    git_dir: &std::path::Path,
    remote_name: &str,
    source_worktree: &std::path::Path,
    worktree_map: &HashMap<String, (PathBuf, bool)>,
    is_bare_layout: bool,
    current_wt_path: &Option<PathBuf>,
    current_branch: &Option<String>,
    force: bool,
) -> (TaskStatus, String) {
    let git = GitCommand::new(false).with_gitoxide(settings.use_gitoxide);
    let ctx = prune::PruneContext {
        git: &git,
        project_root: project_root.to_path_buf(),
        git_dir: git_dir.to_path_buf(),
        remote_name: remote_name.to_string(),
        source_worktree: source_worktree.to_path_buf(),
    };

    let params = prune::PruneParams {
        force,
        use_gitoxide: settings.use_gitoxide,
        is_quiet: true,
        remote_name: remote_name.to_string(),
        prune_cd_target: settings.prune_cd_target,
    };

    let mut sink = NullBridge;
    match prune::prune_single_branch(
        &ctx,
        branch_name,
        worktree_map,
        is_bare_layout,
        current_wt_path,
        current_branch,
        &params,
        &mut sink,
    ) {
        Ok(result) => {
            if result.detail.worktree_removed || result.detail.branch_deleted {
                (TaskStatus::Succeeded, "removed".into())
            } else if result.deferred {
                // Deferred branches (current worktree) are still considered successful
                // but the actual removal happens after the TUI finishes.
                (TaskStatus::Succeeded, "deferred".into())
            } else {
                (TaskStatus::Succeeded, "no action needed".into())
            }
        }
        Err(e) => (TaskStatus::Failed, format!("prune failed: {e}")),
    }
}

/// Execute a single update task for a DAG worker.
fn execute_update_task(
    branch_name: &str,
    worktree_path: Option<&PathBuf>,
    settings: &DaftSettings,
    project_root: &std::path::Path,
    pull_args: &[String],
    force: bool,
) -> (TaskStatus, String) {
    let Some(target_path) = worktree_path else {
        return (TaskStatus::Failed, "no worktree path".into());
    };

    let git = GitCommand::new(false).with_gitoxide(settings.use_gitoxide);

    let worktree_name = target_path
        .strip_prefix(project_root)
        .ok()
        .and_then(|p| p.to_str())
        .unwrap_or(branch_name)
        .to_string();

    let params = fetch::FetchParams {
        targets: vec![],
        all: false,
        force,
        dry_run: false,
        rebase: pull_args.contains(&"--rebase".to_string()),
        autostash: pull_args.contains(&"--autostash".to_string()),
        ff_only: false,
        no_ff_only: false,
        pull_args: vec![],
        quiet: true,
        remote_name: settings.remote.clone(),
    };

    let mut sink = NullSink;
    let result = fetch::update_single_worktree(
        &git,
        target_path,
        &worktree_name,
        pull_args,
        &params,
        &mut sink,
    );

    if result.skipped {
        (TaskStatus::Skipped, result.message)
    } else if result.success {
        (TaskStatus::Succeeded, result.message)
    } else {
        (TaskStatus::Failed, result.message)
    }
}

/// Execute a single rebase task for a DAG worker.
fn execute_rebase_task(
    branch_name: &str,
    worktree_path: Option<&PathBuf>,
    base_branch: &str,
    project_root: &std::path::Path,
    settings: &DaftSettings,
    force: bool,
) -> (TaskStatus, String) {
    let Some(target_path) = worktree_path else {
        return (TaskStatus::Failed, "no worktree path".into());
    };

    let git = GitCommand::new(false).with_gitoxide(settings.use_gitoxide);

    let worktree_name = target_path
        .strip_prefix(project_root)
        .ok()
        .and_then(|p| p.to_str())
        .unwrap_or(branch_name)
        .to_string();

    let mut sink = NullSink;
    let result = rebase::rebase_single_worktree(
        &git,
        target_path,
        &worktree_name,
        base_branch,
        force,
        &mut sink,
    );

    if result.skipped {
        (TaskStatus::Skipped, result.message)
    } else if result.conflict {
        (TaskStatus::Succeeded, "conflict".into())
    } else if result.success {
        (TaskStatus::Succeeded, result.message)
    } else {
        (TaskStatus::Failed, result.message)
    }
}

fn run_prune_phase(
    output: &mut dyn Output,
    settings: &DaftSettings,
    force: bool,
) -> Result<prune::PruneResult> {
    let params = prune::PruneParams {
        force,
        use_gitoxide: settings.use_gitoxide,
        is_quiet: output.is_quiet(),
        remote_name: settings.remote.clone(),
        prune_cd_target: settings.prune_cd_target,
    };

    let hooks_config = HooksConfig::default();
    let executor = HookExecutor::new(hooks_config)?;

    output.start_spinner("Pruning stale branches...");
    let exec_result = {
        let mut bridge = CommandBridge::new(output, executor);
        prune::execute(&params, &mut bridge)
    };
    output.finish_spinner();
    let result = exec_result?;

    if result.nothing_to_prune {
        return Ok(result);
    }

    // Print header
    output.result(&format!("Pruning {}", result.remote_name));
    if let Some(ref url) = result.remote_url {
        output.info(&format!("URL: {url}"));
    }

    // Per-branch detail lines
    for detail in &result.pruned_branches {
        render_pruned_branch(detail, output);
    }

    // Summary
    if result.branches_deleted > 0 || result.worktrees_removed > 0 {
        let branch_word = if result.branches_deleted == 1 {
            "branch"
        } else {
            "branches"
        };
        let mut summary = format!("Pruned {} {branch_word}", result.branches_deleted);
        if result.worktrees_removed > 0 {
            let wt_word = if result.worktrees_removed == 1 {
                "worktree"
            } else {
                "worktrees"
            };
            summary.push_str(&format!(", removed {} {wt_word}", result.worktrees_removed));
        }
        output.success(&summary);
    }

    if result.has_prunable {
        output.warning(
            "Some prunable worktree data may exist. Run 'git worktree prune' to clean up.",
        );
    }

    Ok(result)
}

fn run_update_phase(output: &mut dyn Output, settings: &DaftSettings, force: bool) -> Result<()> {
    let wt_config = WorktreeConfig {
        remote_name: settings.remote.clone(),
        quiet: output.is_quiet(),
    };
    let git = GitCommand::new(wt_config.quiet).with_gitoxide(settings.use_gitoxide);
    let project_root = get_project_root()?;

    // Merge config-based args
    let config_args: Vec<&str> = settings.update_args.split_whitespace().collect();
    let config_has_rebase = config_args.contains(&"--rebase");
    let config_has_autostash = config_args.contains(&"--autostash");

    let params = fetch::FetchParams {
        targets: vec![],
        all: true,
        force,
        dry_run: false,
        rebase: config_has_rebase,
        autostash: config_has_autostash,
        ff_only: false,
        no_ff_only: false,
        pull_args: vec![],
        quiet: output.is_quiet(),
        remote_name: wt_config.remote_name.clone(),
    };

    output.start_spinner("Updating worktrees...");
    let exec_result = {
        let mut sink = OutputSink(output);
        fetch::execute(&params, &git, &project_root, &mut sink)
    };
    output.finish_spinner();
    let result = exec_result?;

    render_fetch_result(&result, output);

    if result.failed_count() > 0 {
        anyhow::bail!("{} worktree(s) failed to update", result.failed_count());
    }

    Ok(())
}

fn render_fetch_result(result: &fetch::FetchResult, output: &mut dyn Output) {
    if result.results.is_empty() {
        output.info("No worktrees to update.");
        return;
    }

    // Header
    output.result(&format!("Updating from {}", result.remote_name));
    if let Some(ref url) = result.remote_url {
        output.info(&format!("URL: {url}"));
    }

    // Per-worktree status
    for r in &result.results {
        render_worktree_status(r, output);
    }

    // Summary
    print_summary(result, output);
}

fn render_worktree_status(r: &fetch::WorktreeFetchResult, output: &mut dyn Output) {
    if r.skipped {
        output.info(&format!(" * {} {}", tag_skipped(), r.worktree_name));
    } else if r.success {
        if r.up_to_date {
            output.info(&format!(" * {} {}", tag_up_to_date(), r.worktree_name));
        } else {
            output.info(&format!(" * {} {}", tag_updated(), r.worktree_name));
            // Show captured pull output indented under the branch name
            if let Some(ref pull_output) = r.pull_output {
                for line in pull_output.lines() {
                    output.info(&format!("   {line}"));
                }
            }
        }
    } else {
        output.error(&format!(
            "Failed to update '{}': {}",
            r.worktree_name, r.message
        ));
        output.info(&format!(" * {} {}", tag_failed(), r.worktree_name));
    }
}

fn print_summary(result: &fetch::FetchResult, output: &mut dyn Output) {
    let updated = result.updated_count();
    let up_to_date = result.up_to_date_count();
    let skipped = result.skipped_count();
    let failed = result.failed_count();

    if failed == 0 {
        let mut parts: Vec<String> = Vec::new();
        if updated > 0 {
            let word = if updated == 1 {
                "worktree"
            } else {
                "worktrees"
            };
            parts.push(format!("Updated {updated} {word}"));
        }
        if up_to_date > 0 {
            let phrase = if up_to_date == 1 {
                "1 already up to date"
            } else {
                &format!("{up_to_date} already up to date")
            };
            parts.push(phrase.to_string());
        }
        if skipped > 0 {
            let word = if skipped == 1 {
                "worktree"
            } else {
                "worktrees"
            };
            if parts.is_empty() {
                parts.push(format!("Skipped {skipped} {word}"));
            } else {
                parts.push(format!("skipped {skipped} {word}"));
            }
        }
        if parts.is_empty() {
            output.info("Nothing to update");
        } else {
            output.success(&parts.join(", "));
        }
    } else {
        let mut parts: Vec<String> = Vec::new();
        if updated > 0 {
            let word = if updated == 1 {
                "worktree"
            } else {
                "worktrees"
            };
            parts.push(format!("{updated} {word} updated"));
        }
        if up_to_date > 0 {
            parts.push(format!("{up_to_date} already up to date"));
        }
        if skipped > 0 {
            let word = if skipped == 1 {
                "worktree"
            } else {
                "worktrees"
            };
            parts.push(format!("{skipped} {word} skipped"));
        }
        let word = if failed == 1 { "worktree" } else { "worktrees" };
        parts.push(format!("{failed} {word} failed"));
        output.error(&parts.join(", "));
    }
}

fn render_pruned_branch(detail: &prune::PrunedBranchDetail, output: &mut dyn Output) {
    // Build a description of what was removed: the branch is one entity
    // with up to three manifestations (worktree, local branch, remote tracking branch).
    let mut removed = Vec::new();
    if detail.worktree_removed {
        removed.push("worktree");
    }
    if detail.branch_deleted {
        removed.push("local branch");
    }
    // The remote tracking branch is always removed (git fetch --prune did it)
    removed.push("remote tracking branch");

    output.info(&format!(
        " * {} {} — removed {}",
        tag_pruned(),
        detail.branch_name,
        removed.join(", ")
    ));
}

fn run_rebase_phase(
    output: &mut dyn Output,
    settings: &DaftSettings,
    base_branch: &str,
    force: bool,
) -> Result<()> {
    let wt_config = WorktreeConfig {
        remote_name: settings.remote.clone(),
        quiet: output.is_quiet(),
    };
    let git = GitCommand::new(wt_config.quiet).with_gitoxide(settings.use_gitoxide);
    let project_root = get_project_root()?;

    let params = rebase::RebaseParams {
        base_branch: base_branch.to_string(),
        force,
        quiet: output.is_quiet(),
    };

    output.start_spinner("Rebasing worktrees...");
    let exec_result = {
        let mut sink = OutputSink(output);
        rebase::execute(&params, &git, &project_root, &mut sink)
    };
    output.finish_spinner();
    let result = exec_result?;

    render_rebase_result(&result, output);

    if result.conflict_count() > 0 {
        output.warning(&format!(
            "{} worktree(s) had conflicts and were aborted",
            result.conflict_count()
        ));
    }

    Ok(())
}

fn render_rebase_result(result: &rebase::RebaseResult, output: &mut dyn Output) {
    if result.results.is_empty() {
        output.info("No worktrees to rebase.");
        return;
    }

    // Header
    output.result(&format!("Rebasing onto {}", result.base_branch));

    // Per-worktree status
    for r in &result.results {
        render_rebase_worktree_status(r, output);
    }

    // Summary
    print_rebase_summary(result, output);
}

fn render_rebase_worktree_status(r: &rebase::WorktreeRebaseResult, output: &mut dyn Output) {
    if r.skipped {
        output.info(&format!(
            " * {} {} — uncommitted changes",
            tag_skipped(),
            r.worktree_name
        ));
    } else if r.conflict {
        output.info(&format!(
            " * {} {} — aborted",
            tag_conflict(),
            r.worktree_name
        ));
    } else if r.already_rebased {
        output.info(&format!(" * {} {}", tag_up_to_date(), r.worktree_name));
    } else if r.success {
        output.info(&format!(" * {} {}", tag_rebased(), r.worktree_name));
    } else {
        output.error(&format!(
            "Failed to rebase '{}': {}",
            r.worktree_name, r.message
        ));
        output.info(&format!(" * {} {}", tag_failed(), r.worktree_name));
    }
}

fn print_rebase_summary(result: &rebase::RebaseResult, output: &mut dyn Output) {
    let rebased = result.rebased_count();
    let already = result.already_rebased_count();
    let conflicts = result.conflict_count();
    let skipped = result.skipped_count();

    let mut parts: Vec<String> = Vec::new();

    if rebased > 0 {
        let word = if rebased == 1 {
            "worktree"
        } else {
            "worktrees"
        };
        parts.push(format!(
            "Rebased {rebased} {word} onto {}",
            result.base_branch
        ));
    }

    if already > 0 {
        let phrase = if already == 1 {
            "1 already up to date".to_string()
        } else {
            format!("{already} already up to date")
        };
        parts.push(phrase);
    }

    if conflicts > 0 {
        let word = if conflicts == 1 {
            "conflict"
        } else {
            "conflicts"
        };
        parts.push(format!("{conflicts} {word} (aborted)"));
    }

    if skipped > 0 {
        let word = if skipped == 1 {
            "worktree"
        } else {
            "worktrees"
        };
        parts.push(format!("skipped {skipped} {word}"));
    }

    if parts.is_empty() {
        output.info("Nothing to rebase");
    } else if conflicts > 0 {
        output.warning(&parts.join(", "));
    } else {
        output.success(&parts.join(", "));
    }
}

// -- Colored status tags --

fn tag_pruned() -> String {
    if styles::colors_enabled() {
        format!("{}[pruned]{}", styles::RED, styles::RESET)
    } else {
        "[pruned]".to_string()
    }
}

fn tag_updated() -> String {
    if styles::colors_enabled() {
        format!("{}[updated]{}", styles::GREEN, styles::RESET)
    } else {
        "[updated]".to_string()
    }
}

fn tag_up_to_date() -> String {
    if styles::colors_enabled() {
        format!("{}[up to date]{}", styles::DIM, styles::RESET)
    } else {
        "[up to date]".to_string()
    }
}

fn tag_skipped() -> String {
    if styles::colors_enabled() {
        format!("{}[skipped]{}", styles::YELLOW, styles::RESET)
    } else {
        "[skipped]".to_string()
    }
}

fn tag_rebased() -> String {
    if styles::colors_enabled() {
        format!("{}[rebased]{}", styles::GREEN, styles::RESET)
    } else {
        "[rebased]".to_string()
    }
}

fn tag_conflict() -> String {
    if styles::colors_enabled() {
        format!("{}[conflict]{}", styles::RED, styles::RESET)
    } else {
        "[conflict]".to_string()
    }
}

fn tag_failed() -> String {
    if styles::colors_enabled() {
        format!("{}[failed]{}", styles::RED, styles::RESET)
    } else {
        "[failed]".to_string()
    }
}
