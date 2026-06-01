//! Live cell-by-cell rendering for `daft list`.
//!
//! Used when stdout is a TTY, the user did not request structured output
//! (`--format`), and `DAFT_NO_LIVE` is not set. Otherwise the dispatcher
//! in `list::run` falls back to `list::run_blocking` (today's behavior).

use crate::{
    commands::list::{Args, resolve_base_branch},
    core::{
        columns::{ColumnSelection, CommandKind, ListColumn, ResolvedColumns},
        repo::{get_current_worktree_path, get_git_common_dir, get_project_root},
        sort::SortSpec,
        worktree::{
            info_field::FieldSet,
            list::{EntryKind, WorktreeInfo, collect_branch_info},
            list_stream,
            sync_dag::{DagEvent, PatchSource},
        },
    },
    git::GitCommand,
    output::tui::{Column, TuiRenderer, TuiState},
    settings::DaftSettings,
};
use anyhow::Result;
use std::{
    collections::HashSet,
    sync::{Arc, mpsc},
};

pub fn run_live(args: Args) -> Result<()> {
    // Construct the body `GitCommand` first and load settings through it so the
    // repo is discovered once and reused for the command body (#584).
    let git = GitCommand::new(false);
    let settings = DaftSettings::load_with(&git)?;
    let git = git.with_gitoxide(settings.use_gitoxide);
    let user_email: Option<String> = git.config_get("user.email").ok().flatten();
    let git_common_dir = get_git_common_dir()?;
    let base_branch = resolve_base_branch(&git_common_dir, &settings);
    let current_path = get_current_worktree_path()
        .ok()
        .and_then(|p| p.canonicalize().ok());
    let project_root = get_project_root()?;
    let cwd = std::env::current_dir().unwrap_or_else(|_| project_root.clone());

    let stat = args.stat.unwrap_or(settings.list_stat);

    // Resolve columns + sort spec (same logic as run_blocking).
    let columns_input = args.columns.clone().or(settings.list_columns.clone());
    let resolved = match columns_input {
        Some(ref input) => {
            ColumnSelection::parse(input, CommandKind::List).map_err(|e| anyhow::anyhow!("{e}"))?
        }
        None => ResolvedColumns::defaults(ListColumn::list_defaults()),
    };
    let selected_columns = resolved.columns.clone();
    let columns_explicit = resolved.explicit;
    let sort_input = args.sort.clone().or(settings.list_sort.clone());
    let sort_spec = match sort_input {
        Some(ref input) => Some(
            SortSpec::parse(input)
                .map_err(|e| anyhow::anyhow!("{e}"))?
                .with_stat(stat),
        ),
        None => None,
    };

    let show_local = args.branches || args.all;
    let show_remote = args.remotes || args.all;

    // Cheap porcelain seed: branches, paths, is_default, is_current.
    let porcelain = git
        .worktree_list_porcelain()
        .map_err(|e| anyhow::anyhow!("{e}"))?;
    let entries = crate::core::worktree::porcelain::parse_worktree_list_porcelain(&porcelain);
    let mut worktree_infos: Vec<WorktreeInfo> = Vec::new();
    let mut targets: Vec<list_stream::CollectorTarget> = Vec::new();
    let mut worktree_branches: HashSet<String> = HashSet::new();

    for entry in entries {
        if entry.is_bare {
            continue;
        }
        let branch_display = if entry.is_detached {
            "(detached)".to_string()
        } else {
            entry.branch.clone().unwrap_or_default()
        };
        let canonical = entry
            .path
            .canonicalize()
            .unwrap_or_else(|_| entry.path.clone());
        let is_current = current_path.as_deref() == Some(canonical.as_path());
        let is_default_branch = entry.branch.as_deref() == Some(base_branch.as_str());

        let mut info = WorktreeInfo::empty(&branch_display);
        info.path = Some(entry.path.clone());
        info.is_current = is_current;
        info.is_default_branch = is_default_branch;
        info.is_sandbox = entry.is_detached;
        info.kind = EntryKind::Worktree;
        worktree_infos.push(info);

        targets.push(list_stream::CollectorTarget {
            branch_name: branch_display.clone(),
            path: Some(entry.path.clone()),
            kind: EntryKind::Worktree,
            is_detached: entry.is_detached,
        });

        if !branch_display.is_empty() && !entry.is_detached {
            worktree_branches.insert(branch_display);
        }
    }

    // Optionally enumerate non-worktree branches (sync — cheap git for-each-ref).
    if show_local || show_remote {
        let branch_infos = collect_branch_info(
            &git,
            &base_branch,
            stat,
            show_local,
            show_remote,
            &worktree_branches,
            &project_root,
            settings.ownership_strategy,
            user_email.as_deref(),
            &settings.remote,
        )?;
        for info in branch_infos {
            targets.push(list_stream::CollectorTarget {
                branch_name: info.name.clone(),
                path: info.path.clone(),
                kind: info.kind,
                is_detached: false,
            });
            worktree_infos.push(info);
        }
    }

    // Short-circuit when the merged set is empty: skip TUI bringup and
    // print a static empty-state hint. Avoids ratatui flicker and a
    // raw-mode bringup just to render three lines of static text.
    if worktree_infos.is_empty() {
        crate::commands::list_empty::print(
            &mut std::io::stdout(),
            crate::styles::colors_enabled(),
        )?;
        return Ok(());
    }

    // Build TUI state — pin_default_branch=false, partition_by_owner=false
    // for `daft list` (per spec).
    let tui_columns: Vec<Column> = selected_columns
        .iter()
        .map(|c| Column::from_list_column(*c))
        .collect();

    let (tx, rx) = mpsc::channel::<DagEvent>();

    // Spawn the streaming collector. Cells stream into LiveTable as they
    // arrive.
    let collector_ctx = Arc::new(list_stream::CollectorContext {
        use_gitoxide: settings.use_gitoxide,
        base_branch: base_branch.clone(),
        remote_name: settings.remote.clone(),
        ownership_strategy: settings.ownership_strategy,
        user_email: user_email.clone(),
        git_common_dir: git_common_dir.clone(),
    });
    let collector_handle = list_stream::spawn(
        list_stream::CollectorRequest {
            targets,
            fields: FieldSet::ALL,
            stat,
            source: PatchSource::Collector,
            ctx: collector_ctx,
        },
        tx,
    );

    let state = TuiState::new(
        Vec::new(), // no phases
        worktree_infos,
        project_root.clone(),
        cwd,
        stat,
        u8::from(args.verbose),
        Some(tui_columns),
        columns_explicit,
        None, // unowned_start_index — list does not partition
        sort_spec,
        false, // pin_default_branch
        false, // partition_by_owner
        // The streaming collector requests `FieldSet::ALL` (see CollectorRequest
        // above), so the seed never authoritatively finalizes any field for
        // `daft list` — every cell starts in the loading state and transitions
        // when its patch lands.
        FieldSet::EMPTY,
    );

    // Single source of truth for cancellation: the renderer's Ctrl-C handler
    // flips the same flag the collector workers observe between cluster calls.
    let cancel = collector_handle.cancel_flag();

    // The inline viewport doesn't enable raw mode, so crossterm's keyboard
    // event loop never receives Ctrl-C — the OS sends SIGINT first and the
    // process terminates before our handler can run. Install a SIGINT handler
    // that flips the same cancel flag, so the renderer's cancel-signal poll
    // path picks it up and runs the graceful exit.
    // `ctrlc::set_handler` is process-global and can only be installed once;
    // swallow the error so nested invocations and tests don't panic.
    let signal_cancel = std::sync::Arc::clone(&cancel);
    let _ = ctrlc::set_handler(move || {
        signal_cancel.store(true, std::sync::atomic::Ordering::Relaxed);
    });

    // The renderer waits for `WorktreeInfoCollectionDone`, which the collector
    // handle emits on `join()`. Joining must happen on a separate thread so the
    // sentinel fires while the renderer is still listening on the channel.
    let join_thread = std::thread::spawn(move || {
        collector_handle.join();
    });

    // Enable raw mode so crossterm's event loop receives Ctrl-C as a key event
    // (ISIG off) and the terminal driver doesn't echo `^C` mid-render. The
    // SIGINT handler above stays installed as a fallback in case raw mode fails
    // to enable. RAII guard restores cooked mode on every exit path.
    let _raw_guard = enable_raw_mode_guard();

    let renderer = TuiRenderer::new(state, rx).with_cancel_signal(cancel);
    let final_state = renderer.run()?;

    // On normal completion, workers have already finished and `join()` returns
    // immediately. On cancellation, workers may still be mid-`git` invocation
    // (the cancel flag is checked between clusters, not mid-command). Skip the
    // join so the user gets an instant prompt back; the OS reaps any in-flight
    // git children when the process exits — they're read-only and safe to abort.
    if !final_state.live.cancelled {
        let _ = join_thread.join();
    }
    Ok(())
}

/// RAII guard that enables crossterm raw mode now and restores cooked mode on
/// drop. Best-effort: if `enable_raw_mode` fails (e.g. stdin isn't a terminal),
/// the guard is still returned so its `Drop` is safe to run. Disabling raw
/// mode on a terminal that wasn't in raw mode is a no-op.
fn enable_raw_mode_guard() -> RawModeGuard {
    let _ = crossterm::terminal::enable_raw_mode();
    RawModeGuard
}

struct RawModeGuard;

impl Drop for RawModeGuard {
    fn drop(&mut self) {
        let _ = crossterm::terminal::disable_raw_mode();
    }
}
