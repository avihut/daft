//! Shared helpers used by both `sync` and `prune` TUI commands.
//!
//! Extracted to avoid code duplication between the two modules.

use crate::{
    core::{
        worktree::{
            prune,
            sync_dag::{DagEvent, OperationPhase, TaskStatus},
        },
        NullBridge,
    },
    git::GitCommand,
    output::{
        tui::{FinalStatus, TuiState, WorktreeStatus},
        CliOutput, Output, OutputConfig,
    },
    settings::DaftSettings,
    styles, CD_FILE_ENV,
};
use std::collections::HashMap;
use std::path::PathBuf;

/// Execute a single prune task for a DAG worker.
#[allow(clippy::too_many_arguments)]
pub fn execute_prune_task(
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

/// Render a single pruned branch detail line.
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

/// Colored "pruned" status tag.
fn tag_pruned() -> String {
    if styles::colors_enabled() {
        format!("{}\u{2014} pruned{}", styles::RED, styles::RESET)
    } else {
        "\u{2014} pruned".to_string()
    }
}

/// Handle deferred branch removal after the TUI finishes.
///
/// If a branch was deferred (because it is the current worktree), this function
/// performs the actual removal and writes the cd target for the shell wrapper.
pub fn handle_post_tui_deferred(
    deferred_branch: &std::sync::Arc<std::sync::Mutex<Option<String>>>,
    settings: &DaftSettings,
    project_root: &std::path::Path,
    git_dir: std::path::PathBuf,
    source_worktree: std::path::PathBuf,
    worktree_map: &HashMap<String, (PathBuf, bool)>,
    force: bool,
) {
    let deferred = deferred_branch.lock().unwrap().clone();
    if let Some(ref branch_name) = deferred {
        let git = GitCommand::new(false).with_gitoxide(settings.use_gitoxide);
        let ctx = prune::PruneContext {
            git: &git,
            project_root: project_root.to_path_buf(),
            git_dir,
            remote_name: settings.remote.clone(),
            source_worktree,
        };
        let params = prune::PruneParams {
            force,
            use_gitoxide: settings.use_gitoxide,
            is_quiet: true,
            remote_name: settings.remote.clone(),
            prune_cd_target: settings.prune_cd_target,
        };
        let mut sink = NullBridge;
        let cd_target =
            prune::handle_deferred_prune(&ctx, branch_name, worktree_map, &params, &mut sink);

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
}

/// Check if any TUI tasks failed and bail if so.
pub fn check_tui_failures(final_state: &TuiState) -> anyhow::Result<()> {
    let failed_count = final_state
        .worktrees
        .iter()
        .filter(|w| matches!(&w.status, WorktreeStatus::Done(FinalStatus::Failed)))
        .count();

    if failed_count > 0 {
        anyhow::bail!("{failed_count} task(s) failed");
    }

    Ok(())
}

/// Run the fetch phase of the DAG orchestrator.
///
/// Sends `TaskStarted(Fetch)`, runs `git fetch --prune`, and sends
/// `TaskCompleted` on success or `TaskCompleted(Failed)` + `AllDone` on failure.
/// Returns `true` if the fetch succeeded.
pub fn run_fetch_phase(
    tx: &std::sync::mpsc::Sender<DagEvent>,
    use_gitoxide: bool,
    remote: &str,
) -> bool {
    let _ = tx.send(DagEvent::TaskStarted {
        phase: OperationPhase::Fetch,
        branch_name: String::new(),
    });

    let fetch_git = GitCommand::new(false).with_gitoxide(use_gitoxide);
    let fetch_result = fetch_git.fetch(remote, true);

    if let Err(e) = fetch_result {
        let _ = tx.send(DagEvent::TaskCompleted {
            phase: OperationPhase::Fetch,
            branch_name: String::new(),
            status: TaskStatus::Failed,
            message: format!("fetch failed: {e}"),
            updated_info: None,
        });
        let _ = tx.send(DagEvent::AllDone);
        return false;
    }

    let _ = tx.send(DagEvent::TaskCompleted {
        phase: OperationPhase::Fetch,
        branch_name: String::new(),
        status: TaskStatus::Succeeded,
        message: "fetched".into(),
        updated_info: None,
    });

    true
}

/// Render the result of a sequential prune operation (header, details, summary).
pub fn render_prune_result(result: &prune::PruneResult, output: &mut dyn Output) {
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
}
