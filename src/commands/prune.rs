use crate::{
    core::{
        worktree::{
            list,
            list::Stat,
            prune,
            sync_dag::{
                DagEvent, DagExecutor, OperationPhase, SyncDag, SyncTask, TaskId, TaskStatus,
            },
        },
        CommandBridge, NullBridge,
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
    styles, CD_FILE_ENV,
};
use anyhow::Result;
use clap::Parser;
use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::Arc;

#[derive(Parser)]
#[command(name = "git-worktree-prune")]
#[command(version = crate::VERSION)]
#[command(about = "Remove worktrees and branches for deleted remote branches")]
#[command(long_about = r#"
Removes local branches whose corresponding remote tracking branches have been
deleted, along with any associated worktrees. This is useful for cleaning up
after branches have been merged and deleted on the remote.

The command first fetches from the remote with pruning enabled to update the
list of remote tracking branches. It then identifies local branches that were
tracking now-deleted remote branches, removes their worktrees (if any exist),
and finally deletes the local branches.

If you are currently inside a worktree that is about to be pruned, the command
handles this gracefully. In a bare-repo worktree layout (created by daft), the
current worktree is removed last and the shell is redirected to a safe location
(project root by default, or the default branch worktree if configured via
daft.prune.cdTarget). In a regular repository where the current branch is being
pruned, the command checks out the default branch before deleting the old branch.

Pre-remove and post-remove lifecycle hooks are executed for each worktree
removal if the repository is trusted. See git-daft(1) for hook management.
"#)]
pub struct Args {
    #[arg(short, long, help = "Be verbose; show detailed progress")]
    verbose: bool,

    #[arg(
        short,
        long,
        help = "Force removal of worktrees with uncommitted changes or untracked files"
    )]
    force: bool,
}

pub fn run() -> Result<()> {
    let args = Args::parse_from(crate::get_clap_args("git-worktree-prune"));

    init_logging(args.verbose);

    if !is_git_repository()? {
        anyhow::bail!("Not inside a Git repository");
    }

    let settings = DaftSettings::load()?;

    if std::io::IsTerminal::is_terminal(&std::io::stderr()) && !args.verbose {
        run_tui(args, settings)
    } else {
        run_prune(args, settings)
    }
}

/// Sequential (non-TTY) execution path — the original prune flow.
fn run_prune(args: Args, settings: DaftSettings) -> Result<()> {
    let config = OutputConfig::with_autocd(false, args.verbose, settings.autocd);
    let mut output = CliOutput::new(config);

    run_prune_inner(&mut output, &settings, args.force)?;
    Ok(())
}

fn run_prune_inner(output: &mut dyn Output, settings: &DaftSettings, force: bool) -> Result<()> {
    let params = prune::PruneParams {
        force,
        use_gitoxide: settings.use_gitoxide,
        is_quiet: output.is_quiet(),
        remote_name: settings.remote.clone(),
        prune_cd_target: settings.prune_cd_target,
    };

    let hooks_config = HooksConfig::default();
    let executor = HookExecutor::new(hooks_config)?;

    if should_show_gitoxide_notice(settings.use_gitoxide) {
        output.warning("[experimental] Using gitoxide backend for git operations");
    }

    output.start_spinner("Pruning stale branches...");
    let exec_result = {
        let mut bridge = CommandBridge::new(output, executor);
        prune::execute(&params, &mut bridge)
    };
    output.finish_spinner();
    let result = exec_result?;

    if result.nothing_to_prune {
        return Ok(());
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

    // Pluralized summary
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

    // Write the cd target for the shell wrapper
    if let Some(ref cd_target) = result.cd_target {
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
    // Fetch + identify gone branches before building the DAG.
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

    // ── Build the DAG (twice: one for executor, one for renderer) ──────
    let dag_for_renderer = SyncDag::build_prune(gone_branches.clone());
    let dag_for_executor = SyncDag::build_prune(gone_branches.clone());

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
    // Only show Fetch and Prune phases (not Update) for the prune command.
    let phases = vec![OperationPhase::Fetch, OperationPhase::Prune];
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
    let shared_force = args.force;
    let shared_is_bare_layout = is_bare_layout;

    // Pre-compute prune context values
    let shared_git_dir = Arc::new(get_git_common_dir()?);
    let shared_remote_name = Arc::new(settings.remote.clone());
    let shared_source_worktree = Arc::new(std::env::current_dir()?);

    // ── Spawn executor in a background thread ──────────────────────────
    let executor = DagExecutor::new(dag_for_executor, tx);

    let executor_handle = std::thread::spawn(move || {
        executor.run(move |task: &SyncTask| -> (TaskStatus, String) {
            match &task.id {
                TaskId::Fetch => {
                    // Already done pre-TUI
                    (TaskStatus::Succeeded, "fetched".into())
                }
                TaskId::Prune(branch_name) => execute_prune_task(
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
                ),
                TaskId::Update(_) | TaskId::Rebase(_) => {
                    // Should never happen in a prune-only DAG
                    (TaskStatus::Skipped, "not applicable".into())
                }
            }
        });
    });

    // ── Run TUI renderer on main thread ────────────────────────────────
    let renderer = TuiRenderer::new(state, Arc::clone(&dag_arc), rx);
    let final_state = renderer.run()?;

    // Wait for executor thread to finish
    executor_handle
        .join()
        .map_err(|_| anyhow::anyhow!("DAG executor thread panicked"))?;

    // ── Post-TUI: handle cd_target ─────────────────────────────────────
    // Check if the current worktree was pruned
    let current_was_pruned = final_state.worktrees.iter().any(|w| {
        w.info.is_current
            && matches!(
                &w.status,
                crate::output::tui::WorktreeStatus::Done(crate::output::tui::FinalStatus::Pruned)
            )
    });

    if current_was_pruned {
        let cd_target = project_root.clone();
        let config = OutputConfig::with_autocd(false, false, settings.autocd);
        let mut output = CliOutput::new(config);
        if std::env::var(CD_FILE_ENV).is_ok() {
            output.cd_path(&cd_target);
        } else {
            output.result(&format!(
                "Run `cd {}` (your previous working directory was removed)",
                cd_target.display()
            ));
        }
    }

    Ok(())
}

// ── DAG task execution function ────────────────────────────────────────────

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

fn render_pruned_branch(detail: &prune::PrunedBranchDetail, output: &mut dyn Output) {
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

fn tag_pruned() -> String {
    if styles::colors_enabled() {
        format!("{}[pruned]{}", styles::RED, styles::RESET)
    } else {
        "[pruned]".to_string()
    }
}
