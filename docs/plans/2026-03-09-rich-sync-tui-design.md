# Rich TUI for Sync & Prune Commands

## Summary

Replace the sequential text output of `daft sync` and `daft prune` with an
inline ratatui-based TUI that shows an operation progress header and a live
worktree table. Parallelize execution via a dependency graph so independent
operations (prune, update, rebase) run concurrently across worktrees.

## Visual Layout

Two regions rendered inline (no alternate screen) via `ratatui` with
`Viewport::Inline`:

### Region 1: Operation Header

Ordered list of high-level operations. Each line shows a status indicator:

- Spinner while running (only one active at a time)
- Green checkmark when completed (text dimmed)
- Plain text for upcoming operations

Operations by command:

- `daft prune`: Fetch remote, Prune stale branches
- `daft sync`: Fetch remote, Prune stale branches, Update worktrees
- `daft sync --rebase BRANCH`: Fetch remote, Prune stale branches, Update
  worktrees, Rebase onto BRANCH

An operation's spinner runs while any task of that type is active. The checkmark
appears when all tasks of that type are done. This is a completion indicator,
not a success indicator -- partial success (3 of 5 worktrees rebased) is still
"completed".

### Region 2: Worktree Table

Same columns as `daft list` with a Status column prepended before the annotation
column. All worktrees are shown from the start. Status updates in-place as
operations traverse through them.

Status values:

- `(spinner) fetching` / `updating` / `pruning` / `rebasing` -- yellow, spinner
- `(checkmark) updated` -- green
- `(checkmark) up to date` -- dim
- `(checkmark) rebased` -- green
- `(x) conflict` -- red
- `(circle-slash) skipped` -- yellow
- `(dash) pruned` -- red
- `(x) failed` -- red

Status shows the current/latest operation for each worktree and updates as
phases progress (e.g., updating -> updated -> rebasing -> rebased). No square
brackets around statuses.

## Column Priority

Columns degrade gracefully on narrow terminals. Priority (highest to lowest):

1. Status -- always shown
2. Annotation -- negligible width
3. Branch -- essential identifier
4. Path -- important context
5. Base -- compact
6. Remote -- compact
7. Changes -- compact
8. Age -- compact
9. Last Commit -- widest, lowest priority

Last Commit degradation: full column -> age only -> removed entirely. Other
low-priority columns are removed from the bottom of the list as needed.

## Parallelized Execution

### Dependency Graph

Operations are broken into fine-grained tasks in a DAG:

```
fetch_remote
  |
  +-> prune(branch_a)       (all prunes parallel)
  +-> prune(branch_b)
  |
  +-> update(master)         (all updates parallel)
  +-> update(feat/login)
  +-> update(feat/dirty)
  |
  +-> rebase(feat/login, master)   (depends on update(master))
  +-> rebase(feat/dirty, master)   (depends on update(master))
```

Dependency rules:

- `fetch_remote` blocks everything
- `prune(X)` depends only on `fetch_remote`
- `update(X)` depends only on `fetch_remote`
- `rebase(X, base)` depends on `update(base)`
- Prunes and updates are independent of each other
- Prune of current worktree is deferred (depends on all other prune tasks)

### Execution Model

Reuses the `Mutex<DagState> + Condvar` worker pool pattern from
`src/hooks/yaml_executor/dependency.rs`.

Workers send status updates through `mpsc` channel. The main thread owns the
ratatui Terminal, reads from the channel, and re-renders on each update. Workers
never touch the terminal directly.

## Architecture

### New Modules

- `src/core/worktree/sync_dag.rs` -- DAG construction, `SyncTask` enum (Fetch,
  Prune, Update, Rebase), task state management
- `src/output/tui.rs` -- ratatui inline renderer, operation header + worktree
  table, column priority logic

### Modified Modules

- `src/commands/sync.rs` -- Replace three-phase sequential flow with: collect
  worktree info, build DAG, spawn TUI renderer, run worker pool, finalize
- `src/commands/prune.rs` -- Same treatment, simpler DAG (fetch + prune tasks)
- `Cargo.toml` -- Add `ratatui` and `crossterm`

### Unchanged

- `src/core/worktree/prune.rs` -- Per-branch logic extracted into callable
  functions for DAG workers
- `src/core/worktree/fetch.rs` -- Per-worktree pull logic extracted similarly
- `src/core/worktree/rebase.rs` -- Per-worktree rebase extracted similarly
- `src/core/worktree/list.rs` -- `WorktreeInfo` and `collect_worktree_info`
  reused for initial table data
- `src/commands/fetch.rs` -- Standalone `daft update` keeps current interface

### Data Flow

```
sync.rs: run()
  |
  +- collect_worktree_info()  -> Vec<WorktreeInfo>
  +- identify_gone_branches() -> Vec<String>
  +- build_sync_dag()         -> DagState + tasks
  |
  +- main thread: TUI renderer loop (recv channel -> re-render)
  +- scoped worker threads: pick ready task -> execute -> send update -> unlock
```

## Non-TTY Fallback

When stderr is not a TTY (piped, CI), skip ratatui and fall back to sequential
text output with the new status wording (no square brackets). The DAG still
parallelizes execution; only the rendering changes.

## Edge Cases

- No worktrees to prune: operation header shows completed immediately, table
  shows all worktrees unchanged
- No worktrees at all: just the operation header, no table
- `git fetch` failure: entire command fails before showing the table
- Individual task failure: row marked as failed, other tasks continue, exit code
  non-zero
- Current worktree pruned: `cd_target` written to `DAFT_CD_FILE` after all tasks
  complete
- Terminal resize: ratatui handles natively, column priority re-evaluates on
  each render
- Hook execution: prune tasks run pre/post-remove hooks, output captured and
  sent through channel. Interactive hooks not supported in parallel mode.

## Dependencies

- `ratatui` (with `crossterm` backend) -- inline viewport TUI rendering
- `crossterm` -- terminal manipulation (already an indirect dependency via
  indicatif)
