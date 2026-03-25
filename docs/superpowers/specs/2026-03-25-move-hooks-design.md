# Move Hooks: Identity-Tracked Hook Replay on Worktree Move

## Problem

When worktrees are moved — via rename, layout transform, or adopt — hooks that
set up path-dependent or branch-dependent state are not re-run. This leaves
stale state at the old identity (e.g., orphaned direnv allowlists, docker
environments named after the old branch) and missing state at the new identity.

Running full `worktree-pre-remove` + `worktree-post-create` is too heavy and
destructive — it tears down state that's perfectly fine (like installed
dependencies) and rebuilds from scratch unnecessarily.

## Solution: The `tracks` Field

A new optional `tracks` field on hook job definitions declares which worktree
attributes a job is sensitive to. When a move operation changes those
attributes, daft selectively re-runs only the relevant jobs — teardown with the
old identity, setup with the new identity.

### Trackable Attributes

- **`path`** — the worktree's filesystem path
- **`branch`** — the worktree's branch name

### Detection: Explicit and Implicit

**Explicit:** Jobs declare `tracks` directly:

```yaml
worktree-post-create:
  jobs:
    - name: docker-up
      run: ./scripts/docker-up.sh
      tracks: [branch]
```

**Implicit:** Daft scans `run` strings (all platform variants) for template
variables and infers tracking:

- `{worktree_path}` implies `tracks: [path]`
- `{branch}`, `{worktree_branch}` implies `tracks: [branch]`

The effective tracking set is the union of explicit and implicit.

### Implicit Detection for Platform and List Variants

The `run` field can be a simple string, a platform-specific map, or a list of
strings (joined with `&&` at execution). Implicit scanning applies to every
command string across all variants — each element of a list variant and each OS
entry of a platform variant is scanned individually.

### Validation

- `tracks` accepts a list of: `path`, `branch`
- Empty list or omitted means "not tracked" — job only runs during normal
  create/remove, never during move
- Invalid values produce a config validation error (not silently dropped)

### Data Model

```rust
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum TrackedAttribute {
    Path,
    Branch,
}
```

`JobDef` gains: `pub tracks: Option<Vec<TrackedAttribute>>`

### Group Jobs

`tracks` can be set on jobs inside a `GroupDef`. A group job's `tracks` does NOT
propagate to its children — each child declares or implies its own tracking
independently. Implicit detection scans each child's `run` string individually.

## Move Hook Flow

When a move operation occurs, daft determines what changed and runs a filtered
hook sequence:

### Change Detection Per Operation

- **Rename** — branch always changes; path changes if the directory name derives
  from the branch
- **Layout transform** — path always changes; branch unchanged
- **Adopt** — path changes; branch unchanged

### Execution Sequence

1. Determine changed attributes (`path`, `branch`, or both)
2. Across all four hook entry points, collect jobs whose effective `tracks` set
   intersects with the changed attributes
3. Resolve dependency graphs for the collected jobs (pulling in `needs`
   dependencies)
4. Execute:
   - `worktree-pre-remove` tracked jobs — old identity
   - `worktree-post-remove` tracked jobs — old identity
   - **Perform the move**
   - `worktree-pre-create` tracked jobs — new identity
   - `worktree-post-create` tracked jobs — new identity

**Hook config source during moves:** During all four phases, the YAML config
(`daft.yml`) is loaded from the worktree that exists at that moment — for
teardown phases the worktree is still at the old path, for setup phases it has
been moved to the new path. Since `daft.yml` is tracked in git and identical
across worktrees, the source path does not affect content. The
`get_hook_source_worktree` logic should use the current (old) worktree path for
pre-remove and post-remove, and the new worktree path for pre-create and
post-create. During a move, the worktree is never deleted — it is moved — so
`post-remove` can still read config from the old path (unlike a real remove
where the directory is gone).

### Dependency Resolution

- **Selected:** A job is selected if its effective `tracks` intersects with
  changed attributes
- **Pull-in:** If a selected job declares `needs: [other-job]`, that dependency
  is pulled into the execution set even if not itself tracked. This ensures
  correctness. Dependencies are resolved within the same hook entry point only —
  `needs` in `worktree-post-create` references jobs in `worktree-post-create`,
  not across hook types.
- **Skip conditions do the heavy lifting:** Pulled-in non-tracked jobs typically
  no-op via their existing skip/cache checks (e.g., `mise install` with
  everything already installed finishes instantly)
- **No reverse pull-in:** If `bun-install` needs `mise-trust` and only
  `mise-trust` is tracked, `bun-install` is NOT pulled in. Tracking is the
  selection trigger.
- **Execution mode:** The filtered job set runs through the same executor as
  normal hooks — respecting `parallel`/`piped`/`follow` modes, `skip`/`only`
  conditions, platform variants, and `priority` ordering

## HookContext Changes for Move Hooks

Move hooks require carrying additional state through `HookContext`. New fields:

```rust
pub struct HookContext {
    // ... existing fields ...
    pub is_move: bool,
    pub old_worktree_path: Option<PathBuf>,
    pub old_branch_name: Option<String>,
}
```

**How the four move phases construct `HookContext`:**

| Phase       | `hook_type`  | `worktree_path` | `branch_name` | `old_worktree_path` | `old_branch_name`  | `is_move` |
| ----------- | ------------ | --------------- | ------------- | ------------------- | ------------------ | --------- |
| Pre-remove  | `PreRemove`  | old path        | old branch    | `Some(old path)`    | `Some(old branch)` | `true`    |
| Post-remove | `PostRemove` | old path        | old branch    | `Some(old path)`    | `Some(old branch)` | `true`    |
| Pre-create  | `PreCreate`  | new path        | new branch    | `Some(old path)`    | `Some(old branch)` | `true`    |
| Post-create | `PostCreate` | new path        | new branch    | `Some(old path)`    | `Some(old branch)` | `true`    |

`old_worktree_path` and `old_branch_name` always hold the pre-move values in all
four phases. The standard `worktree_path` and `branch_name` fields carry the
current-phase identity (old during teardown, new during setup).

The caller constructs two separate `HookContext` values — one for teardown (old
path in `worktree_path`) and one for setup (new path in `worktree_path`). Both
carry the old path in `old_worktree_path` for reference. When `is_move` is
false, the old fields are `None` and behavior is unchanged.

**`get_hook_source_worktree` during moves:** When `ctx.is_move` is true, the
hook config source is always `ctx.worktree_path` — during teardown this is the
old path (which still exists), during setup this is the new path (which now
exists). This avoids the normal `PostRemove` fallback to `source_worktree` which
exists for real removes where the target directory is gone.

**`working_directory` during moves:** Returns `ctx.worktree_path` for all move
phases — the old path during teardown (still exists), the new path during setup
(now exists).

## Failure Semantics

Hook failures during all four move phases emit warnings but do **not** abort the
move operation. This applies to both teardown and setup phases — consistent with
how hook failures are handled elsewhere in daft (hooks are best-effort, not
transactional). The `FailMode` on the hook config is respected: `Warn` emits a
warning, `Abort` emits a warning but still does not abort the move itself (move
hooks downgrade `Abort` to `Warn` since the filesystem operation must not be
blocked by hook failures).

## Environment and Templates

### Standard Variables

All standard hook env vars (`DAFT_WORKTREE_PATH`, `DAFT_BRANCH_NAME`, etc.) are
set as usual:

- During teardown phases (pre-remove, post-remove): values reflect the **old**
  identity
- During setup phases (pre-create, post-create): values reflect the **new**
  identity

### New Variables for Move Hooks

**Environment variables:**

- `DAFT_OLD_WORKTREE_PATH` — path before the move
- `DAFT_OLD_BRANCH_NAME` — branch name before the move (same as new if only path
  changed)
- `DAFT_IS_MOVE` — `true` during move hooks, unset otherwise

**Template variables:**

- `{old_worktree_path}` — path before the move (empty outside move hooks)
- `{old_branch}` — branch name before the move (empty outside move hooks)

`DAFT_COMMAND` is set to the originating command (e.g., `rename`,
`layout-transform`). `DAFT_HOOK` retains its standard value during move hook
phases — `worktree-pre-remove`, `worktree-post-remove`, `worktree-pre-create`,
`worktree-post-create`. Scripts distinguish move vs. normal lifecycle via
`DAFT_IS_MOVE`.

`DAFT_IS_MOVE` lets jobs behave differently during a move vs. a fresh create —
e.g., a docker script could rename a container instead of doing a full
teardown + recreate.

## Integration Points

### Rename (`src/core/worktree/rename.rs`)

Currently does branch rename, filesystem move, and remote update with no hooks.
The move hook flow is injected around the filesystem move:

Teardown hooks (old identity) → rename branch → move directory → setup hooks
(new identity) → remote update

### Layout Transform (`src/core/layout/transform/execute.rs`)

For transforms that move worktrees (e.g., contained to sibling), move hooks are
injected as additional steps in the transform plan. For each
`TransformOp::MoveWorktree`, the executor wraps it with hook phases:

For each moved worktree: teardown tracked jobs (old path) → move directory →
setup tracked jobs (new path)

Move hooks are modeled as steps around each `MoveWorktree` op in the plan, not
as separate `TransformOp` variants. The existing rollback stack handles the move
itself; hook failures during setup emit warnings but do not trigger rollback of
the filesystem move (consistent with how hook failures are handled elsewhere in
daft — hooks are best-effort, not transactional).

### Adopt (`src/core/layout/transform/`)

Adopt routes through `transform::convert_to_bare()` in `legacy.rs`. This code
path uses its own internal file-move logic (`move_files_to_worktree`) rather
than the plan-based `TransformOp::MoveWorktree` executor. Move hooks must be
injected directly into `convert_to_bare`'s worktree relocation steps — it cannot
inherit the hook integration from the plan executor automatically.

Note: `src/core/worktree/flow_adopt.rs` is a thin wrapper. If adopt is migrated
to the plan-based executor in the future, it would inherit move hooks for free.

### Eject (`src/core/worktree/flow_eject.rs`)

Worktrees are destroyed, not moved. The existing full pre-remove/post-remove
flow is correct. No change needed.

### Post-Clone

Clone is a fresh setup with no prior identity. `tracks` has no effect.
`post-clone` runs unchanged.

## Example Configuration

```yaml
hooks:
  worktree-post-create:
    jobs:
      - name: mise-trust
        run: mise trust
        tracks: [path]

      - name: direnv-allow
        run: direnv allow {worktree_path}
        # Implicit tracks: [path] via {worktree_path}

      - name: docker-up
        run: ./scripts/docker-up.sh
        tracks: [branch]

      - name: bun-install
        run: bun install
        # No tracks — content-setup, not identity-sensitive

  worktree-pre-remove:
    jobs:
      - name: docker-down
        run: ./scripts/docker-down.sh
        tracks: [branch]

      - name: sandbox-clean
        run: mise run sandbox:clean
        # No tracks — runs on full remove only, not on move
```

**Scenario: rename `feat/auth` to `feat/auth-v2`** (both path and branch
change):

1. **Pre-remove (old identity):** `docker-down` runs (tracks branch), tears down
   `feat/auth` docker env
2. **Post-remove (old identity):** (no tracked jobs in this example)
3. **Move:** branch renamed, directory moved
4. **Pre-create (new identity):** (no tracked jobs in this example)
5. **Post-create (new identity):** `mise-trust` runs (tracks path),
   `direnv-allow` runs (tracks path via template), `docker-up` runs (tracks
   branch) — sets up for `feat/auth-v2`
6. `bun-install` and `sandbox-clean` are untouched

## Future Direction

The `tracks` field is a lightweight precursor to a resource-based hook model
where environments declare the resources they need and their dependencies,
rather than imperative jobs at specific hook entry points. In that future model,
`tracks` evolves naturally into resource identity bindings.
