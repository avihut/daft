//! git-worktree-sync - Synchronize worktrees with remote
//!
//! Orchestrates pruning stale branches/worktrees and updating all remaining
//! worktrees in a single command.
//!
//! When running in an interactive terminal, uses a DAG-based parallel executor
//! with an inline TUI (ratatui). Falls back to sequential execution when
//! stderr is not a TTY or verbose mode is enabled.

use super::sync_shared;
use crate::{
    core::{
        worktree::{
            fetch, list,
            list::Stat,
            prune, push, rebase,
            sync_dag::{
                self, DagExecutor, SyncDag, SyncTask, TaskId, TaskMessage, TaskOutcome, TaskStatus,
            },
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
use std::collections::{HashMap, HashSet};
use std::path::PathBuf;
use std::sync::Arc;

/// Parsed `--include` value.
enum IncludeFilter {
    Unowned,
    Email(String),
    Branch(String),
}

impl IncludeFilter {
    fn parse(value: &str) -> Self {
        if value == "unowned" {
            Self::Unowned
        } else if value.contains('@') {
            Self::Email(value.to_string())
        } else {
            Self::Branch(value.to_string())
        }
    }
}

/// Check if a branch is included by the filters or by ownership.
fn is_branch_included(
    branch: &str,
    owner_email: Option<&str>,
    user_email: Option<&str>,
    filters: &[IncludeFilter],
) -> bool {
    // Check ownership first
    if let (Some(owner), Some(user)) = (owner_email, user_email) {
        if owner == user {
            return true;
        }
    }
    // Check include filters
    for filter in filters {
        match filter {
            IncludeFilter::Unowned => return true,
            IncludeFilter::Email(email) => {
                if owner_email == Some(email.as_str()) {
                    return true;
                }
            }
            IncludeFilter::Branch(name) => {
                if branch == name {
                    return true;
                }
            }
        }
    }
    false
}

#[derive(Parser)]
#[command(name = "git-worktree-sync")]
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
  4. Push (--push): pushes all branches to their remote tracking branches.
     Branches without an upstream are skipped. Push failures are reported as
     warnings; they do not cause sync to fail. Use --force-with-lease with
     --push to force-push rebased branches.

If you are currently inside a worktree that gets pruned, the shell is redirected
to a safe location (project root by default, or as configured via
daft.prune.cdTarget).

For fine-grained control over either phase, use `daft prune` and `daft update`
separately.
"#)]
pub struct Args {
    #[arg(short, long, action = clap::ArgAction::Count,
          help = "Increase verbosity (-v for hook details, -vv for full sequential output)")]
    verbose: u8,

    #[arg(
        short = 'f',
        long = "prune-dirty",
        help = "Force removal of worktrees with uncommitted changes"
    )]
    prune_dirty: bool,

    /// Hidden deprecated alias for --prune-dirty.
    #[arg(long = "force", hide = true)]
    force_deprecated: bool,

    #[arg(
        long,
        value_name = "BRANCH",
        help = "Rebase all branches onto BRANCH after updating"
    )]
    rebase: Option<String>,

    #[arg(
        long,
        requires = "rebase",
        help = "Automatically stash and unstash uncommitted changes before/after rebase"
    )]
    autostash: bool,

    #[arg(long, help = "Push all branches to their remotes after syncing")]
    push: bool,

    #[arg(
        long,
        requires = "push",
        help = "Use --force-with-lease when pushing (requires --push)"
    )]
    force_with_lease: bool,

    #[arg(
        long,
        help = "Include additional branches in rebase/push (email, branch name, or 'unowned')"
    )]
    include: Vec<String>,

    #[arg(
        long,
        value_enum,
        help = "Statistics mode: summary or lines (default: from git config daft.sync.stat, or summary)"
    )]
    stat: Option<Stat>,

    #[arg(
        long,
        help = "Columns to display (comma-separated). Replace: branch,path,age. Modify defaults: +col,-col"
    )]
    columns: Option<String>,
}

impl Args {
    fn force(&self) -> bool {
        self.prune_dirty || self.force_deprecated
    }
}

pub fn run() -> Result<()> {
    let args = Args::parse_from(crate::get_clap_args("git-worktree-sync"));

    init_logging(args.verbose >= 2);

    if !is_git_repository()? {
        anyhow::bail!("Not inside a Git repository");
    }

    let settings = DaftSettings::load()?;

    // Validate --columns early so errors surface in both sequential and TUI modes.
    let columns_input = args.columns.as_deref().or(settings.sync_columns.as_deref());
    if let Some(input) = columns_input {
        use crate::core::columns::{ColumnSelection, CommandKind};
        ColumnSelection::parse(input, CommandKind::Sync).map_err(|e| anyhow::anyhow!("{e}"))?;
    }

    if !std::io::IsTerminal::is_terminal(&std::io::stderr()) || args.verbose >= 2 {
        run_sequential(args, settings)
    } else {
        run_tui(args, settings)
    }
}

/// Sequential (non-TTY) execution path — the original sync flow.
fn run_sequential(args: Args, settings: DaftSettings) -> Result<()> {
    let config = OutputConfig::with_autocd(false, args.verbose >= 2, settings.autocd);
    let mut output = CliOutput::new(config);

    if args.force_deprecated {
        output.warning("--force is deprecated, use --prune-dirty (or -f) instead");
    }

    if should_show_gitoxide_notice(settings.use_gitoxide) {
        output.warning("[experimental] Using gitoxide backend for git operations");
    }

    let force = args.force();

    // Determine the default branch for display annotations
    let default_branch = get_default_branch_local(
        &get_git_common_dir()?,
        &settings.remote,
        settings.use_gitoxide,
    )?;

    // Phase 1: Prune stale branches and worktrees
    let prune_result = run_prune_phase(&mut output, &settings, force)?;

    // Phase 2: Update all remaining worktrees
    run_update_phase(&mut output, &settings, force, &default_branch)?;

    // Phase 3: Rebase all worktrees onto base branch (if requested)
    let conflicted_branches: HashSet<String> = if let Some(ref base_branch) = args.rebase {
        let result = run_rebase_phase(
            &mut output,
            &settings,
            base_branch,
            force,
            args.autostash,
            &default_branch,
        )?;
        result
            .results
            .iter()
            .filter(|r| r.conflict)
            .map(|r| r.branch_name.clone())
            .collect()
    } else {
        HashSet::new()
    };

    // Phase 4: Push all branches to their remotes (if requested)
    if args.push {
        run_push_phase(
            &mut output,
            &settings,
            args.force_with_lease,
            &conflicted_branches,
            &default_branch,
        )?;
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
    let stat = args.stat.unwrap_or(settings.sync_stat);

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

    let worktree_infos = if stat == Stat::Lines {
        let mut output = CliOutput::new(OutputConfig::new(false, false));
        output.start_spinner("Computing line statistics...");
        let result =
            list::collect_worktree_info(&git, &base_branch, current_path.as_deref(), stat)?;
        output.finish_spinner();
        result
    } else {
        list::collect_worktree_info(&git, &base_branch, current_path.as_deref(), stat)?
    };

    // Get worktree list for DAG (branch name + path pairs)
    let all_worktrees = fetch::get_all_worktrees_with_branches(&git)?;

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
    if args.force_deprecated {
        eprintln!("warning: --force is deprecated, use --prune-dirty (or -f) instead");
    }

    let force = args.force();

    let mut phases = vec![
        sync_dag::OperationPhase::Fetch,
        sync_dag::OperationPhase::Prune,
        sync_dag::OperationPhase::Update,
    ];
    if let Some(ref base) = args.rebase {
        phases.push(sync_dag::OperationPhase::Rebase(base.clone()));
    }
    if args.push {
        phases.push(sync_dag::OperationPhase::Push);
    }
    let hooks_config = HooksConfig::default();
    let shared_hooks_config = Arc::new(hooks_config.clone());

    use crate::core::columns::{ColumnSelection, CommandKind};
    use crate::output::tui::Column;

    let columns_input = args.columns.or_else(|| settings.sync_columns.clone());
    let (tui_columns, columns_explicit) = match columns_input {
        Some(ref input) => {
            let resolved = ColumnSelection::parse(input, CommandKind::Sync)
                .map_err(|e| anyhow::anyhow!("{e}"))?;
            let tui_cols: Vec<Column> = resolved
                .columns
                .iter()
                .map(|c| Column::from_list_column(*c))
                .collect();
            (Some(tui_cols), resolved.explicit)
        }
        None => (None, false),
    };

    let cwd = std::env::current_dir().unwrap_or_else(|_| project_root.clone());

    // Compute the unowned section boundary for the TUI divider.
    // The TuiState sorts rows by kind (worktree < local-branch < remote-branch)
    // then alphabetically. Mirror that sort on the infos so the index matches.
    let user_email: Option<String> = git.config_get("user.email").ok().flatten();
    let include_filters: Vec<IncludeFilter> = args
        .include
        .iter()
        .map(|v| IncludeFilter::parse(v))
        .collect();
    let unowned_start_index = {
        let mut sorted = worktree_infos.clone();
        sorted.sort_by(|a, b| {
            let kind_order = |k: &list::EntryKind| match k {
                list::EntryKind::Worktree => 0,
                list::EntryKind::LocalBranch => 1,
                list::EntryKind::RemoteBranch => 2,
            };
            kind_order(&a.kind)
                .cmp(&kind_order(&b.kind))
                .then_with(|| a.name.to_lowercase().cmp(&b.name.to_lowercase()))
        });
        // Only show divider when user_email is known (otherwise all are unowned)
        user_email.as_ref().and_then(|_| {
            let idx = sorted.iter().position(|info| {
                !is_branch_included(
                    &info.name,
                    info.owner_email.as_deref(),
                    user_email.as_deref(),
                    &include_filters,
                )
            });
            // Only emit a boundary when there are both owned and unowned rows
            idx.filter(|&i| i > 0 && i < sorted.len())
        })
    };

    let state = TuiState::new(
        phases,
        worktree_infos,
        project_root.clone(),
        cwd,
        stat,
        args.verbose,
        tui_columns,
        columns_explicit,
        unowned_start_index,
    );

    // ── Create channel and spawn orchestrator ──────────────────────────
    let (tx, rx) = std::sync::mpsc::channel();

    // Shared context for workers
    let shared_settings = Arc::new(settings.clone());
    let shared_project_root = Arc::new(project_root.clone());
    let shared_worktree_map = Arc::new(worktree_map.clone());
    let shared_current_wt_path = Arc::new(git.get_current_worktree_path().ok());
    let shared_current_branch = Arc::new(git.symbolic_ref_short_head().ok());

    let config_args: Vec<&str> = settings.update_args.split_whitespace().collect();
    let config_has_rebase = config_args.contains(&"--rebase");
    let config_has_autostash = config_args.contains(&"--autostash");
    let shared_pull_args = Arc::new(fetch::build_pull_args(&fetch::FetchParams {
        targets: vec![],
        all: true,
        force,
        dry_run: false,
        rebase: config_has_rebase,
        autostash: config_has_autostash,
        ff_only: false,
        no_ff_only: false,
        pull_args: vec![],
        quiet: false,
        remote_name: settings.remote.clone(),
    }));
    let shared_force = force;
    let shared_autostash = args.autostash;
    let shared_is_bare_layout = is_bare_layout;

    let git_dir = get_git_common_dir()?;
    let shared_git_dir = Arc::new(git_dir.clone());
    let shared_remote_name = Arc::new(settings.remote.clone());
    let source_worktree = std::env::current_dir()?;
    let shared_source_worktree = Arc::new(source_worktree.clone());
    let shared_rebase_branch: Arc<Option<String>> = Arc::new(args.rebase.clone());
    let shared_push = args.push;
    let shared_force_with_lease = args.force_with_lease;

    let deferred_branch: Arc<std::sync::Mutex<Option<String>>> =
        Arc::new(std::sync::Mutex::new(None));
    let deferred_branch_writer = Arc::clone(&deferred_branch);

    // Shared worktree info map for live refresh after tasks complete
    let shared_info_map: Arc<HashMap<String, list::WorktreeInfo>> = Arc::new(
        state
            .worktrees
            .iter()
            .map(|wt| (wt.info.name.clone(), wt.info.clone()))
            .collect(),
    );
    let shared_base_branch = Arc::new(base_branch.clone());

    // Clone values needed by orchestrator
    let orch_settings = Arc::clone(&shared_settings);
    let orch_all_worktrees: Vec<(String, PathBuf)> = all_worktrees
        .iter()
        .map(|(p, b)| (b.clone(), p.clone()))
        .collect();
    let orch_info_map = Arc::clone(&shared_info_map);
    let orch_base_branch = Arc::clone(&shared_base_branch);
    let orch_stat = stat;

    // Ownership filtering for the orchestrator
    let shared_include: Arc<Vec<String>> = Arc::new(args.include.clone());
    // Build owner lookup from the worktree_infos collected before TUI started
    let shared_owner_lookup: Arc<HashMap<String, Option<String>>> = Arc::new(
        state
            .worktrees
            .iter()
            .map(|wt| (wt.info.name.clone(), wt.info.owner_email.clone()))
            .collect(),
    );
    let shared_user_email: Arc<Option<String>> =
        Arc::new(git.config_get("user.email").ok().flatten());

    let orchestrator_handle = std::thread::spawn(move || {
        // ── Phase 1: Fetch ─────────────────────────────────────────────
        if !sync_shared::run_fetch_phase(&tx, orch_settings.use_gitoxide, &orch_settings.remote) {
            return;
        }

        // ── Phase 2: Identify gone branches + build DAG ────────────────
        let gone_branches = {
            let git = GitCommand::new(false).with_gitoxide(orch_settings.use_gitoxide);
            let mut sink = NullBridge;
            prune::identify_gone_branches(
                &git,
                &shared_worktree_map,
                &orch_settings.remote,
                orch_settings.use_gitoxide,
                &mut sink,
            )
            .unwrap_or_default()
        };

        // Filter out gone branches so they don't get Update/Rebase tasks
        // (their worktree paths will be removed by the Prune tasks).
        let live_worktrees: Vec<(String, PathBuf)> = orch_all_worktrees
            .into_iter()
            .filter(|(branch, _)| !gone_branches.contains(branch))
            .collect();

        // Split worktrees into owned (rebase+push) and unowned (update only).
        let include_filters: Vec<IncludeFilter> = shared_include
            .iter()
            .map(|v| IncludeFilter::parse(v))
            .collect();

        let (owned, unowned): (Vec<_>, Vec<_>) =
            live_worktrees.into_iter().partition(|(branch, _)| {
                is_branch_included(
                    branch,
                    shared_owner_lookup.get(branch).and_then(|e| e.as_deref()),
                    shared_user_email.as_deref(),
                    &include_filters,
                )
            });

        let dag = SyncDag::build_sync(
            owned,
            unowned,
            gone_branches,
            shared_rebase_branch.as_ref().clone(),
            shared_push,
        );

        // ── Phase 3: Run the DAG executor (skips the Fetch task) ───────
        let tx_for_tasks = tx.clone();
        let executor = DagExecutor::new(dag, tx);
        executor.run(
            move |task: &SyncTask,
                  outcomes: &std::collections::HashSet<
                crate::core::worktree::sync_dag::TaskOutcome,
            >|
                  -> (
                TaskStatus,
                TaskMessage,
                std::collections::HashSet<crate::core::worktree::sync_dag::TaskOutcome>,
                Option<Box<list::WorktreeInfo>>,
            ) {
                match &task.id {
                    TaskId::Fetch => {
                        // Already done above
                        (
                            TaskStatus::Succeeded,
                            TaskMessage::Ok("fetched".into()),
                            outcomes.clone(),
                            None,
                        )
                    }
                    TaskId::Prune(branch_name) => {
                        let (status, message) = sync_shared::execute_prune_task(
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
                            &shared_hooks_config,
                            &tx_for_tasks,
                        );
                        if matches!(message, TaskMessage::Deferred) {
                            *deferred_branch_writer.lock().unwrap() = Some(branch_name.clone());
                        }
                        (status, message, outcomes.clone(), None)
                    }
                    TaskId::Update(branch_name) => {
                        let (status, message) = execute_update_task(
                            branch_name,
                            task.worktree_path.as_ref(),
                            &shared_settings,
                            &shared_project_root,
                            &shared_pull_args,
                            shared_force,
                        );
                        let updated = if status == TaskStatus::Succeeded {
                            orch_info_map.get(branch_name.as_str()).map(|info| {
                                let mut refreshed = info.clone();
                                refreshed.refresh_dynamic_fields(&orch_base_branch, orch_stat);
                                Box::new(refreshed)
                            })
                        } else {
                            None
                        };
                        (status, message, outcomes.clone(), updated)
                    }
                    TaskId::Rebase(branch_name) => {
                        let base = shared_rebase_branch.as_deref().unwrap_or("master");
                        let (status, message, new_outcomes) = execute_rebase_task(
                            branch_name,
                            task.worktree_path.as_ref(),
                            base,
                            &shared_project_root,
                            &shared_settings,
                            shared_force,
                            shared_autostash,
                            outcomes,
                        );
                        let updated = if status == TaskStatus::Succeeded
                            && !matches!(message, TaskMessage::Conflict)
                        {
                            orch_info_map.get(branch_name.as_str()).map(|info| {
                                let mut refreshed = info.clone();
                                refreshed.refresh_dynamic_fields(&orch_base_branch, orch_stat);
                                Box::new(refreshed)
                            })
                        } else {
                            None
                        };
                        (status, message, new_outcomes, updated)
                    }
                    TaskId::Push(branch_name) => {
                        let (status, message, new_outcomes) = execute_push_task(
                            branch_name,
                            task.worktree_path.as_ref(),
                            &shared_project_root,
                            &shared_settings,
                            shared_force_with_lease,
                            outcomes,
                        );
                        let updated = if status == TaskStatus::Succeeded {
                            orch_info_map.get(branch_name.as_str()).map(|info| {
                                let mut refreshed = info.clone();
                                refreshed.refresh_dynamic_fields(&orch_base_branch, orch_stat);
                                Box::new(refreshed)
                            })
                        } else {
                            None
                        };
                        (status, message, new_outcomes, updated)
                    }
                }
            },
        );
    });

    // ── Run TUI renderer on main thread ────────────────────────────────
    // Budget hook + job sub-rows per worktree (2 hooks × ~3 jobs each).
    // Not all worktrees will have hooks, but the ratatui inline viewport
    // cannot grow after creation, so over-allocate.
    let hook_extra_rows = if args.verbose >= 1 {
        (state.worktrees.len() as u16) * 8
    } else {
        0
    };
    let renderer = TuiRenderer::new(state, rx).with_extra_rows(5 + hook_extra_rows);
    let final_state = renderer.run()?;

    // Wait for orchestrator thread to finish
    orchestrator_handle
        .join()
        .map_err(|_| anyhow::anyhow!("DAG orchestrator thread panicked"))?;

    // ── Post-TUI: handle deferred branch (current worktree) ────────────
    sync_shared::handle_post_tui_deferred(
        &deferred_branch,
        &settings,
        &project_root,
        git_dir,
        source_worktree,
        &worktree_map,
        force,
        &hooks_config,
    );

    // ── Post-TUI: print hook summary ────────────────────────────────────
    if !final_state.hook_summaries.is_empty() {
        eprintln!();
        eprintln!("Hooks:");
        for entry in &final_state.hook_summaries {
            let status_word = if entry.warned { "warned" } else { "failed" };
            let exit_str = entry
                .exit_code
                .map(|c| format!("exit {c}"))
                .unwrap_or_else(|| "error".to_string());
            eprintln!(
                "  {}: {} {} ({}, {}ms)",
                entry.branch_name,
                entry.hook_type.filename(),
                status_word,
                exit_str,
                entry.duration.as_millis(),
            );
            if let Some(ref output) = entry.output {
                for line in output.lines() {
                    eprintln!("    {line}");
                }
            }
            if !entry.success && !entry.warned {
                eprintln!("    Prune was aborted for this branch.");
            }
        }
    }

    // ── Check for failures ────────────────────────────────────────────────
    sync_shared::check_tui_failures(&final_state)?;

    Ok(())
}

// ── DAG task execution functions ───────────────────────────────────────────

/// Execute a single update task for a DAG worker.
fn execute_update_task(
    branch_name: &str,
    worktree_path: Option<&PathBuf>,
    settings: &DaftSettings,
    project_root: &std::path::Path,
    pull_args: &[String],
    force: bool,
) -> (TaskStatus, TaskMessage) {
    let Some(target_path) = worktree_path else {
        return (
            TaskStatus::Failed,
            TaskMessage::Failed("no worktree path".into()),
        );
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
        (TaskStatus::Skipped, TaskMessage::Ok(result.message))
    } else if result.diverged {
        (TaskStatus::Succeeded, TaskMessage::Diverged)
    } else if result.success && result.up_to_date {
        (TaskStatus::Succeeded, TaskMessage::UpToDate)
    } else if result.success {
        (TaskStatus::Succeeded, TaskMessage::Ok(result.message))
    } else {
        (TaskStatus::Failed, TaskMessage::Failed(result.message))
    }
}

/// Execute a single rebase task for a DAG worker.
#[allow(clippy::too_many_arguments)]
fn execute_rebase_task(
    branch_name: &str,
    worktree_path: Option<&PathBuf>,
    base_branch: &str,
    project_root: &std::path::Path,
    settings: &DaftSettings,
    force: bool,
    autostash: bool,
    branch_outcomes: &HashSet<TaskOutcome>,
) -> (TaskStatus, TaskMessage, HashSet<TaskOutcome>) {
    let Some(target_path) = worktree_path else {
        return (
            TaskStatus::Failed,
            TaskMessage::Failed("no worktree path".into()),
            branch_outcomes.clone(),
        );
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
        branch_name,
        base_branch,
        force,
        autostash,
        &mut sink,
    );

    if result.skipped {
        (
            TaskStatus::Skipped,
            TaskMessage::Ok(result.message),
            branch_outcomes.clone(),
        )
    } else if result.conflict {
        let mut out = branch_outcomes.clone();
        out.insert(TaskOutcome::Conflict);
        (TaskStatus::Succeeded, TaskMessage::Conflict, out)
    } else if result.success {
        (
            TaskStatus::Succeeded,
            TaskMessage::Ok(result.message),
            branch_outcomes.clone(),
        )
    } else {
        (
            TaskStatus::Failed,
            TaskMessage::Failed(result.message),
            branch_outcomes.clone(),
        )
    }
}

/// Execute a single push task for a DAG worker.
fn execute_push_task(
    branch_name: &str,
    worktree_path: Option<&PathBuf>,
    project_root: &std::path::Path,
    settings: &DaftSettings,
    force_with_lease: bool,
    branch_outcomes: &HashSet<TaskOutcome>,
) -> (TaskStatus, TaskMessage, HashSet<TaskOutcome>) {
    let Some(target_path) = worktree_path else {
        return (
            TaskStatus::Failed,
            TaskMessage::Failed("no worktree path".into()),
            branch_outcomes.clone(),
        );
    };

    if branch_outcomes.contains(&TaskOutcome::Conflict) {
        return (
            TaskStatus::PreconditionFailed,
            TaskMessage::Failed("rebase conflict".into()),
            branch_outcomes.clone(),
        );
    }

    let git = GitCommand::new(false).with_gitoxide(settings.use_gitoxide);

    let worktree_name = target_path
        .strip_prefix(project_root)
        .ok()
        .and_then(|p| p.to_str())
        .unwrap_or(branch_name)
        .to_string();

    let params = push::PushParams {
        force_with_lease,
        remote_name: settings.remote.clone(),
    };

    let mut sink = NullSink;
    let result = push::push_single_worktree(
        &git,
        target_path,
        &worktree_name,
        branch_name,
        &params,
        &mut sink,
    );

    if result.no_upstream {
        (
            TaskStatus::Succeeded,
            TaskMessage::NoPushUpstream,
            branch_outcomes.clone(),
        )
    } else if result.success && result.up_to_date {
        (
            TaskStatus::Succeeded,
            TaskMessage::UpToDate,
            branch_outcomes.clone(),
        )
    } else if result.success {
        (
            TaskStatus::Succeeded,
            TaskMessage::Pushed,
            branch_outcomes.clone(),
        )
    } else {
        // Push failures are warnings, not hard failures — use Succeeded + Diverged
        // so that check_tui_failures does not count them as Failed.
        (
            TaskStatus::Succeeded,
            TaskMessage::Diverged,
            branch_outcomes.clone(),
        )
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

    sync_shared::render_prune_result(&result, output);

    Ok(result)
}

fn run_update_phase(
    output: &mut dyn Output,
    settings: &DaftSettings,
    force: bool,
    base_branch: &str,
) -> Result<()> {
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

    render_fetch_result(&result, output, base_branch);

    if result.failed_count() > 0 {
        anyhow::bail!("{} worktree(s) failed to update", result.failed_count());
    }

    Ok(())
}

fn render_fetch_result(result: &fetch::FetchResult, output: &mut dyn Output, base_branch: &str) {
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
        render_worktree_status(r, output, base_branch);
    }

    // Summary
    print_summary(result, output);
}

fn render_worktree_status(
    r: &fetch::WorktreeFetchResult,
    output: &mut dyn Output,
    base_branch: &str,
) {
    let name = styles::format_with_default_marker(&r.worktree_name, r.worktree_name == base_branch);
    if r.skipped {
        output.info(&format!(" * {} {name}", tag_skipped()));
    } else if r.diverged {
        output.warning(&format!(" * {} {name}", tag_diverged()));
    } else if r.success {
        if r.up_to_date {
            output.info(&format!(" * {} {name}", tag_up_to_date()));
        } else {
            output.info(&format!(" * {} {name}", tag_updated()));
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
        output.info(&format!(" * {} {name}", tag_failed()));
    }
}

fn print_summary(result: &fetch::FetchResult, output: &mut dyn Output) {
    let updated = result.updated_count();
    let up_to_date = result.up_to_date_count();
    let skipped = result.skipped_count();
    let diverged = result.diverged_count();
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
        if diverged > 0 {
            let word = if diverged == 1 {
                "worktree"
            } else {
                "worktrees"
            };
            if parts.is_empty() {
                parts.push(format!("{diverged} {word} diverged"));
            } else {
                parts.push(format!("{diverged} diverged"));
            }
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
        if diverged > 0 {
            parts.push(format!("{diverged} diverged"));
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

fn run_rebase_phase(
    output: &mut dyn Output,
    settings: &DaftSettings,
    base_branch: &str,
    force: bool,
    autostash: bool,
    default_branch: &str,
) -> Result<rebase::RebaseResult> {
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
        autostash,
    };

    output.start_spinner("Rebasing worktrees...");
    let exec_result = {
        let mut sink = OutputSink(output);
        rebase::execute(&params, &git, &project_root, &mut sink)
    };
    output.finish_spinner();
    let result = exec_result?;

    render_rebase_result(&result, output, default_branch);

    if result.conflict_count() > 0 {
        output.warning(&format!(
            "{} worktree(s) had conflicts and were aborted",
            result.conflict_count()
        ));
    }

    Ok(result)
}

fn render_rebase_result(
    result: &rebase::RebaseResult,
    output: &mut dyn Output,
    default_branch: &str,
) {
    if result.results.is_empty() {
        output.info("No worktrees to rebase.");
        return;
    }

    // Header
    output.result(&format!("Rebasing onto {}", result.base_branch));

    // Per-worktree status
    for r in &result.results {
        render_rebase_worktree_status(r, output, default_branch);
    }

    // Summary
    print_rebase_summary(result, output);
}

fn render_rebase_worktree_status(
    r: &rebase::WorktreeRebaseResult,
    output: &mut dyn Output,
    default_branch: &str,
) {
    let name =
        styles::format_with_default_marker(&r.worktree_name, r.branch_name == default_branch);
    if r.skipped {
        output.info(&format!(
            " * {} {name} — uncommitted changes",
            tag_skipped(),
        ));
    } else if r.conflict {
        output.info(&format!(" * {} {name} — aborted", tag_conflict(),));
    } else if r.already_rebased {
        output.info(&format!(" * {} {name}", tag_up_to_date()));
    } else if r.success {
        output.info(&format!(" * {} {name}", tag_rebased()));
    } else {
        output.error(&format!(
            "Failed to rebase '{}': {}",
            r.worktree_name, r.message
        ));
        output.info(&format!(" * {} {name}", tag_failed()));
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

fn run_push_phase(
    output: &mut dyn Output,
    settings: &DaftSettings,
    force_with_lease: bool,
    skip_branches: &HashSet<String>,
    base_branch: &str,
) -> Result<()> {
    let wt_config = WorktreeConfig {
        remote_name: settings.remote.clone(),
        quiet: output.is_quiet(),
    };
    let git = GitCommand::new(wt_config.quiet).with_gitoxide(settings.use_gitoxide);
    let project_root = get_project_root()?;

    let params = push::PushParams {
        force_with_lease,
        remote_name: wt_config.remote_name.clone(),
    };

    output.start_spinner("Pushing branches...");
    let exec_result = {
        let mut sink = OutputSink(output);
        push::execute(&params, &git, &project_root, &mut sink, skip_branches)
    };
    output.finish_spinner();
    let result = exec_result?;

    render_push_result(&result, output, base_branch);

    if result.failed_count() > 0 {
        output.warning(&format!(
            "{} branch(es) failed to push",
            result.failed_count()
        ));
    }

    Ok(())
}

fn render_push_result(result: &push::PushResult, output: &mut dyn Output, base_branch: &str) {
    if result.results.is_empty() {
        output.info("No branches to push.");
        return;
    }

    // Header
    output.result(&format!("Pushing to {}", result.remote_name));

    // Per-worktree status
    for r in &result.results {
        render_push_worktree_status(r, output, base_branch);
    }

    // Summary
    print_push_summary(result, output);
}

fn render_push_worktree_status(
    r: &push::WorktreePushResult,
    output: &mut dyn Output,
    base_branch: &str,
) {
    let name = styles::format_with_default_marker(&r.worktree_name, r.branch_name == base_branch);
    if r.no_upstream {
        output.info(&format!(" * {} {name}", tag_no_upstream()));
    } else if r.success {
        if r.up_to_date {
            output.info(&format!(" * {} {name}", tag_up_to_date()));
        } else {
            output.info(&format!(" * {} {name}", tag_pushed()));
        }
    } else {
        output.warning(&format!(" * {} {name}", tag_diverged()));
    }
}

fn print_push_summary(result: &push::PushResult, output: &mut dyn Output) {
    let pushed = result.pushed_count();
    let up_to_date = result.up_to_date_count();
    let no_upstream = result.no_upstream_count();
    let failed = result.failed_count();

    let mut parts: Vec<String> = Vec::new();

    if pushed > 0 {
        let word = if pushed == 1 { "branch" } else { "branches" };
        parts.push(format!("Pushed {pushed} {word}"));
    }

    if up_to_date > 0 {
        let phrase = if up_to_date == 1 {
            "1 already up to date".to_string()
        } else {
            format!("{up_to_date} already up to date")
        };
        parts.push(phrase);
    }

    if no_upstream > 0 {
        let word = if no_upstream == 1 {
            "branch"
        } else {
            "branches"
        };
        parts.push(format!("{no_upstream} {word} skipped (no remote)"));
    }

    if failed > 0 {
        let word = if failed == 1 { "branch" } else { "branches" };
        parts.push(format!("{failed} {word} failed to push"));
    }

    if parts.is_empty() {
        output.info("Nothing to push");
    } else if failed > 0 {
        output.warning(&parts.join(", "));
    } else {
        output.success(&parts.join(", "));
    }
}

// -- Colored status tags --

fn tag_updated() -> String {
    if styles::colors_enabled() {
        format!("{}\u{2713} updated{}", styles::GREEN, styles::RESET)
    } else {
        "\u{2713} updated".to_string()
    }
}

fn tag_up_to_date() -> String {
    if styles::colors_enabled() {
        format!("{}\u{2713} up to date{}", styles::DIM, styles::RESET)
    } else {
        "\u{2713} up to date".to_string()
    }
}

fn tag_diverged() -> String {
    if styles::colors_enabled() {
        format!("{}\u{2298} diverged{}", styles::YELLOW, styles::RESET)
    } else {
        "\u{2298} diverged".to_string()
    }
}

fn tag_skipped() -> String {
    if styles::colors_enabled() {
        format!("{}\u{2298} skipped{}", styles::YELLOW, styles::RESET)
    } else {
        "\u{2298} skipped".to_string()
    }
}

fn tag_rebased() -> String {
    if styles::colors_enabled() {
        format!("{}\u{2713} rebased{}", styles::GREEN, styles::RESET)
    } else {
        "\u{2713} rebased".to_string()
    }
}

fn tag_conflict() -> String {
    if styles::colors_enabled() {
        format!("{}\u{2717} conflict{}", styles::RED, styles::RESET)
    } else {
        "\u{2717} conflict".to_string()
    }
}

fn tag_failed() -> String {
    if styles::colors_enabled() {
        format!("{}\u{2717} failed{}", styles::RED, styles::RESET)
    } else {
        "\u{2717} failed".to_string()
    }
}

fn tag_pushed() -> String {
    if styles::colors_enabled() {
        format!("{}\u{2713} pushed{}", styles::GREEN, styles::RESET)
    } else {
        "\u{2713} pushed".to_string()
    }
}

fn tag_no_upstream() -> String {
    if styles::colors_enabled() {
        format!("{}\u{2298} no remote{}", styles::YELLOW, styles::RESET)
    } else {
        "\u{2298} no remote".to_string()
    }
}
