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
    CD_FILE_ENV, WorktreeConfig,
    core::{
        CommandBridge, NullBridge, NullSink, OutputSink,
        sort::SortSpec,
        worktree::{
            fetch,
            info_field::FieldSet,
            list,
            list::{EntryKind, Stat},
            list_stream, prune, push, rebase,
            sync_dag::{
                self, DagExecutor, OperationPhase, PatchSource, StaticCapGovernor, SyncDag,
                SyncTask, TaskId, TaskMessage, TaskOutcome, TaskStatus,
            },
            temp_worktree,
        },
    },
    get_git_common_dir, get_project_root,
    git::{GitCommand, cancel::CancelFlag, should_show_gitoxide_notice},
    hooks::HookExecutor,
    is_git_repository,
    logging::init_logging,
    output::{
        CliOutput, Output, OutputConfig,
        tui::operation_table::{OperationTable, TableConfig},
    },
    remote::get_default_branch_local,
    settings::{DaftSettings, GovernorJobs, GovernorMode},
    styles,
};
use anyhow::Result;
use clap::Parser;
use std::collections::{HashMap, HashSet};
use std::path::PathBuf;
use std::sync::{Arc, mpsc};

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
    owner: Option<&crate::core::ownership::BranchOwner>,
    filters: &[IncludeFilter],
) -> bool {
    if owner.is_some_and(|o| o.is_current_user) {
        return true;
    }
    for filter in filters {
        match filter {
            IncludeFilter::Unowned => return true,
            IncludeFilter::Email(email) => {
                if owner.is_some_and(|o| o.email.eq_ignore_ascii_case(email)) {
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

Resource governing: parallel pushes with a pre-push hook are memory-governed.
Concurrency is capped (default max(2, cores/4); `--jobs N` overrides,
`--no-throttle` disables), admissions pause under memory pressure, each hook's
peak memory is learned across runs, and under sustained pressure the newest
push is frozen — then killed and retried — instead of exhausting the machine.
Every push unit gets a wall-clock budget (`daft.sync.pushTimeout`, default
30m). `daft.sync.pushHookStrategy batched` pushes all branches in one
`git push` so the hook runs once with every ref.

Cancellation: the first Ctrl+C (or SIGTERM) cancels gracefully — no new work
starts and every running git subprocess is torn down. A pre-push hook and all
of its descendants are killed by process group, reaching even stages that moved
to their own process groups or were stopped by terminal job control; an
interrupted rebase is aborted to restore the worktree; and sync prints partial
results and exits 130. A second Ctrl+C force-kills anything still running and
exits immediately.

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
        requires = "push",
        help = "Skip the repo's pre-push hook when pushing (requires --push)"
    )]
    no_verify: bool,

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
        help = "Columns to display (comma-separated). Replace: branch,path,age. Modify defaults: +col,-col. Available: branch, path, size, base, changes, remote, age, annotation, owner, hash, last-commit"
    )]
    columns: Option<String>,

    #[arg(
        long,
        help = "Sort order (comma-separated). +col ascending, -col descending. Columns: branch, path, size, base, changes, remote, age, owner, hash, activity, commit"
    )]
    sort: Option<String>,

    #[arg(
        long,
        value_name = "N",
        requires = "push",
        conflicts_with = "no_throttle",
        help = "Cap concurrent pushes when a pre-push hook is present (requires --push; default: from daft.governor.jobs, or max(2, cores/4))"
    )]
    jobs: Option<std::num::NonZeroUsize>,

    #[arg(
        long,
        requires = "push",
        help = "Disable the push resource governor for this run (requires --push)"
    )]
    no_throttle: bool,
}

impl Args {
    fn force(&self) -> bool {
        self.prune_dirty || self.force_deprecated
    }
}

/// How the push phase is governed (#678). Resolved once per run from
/// flags + config; pure so the gate is unit-testable.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum PushGovernorPlan {
    /// No governor at all — byte-identical pre-#678 behavior.
    Ungoverned,
    /// Fixed concurrency cap only (`daft.governor.mode=off` with an
    /// explicit `--jobs`).
    StaticCap(usize),
    /// Full dynamic governor (memory-aware admission) with a hard cap.
    Dynamic { cap: usize },
}

fn resolve_push_governor(
    hook_present: bool,
    no_verify: bool,
    no_throttle: bool,
    jobs: Option<usize>,
    mode: GovernorMode,
    config_jobs: GovernorJobs,
    cores: usize,
) -> PushGovernorPlan {
    // Without a pre-push hook there is nothing that multiplies memory use;
    // with hooks bypassed or throttling refused, the user has spoken.
    if !hook_present || no_verify || no_throttle {
        return PushGovernorPlan::Ungoverned;
    }
    match (mode, jobs) {
        // Mode off: only an explicit --jobs still caps.
        (GovernorMode::Off, Some(cap)) => PushGovernorPlan::StaticCap(cap),
        (GovernorMode::Off, None) => PushGovernorPlan::Ungoverned,
        (GovernorMode::Auto, jobs) => PushGovernorPlan::Dynamic {
            cap: jobs.unwrap_or_else(|| config_jobs.effective(cores)),
        },
    }
}

pub fn run() -> Result<()> {
    let args = Args::parse_from(crate::get_clap_args("git-worktree-sync"));

    init_logging(args.verbose >= 2);

    if !is_git_repository()? {
        anyhow::bail!("Not inside a Git repository");
    }

    let settings = DaftSettings::load()?;

    // Validate --columns and --sort early so errors surface in both sequential and TUI modes.
    let columns_input = args.columns.as_deref().or(settings.sync_columns.as_deref());
    if let Some(input) = columns_input {
        use crate::core::columns::{ColumnSelection, CommandKind};
        ColumnSelection::parse(input, CommandKind::Sync).map_err(|e| anyhow::anyhow!("{e}"))?;
    }
    let sort_input = args.sort.as_deref().or(settings.sync_sort.as_deref());
    if let Some(input) = sort_input {
        SortSpec::parse(input).map_err(|e| anyhow::anyhow!("{e}"))?;
    }

    // Two-stage cancellation (#663): the first Ctrl+C (or SIGTERM/SIGHUP —
    // ctrlc's `termination` feature) soft-cancels — every in-flight git
    // subtree gets TERM+CONT by process group and no new work starts. A
    // second one hard-cancels with SIGKILL. The handler must be installed
    // before anything else can claim the process-global ctrlc slot.
    let cancel = Arc::new(CancelFlag::new());
    // The TUI renderer's cancel channel is a plain AtomicBool; the handler
    // flips both so raw-mode and signal paths converge on the same state.
    let cancel_render = Arc::new(std::sync::atomic::AtomicBool::new(false));
    {
        let flag = Arc::clone(&cancel);
        let render = Arc::clone(&cancel_render);
        let _ = ctrlc::set_handler(move || {
            // Owning the process-global ctrlc slot means the prune
            // confirmation prompt's own handler never registered; a
            // prompt parked in term.read_key() cannot observe the flag,
            // so honor its cancel contract (message + exit 130) here —
            // otherwise a SIGTERM mid-prompt hangs until a keypress.
            crate::prompt::exit_if_prompt_active();
            flag.escalate();
            render.store(true, std::sync::atomic::Ordering::Relaxed);
        });
    }

    if !std::io::IsTerminal::is_terminal(&std::io::stderr()) || args.verbose >= 2 {
        run_sequential(args, settings, &cancel)
    } else {
        run_tui(args, settings, &cancel, cancel_render)
    }
}

/// Shared exit path once a run observed a cancel: print the partial-result
/// note plus any surviving process groups, then exit 130 (128 + SIGINT).
fn exit_cancelled(done: usize, unfinished: usize) -> ! {
    eprintln!();
    if done > 0 || unfinished > 0 {
        eprintln!("Sync cancelled — {done} task row(s) finished, {unfinished} unfinished.");
    } else {
        eprintln!("Sync cancelled.");
    }
    let survivors = crate::git::cancel::surviving_groups();
    if !survivors.is_empty() {
        let list = survivors
            .iter()
            .map(|p| p.to_string())
            .collect::<Vec<_>>()
            .join(", ");
        eprintln!("warning: hook processes may still be running (process group(s): {list}).");
        eprintln!("         Recover manually with: kill -KILL -<pgid>");
    }
    std::process::exit(130);
}

/// Write the shell wrapper's cd redirect (or a fallback hint) when the
/// prune phase removed the worktree the user's shell is sitting in.
/// Shared by the normal exit and the cancelled-exit paths.
fn write_cd_redirect(output: &mut CliOutput, cd_target: &std::path::Path) {
    if std::env::var(CD_FILE_ENV).is_ok() {
        output.cd_path(cd_target);
    } else {
        output.result(&format!(
            "Run `cd {}` (your previous working directory was removed)",
            cd_target.display()
        ));
    }
}

/// Cancelled-exit for the sequential path: flush the prune phase's cd
/// redirect (so the shell leaves a just-removed worktree) *before*
/// diverging via [`exit_cancelled`]. Without this, exiting 130 skips the
/// end-of-function cd write and strands the shell in the deleted directory
/// the prune already removed (#663). The sequential path streams per-phase
/// progress as it runs, so it has no TUI row tallies — (0, 0) yields the
/// bare "Sync cancelled." line, which is the right message there.
fn exit_cancelled_with_cd(output: &mut CliOutput, cd_target: Option<&PathBuf>) -> ! {
    if let Some(cd) = cd_target {
        write_cd_redirect(output, cd);
    }
    exit_cancelled(0, 0);
}

/// Sequential (non-TTY) execution path — the original sync flow.
///
/// Cancellation here is coarser than the TUI path: the active git
/// subprocess is torn down by the shared seams, and the flag is checked
/// at phase boundaries — a cancelled phase exits 130 without starting
/// the next one.
fn run_sequential(args: Args, settings: DaftSettings, cancel: &Arc<CancelFlag>) -> Result<()> {
    let config = OutputConfig::with_autocd(false, args.verbose >= 2, settings.autocd);
    let mut output = CliOutput::new(config);

    // Clean up stale temp worktrees from previous crashes.
    if let Ok(project_root) = get_project_root() {
        let _ = temp_worktree::cleanup_stale(&project_root);
    }

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
    let prune_result = run_prune_phase(&mut output, &settings, force, cancel);
    if cancel.is_cancelled() {
        // Prune may have removed our cwd before the cancel landed — salvage
        // its cd target so the shell still redirects out of the deleted
        // worktree (#663).
        let cd = prune_result.as_ref().ok().and_then(|r| r.cd_target.clone());
        exit_cancelled_with_cd(&mut output, cd.as_ref());
    }
    let prune_result = prune_result?;

    // Phase 2: Update all remaining worktrees
    let update_result = run_update_phase(&mut output, &settings, force, &default_branch, cancel);
    if cancel.is_cancelled() {
        exit_cancelled_with_cd(&mut output, prune_result.cd_target.as_ref());
    }
    update_result?;

    // Compute ownership filter for rebase/push phases.
    // When --include filters are specified or user.email is set, only owned
    // (and explicitly included) branches are rebased and pushed.
    let include_filters: Vec<IncludeFilter> = args
        .include
        .iter()
        .map(|v| IncludeFilter::parse(v))
        .collect();
    let git_for_email = GitCommand::new(true).with_gitoxide(settings.use_gitoxide);
    let user_email: Option<String> = git_for_email.config_get("user.email").ok().flatten();
    // Build the included-branch set only when ownership filtering is active
    // (i.e. when user.email is known or explicit --include filters are present).
    let included_branches: Option<HashSet<String>> = if user_email.is_some()
        || !include_filters.is_empty()
    {
        let worktrees = fetch::get_all_worktrees_with_branches(&git_for_email).unwrap_or_default();
        let project_root = get_project_root()?;
        let mut set = HashSet::new();
        for (path, branch) in &worktrees {
            let owner = crate::core::ownership::resolve_owner_with_fallbacks(
                &default_branch,
                branch,
                path,
                settings.ownership_strategy,
                user_email.as_deref(),
                Some(&settings.remote),
            );
            if is_branch_included(branch, owner.as_ref(), &include_filters) {
                set.insert(branch.clone());
            }
        }
        // Also check local-only branches
        if let Ok(ref_output) = git_for_email.for_each_ref("%(refname:short)", "refs/heads") {
            let worktree_set: HashSet<&str> = worktrees.iter().map(|(_, b)| b.as_str()).collect();
            for branch in ref_output.lines() {
                let branch = branch.trim();
                if branch.is_empty() || worktree_set.contains(branch) {
                    continue;
                }
                let owner = crate::core::ownership::resolve_owner_with_fallbacks(
                    &default_branch,
                    branch,
                    &project_root,
                    settings.ownership_strategy,
                    user_email.as_deref(),
                    Some(&settings.remote),
                );
                if is_branch_included(branch, owner.as_ref(), &include_filters) {
                    set.insert(branch.to_string());
                }
            }
        }
        Some(set)
    } else {
        None
    };

    // Phase 3: Rebase all worktrees onto base branch (if requested)
    let conflicted_branches: HashSet<String> = if let Some(ref base_branch) = args.rebase {
        let result = run_rebase_phase(
            &mut output,
            &settings,
            base_branch,
            force,
            args.autostash,
            &default_branch,
            included_branches.as_ref(),
            cancel,
        );
        if cancel.is_cancelled() {
            exit_cancelled_with_cd(&mut output, prune_result.cd_target.as_ref());
        }
        let result = result?;
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
        // When ownership filtering is active, skip unowned branches from push.
        let mut push_skip = conflicted_branches.clone();
        // Never push the rebase base branch: it was used only as a local rebase
        // target, and pushing it could clobber commits other devs landed between
        // fetch and sync completion.
        if let Some(ref base) = args.rebase {
            push_skip.insert(base.clone());
        }
        if let Some(ref included) = included_branches {
            // Collect all worktree branches and skip those not included.
            let git_tmp = GitCommand::new(output.is_quiet()).with_gitoxide(settings.use_gitoxide);
            if let Ok(wts) = fetch::get_all_worktrees_with_branches(&git_tmp) {
                for (_, branch) in wts {
                    if !included.contains(&branch) {
                        push_skip.insert(branch);
                    }
                }
            }
        }
        let push_result = run_push_phase(
            &mut output,
            &settings,
            args.force_with_lease,
            args.no_verify,
            &push_skip,
            &default_branch,
            cancel,
        );
        if cancel.is_cancelled() {
            exit_cancelled_with_cd(&mut output, prune_result.cd_target.as_ref());
        }
        push_result?;
    }

    // Write the cd target for the shell wrapper (from prune phase)
    if let Some(ref cd_target) = prune_result.cd_target {
        write_cd_redirect(&mut output, cd_target);
    }

    Ok(())
}

/// Interactive TUI execution path — parallel DAG executor with inline ratatui display.
fn run_tui(
    args: Args,
    settings: DaftSettings,
    cancel: &Arc<CancelFlag>,
    cancel_render: Arc<std::sync::atomic::AtomicBool>,
) -> Result<()> {
    let git = GitCommand::new(false)
        .with_gitoxide(settings.use_gitoxide)
        .with_cancel(Arc::clone(cancel));
    let project_root = get_project_root()?;

    // Clean up stale temp worktrees from previous crashes.
    let _ = temp_worktree::cleanup_stale(&project_root);

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

    let sort_spec = {
        let sort_input = args.sort.as_deref().or(settings.sync_sort.as_deref());
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
            .or(settings.sync_columns.as_deref())
            .and_then(|input| ColumnSelection::parse(input, CommandKind::Sync).ok())
            .is_some_and(|r| r.columns.contains(&ListColumn::Size));
        from_columns || sort_spec.as_ref().is_some_and(|s| s.needs_size())
    };
    let compute_mtime = sort_spec.as_ref().is_some_and(|s| s.needs_mtime());
    let user_email: Option<String> = git.config_get("user.email").ok().flatten();
    // Synchronous seed: compute everything EXCEPT the heavy cells (size,
    // mtime, line stats). Those will stream in via the collector below.
    // shared_owner_lookup depends on this call's output.
    let worktree_infos = list::collect_worktree_info(
        &git,
        &base_branch,
        current_path.as_deref(),
        Stat::Summary, // Force Summary for the seed; line stats stream below.
        false,         // has_size = false: stream the size cluster instead
        false,         // compute_mtime = false: stream the mtime cluster
        settings.ownership_strategy,
        user_email.as_deref(),
        &settings.remote,
    )?;

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

    // Heavy cells that the user requested but the seed deliberately skipped.
    // These will arrive via the streaming collector in parallel with the
    // orchestrator.
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
    let hooks_config = crate::core::settings::load_hooks_config()?;
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
    let include_filters: Vec<IncludeFilter> = args
        .include
        .iter()
        .map(|v| IncludeFilter::parse(v))
        .collect();
    let unowned_start_index = {
        let mut sorted = worktree_infos.clone();
        sorted.sort_by(|a, b| {
            let default_order = |w: &list::WorktreeInfo| u8::from(!w.is_default_branch);
            let kind_order = |k: &list::EntryKind| match k {
                list::EntryKind::Worktree => 0,
                list::EntryKind::LocalBranch => 1,
                list::EntryKind::RemoteBranch => 2,
            };
            default_order(a)
                .cmp(&default_order(b))
                .then_with(|| kind_order(&a.kind).cmp(&kind_order(&b.kind)))
                .then_with(|| match &sort_spec {
                    Some(spec) => spec.compare(a, b),
                    None => a.name.to_lowercase().cmp(&b.name.to_lowercase()),
                })
        });
        // Only show divider when user_email is known (otherwise all are unowned)
        user_email.as_ref().and_then(|_| {
            let idx = sorted.iter().position(|info| {
                !is_branch_included(&info.name, info.owner.as_ref(), &include_filters)
            });
            // Only emit a boundary when there are both owned and unowned rows
            idx.filter(|&i| i > 0 && i < sorted.len())
        })
    };

    // ── Build shared maps from worktree_infos before they move into OperationTable ──
    // Build worktree list including local-only branches (with None paths).
    let worktree_branch_set: HashSet<String> =
        all_worktrees.iter().map(|(_, b)| b.clone()).collect();
    let orch_all_worktrees: Vec<(String, Option<PathBuf>)> = all_worktrees
        .iter()
        .map(|(p, b)| (b.clone(), Some(p.clone())))
        .chain(
            worktree_infos
                .iter()
                .filter(|info| {
                    info.kind == list::EntryKind::LocalBranch
                        && !worktree_branch_set.contains(&info.name)
                })
                .map(|info| (info.name.clone(), None)),
        )
        .collect();
    // Build owner lookup from the worktree_infos collected before TUI started
    let shared_owner_lookup: Arc<HashMap<String, Option<crate::core::ownership::BranchOwner>>> =
        Arc::new(
            worktree_infos
                .iter()
                .map(|info| (info.name.clone(), info.owner.clone()))
                .collect(),
        );
    // Budget hook + job sub-rows per worktree (2 hooks x ~3 jobs each).
    // Not all worktrees will have hooks, but the ratatui inline viewport
    // cannot grow after creation, so over-allocate.
    let hook_extra_rows = if args.verbose >= 1 {
        (worktree_infos.len() as u16) * 8
    } else {
        0
    };

    // ── Create channel and spawn orchestrator ──────────────────────────
    let (tx, rx) = std::sync::mpsc::channel();
    // Clone for the streaming collector below, since `tx` is moved into the
    // orchestrator closure.
    let tx_for_collector = tx.clone();

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
    let shared_no_verify = args.no_verify;
    // One probe for the whole run — every worktree shares the repo's hooks
    // dir, and DAG workers shouldn't each pay a subprocess for it (#599).
    // The resolved path doubles as the governor's profile identity (#678).
    let shared_hook_path = if shared_push {
        GitCommand::new(true)
            .with_gitoxide(settings.use_gitoxide)
            .pre_push_hook_path(&project_root)
    } else {
        None
    };
    let shared_hook_present = shared_hook_path.is_some();

    // Resource governor plan (#678): resolved from flags + config here,
    // constructed inside the orchestrator once the DAG's push-task count
    // is known.
    let cores = std::thread::available_parallelism()
        .map(|p| p.get())
        .unwrap_or(4);
    let shared_governor_plan = resolve_push_governor(
        shared_hook_present,
        shared_no_verify,
        args.no_throttle,
        args.jobs.map(std::num::NonZeroUsize::get),
        settings.governor_mode,
        settings.governor_jobs,
        cores,
    );
    let shared_memory_reserve = settings.governor_memory_reserve;
    let shared_jobserver_mode = settings.governor_jobserver;
    let cores_for_jobserver = cores;
    let shared_push_strategy = settings.sync_push_hook_strategy;
    // Branches whose rebase conflicted — the batched push excludes them at
    // execution time (per-branch mode handles this via task preconditions).
    let shared_conflicts: Arc<std::sync::Mutex<HashSet<String>>> =
        Arc::new(std::sync::Mutex::new(HashSet::new()));

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

    let shared_base_branch = Arc::new(base_branch.clone());

    // Clone values needed by orchestrator. Whole-WorktreeInfo snapshots are no
    // longer threaded through TaskCompleted — instead, each task spawns a
    // streaming-collector run with `PatchSource::PostTask(phase)` to refresh
    // just the fields it touched.
    let orch_settings = Arc::clone(&shared_settings);
    let orch_base_branch = Arc::clone(&shared_base_branch);
    let orch_stat = stat;
    let orch_user_email: Arc<Option<String>> = Arc::new(user_email.clone());

    // Ownership filtering for the orchestrator
    let shared_include: Arc<Vec<String>> = Arc::new(args.include.clone());

    let orch_cancel = Arc::clone(cancel);

    let orchestrator_handle = std::thread::spawn(move || {
        // ── Phase 1: Fetch ─────────────────────────────────────────────
        if !sync_shared::run_fetch_phase(
            &tx,
            orch_settings.use_gitoxide,
            &orch_settings.remote,
            Some(&orch_cancel),
        ) {
            return;
        }

        // Cancelled during (or right after) the fetch: skip the refresh
        // and never build the DAG — nothing per-branch has started yet.
        if orch_cancel.is_cancelled() {
            let _ = tx.send(sync_dag::DagEvent::AllDone);
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
            // Carry the cancel flag: its `ls-remote` probes are network
            // calls that must be torn down on the first Ctrl+C, like every
            // other orchestrator git op (#663) — otherwise a stalled remote
            // here ignores the cancel until it returns.
            let git = GitCommand::new(false)
                .with_gitoxide(orch_settings.use_gitoxide)
                .with_cancel(Arc::clone(&orch_cancel));
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

        // Filter out gone branches so they don't get Update/Rebase tasks
        // (their worktree paths will be removed by the Prune tasks).
        let live_worktrees: Vec<(String, Option<PathBuf>)> = orch_all_worktrees
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
                    shared_owner_lookup.get(branch).and_then(|o| o.as_ref()),
                    &include_filters,
                )
            });

        // Branch list the batched push covers (owned minus the rebase
        // base), captured before `owned` moves into the DAG builder.
        let batch_branches: Arc<Vec<String>> = Arc::new(
            owned
                .iter()
                .map(|(branch, _)| branch.clone())
                .filter(|branch| shared_rebase_branch.as_deref() != Some(branch.as_str()))
                .collect(),
        );
        let dag = SyncDag::build_sync_with_strategy(
            owned,
            unowned,
            gone_branches,
            shared_rebase_branch.as_ref().clone(),
            shared_push,
            shared_push_strategy == crate::settings::PushHookStrategy::Batched,
        );

        // ── Phase 3: Run the DAG executor (skips the Fetch task) ───────
        let push_task_count = dag
            .tasks
            .iter()
            .filter(|t| matches!(t.id, TaskId::Push(_)))
            .count()
            .max(if dag.tasks.iter().any(|t| t.id == TaskId::PushBatch) {
                batch_branches.len()
            } else {
                0
            });
        let tx_for_tasks = tx.clone();
        let task_cancel = Arc::clone(&orch_cancel);
        let mut executor = DagExecutor::new(dag, tx).with_cancel(Arc::clone(&orch_cancel));
        // A single push can never exceed a cap — skip the admission scan
        // (and never pay for a probe or sampler thread).
        let sync_governor: Option<Arc<crate::governor::SyncGovernor>> = if push_task_count >= 2 {
            match shared_governor_plan {
                PushGovernorPlan::Ungoverned => None,
                PushGovernorPlan::StaticCap(cap) => {
                    executor = executor.with_governor(Arc::new(StaticCapGovernor::new(cap)));
                    None
                }
                PushGovernorPlan::Dynamic { cap } => {
                    // Profile persistence (#678 stage 2): identity is the
                    // resolved hook file's content hash under this repo's
                    // coordinator DB. Any piece missing → run unprofiled.
                    let profiles = shared_hook_path.as_deref().and_then(|hook_path| {
                        let repo_hash =
                            crate::core::repo_identity::compute_repo_id_from_common_dir(
                                &shared_git_dir,
                            )
                            .ok()?;
                        let hook_hash = crate::governor::hook_script_hash(hook_path)?;
                        let store = crate::governor::adapters::SqliteProfileStore::open_for_repo(
                            &repo_hash,
                        )?;
                        Some((
                            Box::new(store) as Box<dyn crate::governor::ports::ProfileStore>,
                            crate::governor::ports::ProfileKey {
                                repo_hash,
                                stage: "pre-push".into(),
                                hook_hash,
                            },
                        ))
                    });
                    let governor = crate::governor::SyncGovernor::spawn(
                        crate::governor::adapters::build_probe(),
                        profiles,
                        Some(tx_for_tasks.clone()),
                        Arc::clone(&orch_cancel),
                        |first| {
                            crate::governor::domain::GovernorParams::new(
                                cap,
                                shared_memory_reserve.resolve(first.mem_total),
                            )
                        },
                    );
                    executor = executor.with_governor(Arc::clone(&governor) as Arc<_>);
                    Some(governor)
                }
            }
        } else {
            None
        };
        // Shared jobserver (#678 stage 4): one machine-wide token pool for
        // cooperating toolchains inside concurrent hooks. Lives while the
        // executor runs; None when the push phase is ungoverned or the
        // export is configured off.
        #[cfg(unix)]
        let jobserver = if (sync_governor.is_some()
            || !matches!(shared_governor_plan, PushGovernorPlan::Ungoverned))
            && push_task_count >= 2
            && shared_jobserver_mode == GovernorMode::Auto
        {
            crate::governor::jobserver::PushJobserver::create(cores_for_jobserver)
        } else {
            None
        };
        #[cfg(unix)]
        let jobserver_env: Option<Arc<(String, String)>> =
            jobserver.as_ref().map(|j| Arc::new(j.env()));
        #[cfg(not(unix))]
        let jobserver_env: Option<Arc<(String, String)>> = None;
        let push_jobserver_env = jobserver_env.clone();
        let push_governor = sync_governor.clone();
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
                    TaskId::Fetch => {
                        // Already done above
                        (
                            TaskStatus::Succeeded,
                            TaskMessage::Ok("fetched".into()),
                            outcomes.clone(),
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
                        // Prune removes the row entirely; no patch to emit.
                        (status, message, outcomes.clone())
                    }
                    TaskId::Update(branch_name) => {
                        let (status, message) = execute_update_task(
                            branch_name,
                            task.worktree_path.as_ref(),
                            &shared_settings,
                            &shared_project_root,
                            &shared_pull_args,
                            shared_force,
                            &task_cancel,
                        );
                        if status == TaskStatus::Succeeded {
                            spawn_post_task_refresh(
                                branch_name,
                                OperationPhase::Update,
                                update_post_task_fields(),
                                &shared_worktree_map,
                                &orch_settings,
                                &orch_base_branch,
                                orch_user_email.as_deref(),
                                orch_stat,
                                &shared_git_dir,
                                &tx_for_tasks,
                            );
                        }
                        (status, message, outcomes.clone())
                    }
                    TaskId::Rebase(branch_name) => {
                        let conflicts = Arc::clone(&shared_conflicts);
                        let branch_for_conflicts = branch_name.clone();
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
                            &task_cancel,
                        );
                        if new_outcomes.contains(&TaskOutcome::Conflict) {
                            conflicts.lock().unwrap().insert(branch_for_conflicts);
                        }
                        if status == TaskStatus::Succeeded
                            && !matches!(message, TaskMessage::Conflict)
                        {
                            spawn_post_task_refresh(
                                branch_name,
                                OperationPhase::Rebase(base.to_string()),
                                FieldSet::BASE_AHEAD_BEHIND
                                    | FieldSet::LAST_COMMIT
                                    | FieldSet::REMOTE_AHEAD_BEHIND,
                                &shared_worktree_map,
                                &orch_settings,
                                &orch_base_branch,
                                orch_user_email.as_deref(),
                                orch_stat,
                                &shared_git_dir,
                                &tx_for_tasks,
                            );
                        }
                        (status, message, new_outcomes)
                    }
                    TaskId::Push(branch_name) => {
                        let (status, message, new_outcomes) = execute_push_task(
                            branch_name,
                            task.worktree_path.as_ref(),
                            &shared_project_root,
                            &shared_settings,
                            shared_force_with_lease,
                            shared_no_verify,
                            shared_hook_present,
                            &tx_for_tasks,
                            outcomes,
                            &task_cancel,
                            push_governor.as_ref(),
                            push_jobserver_env.as_deref(),
                        );
                        if status == TaskStatus::Succeeded {
                            spawn_post_task_refresh(
                                branch_name,
                                OperationPhase::Push,
                                FieldSet::REMOTE_AHEAD_BEHIND,
                                &shared_worktree_map,
                                &orch_settings,
                                &orch_base_branch,
                                orch_user_email.as_deref(),
                                orch_stat,
                                &shared_git_dir,
                                &tx_for_tasks,
                            );
                        }
                        (status, message, new_outcomes)
                    }
                    TaskId::PushBatch => execute_push_batch_task(
                        &batch_branches,
                        &shared_worktree_map,
                        &shared_project_root,
                        &shared_settings,
                        shared_force_with_lease,
                        shared_no_verify,
                        shared_hook_present,
                        &tx_for_tasks,
                        outcomes,
                        &task_cancel,
                        push_governor.as_ref(),
                        push_jobserver_env.as_deref(),
                        &shared_conflicts,
                    ),
                    TaskId::Setup(_) => unreachable!("Setup is only used by clone"),
                    TaskId::RemoveWorktree(_) | TaskId::RemoveBare => {
                        unreachable!("RemoveWorktree/RemoveBare are only used by repo remove")
                    }
                }
            },
        );
        // Stop the governor's sampler thread with the run it belongs to.
        if let Some(governor) = sync_governor {
            governor.shutdown();
        }
    });

    // Spawn the streaming collector for heavy cells (concurrent with
    // orchestrator). No-op if streaming_fields is empty.
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
            },
            tx_for_collector,
        ))
    } else {
        None
    };

    // ── Run TUI via OperationTable on main thread ──────────────────────
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
            partition_by_owner: false, // External unowned_start_index drives the partition.
            seeded_fields,
        },
        unowned_start_index,
    )
    .with_cancel_signal(Arc::clone(&cancel_render));

    // Raw mode so Ctrl+C reaches the render loop as a key event (and ^C
    // isn't echoed mid-render); the process-global SIGINT handler stays
    // as the fallback. Scoped: the guard must drop before the joins
    // below so a second Ctrl+C — cooked mode again — reaches the handler
    // as a signal and escalates to hard-kill.
    let completed = {
        let _raw_guard = crate::output::tui::enable_raw_mode_guard();
        table.run()?
    };

    // A cancel that arrived as a raw-mode key event never went through
    // the signal handler: the flag may still be at level 0. Lift it 0 → 1
    // atomically — a check-then-escalate would race the ctrlc handler
    // thread (SIGTERM/SIGHUP via the `termination` feature) and could
    // compound to level 2, SIGKILL'ing hook subtrees off one graceful
    // keypress (#8). `soft_escalate_once` no-ops if a cancel already landed.
    let run_cancelled = completed.cancelled || cancel.is_cancelled();
    if run_cancelled {
        cancel.soft_escalate_once();
        eprintln!();
        eprintln!("Cancelling — waiting for running tasks (press Ctrl+C again to force-kill)…");
    }

    if let Some(handle) = collector_handle {
        handle.cancel(); // Renderer is gone, don't keep workers running.
        handle.join();
    }

    // Wait for orchestrator thread to finish
    orchestrator_handle
        .join()
        .map_err(|_| anyhow::anyhow!("DAG orchestrator thread panicked"))?;

    // ── Cancelled: report partial progress and exit 130 ────────────────
    // Deliberately skips the deferred-prune handling below — removing the
    // user's current worktree after they aborted the run is the opposite
    // of what Ctrl+C asked for. Stale temp worktrees are reclaimed by
    // cleanup_stale on the next sync.
    if run_cancelled {
        use crate::output::tui::{FinalStatus, WorktreeStatus};
        let done = completed
            .rows
            .iter()
            .filter(|w| {
                matches!(&w.status, WorktreeStatus::Done(s) if !matches!(s, FinalStatus::Skipped))
            })
            .count();
        exit_cancelled(done, completed.rows.len().saturating_sub(done));
    }

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
        force,
        &hooks_config,
    );

    // ── Post-TUI: print hook summary ────────────────────────────────────
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

    // ── Post-TUI: governor throttle summary (#678) ─────────────────────
    if let Some(governor) = &completed.governor {
        let noun = if governor.throttled_pushes == 1 {
            "push"
        } else {
            "pushes"
        };
        let total = governor.throttled_total;
        let held = if total.as_secs() >= 1 {
            format!("{}s", total.as_secs())
        } else {
            format!("{}ms", total.as_millis())
        };
        eprintln!();
        eprintln!(
            "{} {noun} throttled {held} to preserve memory headroom",
            governor.throttled_pushes,
        );
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

// ── DAG task execution functions ───────────────────────────────────────────

/// Must include `REMOTE_AHEAD_BEHIND` — fast-forward clears it, otherwise the post-fetch value persists.
fn update_post_task_fields() -> FieldSet {
    FieldSet::BASE_AHEAD_BEHIND
        | FieldSet::LAST_COMMIT
        | FieldSet::CHANGES
        | FieldSet::REMOTE_AHEAD_BEHIND
}

/// Spawn a streaming-collector run that re-emits the given `fields` for
/// `branch_name` as `PatchSource::PostTask(phase)` patches. Blocks until the
/// workers finish so that patches land before the next dependent task starts;
/// otherwise the renderer can briefly show stale values during a Push that
/// follows a Rebase, etc.
#[allow(clippy::too_many_arguments)]
fn spawn_post_task_refresh(
    branch_name: &str,
    phase: OperationPhase,
    fields: FieldSet,
    worktree_map: &HashMap<String, (PathBuf, bool)>,
    settings: &Arc<DaftSettings>,
    base_branch: &str,
    user_email: Option<&str>,
    stat: Stat,
    git_common_dir: &std::path::Path,
    tx: &mpsc::Sender<sync_dag::DagEvent>,
) {
    let Some((path, _is_main)) = worktree_map.get(branch_name) else {
        return;
    };
    let target = list_stream::CollectorTarget {
        branch_name: branch_name.to_string(),
        path: Some(path.clone()),
        kind: EntryKind::Worktree,
        is_detached: false,
    };
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
            targets: vec![target],
            fields,
            stat,
            source: PatchSource::PostTask(phase),
            ctx,
        },
        tx.clone(),
    );
    handle.join();
}

/// Execute a single update task for a DAG worker.
#[allow(clippy::too_many_arguments)]
fn execute_update_task(
    branch_name: &str,
    worktree_path: Option<&PathBuf>,
    settings: &DaftSettings,
    project_root: &std::path::Path,
    pull_args: &[String],
    force: bool,
    cancel: &Arc<CancelFlag>,
) -> (TaskStatus, TaskMessage) {
    // A worker may pop a task in the same instant the cancel sweep runs;
    // resolve it without spawning anything.
    if cancel.is_cancelled() {
        return (TaskStatus::Cancelled, TaskMessage::Cancelled);
    }

    let Some(target_path) = worktree_path else {
        // Local-only branch: attempt fast-forward from upstream.
        return execute_local_branch_update(branch_name, project_root);
    };

    let git = GitCommand::new(false)
        .with_gitoxide(settings.use_gitoxide)
        .with_cancel(Arc::clone(cancel));

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
    } else if cancel.is_cancelled() {
        // The pull was torn down by the cancel cascade — that's a user
        // decision, not a worktree failure.
        (TaskStatus::Cancelled, TaskMessage::Cancelled)
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
    cancel: &Arc<CancelFlag>,
) -> (TaskStatus, TaskMessage, HashSet<TaskOutcome>) {
    if cancel.is_cancelled() {
        return (
            TaskStatus::Cancelled,
            TaskMessage::Cancelled,
            branch_outcomes.clone(),
        );
    }

    let target_path: PathBuf;
    let _temp_guard: Option<temp_worktree::TempWorktreeGuard>;

    if let Some(path) = worktree_path {
        target_path = path.clone();
        _temp_guard = None;
    } else {
        // Local-only branch: create a temporary worktree for the rebase.
        match temp_worktree::create(project_root, branch_name) {
            Ok(tmp_path) => {
                _temp_guard = Some(temp_worktree::TempWorktreeGuard::new(tmp_path.clone()));
                target_path = tmp_path;
            }
            Err(e) => {
                return (
                    TaskStatus::Failed,
                    TaskMessage::Failed(format!("temp worktree: {e}")),
                    branch_outcomes.clone(),
                );
            }
        }
    }
    let target_path = &target_path;

    let git = GitCommand::new(false)
        .with_gitoxide(settings.use_gitoxide)
        .with_cancel(Arc::clone(cancel));

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

    // A rebase torn down by the cancel cascade surfaces through the same
    // error arm as a conflict (rebase_single_worktree already ran
    // `git rebase --abort`, restoring the worktree — including the
    // --autostash stash). Relabel it: cancellation is not a conflict.
    if cancel.is_cancelled() && (result.conflict || !result.success) {
        return (
            TaskStatus::Cancelled,
            TaskMessage::Cancelled,
            branch_outcomes.clone(),
        );
    }

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

/// Execute the batched push task (#678, `daft.sync.pushHookStrategy =
/// batched`): one `git push` carries every eligible branch, so the
/// pre-push hook fires once with all refs. The barrier node's own
/// `branch_name` is empty (no TUI row); per-branch rows are driven by
/// synthetic `TaskStarted`/`TaskCompleted` events sent from here.
#[allow(clippy::too_many_arguments)]
fn execute_push_batch_task(
    branches: &[String],
    worktree_map: &HashMap<String, (PathBuf, bool)>,
    project_root: &std::path::Path,
    settings: &DaftSettings,
    force_with_lease: bool,
    no_verify: bool,
    hook_present: bool,
    tx: &std::sync::mpsc::Sender<sync_dag::DagEvent>,
    branch_outcomes: &HashSet<TaskOutcome>,
    cancel: &Arc<CancelFlag>,
    governor: Option<&Arc<crate::governor::SyncGovernor>>,
    jobserver_env: Option<&(String, String)>,
    conflicts: &Arc<std::sync::Mutex<HashSet<String>>>,
) -> (TaskStatus, TaskMessage, HashSet<TaskOutcome>) {
    if cancel.is_cancelled() {
        return (
            TaskStatus::Cancelled,
            TaskMessage::Cancelled,
            branch_outcomes.clone(),
        );
    }

    let send_branch = |branch: &str, status: TaskStatus, message: TaskMessage| {
        let _ = tx.send(sync_dag::DagEvent::TaskCompleted {
            phase: OperationPhase::Push,
            branch_name: branch.to_string(),
            status,
            message,
        });
    };

    // Eligibility: conflicted branches keep their rebase-conflict row
    // untouched; branches without an upstream report the per-branch skip.
    let conflicted = conflicts.lock().unwrap().clone();
    let mut git = GitCommand::new(false)
        .with_gitoxide(settings.use_gitoxide)
        .with_cancel(Arc::clone(cancel));

    let governor_unit = governor.map(|gov| Arc::new(gov.begin_unit("(batched push)")));
    if governor_unit.is_some() || settings.sync_push_timeout.is_some() || jobserver_env.is_some() {
        let on_spawn = governor_unit.as_ref().map(|unit| {
            let unit = Arc::clone(unit);
            Arc::new(move |pid: u32| unit.attach_pid(pid)) as Arc<dyn Fn(u32) + Send + Sync>
        });
        let on_clock = governor_unit.as_ref().map(|unit| {
            let unit = Arc::clone(unit);
            Arc::new(move |clock: Arc<crate::git::cancel::UnitClock>| unit.attach_clock(clock))
                as Arc<dyn Fn(Arc<crate::git::cancel::UnitClock>) + Send + Sync>
        });
        git = git.with_push_supervision(crate::git::PushSupervision {
            on_spawn,
            timeout: settings.sync_push_timeout,
            on_clock,
            env: jobserver_env
                .map(|pair| vec![pair.clone()])
                .unwrap_or_default(),
        });
    }

    let mut eligible: Vec<String> = Vec::new();
    let mut batch_cwd: Option<PathBuf> = None;
    for branch in branches {
        if conflicted.contains(branch) {
            // Row already shows the conflict outcome; the push never ran.
            continue;
        }
        let path = worktree_map
            .get(branch)
            .map(|(path, _)| path.clone())
            .unwrap_or_else(|| project_root.to_path_buf());
        match git.get_branch_tracking_remote_from(branch, &path) {
            Ok(None) => {
                send_branch(branch, TaskStatus::Succeeded, TaskMessage::NoPushUpstream);
            }
            _ => {
                // Prefer the default-branch worktree as the batch cwd —
                // the hook runs there once for every ref.
                if batch_cwd.is_none() || worktree_map.get(branch).is_some_and(|(_, main)| *main) {
                    batch_cwd = Some(path.clone());
                }
                eligible.push(branch.clone());
                let _ = tx.send(sync_dag::DagEvent::TaskStarted {
                    phase: OperationPhase::Push,
                    branch_name: branch.clone(),
                });
            }
        }
    }
    if eligible.is_empty() {
        return (
            TaskStatus::Succeeded,
            TaskMessage::Ok("nothing to push".into()),
            branch_outcomes.clone(),
        );
    }

    let params = push::PushParams {
        force_with_lease,
        remote_name: settings.remote.clone(),
        no_verify,
    };
    let cwd = batch_cwd.unwrap_or_else(|| project_root.to_path_buf());
    let results = push::push_batched(&git, &cwd, &params, &eligible, hook_present);

    // Whole-batch classification mirrors execute_push_task's ladder; the
    // per-branch synthetic completions carry the row-level story.
    let batch_failed = results.iter().any(|r| !r.success);
    if cancel.is_cancelled() && batch_failed {
        for branch in &eligible {
            send_branch(branch, TaskStatus::Cancelled, TaskMessage::Cancelled);
        }
        return (
            TaskStatus::Cancelled,
            TaskMessage::Cancelled,
            branch_outcomes.clone(),
        );
    }
    if batch_failed
        && let Some(unit) = &governor_unit
        && unit.was_killed()
    {
        // Evicted: the executor requeues the whole batch; rows stay
        // "pushing" until the retry's own events land.
        return (
            TaskStatus::Evicted,
            TaskMessage::Failed("batched push killed under memory pressure".into()),
            branch_outcomes.clone(),
        );
    }
    for result in &results {
        let (status, message) = if result.success && result.up_to_date {
            (TaskStatus::Succeeded, TaskMessage::UpToDate)
        } else if result.success {
            (TaskStatus::Succeeded, TaskMessage::Pushed)
        } else {
            let hint = result
                .message
                .lines()
                .next()
                .unwrap_or("push failed")
                .to_string();
            (TaskStatus::Failed, TaskMessage::Failed(hint))
        };
        send_branch(&result.branch_name, status, message);
    }

    // The barrier node itself: failed when anything failed so the run's
    // exit code logic sees it, but with an empty row it stays invisible.
    if batch_failed {
        (
            TaskStatus::Failed,
            TaskMessage::Failed("batched push failed".into()),
            branch_outcomes.clone(),
        )
    } else {
        (
            TaskStatus::Succeeded,
            TaskMessage::Pushed,
            branch_outcomes.clone(),
        )
    }
}

/// Execute a single push task for a DAG worker.
#[allow(clippy::too_many_arguments)]
fn execute_push_task(
    branch_name: &str,
    worktree_path: Option<&PathBuf>,
    project_root: &std::path::Path,
    settings: &DaftSettings,
    force_with_lease: bool,
    no_verify: bool,
    hook_present: bool,
    tx: &std::sync::mpsc::Sender<sync_dag::DagEvent>,
    branch_outcomes: &HashSet<TaskOutcome>,
    cancel: &Arc<CancelFlag>,
    governor: Option<&Arc<crate::governor::SyncGovernor>>,
    jobserver_env: Option<&(String, String)>,
) -> (TaskStatus, TaskMessage, HashSet<TaskOutcome>) {
    if cancel.is_cancelled() {
        return (
            TaskStatus::Cancelled,
            TaskMessage::Cancelled,
            branch_outcomes.clone(),
        );
    }

    // Push doesn't need the branch's worktree; any git dir works.
    // For local-only branches, fall back to the project root.
    let fallback_path = project_root.to_path_buf();
    let target_path = worktree_path.unwrap_or(&fallback_path);

    if branch_outcomes.contains(&TaskOutcome::Conflict) {
        return (
            TaskStatus::PreconditionFailed,
            TaskMessage::Failed("rebase conflict".into()),
            branch_outcomes.clone(),
        );
    }

    let mut git = GitCommand::new(false)
        .with_gitoxide(settings.use_gitoxide)
        .with_cancel(Arc::clone(cancel));

    // Register the unit with the resource governor (#678). The guard
    // tracks the unit's lifetime (drop = gone); the spawn callback hands
    // the governor the git-push root pid for the sampler and containment
    // tiers. The timeout arms a per-unit wall clock inside run_push.
    let governor_unit = governor.map(|gov| Arc::new(gov.begin_unit(branch_name)));
    if governor_unit.is_some() || settings.sync_push_timeout.is_some() || jobserver_env.is_some() {
        let on_spawn = governor_unit.as_ref().map(|unit| {
            let unit = Arc::clone(unit);
            Arc::new(move |pid: u32| unit.attach_pid(pid)) as Arc<dyn Fn(u32) + Send + Sync>
        });
        let on_clock = governor_unit.as_ref().map(|unit| {
            let unit = Arc::clone(unit);
            Arc::new(move |clock: Arc<crate::git::cancel::UnitClock>| unit.attach_clock(clock))
                as Arc<dyn Fn(Arc<crate::git::cancel::UnitClock>) + Send + Sync>
        });
        git = git.with_push_supervision(crate::git::PushSupervision {
            on_spawn,
            timeout: settings.sync_push_timeout,
            on_clock,
            env: jobserver_env
                .map(|pair| vec![pair.clone()])
                .unwrap_or_default(),
        });
    }

    let worktree_name = target_path
        .strip_prefix(project_root)
        .ok()
        .and_then(|p| p.to_str())
        .unwrap_or(branch_name)
        .to_string();

    let params = push::PushParams {
        force_with_lease,
        remote_name: settings.remote.clone(),
        no_verify,
    };

    // Report the pre-push hook run as a phase on this branch's TUI row,
    // mirroring how lifecycle hooks surface (#599). Existence-gated so
    // hook-less repos emit nothing.
    let presenter: Option<std::sync::Arc<dyn crate::executor::presenter::JobPresenter>> =
        if hook_present && !no_verify {
            let p: std::sync::Arc<dyn crate::executor::presenter::JobPresenter> =
                crate::output::tui::TuiPresenter::new(
                    tx.clone(),
                    branch_name.to_string(),
                    sync_dag::DagHookPhase::PrePush,
                );
            Some(p)
        } else {
            None
        };

    let mut sink = NullSink;
    let result = push::push_single_worktree(
        &git,
        target_path,
        &worktree_name,
        branch_name,
        &params,
        &mut sink,
        &crate::core::worktree::ports::NoopStageRunner,
        presenter.as_ref(),
        Some(hook_present),
    );

    // A push (or its pre-push hook subtree) torn down by the cancel
    // cascade is a cancellation, not a push failure or a divergence.
    if cancel.is_cancelled() && !result.success {
        return (
            TaskStatus::Cancelled,
            TaskMessage::Cancelled,
            branch_outcomes.clone(),
        );
    }

    // A push that job-control-stopped on an interactive auth prompt daft
    // can't forward is a real, actionable failure — never the benign
    // "diverged" the hook-less branch below would assign it. Independent
    // of the cancel flag: the tty-stop is detected at level 0 (#663).
    if result.needs_terminal {
        let hint = result
            .message
            .lines()
            .next()
            .unwrap_or("push needs terminal input for interactive auth")
            .to_string();
        return (
            TaskStatus::Failed,
            TaskMessage::Failed(hint),
            branch_outcomes.clone(),
        );
    }

    // A push that outran daft.sync.pushTimeout fails terminally (#678):
    // a retry would burn another full budget, and the governor's
    // kill-requeue path must never pick it up either.
    if result.timed_out {
        let hint = result
            .message
            .lines()
            .next()
            .unwrap_or("push timed out")
            .to_string();
        return (
            TaskStatus::Failed,
            TaskMessage::Failed(hint),
            branch_outcomes.clone(),
        );
    }

    // Killed by the resource governor under memory pressure (#678): the
    // executor requeues it — this is deliberately checked after the
    // cancel, needs-terminal, and timeout arms, all of which outrank an
    // eviction. A kill that landed after git already exited 0 never gets
    // here (`result.success` stands, #7 semantics).
    if !result.success
        && let Some(unit) = &governor_unit
        && unit.was_killed()
    {
        let hint = result
            .message
            .lines()
            .next()
            .unwrap_or("killed under memory pressure")
            .to_string();
        return (
            TaskStatus::Evicted,
            TaskMessage::Failed(hint),
            branch_outcomes.clone(),
        );
    }

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
    } else if matches!(
        result.hook,
        push::HookVerdict::Rejected | push::HookVerdict::Passed
    ) {
        // A failed push with the pre-push gate in effect is a real failure
        // (#599), not a divergence warning.
        let first_line = result
            .message
            .lines()
            .next()
            .unwrap_or(result.hook.failure_cause())
            .to_string();
        (
            TaskStatus::Failed,
            TaskMessage::Failed(first_line),
            branch_outcomes.clone(),
        )
    } else {
        // Hook-less push failures stay warnings, not hard failures — use
        // Succeeded + Diverged so check_tui_failures does not count them.
        (
            TaskStatus::Succeeded,
            TaskMessage::Diverged,
            branch_outcomes.clone(),
        )
    }
}

/// Fast-forward a local-only branch from its upstream (no worktree needed).
fn execute_local_branch_update(
    branch_name: &str,
    git_dir: &std::path::Path,
) -> (TaskStatus, TaskMessage) {
    use crate::utils::git_command_at;

    let upstream = format!("{branch_name}@{{upstream}}");

    // Check if the branch can be fast-forwarded to its upstream.
    // git_command_at (not a raw `git` + current_dir) so an inherited
    // GIT_DIR can't retarget these to the hook-calling repo when sync
    // itself runs inside a git hook.
    let is_ancestor = git_command_at(git_dir)
        .args(["merge-base", "--is-ancestor", branch_name, &upstream])
        .output();

    match is_ancestor {
        Ok(output) if output.status.success() => {
            // Can fast-forward — move the branch pointer.
            let ff = git_command_at(git_dir)
                .args(["branch", "-f", branch_name, &upstream])
                .output();

            match ff {
                Ok(o) if o.status.success() => (
                    TaskStatus::Succeeded,
                    TaskMessage::Ok("fast-forwarded".into()),
                ),
                Ok(o) => {
                    let stderr = String::from_utf8_lossy(&o.stderr);
                    (
                        TaskStatus::Failed,
                        TaskMessage::Failed(format!("branch -f failed: {stderr}")),
                    )
                }
                Err(e) => (
                    TaskStatus::Failed,
                    TaskMessage::Failed(format!("branch -f error: {e}")),
                ),
            }
        }
        Ok(output) => {
            // Check if already up-to-date (upstream is ancestor of branch,
            // i.e. branch is at or ahead of upstream).
            let reverse = git_command_at(git_dir)
                .args(["merge-base", "--is-ancestor", &upstream, branch_name])
                .output();

            match reverse {
                Ok(r) if r.status.success() => {
                    // Branch is at or ahead of upstream — nothing to do.
                    (TaskStatus::Succeeded, TaskMessage::UpToDate)
                }
                _ => {
                    // Diverged — cannot fast-forward.
                    let _stderr = String::from_utf8_lossy(&output.stderr);
                    (TaskStatus::Succeeded, TaskMessage::Diverged)
                }
            }
        }
        Err(_) => {
            // No upstream or git error — skip silently.
            (TaskStatus::Succeeded, TaskMessage::UpToDate)
        }
    }
}

fn run_prune_phase(
    output: &mut dyn Output,
    settings: &DaftSettings,
    force: bool,
    cancel: &Arc<CancelFlag>,
) -> Result<prune::PruneResult> {
    let params = prune::PruneParams {
        force,
        use_gitoxide: settings.use_gitoxide,
        is_quiet: output.is_quiet(),
        remote_name: settings.remote.clone(),
        prune_cd_target: settings.prune_cd_target,
        // Sequential path: make the prune's `git fetch --prune` cancellable
        // like the update/rebase/push phases (#663).
        cancel: Some(Arc::clone(cancel)),
    };

    let hooks_config = crate::core::settings::load_hooks_config()?;
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
    cancel: &Arc<CancelFlag>,
) -> Result<()> {
    let wt_config = WorktreeConfig {
        remote_name: settings.remote.clone(),
        quiet: output.is_quiet(),
    };
    let git = GitCommand::new(wt_config.quiet)
        .with_gitoxide(settings.use_gitoxide)
        .with_cancel(Arc::clone(cancel));
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

    // Cancelled mid-phase: the torn-down worktrees would render as genuine
    // update failures. Skip the per-worktree failures + summary and let
    // run_sequential print the cancel notice and exit 130 — cancellation is
    // not a failure (#663; the TUI path relabels these Cancelled likewise).
    if cancel.is_cancelled() {
        return Ok(());
    }

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

#[allow(clippy::too_many_arguments)]
fn run_rebase_phase(
    output: &mut dyn Output,
    settings: &DaftSettings,
    base_branch: &str,
    force: bool,
    autostash: bool,
    default_branch: &str,
    included_branches: Option<&HashSet<String>>,
    cancel: &Arc<CancelFlag>,
) -> Result<rebase::RebaseResult> {
    let wt_config = WorktreeConfig {
        remote_name: settings.remote.clone(),
        quiet: output.is_quiet(),
    };
    let git = GitCommand::new(wt_config.quiet)
        .with_gitoxide(settings.use_gitoxide)
        .with_cancel(Arc::clone(cancel));
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
        rebase::execute(&params, &git, &project_root, &mut sink, included_branches)
    };
    output.finish_spinner();
    let result = exec_result?;

    // Cancelled mid-phase: a rebase torn down by the cancel cascade surfaces
    // as a conflict (rebase_single_worktree aborts on any error). Don't
    // render those as genuine conflicts — run_sequential exits 130 and
    // cancellation is not a conflict (#663).
    if cancel.is_cancelled() {
        return Ok(result);
    }

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

#[allow(clippy::too_many_arguments)]
fn run_push_phase(
    output: &mut dyn Output,
    settings: &DaftSettings,
    force_with_lease: bool,
    no_verify: bool,
    skip_branches: &HashSet<String>,
    base_branch: &str,
    cancel: &Arc<CancelFlag>,
) -> Result<()> {
    let wt_config = WorktreeConfig {
        remote_name: settings.remote.clone(),
        quiet: output.is_quiet(),
    };
    let mut git = GitCommand::new(wt_config.quiet)
        .with_gitoxide(settings.use_gitoxide)
        .with_cancel(Arc::clone(cancel));
    // Per-unit wall-clock budget (#678): run_push arms a fresh clock for
    // every branch this sequential engine pushes.
    if settings.sync_push_timeout.is_some() {
        git = git.with_push_supervision(crate::git::PushSupervision {
            on_spawn: None,
            timeout: settings.sync_push_timeout,
            on_clock: None,
            env: Vec::new(),
        });
    }
    let project_root = get_project_root()?;

    let params = push::PushParams {
        force_with_lease,
        remote_name: wt_config.remote_name.clone(),
        no_verify,
    };

    // When a pre-push hook will run, its reporting renders as phase/job
    // output through the presenter — leave the spinner off so the two don't
    // fight over the terminal (hook-less pushes keep the spinner as before).
    let hook_present = git.pre_push_hook_exists(&project_root);
    let presenter: Option<Arc<dyn crate::executor::presenter::JobPresenter>> =
        if hook_present && !no_verify {
            let p: Arc<dyn crate::executor::presenter::JobPresenter> =
                crate::executor::cli_presenter::CliPresenter::auto(
                    &crate::settings::HookOutputConfig::default(),
                );
            Some(p)
        } else {
            None
        };

    if presenter.is_none() {
        output.start_spinner("Pushing branches...");
    }
    let exec_result = {
        let mut sink = OutputSink(output);
        if settings.sync_push_hook_strategy == crate::settings::PushHookStrategy::Batched {
            push::execute_batched(&params, &git, &project_root, &mut sink, skip_branches)
        } else {
            push::execute(
                &params,
                &git,
                &project_root,
                &mut sink,
                skip_branches,
                &crate::core::worktree::ports::NoopStageRunner,
                presenter.as_ref(),
            )
        }
    };
    if presenter.is_none() {
        output.finish_spinner();
    }
    let result = exec_result?;

    // Cancelled mid-phase: torn-down pushes would render as diverged/failed.
    // Skip the summary; run_sequential prints the cancel notice and exits
    // 130 — cancellation is not a push failure (#663).
    if cancel.is_cancelled() {
        return Ok(());
    }

    render_push_result(&result, output, base_branch);

    if result.failed_count() > 0 {
        output.warning(&format!(
            "{} branch(es) failed to push",
            result.failed_count()
        ));
    }

    // A push that job-control-stopped on interactive auth daft can't forward
    // did not happen and needs terminal action — exit non-zero with the hint
    // rather than leaving it a benign "diverged" warning (#663).
    let needs_terminal = result.needs_terminal_count();
    if needs_terminal > 0 {
        anyhow::bail!(
            "{needs_terminal} push(es) stopped waiting for terminal input (interactive auth?); \
             run the push manually there, or configure an ssh-agent/credential helper"
        );
    }

    // A push unit that outran its wall-clock budget was torn down — the
    // push did not happen. Exit non-zero with the budget hint rather than
    // leaving it a benign "diverged" warning (#678).
    let timed_out = result.timed_out_count();
    if timed_out > 0 {
        anyhow::bail!(
            "{timed_out} push(es) timed out; raise or disable daft.sync.pushTimeout \
             if they legitimately need longer"
        );
    }

    // A pre-push gate saying no must surface as a failure, not a warning.
    let gated = result.gated_failure_count();
    if gated > 0 {
        anyhow::bail!(
            "{gated} push(es) failed with the repo's pre-push hook honored \
             (re-run with --no-verify to bypass the hook)"
        );
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
    } else if r.needs_terminal {
        // Job-control-stopped on interactive auth: not a benign divergence
        // — the push did not happen and needs terminal action (#663).
        output.error(&format!(" * {} {name} — needs terminal auth", tag_failed()));
    } else if r.timed_out {
        // Outran daft.sync.pushTimeout and was torn down: the push did not
        // happen — never a benign divergence (#678).
        output.error(&format!(" * {} {name} — timed out", tag_failed()));
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::core::ownership::BranchOwner;

    fn owner(email: &str, is_current_user: bool) -> BranchOwner {
        BranchOwner {
            name: email.split('@').next().unwrap_or(email).to_string(),
            email: email.to_string(),
            is_current_user,
        }
    }

    #[test]
    fn governor_plan_resolution() {
        use PushGovernorPlan::{Dynamic, StaticCap, Ungoverned};
        let auto = GovernorMode::Auto;
        let off = GovernorMode::Off;
        let config_auto = GovernorJobs::Auto;

        // No hook / hooks bypassed / throttling refused → ungoverned.
        assert_eq!(
            resolve_push_governor(false, false, false, None, auto, config_auto, 16),
            Ungoverned
        );
        assert_eq!(
            resolve_push_governor(true, true, false, None, auto, config_auto, 16),
            Ungoverned
        );
        assert_eq!(
            resolve_push_governor(true, false, true, Some(4), auto, config_auto, 16),
            Ungoverned
        );
        // Auto mode: dynamic governor; cap from --jobs, config, or
        // max(2, cores/4).
        assert_eq!(
            resolve_push_governor(true, false, false, None, auto, config_auto, 16),
            Dynamic { cap: 4 }
        );
        assert_eq!(
            resolve_push_governor(true, false, false, Some(6), auto, config_auto, 16),
            Dynamic { cap: 6 }
        );
        assert_eq!(
            resolve_push_governor(true, false, false, None, auto, GovernorJobs::Fixed(3), 16),
            Dynamic { cap: 3 }
        );
        // Off mode: only an explicit --jobs still caps, statically.
        assert_eq!(
            resolve_push_governor(true, false, false, None, off, config_auto, 16),
            Ungoverned
        );
        assert_eq!(
            resolve_push_governor(true, false, false, Some(2), off, config_auto, 16),
            StaticCap(2)
        );
    }

    #[test]
    fn is_branch_included_true_when_current_user() {
        let me = owner("me@example.com", true);
        assert!(is_branch_included("feat/x", Some(&me), &[]));
    }

    #[test]
    fn is_branch_included_false_without_filters_and_not_current_user() {
        let bob = owner("bob@example.com", false);
        assert!(!is_branch_included("feat/x", Some(&bob), &[]));
    }

    #[test]
    fn is_branch_included_true_for_unowned_filter_even_when_not_current_user() {
        let bob = owner("bob@example.com", false);
        assert!(is_branch_included(
            "feat/x",
            Some(&bob),
            &[IncludeFilter::Unowned]
        ));
    }

    #[test]
    fn is_branch_included_true_for_unowned_filter_when_no_owner() {
        assert!(is_branch_included(
            "feat/x",
            None,
            &[IncludeFilter::Unowned]
        ));
    }

    #[test]
    fn is_branch_included_matches_email_filter_case_insensitive() {
        let bob = owner("Bob@Example.com", false);
        assert!(is_branch_included(
            "feat/x",
            Some(&bob),
            &[IncludeFilter::Email("bob@example.com".into())],
        ));
    }

    #[test]
    fn is_branch_included_matches_branch_filter_by_name() {
        let bob = owner("bob@example.com", false);
        assert!(is_branch_included(
            "feat/x",
            Some(&bob),
            &[IncludeFilter::Branch("feat/x".into())],
        ));
    }

    #[test]
    fn is_branch_included_false_when_filters_dont_match() {
        let bob = owner("bob@example.com", false);
        assert!(!is_branch_included(
            "feat/x",
            Some(&bob),
            &[
                IncludeFilter::Email("alice@example.com".into()),
                IncludeFilter::Branch("feat/y".into()),
            ],
        ));
    }

    #[test]
    fn is_branch_included_falls_back_gracefully_when_owner_is_none() {
        assert!(!is_branch_included("feat/x", None, &[]));
        assert!(!is_branch_included(
            "feat/x",
            None,
            &[IncludeFilter::Email("me@example.com".into())],
        ));
        assert!(is_branch_included(
            "feat/x",
            None,
            &[IncludeFilter::Branch("feat/x".into())],
        ));
    }

    // Regression: #567 — Update fast-forward must clear the PostFetch REMOTE_AHEAD_BEHIND value.
    #[test]
    fn update_post_task_fields_includes_remote_ahead_behind() {
        let fields = update_post_task_fields();
        assert!(fields.contains(FieldSet::REMOTE_AHEAD_BEHIND));
        assert!(fields.contains(FieldSet::BASE_AHEAD_BEHIND));
        assert!(fields.contains(FieldSet::LAST_COMMIT));
        assert!(fields.contains(FieldSet::CHANGES));
    }
}
