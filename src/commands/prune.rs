use super::sync_shared;
use crate::{
    CD_FILE_ENV,
    core::{
        CommandBridge, NullBridge,
        sort::SortSpec,
        worktree::{
            info_field::FieldSet,
            list,
            list::Stat,
            list_stream, prune,
            sync_dag::{
                DagEvent, DagExecutor, OperationPhase, PatchSource, SyncDag, SyncTask, TaskId,
                TaskMessage, TaskStatus,
            },
        },
    },
    get_git_common_dir, get_project_root,
    git::{GitCommand, should_show_gitoxide_notice},
    hooks::HookExecutor,
    is_git_repository,
    logging::init_logging,
    output::{
        CliOutput, Output, OutputConfig,
        tui::operation_table::{OperationTable, TableConfig},
    },
    remote::get_default_branch_local,
    settings::DaftSettings,
};
use anyhow::Result;
use clap::Parser;
use std::collections::{HashMap, HashSet};
use std::path::PathBuf;
use std::sync::Arc;

#[derive(Parser, Clone)]
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

A deleted remote branch does not by itself prove the work was merged, so each
gone branch is verified against the default branch (regular or squash merge)
before anything is deleted; gone-but-unmerged branches are kept with a
warning. Worktrees whose untracked daft files (daft.yml / daft.local.yml)
were refined since daft seeded them are also kept, with a pointer at
daft-file(1) merge for consolidation. --force overrides both: unmerged
branches are deleted and refined daft files are discarded to
`<git-common-dir>/.daft/discarded/<branch>/` — prune never writes another
worktree's files.

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
        help = "Columns to display (comma-separated). Replace: branch,path,age. Modify defaults: +col,-col. Available: branch, path, size, base, changes, remote, pr, age, annotation, owner, hash, last-commit"
    )]
    columns: Option<String>,

    #[arg(
        long,
        help = "Sort order (comma-separated). +col ascending, -col descending. Columns: branch, path, size, base, changes, remote, age, owner, hash, activity, commit"
    )]
    sort: Option<String>,

    #[arg(
        long = "repo",
        value_name = "REPO",
        conflicts_with = "all_repos",
        help = "Prune another cataloged repository"
    )]
    repo: Option<String>,

    #[arg(
        long = "all-repos",
        help = "Prune every cataloged repository (current repo last)"
    )]
    all_repos: bool,
}

pub fn run() -> Result<()> {
    let args = Args::parse_from(crate::get_clap_args("git-worktree-prune"));

    init_logging(args.verbose >= 2);

    // Fleet scopes: sequential path per repo (one TUI per repo would
    // churn), current repo last so its cwd-redirect semantics are intact.
    if args.repo.is_some() || args.all_repos {
        if is_git_repository()? {
            crate::catalog::touch_current_repo();
        }
        let scope = match &args.repo {
            Some(needle) => crate::catalog::fleet::FleetScope::Single(needle.clone()),
            None => crate::catalog::fleet::FleetScope::AllRepos,
        };
        let mut output = CliOutput::new(OutputConfig::default());
        let outcome = crate::catalog::fleet::for_each_repo(
            scope,
            /* current_repo_last */ true,
            &mut output,
            |_row| {
                let mut repo_args = args.clone();
                repo_args.repo = None;
                repo_args.all_repos = false;
                let settings = DaftSettings::load()?;
                validate_view_args(&repo_args, &settings)?;
                let project_root = get_project_root()?;
                crate::core::worktree::temp_worktree::cleanup_stale(&project_root)?;
                run_prune(repo_args, settings)
            },
        )?;
        return outcome.into_result();
    }

    if !is_git_repository()? {
        anyhow::bail!("Not inside a Git repository");
    }
    crate::catalog::touch_current_repo();

    let settings = DaftSettings::load()?;

    validate_view_args(&args, &settings)?;

    let project_root = get_project_root()?;
    crate::core::worktree::temp_worktree::cleanup_stale(&project_root)?;

    if !std::io::IsTerminal::is_terminal(&std::io::stderr()) || args.verbose >= 2 {
        run_prune(args, settings)
    } else {
        run_tui(args, settings)
    }
}

/// Validate --columns and --sort early so errors surface in both
/// sequential and TUI modes.
fn validate_view_args(args: &Args, settings: &DaftSettings) -> Result<()> {
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
    Ok(())
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
        cancel: None,
    };

    let hooks_config = crate::core::settings::load_hooks_config()?;
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
    let user_email: Option<String> = git.config_get("user.email").ok().flatten();
    let project_root = get_project_root()?;
    let stat = args.stat.unwrap_or(settings.prune_stat);

    // ── Pre-TUI: collect worktree info (no fetch needed) ───────────────
    let git_common_dir = get_git_common_dir()?;
    let base_branch =
        get_default_branch_local(&git_common_dir, &settings.remote, settings.use_gitoxide)
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
    // Resolve columns once: defaults follow list's shape (pr included), and
    // the same silent visibility gate as `daft list` drops a default-sourced
    // pr for repos with no forge or a persistently broken one — reading the
    // PR-cache lookup for the table's decorations in the same store open.
    let (table_columns, columns_explicit, forge_lookup) = {
        use crate::core::columns::{ColumnSelection, CommandKind, ListColumn, ResolvedColumns};
        let columns_input = args
            .columns
            .as_deref()
            .or(settings.prune_columns.as_deref());
        let resolved = match columns_input {
            Some(input) => ColumnSelection::parse(input, CommandKind::Prune)
                .map_err(|e| anyhow::anyhow!("{e}"))?,
            None => ResolvedColumns::defaults(ListColumn::tui_defaults()),
        };
        let (effective, gate) = crate::commands::list::gate_pr_column(
            &resolved.columns,
            columns_input,
            &git,
            &git_common_dir,
        );
        let lookup = effective
            .contains(&ListColumn::Pr)
            .then(|| gate.and_then(|g| g.lookup))
            .flatten();
        (effective, resolved.explicit, lookup)
    };
    let has_size = {
        use crate::core::columns::ListColumn;
        table_columns.contains(&ListColumn::Size)
            || sort_spec.as_ref().is_some_and(|s| s.needs_size())
    };
    let has_pr = table_columns.contains(&crate::core::columns::ListColumn::Pr);
    let compute_mtime = sort_spec.as_ref().is_some_and(|s| s.needs_mtime());

    // Synchronous seed: compute everything EXCEPT the heavy cells (size,
    // mtime, line stats). Those will stream in via the collector below.
    // shared_owner_lookup (built later) depends on this call's output.
    let mut worktree_infos = list::collect_worktree_info(
        &git,
        &base_branch,
        current_path.as_deref(),
        Stat::Summary, // Force Summary; line stats stream below.
        false,         // has_size = false: stream the size cluster instead
        false,         // compute_mtime = false: stream the mtime cluster
        has_pr,        // inbound `pr:N` tracking refs for the PR column
        settings.ownership_strategy,
        user_email.as_deref(),
        &settings.remote,
        crate::core::size_walk::resolve_jobs(None), // has_size=false: size streams via the collector
    )?;

    // Parse worktree list for prune context
    let worktree_entries = prune::parse_worktree_list(&git)?;
    let is_bare_layout = worktree_entries.first().map(|e| e.is_bare).unwrap_or(false);

    let mut worktree_map: HashMap<String, (PathBuf, bool)> = HashMap::new();
    for (i, entry) in worktree_entries.iter().enumerate() {
        if let Some(ref branch) = entry.branch {
            worktree_map.insert(branch.clone(), (entry.path.clone(), i == 0));
        }
    }

    // Heavy cells the user requested but the seed deliberately skipped.
    // These will arrive via the streaming collector concurrent with the
    // orchestrator below.
    let mut streaming_fields = FieldSet::EMPTY;
    if has_size {
        streaming_fields |= FieldSet::SIZE;
    }
    if compute_mtime {
        streaming_fields |= FieldSet::MTIME;
    }
    if stat == Stat::Lines {
        streaming_fields |= FieldSet::BASE_LINES | FieldSet::CHANGES_LINES | FieldSet::REMOTE_LINES;
    }
    // Bits for fields *not* arriving via the streaming collector. Pre-marking
    // these in each row's `received_patches` prevents the loading shimmer
    // from animating forever for cells the collector won't emit a patch for
    // (e.g. `info.owner = None` for the default branch row).
    let seeded_fields = !streaming_fields;

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
                Some(base_branch.as_str()),
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
                let owner = crate::core::ownership::resolve_owner_with_fallbacks(
                    &base_branch,
                    branch,
                    &cwd,
                    settings.ownership_strategy,
                    user_email.as_deref(),
                    Some(&settings.remote),
                );
                stubs.push(list::WorktreeInfo::local_branch_stub(branch, owner));
            }
        }
        worktree_infos.extend(stubs);
    }

    // ── Create TUI state with known phases and worktrees ───────────────
    use crate::output::tui::Column;

    // The gated column set resolved above, in TUI form. Always Some: the
    // defaults were already resolved (and pr-gated) at the ListColumn level,
    // so the TUI's own ALL_COLUMNS fallback never applies to prune.
    let tui_columns: Option<Vec<Column>> = Some(
        table_columns
            .iter()
            .map(|c| Column::from_list_column(*c))
            .collect(),
    );

    let phases = vec![OperationPhase::Fetch, OperationPhase::Prune];
    let cwd = std::env::current_dir().unwrap_or_else(|_| project_root.clone());

    // Capture count before worktree_infos is moved into OperationTable.
    let worktree_count = worktree_infos.len();

    let hooks_config = crate::core::settings::load_hooks_config()?;

    // ── Create channel and spawn orchestrator ──────────────────────────
    let (tx, rx) = std::sync::mpsc::channel();
    // Clone for the streaming collector below, since `tx` is moved into the
    // orchestrator closure.
    let tx_for_collector = tx.clone();

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

    // Branches prune deliberately kept (refined daft files / unmerged) —
    // surfaced after the TUI exits, mirroring the deferred-branch pattern.
    let skipped_refined: Arc<std::sync::Mutex<Vec<String>>> =
        Arc::new(std::sync::Mutex::new(Vec::new()));
    let skipped_refined_writer = Arc::clone(&skipped_refined);
    let skipped_unmerged: Arc<std::sync::Mutex<Vec<String>>> =
        Arc::new(std::sync::Mutex::new(Vec::new()));
    let skipped_unmerged_writer = Arc::clone(&skipped_unmerged);

    let orch_settings = Arc::clone(&shared_settings);
    let shared_hooks_config = Arc::new(hooks_config.clone());

    // Captures for the post-fetch refresh inside the orchestrator thread.
    let orch_base_branch = Arc::new(base_branch.clone());
    let orch_user_email: Arc<Option<String>> = Arc::new(user_email.clone());
    let orch_stat = stat;

    let orchestrator_handle = std::thread::spawn(move || {
        // ── Phase 1: Fetch ─────────────────────────────────────────────
        if !sync_shared::run_fetch_phase(
            &tx,
            orch_settings.use_gitoxide,
            &orch_settings.remote,
            None,
        ) {
            return;
        }

        // ── Refresh remote-derived cells now that fetch updated remote refs ──
        sync_shared::spawn_post_fetch_refresh(
            &shared_worktree_map,
            &orch_settings,
            &orch_base_branch,
            orch_user_email.as_deref(),
            orch_stat,
            &shared_git_dir,
            &tx,
        );

        // ── Phase 2: Identify gone branches + build DAG ────────────────
        let gone_branches = {
            let git = GitCommand::new(false).with_gitoxide(orch_settings.use_gitoxide);
            let mut sink = NullBridge;
            prune::identify_gone_branches(
                &git,
                &shared_worktree_map,
                &orch_settings.remote,
                orch_settings.use_gitoxide,
                Some(orch_base_branch.as_str()),
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
            ) {
                match &task.id {
                    TaskId::Fetch => (
                        TaskStatus::Succeeded,
                        TaskMessage::Ok("fetched".into()),
                        outcomes.clone(),
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
                        if matches!(message, TaskMessage::SkippedRefined) {
                            skipped_refined_writer
                                .lock()
                                .unwrap()
                                .push(branch_name.clone());
                        }
                        if matches!(message, TaskMessage::SkippedUnmerged) {
                            skipped_unmerged_writer
                                .lock()
                                .unwrap()
                                .push(branch_name.clone());
                        }
                        (status, message, outcomes.clone())
                    }
                    TaskId::Update(_) | TaskId::Rebase(_) | TaskId::Push(_) | TaskId::PushBatch => {
                        (
                            TaskStatus::Skipped,
                            TaskMessage::Ok("not applicable".into()),
                            outcomes.clone(),
                        )
                    }
                    TaskId::Setup(_) => unreachable!("Setup is only used by clone"),
                    TaskId::RemoveWorktree(_) | TaskId::RemoveBare => {
                        unreachable!("RemoveWorktree/RemoveBare are only used by repo remove")
                    }
                }
            },
        );
    });

    // Streaming collector for heavy cells (concurrent with orchestrator).
    let collector_handle = if !streaming_fields.is_empty() {
        let targets: Vec<list_stream::CollectorTarget> = worktree_map
            .iter()
            .map(
                |(branch_name, (path, _is_main))| list_stream::CollectorTarget {
                    branch_name: branch_name.clone(),
                    path: Some(path.clone()),
                    kind: list::EntryKind::Worktree,
                    is_detached: false,
                },
            )
            .collect();
        let ctx = Arc::new(list_stream::CollectorContext {
            use_gitoxide: settings.use_gitoxide,
            base_branch: base_branch.clone(),
            remote_name: settings.remote.clone(),
            ownership_strategy: settings.ownership_strategy,
            user_email: user_email.clone(),
            git_common_dir: git_dir.clone(),
        });
        Some(list_stream::spawn(
            list_stream::CollectorRequest {
                targets,
                fields: streaming_fields,
                stat,
                source: PatchSource::Collector,
                ctx,
                size_jobs: crate::core::size_walk::resolve_jobs(None),
            },
            tx_for_collector,
        ))
    } else {
        None
    };

    // ── Run TUI via OperationTable on main thread ──────────────────────
    // Budget hook + job sub-rows per worktree (2 hooks × ~3 jobs each).
    // Not all worktrees will have hooks, but the ratatui inline viewport
    // cannot grow after creation, so over-allocate.
    let hook_extra_rows = if args.verbose >= 1 {
        (worktree_count as u16) * 8
    } else {
        0
    };
    let table = OperationTable::new(
        phases,
        worktree_infos,
        project_root.clone(),
        cwd,
        stat,
        rx,
        TableConfig {
            columns: tui_columns,
            columns_explicit,
            sort_spec,
            extra_rows: 5 + hook_extra_rows,
            verbosity: args.verbose,
            pin_default_branch: true,
            forge_prs: forge_lookup,
            partition_by_owner: false, // External unowned_start_index drives the partition.
            seeded_fields,
        },
        None,
    );
    let completed = table.run()?;

    if let Some(handle) = collector_handle {
        handle.cancel(); // Renderer is gone, don't keep workers running.
        handle.join();
    }

    // Wait for orchestrator thread to finish
    orchestrator_handle
        .join()
        .map_err(|_| anyhow::anyhow!("DAG orchestrator thread panicked"))?;

    // Surface untrusted-hook notices the TUI buffered (TuiBridge warnings
    // never reach stderr). Before the deferred-branch removal below, so the
    // aggregated copy wins and any live Deny hit there dedups against it.
    {
        let mut post_tui_output =
            crate::output::CliOutput::new(crate::output::OutputConfig::new(false, false));
        crate::hooks::trust_skip::flush_pending_notice(&git_dir, &mut post_tui_output);
    }

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
    if !completed.hook_summaries.is_empty() {
        eprintln!();
        eprintln!("Hooks:");
        for entry in &completed.hook_summaries {
            let status_word = if entry.warned { "warned" } else { "failed" };
            let exit_str = entry
                .exit_code
                .map(|c| format!("exit {c}"))
                .unwrap_or_else(|| "error".to_string());
            eprintln!(
                "  {}: {} {} ({}, {}ms)",
                entry.branch_name,
                entry.hook_type.hook_name(),
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

    // ── Surface branches prune deliberately kept ──────────────────────────
    {
        let refined = skipped_refined.lock().unwrap().clone();
        let unmerged = skipped_unmerged.lock().unwrap().clone();
        if !refined.is_empty() || !unmerged.is_empty() {
            let config = OutputConfig::with_autocd(false, false, settings.autocd);
            let mut notes_output = CliOutput::new(config);
            sync_shared::render_prune_skip_notes(&refined, &unmerged, &mut notes_output);
        }
    }

    // ── Check for failures ────────────────────────────────────────────────
    sync_shared::check_tui_failures(&completed.rows)?;

    Ok(())
}
