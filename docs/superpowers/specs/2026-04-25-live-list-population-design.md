# Live List Population — Design

## Goal

Replace the spinner-then-snapshot rendering used by `daft list`, `daft prune`,
and `daft sync` with a live UX in which rows appear immediately from cheap
porcelain data and individual cells fill in as their underlying git/FS calls
return. The list becomes interactive almost instantly; slow cells (size, line
counts, ahead/behind) settle progressively without blocking the rest of the
table.

For `daft list`, this also closes the gap between commands that already render
live (prune/sync, via `OperationTable` over a DAG-event channel) and the one
read-only command that still blocks behind a spinner. The new shared
infrastructure reuses the existing `DagEvent` channel and ratatui inline
viewport, extracted into a `LiveTable` widget that all three commands share.

## Motivation

Today every "list-using" command pays the full cost of `collect_worktree_info`
before showing anything:

- `daft list` (`src/commands/list.rs:209`) shows a spinner when `Stat::Lines`,
  `--branches`, `--remotes`, or the size column is requested. The user waits
  several seconds for cells they never asked to wait for (branch name and path
  are known instantly from `git worktree list --porcelain`).
- `daft prune` (`src/commands/prune.rs:221`) and `daft sync`
  (`src/commands/sync.rs:418`) already drive a live ratatui renderer for the
  _task execution_ phase, but they still block on a synchronous
  `collect_worktree_info` call before the TUI can launch. The same spinner
  problem, just inside an otherwise-live command.

Per-entry data collection is independent across worktrees and trivially
parallelizable; the bottleneck is purely that the existing collector is
sequential and that consumers wait for it to finish.

## Non-Goals

- **Restructuring the column model.** `ColumnSelection`, `ListColumn`, `Stat`,
  `SortSpec` stay as they are. The new code consumes them.
- **Streaming structured output.** `--format json|csv|ndjson|...` keeps today's
  one-shot semantics. A partial JSON document mid-stream is a footgun.
- **Streaming non-TTY plain output.** Piping `daft list` to `less` or a file
  also keeps today's one-shot output. "Live" is a TTY-only affordance.
- **Per-cell error indicators.** Today errors are silently swallowed (cells
  return `None`). That stays. Adding error UX is out of scope.
- **Killing in-flight git subprocesses on Ctrl-C.** Cancellation is cooperative
  between cluster calls. Worst case the user waits one git call.
- **Changing `daft list`'s output for non-TTY callers.** Scripts piping
  `daft list` see the exact same output as today.

## Locked Design Decisions

These were decided during brainstorming and are load-bearing for the rest of the
spec:

| #   | Decision                                                                                                                                                                                    |
| --- | ------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------- |
| 1   | This spec covers all three commands. Implementation ships in three phases (shared infra → `daft list` → migrate prune/sync), each in its own PR.                                            |
| 2   | The streaming collector is a **re-runnable subset engine**: it takes a `FieldSet` + a list of branches and emits patches. It knows nothing about phases. Orchestrators trigger re-runs.     |
| 3   | Non-TTY (pipe stdout, redirect to file) and `--format` structured output for `daft list` fall back to today's one-shot blocking path. The new code path is gated on TTY detection.          |
| 4   | Sort tie-break diverges between commands by config: `pin_default_branch=false` for `daft list` (pure `--sort`), `pin_default_branch=true` for prune/sync (anchor on the operation's trunk). |
| 5   | Owner-driven partitioning in prune/sync shuffles live as owners arrive — the partition divider moves mid-render. Same principle as live re-sorting.                                         |
| 6   | The row-rendering core is extracted as `LiveTable`; `OperationTable` becomes a wrapper that adds phase banners + hook sub-rows around `LiveTable`.                                          |

Auto-baked smaller defaults (overridable but assumed throughout):

- **Cancellation in `daft list`**: Ctrl-C cleanly exits, returns whatever cells
  have arrived, exit code 0.
- **Verbose `-v`**: footer shows inflight cell count and total elapsed.
- **Loading glyph**: dim middle-dot `·` for unfilled cells.
- **Per-cell errors**: keep today's silent-fail (None → blank).
- **`--branches`/`--remotes` enumeration**: synchronous porcelain + branch enum
  before the TUI launches; viewport sized once.
- **Collector parallelism**: per-row workers (one thread per worktree, cells
  sequential within), reusing the `thread::scope` pattern from
  `src/core/worktree/exec/progress_renderer.rs:171`.
- **`--no-live` opt-out**: env var `DAFT_NO_LIVE=1` forces the one-shot path.
  Used by integration tests for golden-output stability.

## Architecture

Three layers, one producer, three consumers:

```
┌───────────────────────────────────────────────────────────────────┐
│  Streaming collector (subset engine)                              │
│  - Input: { branches[], FieldSubset, Stat, OwnershipStrategy }    │
│  - Workers: one thread per branch (thread::scope)                 │
│  - Output: WorktreeInfoUpdated { branch, patch, source } events   │
│            + WorktreeInfoCollectionDone for source=Collector      │
└────────────────────────────┬──────────────────────────────────────┘
                             │ mpsc::Sender<DagEvent>
        ┌────────────────────┼────────────────────────┐
        │                    │                        │
        ▼                    ▼                        ▼
   daft list             daft prune                daft sync
   (LiveTable)           (OperationTable           (OperationTable
                          → LiveTable)              → LiveTable)
```

Properties:

- The collector knows nothing about phases. Same code serves the initial run,
  post-fetch refresh, and post-task refresh — only the input `FieldSet`, branch
  list, and `PatchSource` tag differ.
- `LiveTable` owns row rendering, sort, partition, patch application, columns,
  and loading glyphs. `OperationTable` owns phase banners and hook sub-rows.
- The orchestrator in prune/sync drives subset re-runs by calling
  `collector::spawn` again with a narrower `FieldSet` and `source=PostFetch` or
  `source=PostTask(phase)`.
- `daft list` skips the orchestrator entirely: spawn the collector once, consume
  events into `LiveTable`, exit on `WorktreeInfoCollectionDone` or Ctrl-C.

## Core Types

### `FieldSet` — the shared vocabulary

The single bridge between collector subsets, sort keys, and patches.

```rust
// src/core/worktree/info_field.rs (new)

bitflags! {
    pub struct FieldSet: u32 {
        const BASE_AHEAD_BEHIND   = 1 << 0;
        const REMOTE_AHEAD_BEHIND = 1 << 1;
        const CHANGES             = 1 << 2;
        const LAST_COMMIT         = 1 << 3;
        const BRANCH_AGE          = 1 << 4;
        const OWNER               = 1 << 5;
        const BASE_LINES          = 1 << 6;
        const CHANGES_LINES       = 1 << 7;
        const REMOTE_LINES        = 1 << 8;
        const SIZE                = 1 << 9;
        const MTIME               = 1 << 10;

        // Convenience subsets for orchestrator re-runs:
        const REMOTE_DERIVED = Self::REMOTE_AHEAD_BEHIND.bits | Self::REMOTE_LINES.bits;
        const VOLATILE       = Self::BASE_AHEAD_BEHIND.bits | Self::REMOTE_AHEAD_BEHIND.bits
                             | Self::CHANGES.bits           | Self::LAST_COMMIT.bits
                             | Self::BASE_LINES.bits        | Self::CHANGES_LINES.bits
                             | Self::REMOTE_LINES.bits;
        const ALL = !0;
    }
}
```

`FieldSet` is used in three places:

- The collector takes one as input to scope its work.
- `WorktreeInfo::apply_patch` returns one to indicate which fields changed.
- `SortSpec::required_fields()` returns one to indicate sort dependencies.

Re-sort triggers when `touched.intersects(required)`.

### `WorktreeInfoPatch` — typed cell clusters

One variant per underlying git/FS call cluster. Granularity matches
`collect_worktree_info`'s existing call boundaries (see
`src/core/worktree/list.rs:746–928`).

```rust
// src/core/worktree/sync_dag.rs (DagEvent module)

#[derive(Debug, Clone)]
pub enum WorktreeInfoPatch {
    BaseAheadBehind(Option<(usize, usize)>),
    RemoteAheadBehind(Option<(usize, usize)>),
    Changes { staged: usize, unstaged: usize, untracked: usize },
    LastCommit { timestamp: Option<i64>, hash: Option<String>, subject: String },
    BranchAge(Option<i64>),
    Owner(Option<BranchOwner>),
    BaseLines(Option<(usize, usize)>),
    ChangesLines { staged: (usize, usize), unstaged: (usize, usize) },
    RemoteLines(Option<(usize, usize)>),
    Size(Option<u64>),
    Mtime(Option<i64>),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PatchSource {
    Collector,                  // initial streaming run
    PostFetch,                  // orchestrator re-run after Fetch
    PostTask(OperationPhase),   // orchestrator re-run after a per-branch task
}
```

`PatchSource` is load-bearing for staleness suppression: a `Collector` patch
that lands after a `PostFetch` patch covering the same field on the same branch
is dropped. Priority order in `LiveTable`: `PostTask > PostFetch > Collector`.

### `DagEvent` extensions

```rust
pub enum DagEvent {
    // ... existing variants ...
    WorktreeInfoUpdated {
        branch_name: String,
        patch: WorktreeInfoPatch,
        source: PatchSource,
    },
    WorktreeInfoCollectionDone,  // emitted only by source=Collector run
}
```

`TaskCompleted::updated_info: Option<Box<WorktreeInfo>>` is **removed**. The
orchestrator emits `WorktreeInfoUpdated { source: PostTask(phase) }` patches for
whichever fields the task touched. Net effect on rendered output is identical;
the wire format becomes typed and bandwidth-efficient.

### `WorktreeInfo::apply_patch`

```rust
impl WorktreeInfo {
    /// Returns the FieldSet of fields whose value changed.
    pub fn apply_patch(&mut self, patch: &WorktreeInfoPatch) -> FieldSet { /* one arm per variant */ }
}
```

### `LiveTable` widget

Extracted from `OperationTable`. Generic over the consumer (no DAG awareness).

```rust
// src/output/tui/live_table.rs (new)

pub struct LiveTableConfig {
    pub stat: Stat,
    pub columns: Option<Vec<Column>>,
    pub columns_explicit: bool,
    pub sort_spec: Option<SortSpec>,
    pub pin_default_branch: bool,   // list=false, prune/sync=true
    pub partition_by_owner: bool,   // list=false, prune/sync=true
    pub project_root: PathBuf,
    pub cwd: PathBuf,
}

pub struct LiveTable {
    rows: Vec<WorktreeRow>,
    cfg: LiveTableConfig,
    pending_resort: bool,
    collection_complete: bool,
    // received_patches per row, partition state, source-priority log
}

impl LiveTable {
    pub fn new(seed: Vec<WorktreeInfo>, cfg: LiveTableConfig) -> Self;
    pub fn apply_event(&mut self, event: &DagEvent);   // handles WorktreeInfoUpdated, WorktreeInfoCollectionDone
    pub fn tick(&mut self);                            // drains pending_resort, re-partitions
    pub fn render(&self, frame: &mut Frame, area: Rect);
    pub fn into_completed_view(self) -> CompletedTableView;
}
```

`OperationTable::new` keeps its existing signature; internally it constructs a
`LiveTable` and forwards row events while handling phase + hook events itself.

## Streaming Collector Contract

```rust
// src/core/worktree/list_stream.rs (new — sibling of list.rs)

pub struct CollectorRequest {
    pub targets: Vec<CollectorTarget>,
    pub fields: FieldSet,
    pub stat: Stat,
    pub source: PatchSource,
    pub ctx: Arc<CollectorContext>,
}

pub struct CollectorTarget {
    pub branch_name: String,            // "" for detached (sandbox)
    pub path: Option<PathBuf>,
    pub kind: EntryKind,
    pub is_detached: bool,
}

pub struct CollectorContext {
    pub git: GitCommand,
    pub base_branch: String,
    pub remote_name: String,
    pub ownership_strategy: OwnershipStrategy,
    pub user_email: Option<String>,
}

/// Spawns workers and streams patches into `tx`. Returns a handle the caller
/// can drop to request cooperative cancellation.
///
/// Emits `WorktreeInfoCollectionDone` once on completion **only when**
/// `source == PatchSource::Collector`. Subset re-runs do not emit it.
pub fn spawn(req: CollectorRequest, tx: mpsc::Sender<DagEvent>) -> CollectorHandle;

pub struct CollectorHandle { /* cancel flag + join handles */ }
impl CollectorHandle {
    pub fn cancel(&self);
    pub fn join(self);
}
```

### Per-worker loop

One thread per `CollectorTarget`. Cluster calls execute in a fixed cheap-first
order. Cancellation is checked between every emit:

```rust
// Order: BASE_AHEAD_BEHIND → CHANGES → LAST_COMMIT → BRANCH_AGE → OWNER
//        → REMOTE_AHEAD_BEHIND
//        → BASE_LINES → CHANGES_LINES → REMOTE_LINES   (Stat::Lines only)
//        → SIZE → MTIME                                (slowest last)
```

Rationale: the user sees identity (already from porcelain), then ahead/behind,
then change counts, then commit info — all within milliseconds. Sort keys land
predictably, so re-sorts cluster early instead of dribbling in. Slow cells
(`SIZE`, `MTIME`, `*_LINES`) are last, so they're the only cells still showing
the loading glyph in the final seconds — exactly the cells where the user's
intuition expects a small wait.

### Cancellation

`CollectorHandle::cancel` flips an `AtomicBool` checked between cluster calls.
No mid-call interruption. Workers exit cleanly the next time they're between
emits.

### Re-runs

The orchestrator calls `spawn` again with a narrower `fields` and a different
`source` tag. The same channel carries patches from concurrent runs; `LiveTable`
distinguishes them by `PatchSource` and applies the priority ordering rule
above.

## Per-Command Event Flow

### `daft list` (TTY + non-structured only)

```
parse porcelain (sync)              → seed LiveTable rows (identity columns)
parse --branches/--remotes (sync)   → extend rows, lock viewport size
collector::spawn(ALL, Collector)    → stream patches into LiveTable
on WorktreeInfoCollectionDone       → drop loading glyphs, final resort, exit ratatui
on Ctrl-C                           → handle.cancel(); render whatever landed; exit 0
```

Non-TTY / `--format` / `DAFT_NO_LIVE=1` path: skip everything above, fall
through to today's blocking `collect_worktree_info` + one-shot render
(`print_table` / `build_emit_table` unchanged).

### `daft prune`

```
parse porcelain (sync)              → seed OperationTable's inner LiveTable
                                      (identity + is_default_branch only)
collector::spawn(ALL, Collector)    → stream patches concurrently with orchestrator
spawn orchestrator thread:
  Fetch phase                       → on completion: collector::spawn(REMOTE_DERIVED,
                                                                      PostFetch)
  identify gone branches
  for each gone branch:
    Prune task                      → row removed; no patch
on AllDone                          → exit ratatui, print CompletedTableView
```

### `daft sync`

Same shape as prune, with additional post-task subset re-runs after each
per-branch task:

| Task     | Fields re-emitted as `PostTask`                           |
| -------- | --------------------------------------------------------- |
| `Update` | `BASE_AHEAD_BEHIND`, `LAST_COMMIT`, `CHANGES`             |
| `Rebase` | `BASE_AHEAD_BEHIND`, `LAST_COMMIT`, `REMOTE_AHEAD_BEHIND` |
| `Push`   | `REMOTE_AHEAD_BEHIND`                                     |
| `Prune`  | (row removed; no patch)                                   |

## Phasing

### Phase 1 — Shared infrastructure (one PR)

No user-visible behavior change. Lands:

- `FieldSet`, `WorktreeInfoPatch`, `PatchSource`
- `DagEvent::WorktreeInfoUpdated`, `DagEvent::WorktreeInfoCollectionDone`
- `WorktreeInfo::apply_patch`, `SortSpec::required_fields`
- `src/core/worktree/list_stream.rs` (the streaming collector)
- `LiveTable` extracted from `OperationTable`; `OperationTable` refactored to
  wrap it
- Removal of `TaskCompleted::updated_info`; orchestrator emits `PostTask`
  patches with the same end-state information

Verifiable by existing prune/sync integration tests passing unchanged.

### Phase 2 — `daft list` live UX (one PR)

- `daft list` TTY path: cheap porcelain seed → `LiveTable::new` →
  `collector::spawn(ALL, Collector)` → render loop
- Cancellation, loading glyphs, verbose footer
- New PTY-driven YAML scenarios under `tests/manual/scenarios/list/live/`
- Non-TTY / `--format` / `DAFT_NO_LIVE=1` paths unchanged

### Phase 3 — Migrate prune/sync to streaming seed (one PR)

- Replace blocking `collect_worktree_info` pre-seed with cheap porcelain seed
  - `collector::spawn(ALL, Collector)` running concurrently with the
    orchestrator
- Add `collector::spawn(REMOTE_DERIVED, PostFetch)` after Fetch phase
- Add per-task `collector::spawn(subset, PostTask(phase), targets=[branch])`
  calls
- Remove the `needs_spinner` branches and the spinner output in both commands
- Integration tests run with `DAFT_NO_LIVE=1` for golden-output stability

## Testing Strategy

- **Unit**: `apply_patch` truth table per variant; `SortSpec::required_fields`
  per sort key; `LiveTable::apply_event` with constructed patch sequences (sort
  re-eval, partition shuffle, stale-source suppression).
- **Channel-based**: extend the existing
  `let events: Vec<DagEvent> = rx.iter().collect()` pattern from
  `src/core/worktree/sync_dag.rs:776` to the collector. New test: spawn
  collector against a fixture repo, assert the multiset of patches produced for
  a given `FieldSet`.
- **Integration (Phases 1 & 3)**: existing prune/sync YAML scenarios pass
  unchanged in Phase 1; in Phase 3 they run with `DAFT_NO_LIVE=1`.
- **Integration (Phase 2)**: new PTY-driven scenarios under
  `tests/manual/scenarios/list/live/` assert "rows visible before any cell-fill
  events" and "all cells filled before completion sentinel".

## Invariants and Constraints

These are pinned design properties to refer back to during implementation and
review:

1. **Inline viewport height is fixed at creation** in ratatui inline mode. For
   prune/sync this means the row count must be known before the TUI launches —
   already true today since porcelain + branch enum are synchronous. Phase 3
   keeps that property: only slow _cells_ stream.
2. **Opt-out env var**: `DAFT_NO_LIVE=1`. Forces the one-shot path. Tests depend
   on it; do not rename without updating fixtures.
3. **`PatchSource` priority ordering**: `PostTask > PostFetch > Collector`. A
   lower-priority patch arriving for a field already written by a
   higher-priority patch is dropped on the floor in `LiveTable`.
4. **Post-task patch emission lives in the orchestrator**, not the collector.
   The collector is phase-agnostic; the orchestrator owns the
   "what-fields-did-this-task-touch" mapping.
5. **`WorktreeInfoCollectionDone` is emitted only by the initial
   `source=Collector` run.** Subset re-runs (`PostFetch`, `PostTask`) finish
   silently — orchestrator-driven completion is signalled via the existing
   `AllDone` event.

## Related

- `2026-04-21-worktree-exec-design.md` — established the orchestrator-emits-
  events-into-mpsc-channel pattern adopted here.
- `2026-04-22-worktree-exec-ui-revision-design.md` — established the inline
  ratatui viewport pattern that prune/sync already use and that `LiveTable`
  factors out.
- `2026-03-16-list-column-selection-design.md` — defined `ColumnSelection` /
  `ListColumn` / `Stat` consumed unchanged by this design.
