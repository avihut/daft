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
            list::{EntryKind, Stat, WorktreeInfo, collect_branch_info},
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

    // Request only the fields this view renders or sorts by. Requesting more
    // (the old `FieldSet::ALL`) forced every worker through the SIZE cluster —
    // a full recursive walk of each worktree — before the completion sentinel
    // could fire and let the renderer exit (#665).
    let fields = collector_fields(&selected_columns, sort_spec.as_ref(), stat);

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

    // Size cache: seed each worktree row with its last-known size so the Size
    // column shows a value instantly (rendered dim/stale) while the bounded
    // walk refreshes it in the background. Keyed by branch slug in the repo's
    // coordinator store; only worktree rows (those with a path) are seeded,
    // since only they are walked and can supersede the stale value. Held for
    // the post-run persist. Best-effort — a cold/missing cache just yields
    // today's shimmer.
    let size_repo_hash: Option<String> = if fields.contains(FieldSet::SIZE) {
        let hash =
            crate::core::repo_identity::compute_repo_id_from_common_dir(&git_common_dir).ok();
        if let Some(ref h) = hash {
            let cached = crate::commands::size_cache::read_worktree_sizes(h);
            if !cached.is_empty() {
                crate::commands::size_cache::seed_worktree_sizes(&mut worktree_infos, &cached);
            }
        }
        hash
    } else {
        None
    };

    // Forge-PR cache: read decorations (outbound PR numbers + statuses) and
    // kick a detached refresh, only when the PR column is in play —
    // explicitly asking for +pr is asking for forge-derived data (the
    // local-first judgment call). The first paint uses whatever the cache
    // holds now; when a refresh was actually spawned, a poll thread (started
    // below, once the event channel exists) watches the store and swaps the
    // lookup into the live table mid-run, so even a cold first invocation
    // decorates within a couple of seconds instead of on the *next* run.
    // Best-effort — a cold cache and a failed refresh just leave
    // config-only cells.
    let mut forge_refresh_pending = false;
    let mut forge_repo_hash: Option<String> = None;
    let forge_lookup = if fields.contains(FieldSet::FORGE_REF) {
        forge_refresh_pending = crate::commands::forge_cache::spawn_background_refresh();
        forge_repo_hash =
            crate::core::repo_identity::compute_repo_id_from_common_dir(&git_common_dir).ok();
        forge_repo_hash
            .as_deref()
            .map(crate::commands::forge_cache::load_lookup)
    } else {
        None
    };

    // Build TUI state — pin_default_branch=false, partition_by_owner=false
    // for `daft list` (per spec).
    let tui_columns: Vec<Column> = selected_columns
        .iter()
        .map(|c| Column::from_list_column(*c))
        .collect();

    let (tx, rx) = mpsc::channel::<DagEvent>();
    // Cloned before the collector consumes `tx`; feeds the forge-refresh
    // poll thread spawned after TUI state exists.
    let forge_poll_tx = tx.clone();

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
            fields,
            stat,
            source: PatchSource::Collector,
            ctx: collector_ctx,
            size_jobs: crate::core::size_walk::resolve_jobs(settings.list_size_concurrency),
        },
        tx,
    );

    let mut state = TuiState::new(
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
        // Seed-finalize the complement of the collector request: fields the
        // collector won't stream must not read as in-flight (loading shimmer,
        // verbose footer). Every requested cell starts in the loading state
        // and transitions when its patch lands.
        !fields,
    );
    // Post-set like `unowned_start_index`: TuiState::new stays untouched for
    // the one caller that decorates.
    state.live.cfg.forge_prs = forge_lookup.clone();

    // Watch for the detached forge refresh landing while the table is live:
    // poll the store (cheap reader-pool read, no network — the render layer
    // never touches the store itself) and deliver the fresh lookup through
    // the same event channel the collectors use. One-shot: the refresh
    // rewrites its snapshot in a single transaction, so the first observed
    // change is the whole update. No join — the thread spawns no
    // subprocesses and dies with the process (or at the deadline when the
    // refresh never lands: gh missing, network down).
    if forge_refresh_pending && let Some(hash) = forge_repo_hash {
        let seed = forge_lookup.unwrap_or_default();
        let forge_tx = forge_poll_tx;
        std::thread::spawn(move || {
            let deadline = std::time::Instant::now() + std::time::Duration::from_secs(20);
            while std::time::Instant::now() < deadline {
                std::thread::sleep(std::time::Duration::from_millis(750));
                let fresh = crate::commands::forge_cache::load_lookup(&hash);
                if fresh != seed {
                    let _ = forge_tx.send(DagEvent::ForgePrsRefreshed(fresh));
                    return;
                }
            }
        });
    }

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
    let _raw_guard = crate::output::tui::enable_raw_mode_guard();

    let renderer = TuiRenderer::new(state, rx).with_cancel_signal(cancel);
    let final_state = renderer.run()?;

    // Persist freshly-walked sizes so the next run seeds from them. Only rows
    // that actually received a SIZE patch this run are written — a
    // stale-seeded value that never refreshed (e.g. after Ctrl-C) is left
    // untouched so its `measured_at` stays honest. The store helper applies
    // the stat-guard (a vanished path is skipped, so a removed worktree can't
    // clobber a good cached size with the walk's `Some(0)`).
    if let Some(repo_hash) = size_repo_hash {
        let fresh = final_state
            .live
            .rows
            .iter()
            .enumerate()
            .filter(|(idx, row)| {
                final_state.live.received_patches[*idx].contains(FieldSet::SIZE)
                    && row.info.size_bytes.is_some()
                    // Sandboxes collide on one slug — don't cache them (review).
                    && !row.info.is_sandbox
            })
            .filter_map(|(_, row)| {
                Some((
                    row.info.name.clone(),
                    row.info.path.clone()?,
                    row.info.size_bytes?,
                ))
            });
        crate::commands::size_cache::persist_worktree_sizes(&repo_hash, fresh);
    }

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
/// Fields the streaming collector must populate for this view: what the
/// selected columns render plus what the sort inspects
/// (`SortSpec::required_fields`, itself `--stat`-aware). The collector
/// request routes through here so it can't drift back to `FieldSet::ALL`,
/// whose hidden SIZE walk kept the renderer alive for seconds after the
/// table was fully drawn (#665).
fn collector_fields(columns: &[ListColumn], sort_spec: Option<&SortSpec>, stat: Stat) -> FieldSet {
    let mut fields = FieldSet::EMPTY;
    for column in columns {
        fields |= match column {
            // Populated by the porcelain seed, never streamed.
            ListColumn::Annotation | ListColumn::Branch | ListColumn::Path => FieldSet::EMPTY,
            ListColumn::Size => FieldSet::SIZE,
            ListColumn::Base => FieldSet::BASE_AHEAD_BEHIND,
            ListColumn::Changes => FieldSet::CHANGES,
            ListColumn::Remote => FieldSet::REMOTE_AHEAD_BEHIND,
            ListColumn::Age => FieldSet::BRANCH_AGE,
            ListColumn::Owner => FieldSet::OWNER,
            ListColumn::Pr => FieldSet::FORGE_REF,
            ListColumn::Hash | ListColumn::LastCommit => FieldSet::LAST_COMMIT,
        };
    }
    if let Some(spec) = sort_spec {
        fields |= spec.required_fields();
    }
    if stat == Stat::Lines {
        // Rendered diff columns display line counts in lines mode, so their
        // `*_LINES` variants must be collected even without a matching sort
        // key (sort keys arrive pre-upgraded from `required_fields`). The
        // summary bits stay in the set — the loading shimmer keys off them.
        if fields.contains(FieldSet::BASE_AHEAD_BEHIND) {
            fields |= FieldSet::BASE_LINES;
        }
        if fields.contains(FieldSet::CHANGES) {
            fields |= FieldSet::CHANGES_LINES;
        }
        if fields.contains(FieldSet::REMOTE_AHEAD_BEHIND) {
            fields |= FieldSet::REMOTE_LINES;
        }
    }
    fields
}

#[cfg(test)]
mod collector_fields_tests {
    use super::*;

    fn hidden_fields() -> FieldSet {
        FieldSet::SIZE
            | FieldSet::MTIME
            | FieldSet::BASE_LINES
            | FieldSet::CHANGES_LINES
            | FieldSet::REMOTE_LINES
    }

    fn sort(input: &str, stat: Stat) -> SortSpec {
        SortSpec::parse(input).unwrap().with_stat(stat)
    }

    /// #665 regression: the default view must not collect SIZE/MTIME (or any
    /// line-stat field) — the hidden SIZE walk of every worktree is what kept
    /// `daft list` alive for seconds after the table was fully rendered.
    #[test]
    fn default_view_collects_only_rendered_fields() {
        let fields = collector_fields(ListColumn::list_defaults(), None, Stat::Summary);
        for needed in [
            FieldSet::BASE_AHEAD_BEHIND,
            FieldSet::CHANGES,
            FieldSet::REMOTE_AHEAD_BEHIND,
            FieldSet::BRANCH_AGE,
            FieldSet::OWNER,
            FieldSet::LAST_COMMIT,
        ] {
            assert!(
                fields.contains(needed),
                "default view must collect {needed:?}"
            );
        }
        assert!(
            !fields.intersects(hidden_fields()),
            "default view must not collect SIZE/MTIME/line stats"
        );
    }

    #[test]
    fn size_column_selection_requests_size() {
        // Through the real parser, as `--columns +size` resolves it.
        let resolved = ColumnSelection::parse("+size", CommandKind::List).unwrap();
        let fields = collector_fields(&resolved.columns, None, Stat::Summary);
        assert!(fields.contains(FieldSet::SIZE));
        assert!(!fields.contains(FieldSet::MTIME));
    }

    #[test]
    fn size_sort_requests_size_even_without_size_column() {
        let spec = sort("-size", Stat::Summary);
        let fields = collector_fields(ListColumn::list_defaults(), Some(&spec), Stat::Summary);
        assert!(fields.contains(FieldSet::SIZE));
    }

    #[test]
    fn activity_sort_requests_mtime_and_last_commit() {
        let spec = sort("-activity", Stat::Summary);
        let fields = collector_fields(ListColumn::list_defaults(), Some(&spec), Stat::Summary);
        assert!(fields.contains(FieldSet::MTIME | FieldSet::LAST_COMMIT));
        assert!(!fields.contains(FieldSet::SIZE));
    }

    #[test]
    fn lines_stat_upgrades_rendered_groups_to_line_fields() {
        let fields = collector_fields(ListColumn::list_defaults(), None, Stat::Lines);
        assert!(
            fields
                .contains(FieldSet::BASE_LINES | FieldSet::CHANGES_LINES | FieldSet::REMOTE_LINES)
        );
        // Summary bits stay: the loading shimmer keys off them.
        assert!(fields.contains(FieldSet::BASE_AHEAD_BEHIND | FieldSet::CHANGES));
        assert!(!fields.contains(FieldSet::SIZE));
    }

    #[test]
    fn lines_sort_on_hidden_column_still_gets_line_fields() {
        // `--columns branch,path --sort -base --stat lines`: compare() reads
        // base_lines_* even though no base column is rendered.
        let spec = sort("-base", Stat::Lines);
        let fields = collector_fields(
            &[ListColumn::Branch, ListColumn::Path],
            Some(&spec),
            Stat::Lines,
        );
        assert!(fields.contains(FieldSet::BASE_AHEAD_BEHIND | FieldSet::BASE_LINES));
    }

    #[test]
    fn seed_only_view_collects_nothing() {
        let fields = collector_fields(&[ListColumn::Branch, ListColumn::Path], None, Stat::Summary);
        assert!(fields.is_empty());
    }
}
