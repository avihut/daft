//! Shared helpers used by both `sync` and `prune` TUI commands.
//!
//! Extracted to avoid code duplication between the two modules.

use crate::{
    CD_FILE_ENV,
    core::{
        CommandBridge, TuiBridge,
        worktree::{
            info_field::FieldSet,
            list::{EntryKind, Stat},
            list_stream, prune,
            sync_dag::{DagEvent, OperationPhase, PatchSource, TaskMessage, TaskStatus},
        },
    },
    git::GitCommand,
    hooks::{HookExecutor, HooksConfig},
    output::{
        CliOutput, Output, OutputConfig,
        tui::{FinalStatus, WorktreeRow, WorktreeStatus},
    },
    settings::DaftSettings,
    styles,
};
use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::{Arc, mpsc};

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
    hooks_config: &HooksConfig,
    tx: &std::sync::mpsc::Sender<DagEvent>,
) -> (TaskStatus, TaskMessage) {
    let git = GitCommand::new(false).with_gitoxide(settings.use_gitoxide);
    let ctx = prune::PruneContext {
        git: &git,
        project_root: project_root.to_path_buf(),
        git_dir: git_dir.to_path_buf(),
        remote_name: remote_name.to_string(),
        source_worktree: source_worktree.to_path_buf(),
        default_branch: crate::remote::get_default_branch_local(
            git_dir,
            remote_name,
            settings.use_gitoxide,
        )
        .ok(),
    };

    let params = prune::PruneParams {
        force,
        use_gitoxide: settings.use_gitoxide,
        is_quiet: true,
        remote_name: remote_name.to_string(),
        prune_cd_target: settings.prune_cd_target,
    };

    let executor = match HookExecutor::new(hooks_config.clone()) {
        Ok(e) => e,
        Err(e) => {
            return (
                TaskStatus::Failed,
                TaskMessage::Failed(format!("failed to initialize hooks: {e}")),
            );
        }
    };
    let mut sink = TuiBridge::new(executor, tx.clone(), branch_name.to_string());
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
                (TaskStatus::Succeeded, TaskMessage::Removed)
            } else if result.deferred {
                // Deferred branches (current worktree) are still considered successful
                // but the actual removal happens after the TUI finishes.
                (TaskStatus::Succeeded, TaskMessage::Deferred)
            } else if result.skipped_dirty {
                (TaskStatus::Succeeded, TaskMessage::SkippedDirty)
            } else if result.skipped_refined {
                (TaskStatus::Succeeded, TaskMessage::SkippedRefined)
            } else if result.skipped_unmerged {
                (TaskStatus::Succeeded, TaskMessage::SkippedUnmerged)
            } else {
                (TaskStatus::Succeeded, TaskMessage::NoActionNeeded)
            }
        }
        Err(e) => (
            TaskStatus::Failed,
            TaskMessage::Failed(format!("prune failed: {e}")),
        ),
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
#[allow(clippy::too_many_arguments)]
pub fn handle_post_tui_deferred(
    deferred_branch: &std::sync::Arc<std::sync::Mutex<Option<String>>>,
    settings: &DaftSettings,
    project_root: &std::path::Path,
    git_dir: std::path::PathBuf,
    source_worktree: std::path::PathBuf,
    worktree_map: &HashMap<String, (PathBuf, bool)>,
    force: bool,
    hooks_config: &HooksConfig,
) {
    let deferred = deferred_branch.lock().unwrap().clone();
    if let Some(ref branch_name) = deferred {
        let git = GitCommand::new(false).with_gitoxide(settings.use_gitoxide);
        let default_branch = crate::remote::get_default_branch_local(
            &git_dir,
            &settings.remote,
            settings.use_gitoxide,
        )
        .ok();
        let ctx = prune::PruneContext {
            git: &git,
            project_root: project_root.to_path_buf(),
            git_dir,
            remote_name: settings.remote.clone(),
            source_worktree,
            default_branch,
        };
        let params = prune::PruneParams {
            force,
            use_gitoxide: settings.use_gitoxide,
            is_quiet: true,
            remote_name: settings.remote.clone(),
            prune_cd_target: settings.prune_cd_target,
        };
        // After TUI exits, we can use the full CLI output again
        let config = OutputConfig::with_autocd(false, false, settings.autocd);
        let mut cli_output = CliOutput::new(config);
        let executor = match HookExecutor::new(hooks_config.clone()) {
            Ok(e) => e,
            Err(_) => HookExecutor::new(HooksConfig {
                enabled: false,
                ..Default::default()
            })
            .unwrap(),
        };
        let cd_target = {
            let mut sink = CommandBridge::new(&mut cli_output, executor);
            prune::handle_deferred_prune(&ctx, branch_name, worktree_map, &params, &mut sink)
        };
        // sink dropped — cli_output is available again

        if let Some(ref cd_path) = cd_target {
            if std::env::var(CD_FILE_ENV).is_ok() {
                cli_output.cd_path(cd_path);
            } else {
                cli_output.result(&format!(
                    "Run `cd {}` (your previous working directory was removed)",
                    cd_path.display()
                ));
            }
        }
    }
}

/// Check if any TUI tasks failed and bail if so.
///
/// This variant is intentionally tolerant of hook failures: a row whose
/// filesystem-side task succeeded but whose hook aborted is still considered
/// a success. `prune` relies on this — it deliberately keeps going past hook
/// failures and reports them via the post-TUI summary without flipping the
/// process exit code.
pub fn check_tui_failures(rows: &[WorktreeRow]) -> anyhow::Result<()> {
    let failed_count = rows
        .iter()
        .filter(|w| matches!(&w.status, WorktreeStatus::Done(FinalStatus::Failed)))
        .count();

    if failed_count > 0 {
        anyhow::bail!("{failed_count} task(s) failed");
    }

    Ok(())
}

/// Strict variant of [`check_tui_failures`] used by `daft repo remove`.
///
/// repo-remove enforces stricter exit-code semantics than prune: per
/// `docs/superpowers/specs/2026-04-28-remove-repo-design.md` line 178, the
/// process exits non-zero if at least one hook failed in a non-warned mode,
/// even though the filesystem-side worktree removal proceeded (which leaves
/// the row at `WorktreeStatus::Done(FinalStatus::Pruned)`). The TUI marks
/// such a row with `hook_failed = true` (see
/// `src/output/tui/state.rs` — `HookCompleted` handler), so we treat that
/// flag as a run failure here.
///
/// The sequential path enforces the same rule directly in
/// `commands/repo/remove.rs::run_sequential` via `any_failed`. Without this
/// helper the TUI path would silently exit 0 in the same scenario.
///
/// Note: a row may be both `Done(Failed)` and `hook_failed`; we count it
/// once via a single OR-filter to keep the error message accurate.
///
/// Coverage: the strict-vs-tolerant asymmetry is unit-tested below. The
/// abort-mode end-to-end YAML scenario is deferred until the YAML config
/// surface exposes `fail_mode` for `worktree-pre-remove` (documented in
/// `tests/manual/scenarios/repo/remove-with-hooks.yml`).
pub fn check_tui_failures_strict(rows: &[WorktreeRow]) -> anyhow::Result<()> {
    let failed_count = rows
        .iter()
        .filter(|w| matches!(&w.status, WorktreeStatus::Done(FinalStatus::Failed)) || w.hook_failed)
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
            message: TaskMessage::Failed(format!("fetch failed: {e}")),
        });
        let _ = tx.send(DagEvent::AllDone);
        return false;
    }

    let _ = tx.send(DagEvent::TaskCompleted {
        phase: OperationPhase::Fetch,
        branch_name: String::new(),
        status: TaskStatus::Succeeded,
        message: TaskMessage::Ok("fetched".into()),
    });

    true
}

/// After the Fetch phase completes, re-run the streaming collector
/// against `REMOTE_DERIVED` fields for every worktree branch. Patches
/// arrive as `PatchSource::PostFetch` so `LiveTable` can suppress any
/// stale `Collector` patches on the same fields. Blocks on join() so
/// patches land before the orchestrator dispatches per-branch tasks.
pub fn spawn_post_fetch_refresh(
    worktree_map: &HashMap<String, (PathBuf, bool)>,
    settings: &Arc<DaftSettings>,
    base_branch: &str,
    user_email: Option<&str>,
    stat: Stat,
    git_common_dir: &std::path::Path,
    tx: &mpsc::Sender<DagEvent>,
) {
    let targets: Vec<list_stream::CollectorTarget> = worktree_map
        .iter()
        .map(
            |(branch_name, (path, _is_main))| list_stream::CollectorTarget {
                branch_name: branch_name.clone(),
                path: Some(path.clone()),
                kind: EntryKind::Worktree,
                is_detached: false,
            },
        )
        .collect();
    if targets.is_empty() {
        return;
    }
    let ctx = Arc::new(list_stream::CollectorContext {
        use_gitoxide: settings.use_gitoxide,
        base_branch: base_branch.to_string(),
        remote_name: settings.remote.clone(),
        ownership_strategy: settings.ownership_strategy,
        user_email: user_email.map(|s| s.to_string()),
        git_common_dir: git_common_dir.to_path_buf(),
    });
    let handle = list_stream::spawn(
        list_stream::CollectorRequest {
            targets,
            fields: FieldSet::REMOTE_DERIVED,
            stat,
            source: PatchSource::PostFetch,
            ctx,
        },
        tx.clone(),
    );
    handle.join();
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

    render_prune_skip_notes(&result.skipped_refined, &result.skipped_unmerged, output);
}

/// End-of-run notes about branches prune deliberately kept. Shared by the
/// sequential renderer above and the post-TUI summaries in `daft prune` /
/// `daft sync`.
pub fn render_prune_skip_notes(
    skipped_refined: &[String],
    skipped_unmerged: &[String],
    output: &mut dyn Output,
) {
    if !skipped_refined.is_empty() {
        let wt_word = if skipped_refined.len() == 1 {
            "worktree"
        } else {
            "worktrees"
        };
        output.warning(&format!(
            "Kept {} {wt_word} with refined daft files — consolidate with `daft file merge` \
             or re-run with --force: {}",
            skipped_refined.len(),
            skipped_refined.join(", ")
        ));
    }
    if !skipped_unmerged.is_empty() {
        let branch_word = if skipped_unmerged.len() == 1 {
            "branch"
        } else {
            "branches"
        };
        output.warning(&format!(
            "Skipped {} gone-but-unmerged {branch_word} (use --force to delete): {}",
            skipped_unmerged.len(),
            skipped_unmerged.join(", ")
        ));
    }
}

// ─────────────────────────────────────────────────────────────────────
// `daft repo remove` task execution
// ─────────────────────────────────────────────────────────────────────

use crate::core::worktree::remove_repo::{self, RepoTarget, WorktreeEntry};
use crate::hooks::{HookContext, HookType, RemovalReason};
use crate::output::BufferingOutput;
use crate::output::tui::TuiPresenter;

/// Execute one `RemoveWorktree` task.
///
/// Runs `worktree-pre-remove`, removes the worktree from disk via
/// [`remove_repo::remove_worktree_filesystem`], then runs
/// `worktree-post-remove`. Hook failures never abort the removal — they are
/// surfaced via `DagEvent::HookCompleted` events for the renderer to summarize.
///
/// `remote_name` is exposed to hooks as `$DAFT_REMOTE_NAME` (e.g. `origin`,
/// or whatever the user configured via `daft.remote`). `main_worktree_path`
/// is exposed as `$DAFT_SOURCE_WORKTREE` and should point at a directory
/// that is a real git working tree (typically the main worktree). When the
/// caller has no main worktree to offer (e.g. bare-only repo), pass `None`
/// and we fall back to `target.project_root`.
pub fn execute_remove_worktree_task(
    target: &RepoTarget,
    entry: &WorktreeEntry,
    hooks_config: &crate::hooks::HooksConfig,
    remote_name: &str,
    main_worktree_path: Option<&std::path::Path>,
    tx: &mpsc::Sender<DagEvent>,
) -> (TaskStatus, TaskMessage) {
    let label = entry.branch.clone().unwrap_or_else(|| "(detached)".into());
    let source_worktree = main_worktree_path.unwrap_or(&target.project_root);

    run_remove_hook_best_effort(
        target,
        entry,
        HookType::PreRemove,
        hooks_config,
        remote_name,
        source_worktree,
        tx,
        &label,
    );

    let outcome = remove_repo::remove_worktree_filesystem(target, &entry.path);
    if let Err(e) = outcome {
        return (
            TaskStatus::Failed,
            TaskMessage::Failed(format!("remove failed: {e}")),
        );
    }

    run_remove_hook_best_effort(
        target,
        entry,
        HookType::PostRemove,
        hooks_config,
        remote_name,
        source_worktree,
        tx,
        &label,
    );

    (TaskStatus::Succeeded, TaskMessage::Removed)
}

/// Execute the terminal `RemoveBare` task.
pub fn execute_remove_bare_task(target: &RepoTarget) -> (TaskStatus, TaskMessage) {
    match remove_repo::remove_bare_directory(target) {
        Ok(()) => (TaskStatus::Succeeded, TaskMessage::Removed),
        Err(e) => (
            TaskStatus::Failed,
            TaskMessage::Failed(format!("bare removal failed: {e}")),
        ),
    }
}

/// Run a remove hook for `entry` and forward lifecycle events to `tx`.
///
/// Uses [`TuiPresenter`] to send `HookStarted`/`HookCompleted` events through
/// the channel — the same machinery `TuiBridge` uses for sync/prune. If the
/// executor cannot be constructed (e.g. trust DB load fails), the call is a
/// silent no-op so the removal still proceeds. If `executor.execute()`
/// short-circuits with `Err` (FailMode::Abort), we still send a synthetic
/// `HookCompleted` so the renderer sees the failure — mirrors `TuiBridge`.
#[allow(clippy::too_many_arguments)]
fn run_remove_hook_best_effort(
    target: &RepoTarget,
    entry: &WorktreeEntry,
    hook_type: HookType,
    hooks_config: &crate::hooks::HooksConfig,
    remote_name: &str,
    source_worktree: &std::path::Path,
    tx: &mpsc::Sender<DagEvent>,
    label: &str,
) {
    let executor = match HookExecutor::new(hooks_config.clone()) {
        Ok(e) => e,
        Err(_) => return,
    };

    let ctx = HookContext::new(
        hook_type,
        "repo-remove",
        &target.project_root,
        &target.bare_git_dir,
        remote_name,
        source_worktree,
        &entry.path,
        entry.branch.clone().unwrap_or_default(),
    )
    .with_removal_reason(RemovalReason::Manual);

    let presenter = TuiPresenter::new(tx.clone(), label.to_string(), hook_type);
    let mut output = BufferingOutput::new();

    if let Err(e) = executor.execute(&ctx, &mut output, presenter) {
        // FailMode::Abort path — execute() bailed before on_phase_complete
        // ran, so HookStarted may be the only event the presenter sent.
        // Synthesize a HookCompleted with the bail message so the renderer
        // can surface it. Mirrors `TuiBridge::run_hook`.
        let _ = tx.send(DagEvent::HookCompleted {
            branch_name: label.to_string(),
            hook_type: hook_type.into(),
            success: false,
            warned: false,
            duration: std::time::Duration::ZERO,
            exit_code: None,
            output: Some(format!("{e:#}")),
        });
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::core::worktree::list::WorktreeInfo;
    use crate::output::tui::WorktreeRow;

    fn pruned_row(branch: &str) -> WorktreeRow {
        let mut row = WorktreeRow::idle(WorktreeInfo::local_branch_stub(branch, None));
        row.status = WorktreeStatus::Done(FinalStatus::Pruned);
        row
    }

    fn failed_row(branch: &str) -> WorktreeRow {
        let mut row = WorktreeRow::idle(WorktreeInfo::local_branch_stub(branch, None));
        row.status = WorktreeStatus::Done(FinalStatus::Failed);
        row
    }

    /// Regression test for the asymmetry fixed by `check_tui_failures_strict`:
    /// a non-warned hook failure leaves the row at `Done(Pruned)` because the
    /// filesystem-side worktree removal still succeeds. The tolerant variant
    /// (used by prune) returns Ok; the strict variant (used by repo-remove)
    /// must bail. See spec line 178 of the remove-repo design doc.
    #[test]
    fn strict_variant_bails_on_hook_failed_only() {
        let mut row = pruned_row("main");
        row.hook_failed = true;

        let strict_rows = [row];
        assert!(
            check_tui_failures_strict(&strict_rows).is_err(),
            "strict variant must bail on hook_failed even with Done(Pruned) status"
        );

        // Build a fresh equivalent row for the tolerant variant — WorktreeRow
        // isn't Clone, so we re-construct rather than try to share state.
        let mut tolerant_row = pruned_row("main");
        tolerant_row.hook_failed = true;
        let tolerant_rows = [tolerant_row];
        assert!(
            check_tui_failures(&tolerant_rows).is_ok(),
            "tolerant variant (used by prune) must NOT bail on hook_failed alone"
        );
    }

    /// Both variants agree when the row's task itself failed.
    #[test]
    fn both_variants_bail_on_done_failed() {
        let strict_rows = [failed_row("main")];
        assert!(check_tui_failures_strict(&strict_rows).is_err());
        let tolerant_rows = [failed_row("main")];
        assert!(check_tui_failures(&tolerant_rows).is_err());
    }

    /// A row marked both `Done(Failed)` AND `hook_failed` counts only once
    /// in the strict variant — no double-counting in the error message.
    #[test]
    fn strict_variant_counts_combined_failure_once() {
        let mut row = failed_row("main");
        row.hook_failed = true;
        let rows = [row];
        let err = check_tui_failures_strict(&rows).unwrap_err();
        assert!(
            err.to_string().starts_with("1 task(s) failed"),
            "expected single-failure count, got: {err}"
        );
    }

    /// A clean run (no hook failures, all rows succeeded) passes both
    /// variants.
    #[test]
    fn neither_variant_bails_on_clean_rows() {
        let rows = [pruned_row("main"), pruned_row("develop")];
        assert!(check_tui_failures(&rows).is_ok());
        assert!(check_tui_failures_strict(&rows).is_ok());
    }
}
