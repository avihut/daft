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

    // ── Pre-TUI: collect worktree info (no fetch needed) ───────────────
    let base_branch = get_default_branch_local(
        &get_git_common_dir()?,
        &settings.remote,
        settings.use_gitoxide,
    )
    .unwrap_or_else(|_| "master".to_string());

    let current_path = crate::core::repo::get_current_worktree_path()
        .ok()
        .and_then(|p| p.canonicalize().ok());

    let worktree_infos =
        list::collect_worktree_info(&git, &base_branch, current_path.as_deref(), Stat::Summary)?;

    // Parse worktree list for prune context
    let worktree_entries = prune::parse_worktree_list(&git)?;
    let is_bare_layout = worktree_entries.first().map(|e| e.is_bare).unwrap_or(false);

    let mut worktree_map: HashMap<String, (PathBuf, bool)> = HashMap::new();
    for (i, entry) in worktree_entries.iter().enumerate() {
        if let Some(ref branch) = entry.branch {
            worktree_map.insert(branch.clone(), (entry.path.clone(), i == 0));
        }
    }

    // ── Create TUI state with known phases and worktrees ───────────────
    let phases = vec![OperationPhase::Fetch, OperationPhase::Prune];
    let cwd = std::env::current_dir().unwrap_or_else(|_| project_root.clone());
    let state = TuiState::new(phases, worktree_infos, project_root.clone(), cwd);

    // ── Create channel and spawn orchestrator ──────────────────────────
    let (tx, rx) = std::sync::mpsc::channel();

    let shared_settings = Arc::new(settings.clone());
    let shared_project_root = Arc::new(project_root.clone());
    let shared_worktree_map = Arc::new(worktree_map.clone());
    let shared_current_wt_path = Arc::new(git.get_current_worktree_path().ok());
    let shared_current_branch = Arc::new(git.symbolic_ref_short_head().ok());
    let shared_force = args.force;
    let shared_is_bare_layout = is_bare_layout;

    let git_dir = get_git_common_dir()?;
    let shared_git_dir = Arc::new(git_dir.clone());
    let shared_remote_name = Arc::new(settings.remote.clone());
    let source_worktree = std::env::current_dir()?;
    let shared_source_worktree = Arc::new(source_worktree.clone());

    let deferred_branch: Arc<std::sync::Mutex<Option<String>>> =
        Arc::new(std::sync::Mutex::new(None));
    let deferred_branch_writer = Arc::clone(&deferred_branch);

    let orch_settings = Arc::clone(&shared_settings);

    let orchestrator_handle = std::thread::spawn(move || {
        // ── Phase 1: Fetch ─────────────────────────────────────────────
        let _ = tx.send(DagEvent::TaskStarted {
            phase: OperationPhase::Fetch,
            branch_name: String::new(),
        });

        let fetch_git = GitCommand::new(false).with_gitoxide(orch_settings.use_gitoxide);
        let fetch_result = fetch_git.fetch(&orch_settings.remote, true);

        if let Err(e) = fetch_result {
            let _ = tx.send(DagEvent::TaskCompleted {
                phase: OperationPhase::Fetch,
                branch_name: String::new(),
                status: TaskStatus::Failed,
                message: format!("fetch failed: {e}"),
            });
            let _ = tx.send(DagEvent::AllDone);
            return;
        }

        let _ = tx.send(DagEvent::TaskCompleted {
            phase: OperationPhase::Fetch,
            branch_name: String::new(),
            status: TaskStatus::Succeeded,
            message: "fetched".into(),
        });

        // ── Phase 2: Identify gone branches + build DAG ────────────────
        let gone_branches = {
            let mut sink = NullBridge;
            prune::identify_gone_branches(
                &fetch_git,
                &shared_worktree_map,
                &orch_settings.remote,
                orch_settings.use_gitoxide,
                &mut sink,
            )
            .unwrap_or_default()
        };

        if gone_branches.is_empty() {
            // Nothing to prune — complete immediately
            let _ = tx.send(DagEvent::AllDone);
            return;
        }

        let dag = SyncDag::build_prune(gone_branches);

        // ── Phase 3: Run the DAG executor (skips the Fetch task) ───────
        let executor = DagExecutor::new(dag, tx);
        executor.run(move |task: &SyncTask| -> (TaskStatus, String) {
            match &task.id {
                TaskId::Fetch => (TaskStatus::Succeeded, "fetched".into()),
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
                TaskId::Update(_) | TaskId::Rebase(_) => {
                    (TaskStatus::Skipped, "not applicable".into())
                }
            }
        });
    });

    // ── Run TUI renderer on main thread ────────────────────────────────
    let renderer = TuiRenderer::new(state, rx).with_extra_rows(5);
    let final_state = renderer.run()?;

    // Wait for orchestrator thread to finish
    orchestrator_handle
        .join()
        .map_err(|_| anyhow::anyhow!("DAG orchestrator thread panicked"))?;

    // ── Post-TUI: handle deferred branch (current worktree) ────────────
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

    // ── Check for failures ────────────────────────────────────────────────
    let failed_count = final_state
        .worktrees
        .iter()
        .filter(|w| {
            matches!(
                &w.status,
                crate::output::tui::WorktreeStatus::Done(crate::output::tui::FinalStatus::Failed)
            )
        })
        .count();

    if failed_count > 0 {
        anyhow::bail!("{failed_count} task(s) failed");
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
        format!("{}\u{2014} pruned{}", styles::RED, styles::RESET)
    } else {
        "\u{2014} pruned".to_string()
    }
}
