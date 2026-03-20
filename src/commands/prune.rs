use super::sync_shared;
use crate::{
    core::{
        sort::SortSpec,
        worktree::{
            list,
            list::Stat,
            prune,
            sync_dag::{
                DagEvent, DagExecutor, OperationPhase, SyncDag, SyncTask, TaskId, TaskMessage,
                TaskStatus,
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
    CD_FILE_ENV,
};
use anyhow::Result;
use clap::Parser;
use std::collections::{HashMap, HashSet};
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
    #[arg(short, long, action = clap::ArgAction::Count,
          help = "Increase verbosity (-v for hook details, -vv for full sequential output)")]
    verbose: u8,

    #[arg(
        short,
        long,
        help = "Force removal of worktrees with uncommitted changes or untracked files"
    )]
    force: bool,

    #[arg(
        long,
        value_enum,
        help = "Statistics mode: summary or lines (default: from git config daft.prune.stat, or summary)"
    )]
    stat: Option<Stat>,

    #[arg(
        long,
        help = "Columns to display (comma-separated). Replace: branch,path,age. Modify defaults: +col,-col"
    )]
    columns: Option<String>,

    #[arg(
        long,
        help = "Sort order (comma-separated). +col ascending, -col descending. Columns: branch, path, size, base, changes, remote, age, owner, activity, commit"
    )]
    sort: Option<String>,
}

pub fn run() -> Result<()> {
    let args = Args::parse_from(crate::get_clap_args("git-worktree-prune"));

    init_logging(args.verbose >= 2);

    if !is_git_repository()? {
        anyhow::bail!("Not inside a Git repository");
    }

    let settings = DaftSettings::load()?;

    // Validate --columns and --sort early so errors surface in both sequential and TUI modes.
    let columns_input = args
        .columns
        .as_deref()
        .or(settings.prune_columns.as_deref());
    if let Some(input) = columns_input {
        use crate::core::columns::{ColumnSelection, CommandKind};
        ColumnSelection::parse(input, CommandKind::Prune).map_err(|e| anyhow::anyhow!("{e}"))?;
    }
    let sort_input = args.sort.as_deref().or(settings.prune_sort.as_deref());
    if let Some(input) = sort_input {
        SortSpec::parse(input).map_err(|e| anyhow::anyhow!("{e}"))?;
    }

    let project_root = get_project_root()?;
    crate::core::worktree::temp_worktree::cleanup_stale(&project_root)?;

    if !std::io::IsTerminal::is_terminal(&std::io::stderr()) || args.verbose >= 2 {
        run_prune(args, settings)
    } else {
        run_tui(args, settings)
    }
}

/// Sequential (non-TTY) execution path — the original prune flow.
fn run_prune(args: Args, settings: DaftSettings) -> Result<()> {
    let config = OutputConfig::with_autocd(false, args.verbose >= 2, settings.autocd);
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

    sync_shared::render_prune_result(&result, output);

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
    let stat = args.stat.unwrap_or(settings.prune_stat);

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

    let sort_spec = {
        let sort_input = args.sort.as_deref().or(settings.prune_sort.as_deref());
        sort_input
            .map(|input| {
                SortSpec::parse(input)
                    .map(|s| s.with_stat(stat))
                    .map_err(|e| anyhow::anyhow!("{e}"))
            })
            .transpose()?
    };
    let has_size = {
        use crate::core::columns::{ColumnSelection, CommandKind, ListColumn};
        let from_columns = args
            .columns
            .as_deref()
            .or(settings.prune_columns.as_deref())
            .and_then(|input| ColumnSelection::parse(input, CommandKind::Prune).ok())
            .is_some_and(|r| r.columns.contains(&ListColumn::Size));
        from_columns || sort_spec.as_ref().is_some_and(|s| s.needs_size())
    };
    let compute_mtime = sort_spec.as_ref().is_some_and(|s| s.needs_mtime());
    let needs_spinner = stat == Stat::Lines || has_size;
    let mut worktree_infos = if needs_spinner {
        let mut output = CliOutput::new(OutputConfig::new(false, false));
        let msg = if stat == Stat::Lines {
            "Computing line statistics..."
        } else {
            "Computing worktree sizes..."
        };
        output.start_spinner(msg);
        let result = list::collect_worktree_info(
            &git,
            &base_branch,
            current_path.as_deref(),
            stat,
            has_size,
            compute_mtime,
        )?;
        output.finish_spinner();
        result
    } else {
        list::collect_worktree_info(
            &git,
            &base_branch,
            current_path.as_deref(),
            stat,
            has_size,
            compute_mtime,
        )?
    };

    // Parse worktree list for prune context
    let worktree_entries = prune::parse_worktree_list(&git)?;
    let is_bare_layout = worktree_entries.first().map(|e| e.is_bare).unwrap_or(false);

    let mut worktree_map: HashMap<String, (PathBuf, bool)> = HashMap::new();
    for (i, entry) in worktree_entries.iter().enumerate() {
        if let Some(ref branch) = entry.branch {
            worktree_map.insert(branch.clone(), (entry.path.clone(), i == 0));
        }
    }

    // ── Seed local-only gone branches (pre-fetch best-effort) ──────────
    // Identify branches already known to be gone from the last fetch so they
    // appear in the table immediately rather than popping in after fetch.
    {
        let gone_branches = {
            let mut sink = crate::core::NullBridge;
            prune::identify_gone_branches(
                &git,
                &worktree_map,
                &settings.remote,
                settings.use_gitoxide,
                &mut sink,
            )
            .unwrap_or_default()
        };

        let worktree_branch_set: HashSet<String> =
            worktree_infos.iter().map(|i| i.name.clone()).collect();

        let cwd = std::env::current_dir().unwrap_or_else(|_| project_root.clone());
        let mut stubs = Vec::new();
        for branch in &gone_branches {
            if !worktree_branch_set.contains(branch.as_str()) {
                let owner_email = list::get_author_email_for_ref(branch, &cwd);
                stubs.push(list::WorktreeInfo::local_branch_stub(branch, owner_email));
            }
        }
        worktree_infos.extend(stubs);
    }

    // ── Create TUI state with known phases and worktrees ───────────────
    use crate::core::columns::{ColumnSelection, CommandKind};
    use crate::output::tui::Column;

    let columns_input = args.columns.or_else(|| settings.prune_columns.clone());
    let (tui_columns, columns_explicit) = match columns_input {
        Some(ref input) => {
            let resolved = ColumnSelection::parse(input, CommandKind::Prune)
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

    let phases = vec![OperationPhase::Fetch, OperationPhase::Prune];
    let cwd = std::env::current_dir().unwrap_or_else(|_| project_root.clone());
    let state = TuiState::new(
        phases,
        worktree_infos,
        project_root.clone(),
        cwd,
        stat,
        args.verbose,
        tui_columns,
        columns_explicit,
        None,
        sort_spec,
    );

    let hooks_config = HooksConfig::default();

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
    let shared_hooks_config = Arc::new(hooks_config.clone());

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

        if gone_branches.is_empty() {
            // Nothing to prune — complete immediately
            let _ = tx.send(DagEvent::AllDone);
            return;
        }

        let dag = SyncDag::build_prune(gone_branches);

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
                    TaskId::Fetch => (
                        TaskStatus::Succeeded,
                        TaskMessage::Ok("fetched".into()),
                        outcomes.clone(),
                        None,
                    ),
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
                    TaskId::Update(_) | TaskId::Rebase(_) | TaskId::Push(_) => (
                        TaskStatus::Skipped,
                        TaskMessage::Ok("not applicable".into()),
                        outcomes.clone(),
                        None,
                    ),
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
        args.force,
        &hooks_config,
    );

    // ── Print hook summaries (warnings/failures) ──────────────────────────
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
