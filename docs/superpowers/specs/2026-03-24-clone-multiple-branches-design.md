# Clone Multiple Branches

## Overview

Make the `-b` option in `git-worktree-clone` repeatable so users can check out
multiple branches during initial clone. Unify the internal handling of `-b` and
`--all-branches` under a single `BranchSource` abstraction, and introduce a
shared TUI table component that clone, sync, and prune all consume.

## Motivation

Today, cloning a repository and working on multiple branches requires cloning
first, then running `git worktree add` for each additional branch. A repeatable
`-b` flag lets users express the full set of branches they need up front:

```sh
git worktree clone https://github.com/user/repo -b feat-a -b feat-b
git worktree clone https://github.com/user/repo -b @ -b feat-a -b feat-b
```

## Default Branch Tokens

Both `HEAD` and `@` are accepted as values to `-b`, meaning "the remote's
default branch" without the user needing to know whether it's `main`, `master`,
or something else. This follows git conventions:

- `HEAD` is a symbolic ref pointing to the default branch on the remote. This is
  what `git ls-remote --symref <url> HEAD` queries.
- `@` is git's shorthand for `HEAD` (since v1.8.5).

These tokens are resolved early (Phase 2) to the actual branch name. After
resolution, downstream code is unaware of the aliases.

Deduplication: if `HEAD`/`@` resolves to `main` and `-b main` is also in the
list, it collapses to one entry. `-b @ -b HEAD` also collapses. Order is
preserved from first occurrence.

If a repository has a branch literally named `HEAD` or `@` (extremely unlikely),
these tokens are still interpreted as default branch aliases. There is no escape
hatch, matching git's own treatment of `HEAD` as reserved.

## BranchSource Abstraction

A unified enum replaces the separate `-b` and `--all-branches` internal
handling:

```rust
enum BranchSource {
    /// No -b, no --all-branches: just the default branch
    Default,
    /// Single -b <branch>: one explicit branch (today's behavior, unchanged)
    Single(String),
    /// Multiple -b flags: explicit list of branches
    Multiple(Vec<String>),
    /// --all-branches: discover all remote branches
    All,
}
```

`BranchSource` resolves to a `BranchPlan`:

```rust
struct BranchPlan {
    /// Branch for the base worktree (non-bare layouts only, None for bare)
    base: Option<String>,
    /// Branches for satellite worktrees
    satellites: Vec<String>,
    /// Which worktree to cd into (first valid from original -b order)
    cd_target: Option<String>,
    /// Branches that weren't found on remote (for warnings)
    not_found: Vec<String>,
}
```

### Resolution Rules

| BranchSource     | Non-bare layout                                         | Bare/centralized layout             |
| ---------------- | ------------------------------------------------------- | ----------------------------------- |
| `Default`        | base = default branch, satellites = []                  | base = None, satellites = [default] |
| `Single(b)`      | base = b, satellites = []                               | base = None, satellites = [b]       |
| `Multiple(list)` | base = default (injected if missing), satellites = rest | base = None, satellites = list      |
| `All`            | base = default, satellites = all others                 | base = None, satellites = all       |

For `BranchSource::All` with non-bare layouts: if the default branch is not
present in the remote's branch list, fall back to the first alphabetical branch
as the base and warn.

Key rules for `Multiple`:

- **Single `-b`**: that branch goes in the base worktree regardless of which
  branch it is. Identical to today's behavior.
- **Two or more `-b`**: the default branch is always checked out in the base
  worktree for non-bare layouts, even if not explicitly specified. This resolves
  the ambiguity of which branch should occupy the base. The default branch is
  injected implicitly if not in the list.
- **Bare/centralized layouts**: only the explicitly requested branches get
  worktrees. No implicit default branch injection.

### cd Target

The first valid branch from the original `-b` order determines where daft cds
after clone. If the first branch doesn't exist on remote, the next valid one in
the list is used. If no valid branches from the original list exist but a base
worktree was created (default branch injected for non-bare layouts), cd into the
base worktree. For bare layouts with no valid branches, cd into the repo
directory itself. For `BranchSource::All`, the cd target is the base worktree
(default branch, or first alphabetical branch if the default is absent on the
remote).

## Clone Phases

### Phase 1: Bare Clone (unchanged)

`git clone --bare` creates the repo. Default branch detected via
`git ls-remote --symref HEAD`. Gitoxide is unavailable here (no local repo
exists yet). Runs with a spinner before the TUI starts.

### Phase 2: Branch Resolution (new)

The repo now exists, so gitoxide becomes available. This phase:

1. Expands `HEAD`/`@` to the actual default branch name
2. For `All`: enumerates remote branches
3. For `Multiple`: validates each branch exists
4. Deduplicates
5. Applies layout rules (default branch injection for non-bare)
6. Determines cd target
7. Collects `not_found` branches for warnings

### Phase 3: Layout Resolution (unchanged)

Reads daft.yml, resolves layout via the 5-level chain (CLI > repo store >
daft.yml > global config > detection > fallback), prompts if needed.

### Phase 4: Worktree Setup (refactored)

Unified for all `BranchSource` variants:

1. **Base worktree** (sequential): checkout/unbare/wrap depending on layout type
2. **Satellite worktrees** (parallel): `git worktree add` for each, with
   per-worktree hooks (`worktree-pre-create`, `worktree-post-create`)
3. **Post-clone hook** runs after TUI completes (repo-level, fires once)

### TUI Activation

The TUI table only activates for `Multiple` and `All`. `Default` and `Single`
use the existing spinner-based output for full backward compatibility.

## Shared TUI Table Component: OperationTable

Three commands (clone, sync, prune) now use the same TUI table pattern. Extract
a reusable `OperationTable` component.

### API

```rust
/// Reusable TUI table for any command that processes worktrees in parallel.
pub struct OperationTable {
    phases: Vec<PhaseState>,
    initial_rows: Vec<WorktreeRow>,
    receiver: mpsc::Receiver<DagEvent>,
    config: TableConfig,
}

pub struct TableConfig {
    pub columns: Option<Vec<Column>>,
    pub columns_explicit: bool,
    pub sort_spec: Option<SortSpec>,
    pub extra_rows: u16,
    pub verbosity: u8,
}

impl OperationTable {
    /// Run the TUI render loop. Blocks until AllDone received.
    pub fn run(self) -> Result<CompletedTable> { ... }
}

pub struct CompletedTable {
    pub rows: Vec<WorktreeRow>,
    pub hook_summaries: Vec<HookSummaryEntry>,
}
```

`CompletedTable` wraps the final `TuiState` fields. Failures are derived
post-run by filtering `rows` for entries with `FinalStatus::Failed` — no
separate failure accumulation needed.

### What each command provides

Each command is responsible for:

1. **Defining its phases** (sync: Fetch/Prune/Update/Rebase/Push; prune:
   Fetch/Prune; clone: Clone/Setup)
2. **Spawning worker threads** that send `DagEvent`s through the channel
3. **Post-TUI handling** (deferred branches, warnings, summary output)

### What the component owns

- Render loop, viewport calculation, cursor positioning (`driver.rs`)
- TUI state and event application (`state.rs`)
- Table/header/hook sub-row rendering (`render.rs`)
- Column definitions and responsive selection (`columns.rs`)
- Hook event forwarding (`presenter.rs`, `tui_bridge.rs`)

### Refactoring scope

Sync and prune are refactored to consume `OperationTable` instead of inline TUI
wiring. Their behavior does not change.

## Clone TUI Details

### Header phases

```
 Cloning repository (bare)          <- completed before TUI starts
 Setting up worktrees               <- active during TUI
```

### Table columns

| Status  | Branch | Path           | Annotation                 |
| ------- | ------ | -------------- | -------------------------- |
| spinner | feat-a | ../repo.feat-a | worktree-pre-create        |
| check   | main   | ./repo         | Base worktree              |
| spinner | feat-b | ../repo.feat-b | Running post-clone hook... |

Responsive column dropping follows the existing priority system. `--columns` and
`--sort` flags apply.

### Hook sub-rows

Each satellite worktree shows `worktree-pre-create` and `worktree-post-create`
hook sub-rows, identical to how prune/sync show `worktree-pre-remove` and
`worktree-post-remove`:

```
  feat-a    ../repo.feat-a    worktree-pre-create
   +-- worktree-pre-create         spinner running...        <- verbose
   +--                             check 0.3s               <- verbose
  feat-a    ../repo.feat-a    Creating worktree
  feat-a    ../repo.feat-a    worktree-post-create
   +-- worktree-post-create        spinner running...        <- verbose
   |     install-deps              check 2.1s               <- verbose (job)
   +--   build                     spinner running...        <- verbose (job)
check feat-a    ../repo.feat-a    Created
```

Uses the same `HookStarted` / `JobStarted` / `JobCompleted` / `HookCompleted`
event flow through `TuiBridge` + `TuiPresenter`.

### Hook failure handling

- **worktree-pre-create abort**: skip that worktree, report failure in TUI,
  continue with remaining satellites
- **worktree-pre-create warn**: create the worktree anyway, show warning
- Same semantics as prune/sync hook failure handling

### Verbosity

- Default: table with status + branch + path + annotation
- `-v`: hook sub-rows with individual job status visible
- `-vv`: sequential mode, no TUI, full hook output to terminal

### Non-TTY fallback

When stderr is not a TTY (CI, piped output), fall back to sequential
line-by-line output using the trait abstraction (`ProgressSink` + `HookRunner`).

### Partial failure summary

After TUI completes, print a summary if any worktrees failed:

```
Created 3 of 5 worktrees (2 failed)
  x feat-c: worktree-pre-create hook failed (exit 1)
  x typo-branch: branch not found on remote
```

## Event Model Extensions

### New OperationPhase variant

```rust
pub enum OperationPhase {
    Fetch,
    Prune,
    Update,
    Rebase(String),
    Push,
    Setup,  // new: clone worktree setup
}
```

Adding `Setup` requires updating all exhaustive matches on `OperationPhase`:

- `sync_dag.rs` — `OperationPhase::label()` method: add
  `Setup => "Setting up worktrees"`
- `state.rs` — `apply_event()` active label match: add `Setup => "setting up"`
- `sync_dag.rs` — `TaskId` enum (line 24): add `Setup(String)` variant; update
  all match sites in `sync.rs` and `prune.rs` task executor closures

### New TaskMessage variants

```rust
pub enum TaskMessage {
    // ... existing variants ...
    Created,      // worktree successfully set up
    BaseCreated,  // base worktree checked out (non-bare layouts)
    NotFound,     // branch didn't exist on remote (warning)
}
```

Mapping to `FinalStatus` in `state.rs` `apply_event()`:

- `TaskMessage::Created` → `FinalStatus::Updated` (green checkmark, "Created"
  annotation)
- `TaskMessage::BaseCreated` → `FinalStatus::Updated` (green checkmark, "Base
  worktree" annotation)
- `TaskMessage::NotFound` → `FinalStatus::Skipped` (yellow warning, "Not found
  on remote" annotation)

## CLI Surface

### Clap argument change

```rust
#[arg(
    short = 'b',
    long = "branch",
    value_name = "BRANCH",
    action = ArgAction::Append,
    help = "Branch to check out (repeatable; use HEAD or @ for default branch)"
)]
pub branch: Vec<String>,
```

- `-b` and `--all-branches` remain mutually exclusive (`conflicts_with`)
- `--no-checkout` conflicts with `-b` and `--all-branches` (unchanged from
  today's single `-b` + `--no-checkout` conflict)
- Empty Vec -> `BranchSource::Default`
- Single element -> `BranchSource::Single` (backward compatible)
- Two or more -> `BranchSource::Multiple`

### Interaction with --remote (multi-remote mode)

Multi-remote mode (`--remote`) organizes worktrees under a remote folder and is
not supported with multiple `-b` flags in this iteration. The `conflicts_with`
list will include `remote` for the `Multiple` case. `Single` `-b` + `--remote`
continues to work as today. This can be revisited in a future iteration if
needed.

### Backward compatibility note

The clap field changes from `Option<String>` to `Vec<String>`. Internally,
`BareCloneParams.branch` remains `Option<String>` — it receives the resolved
target branch for Phase 1 (either the single `-b` value or the default branch).
The `Vec<String>` is consumed only by `BranchSource` construction, keeping Phase
1 unchanged.

### Shell completions

Add `HEAD` and `@` as static completions for the `-b` value in bash.rs, zsh.rs,
fish.rs. fig.rs auto-generates from clap.

### Man page

Regenerate after help text changes (`mise run man:gen`).

## Gitoxide Integration

After Phase 1 (bare clone creates the repo), gitoxide becomes available for
Phase 2 branch resolution.

### Phase 2 fast paths

| Operation                | CLI (network)                                      | Gitoxide (local)                                          |
| ------------------------ | -------------------------------------------------- | --------------------------------------------------------- |
| Validate branch exists   | `git ls-remote --heads origin refs/heads/<branch>` | `repo.try_find_reference("refs/remotes/origin/<branch>")` |
| List all remote branches | `git ls-remote --heads origin`                     | `repo.references().prefixed("refs/remotes/origin/")`      |

After `git clone --bare`, all remote refs are fetched locally. Gitoxide
validates branches by reading local refs with zero network round-trips.

These are **new functions** in `oxide.rs`, not wrappers around the existing
network-based `ls_remote_*` functions. The existing `oxide::ls_remote_heads` and
`oxide::ls_remote_branch_exists` make live network connections via
`remote.connect(Direction::Fetch)`. The new functions instead read from the
local ref store (`refs/remotes/origin/`), which is populated after the bare
clone:

```rust
/// Check if a branch exists in the already-fetched remote refs (no network).
pub fn validate_branch_in_remotes(
    repo: &Repository,
    remote_name: &str,
    branch: &str,
) -> Result<bool> {
    let ref_name = format!("refs/remotes/{remote_name}/{branch}");
    Ok(repo.try_find_reference(&ref_name)?.is_some())
}
```

### Phase 4 fast paths

- Upstream tracking setup: direct config file writes via the git CLI
  (`git config branch.<name>.remote` / `git config branch.<name>.merge`), or via
  gitoxide's config mutation API when available. The existing codebase uses CLI
  for config writes, so this follows the same pattern.
- Post-setup validation: existing gitoxide functions

### CLI-only operations

- `git clone --bare` (gitoxide clone not mature enough)
- `git worktree add` (no gitoxide equivalent)
- `git checkout` (no gitoxide equivalent)
- `git remote set-head --auto` (simpler via CLI)

### Implementation pattern

Follow the existing `GitCommand` dual-path pattern:

```rust
pub fn validate_branches_exist(
    &self,
    branches: &[String],
) -> Result<Vec<(String, bool)>> {
    if self.use_gitoxide {
        if let Ok(repo) = self.gix_repo() {
            return oxide::validate_branches_exist(&repo, branches);
        }
    }
    // CLI fallback
    ...
}
```

## Edge Cases

### Empty repository

If the remote has no commits:

- `Default` / `Single`: unchanged behavior (creates orphan branch)
- `Multiple`: warn that multi-branch has no effect, fall back to single-branch
  with the first `-b` value
- `All`: same warning, no branches to discover

### All specified branches are invalid

- Bare clone still succeeds
- Non-bare layouts: default branch still checked out in base (implicitly
  injected)
- Bare layouts: no worktrees created, warn about each missing branch
- No cd target -> cd into repo directory
- Exit with warning, not error

### Single -b backward compatibility

`BranchSource::Single` follows the exact same code path as today. No implicit
default branch injection, no TUI table (just spinner), identical output.

### Hooks

- `worktree-pre-create` and `worktree-post-create` fire once per satellite
  worktree
- `post-clone` fires once for the whole clone, after TUI completes
- `--all-branches` is updated to fire hooks per worktree if it doesn't already
