# Clone Refactor: Always Bare First

## Problem

The clone flow decides bare vs regular before cloning, but daft.yml (which may
specify a layout) can only be read after cloning. This creates a conflict: the
first-time layout prompt fires pre-clone, before we know if daft.yml already
specifies a layout. When it does, the prompt is wrong to appear — daft.yml
should take priority silently.

Additionally, post-clone reconciliation (transforming after the fact) is
wasteful and fragile: it clones with one layout, then immediately converts to
another. With any default layout (not just sibling), this means potentially
converting between bare and non-bare repos unnecessarily.

## Solution

Always clone bare first into `<repo>/.git`, read daft.yml from the bare repo,
resolve the final layout with full context, then set up the repo in the correct
layout from the start.

## New Clone Flow

```
1. Detect branches (git ls-remote)
2. git clone --bare <url> <repo>/.git
3. Read daft.yml from bare repo via git show HEAD:daft.yml
4. Resolve layout (CLI flag > daft.yml > global config > first-time prompt > built-in default)
5. If layout needs bare → keep bare, create worktrees via git worktree add
6. If layout needs non-bare → unbare (core.bare=false + checkout), done
```

The first-time prompt (step 4) only fires when no `--layout` flag, no daft.yml
layout, and no global config default. By this point we've cloned and read
daft.yml, so we know whether to prompt.

## Implementation

### Phase 1: Bare clone (`clone_bare_phase`)

Extract from `execute_bare` lines 82-161: detect branches, `git clone --bare`
into `<repo>/.git`, canonicalize git_dir, set up fetch refspec, multi-remote
config. `cd` into `parent_dir` before returning.

Returns a `BareCloneResult`:

```rust
pub struct BareCloneResult {
    pub repo_name: String,
    pub parent_dir: PathBuf,
    pub git_dir: PathBuf,       // canonicalized
    pub default_branch: String,
    pub target_branch: String,
    pub branch_exists: bool,
    pub is_empty: bool,
    pub remote_name: String,
}
```

Input is a new `BareCloneParams` (subset of today's `CloneParams` — everything
except `layout`):

```rust
pub struct BareCloneParams {
    pub repository_url: String,
    pub branch: Option<String>,
    pub no_checkout: bool,
    pub all_branches: bool,
    pub remote: Option<String>,
    pub remote_name: String,
    pub multi_remote_enabled: bool,
    pub multi_remote_default: String,
    pub checkout_upstream: bool,
    pub use_gitoxide: bool,
}
```

This carries all fields from today's `CloneParams` except `layout`. The
`no_checkout`, `all_branches`, and `checkout_upstream` fields are passed through
to `setup_bare_worktrees()` or `unbare_and_checkout()`.

This phase is shared by all layouts — every clone starts here.

### Phase 2: Read daft.yml

After the bare clone, use `try_load_config_from_ref()` from
`yaml_config_loader.rs` to read daft.yml via `git show HEAD:<candidate>`. This
function is currently private (`fn`). Add a public wrapper:

```rust
/// Load daft.yml from a bare repository's HEAD.
///
/// Used by the clone command to read the team's layout preference before
/// deciding the final layout.
pub fn load_config_from_bare(git_dir: &Path) -> Result<Option<YamlConfig>> {
    try_load_config_from_ref(git_dir, "HEAD")
}
```

For empty repos (`is_empty == true`), skip this step — HEAD has no commits.

### Phase 3: Resolve layout

Call `resolve_layout()` with full context — now including `yaml_layout` from
step 2. If the source is `Default` (no CLI, no daft.yml, no global config), show
the first-time prompt. The prompt is identical to today.

### Phase 4a: Keep bare (layout needs bare)

Extract from `execute_bare` lines 160-261 into `setup_bare_worktrees()`. Input
is the `BareCloneResult` plus the resolved layout. Creates worktrees via
`git worktree add`, sets up tracking, stores layout in repos.json. Returns the
final `CloneResult`.

### Phase 4b: Convert to regular (layout needs non-bare)

For a fresh bare clone into `<repo>/.git`, the directory structure is already
correct for a regular repo — git metadata is in `.git/` with `core.bare=true`.
Converting is trivial:

1. `git -C <repo> config core.bare false`
2. `git -C <repo> checkout` (populates working tree from HEAD)

This does NOT use `collapse_bare_to_non_bare()` from the transform module. That
function is designed for repos with existing worktrees (layout transform). For a
fresh clone with no worktrees, the two-step unbare+checkout is simpler and more
correct.

Implement as a new `unbare_and_checkout()` function in `clone.rs` (core). Takes
`BareCloneResult`, the resolved `Layout`, and `BareCloneParams` as input (needs
`git_dir` from the result, and `no_checkout` to decide whether to run
`git checkout`). Stores layout in repos.json. Returns `CloneResult` with
`cd_target: Some(parent_dir)`, `worktree_dir: Some(parent_dir)`,
`no_checkout: params.no_checkout`.

### Progress reporting

Today `report_plan()` fires before cloning, printing "Initial worktree will be
in: ./repo/branch". After the refactor, we don't know the final layout until
after the bare clone. Report in two stages:

1. Before clone: print repo name and "Cloning repository..." (no worktree path)
2. After layout resolution: print the worktree placement plan

### Changes to `src/core/worktree/clone.rs`

- Replace `execute()` with `clone_bare_phase()` (public)
- Extract `setup_bare_worktrees()` from `execute_bare` post-clone logic (public)
- Add `unbare_and_checkout()` for non-bare conversion (public)
- Add `BareCloneParams` and `BareCloneResult` structs
- Remove `execute_bare()` and `execute_regular()` (subsumed)
- Keep `CloneResult` for the final result (used by command layer for hooks, cd)
- Update `detect_branches()` to accept `&BareCloneParams` (was `&CloneParams`)
- Keep `store_layout()` and worktree creation helpers

### Changes to `src/commands/clone.rs`

`run_clone()` restructured:

```
 1. Build BareCloneParams from Args
 2. clone_bare_phase() → BareCloneResult (repo cloned, cwd is parent_dir)
 3. If --layout given → resolve layout now, skip daft.yml check
 4. Else → read daft.yml from git_dir via load_config_from_bare()
 5. Resolve layout with full context (CLI, yaml, global config)
 6. If source is Default → show first-time prompt, re-resolve if user chose
 7. Store layout in repos.json
 8. If bare layout → setup_bare_worktrees()
 9. If non-bare layout → unbare_and_checkout()
10. Render result, run hooks, cd
```

Remove `reconcile_layout()` — daft.yml is now read before the layout decision.

Remove `CloneParams` — replaced by `BareCloneParams` (layout is not part of
clone params anymore; it's decided between phases).

### Prompt cancellation cleanup

Today the prompt fires pre-clone, so cancellation is a no-op (`return Ok(())`).
After this change, the bare clone has already happened. The `Cancelled` arm
must:

1. Delete the cloned repo directory (`parent_dir`)
2. `cd` back to the original working directory
3. Return `Ok(())`

Track the original cwd at the start of `run_clone` (before `clone_bare_phase`
changes directory).

### Changes to `src/hooks/yaml_config_loader.rs`

Add public `load_config_from_bare()` wrapper around the existing private
`try_load_config_from_ref()`.

### Changes to `src/git/clone.rs`

Remove `clone_regular()` and `clone_regular_branch()` — no longer called.

## Edge Cases

### Empty repositories

No commits, so `git show HEAD:daft.yml` fails. Skip daft.yml read when
`is_empty` is true. Proceed with normal resolution chain (global config, prompt,
or built-in default). Bare layout creates an orphan worktree; non-bare layout
just unbares (checkout is a no-op on empty repo).

### `--no-checkout` mode

Resolve and store the layout in repos.json, but skip worktree creation entirely.
If the resolved layout is non-bare, still convert bare→non-bare (set
`core.bare=false`, skip `git checkout`) so the repo is in the correct state for
future `daft start` commands. The user asked for no checkout, not a bare repo.
`CloneResult` has `cd_target: None`, `worktree_dir: None`, `no_checkout: true`
(matching current behavior for `--no-checkout`).

### Branch doesn't exist on remote

Same as today: clone succeeds, no worktree created, warning printed. Layout is
still resolved and stored.

### `--all-branches`

Only applicable to bare layouts. For non-bare, print warning (same as today).

### Prompt cancellation (Ctrl+C)

Delete the cloned directory, `cd` back to original dir, exit. See "Prompt
cancellation cleanup" section above.

## Files to Modify

- `src/core/worktree/clone.rs` — split into phases, add BareCloneParams/Result,
  add unbare_and_checkout, remove execute/execute_bare/execute_regular
- `src/commands/clone.rs` — restructure run_clone, remove reconcile_layout, add
  post-clone daft.yml reading and prompt logic
- `src/hooks/yaml_config_loader.rs` — add public load_config_from_bare wrapper
- `src/git/clone.rs` — remove clone_regular and clone_regular_branch

## Test Scenarios to Update

- `tests/manual/scenarios/clone/layout-from-daft-yml.yml` — remove
  `output_contains: "Transforming"`. The repo is now cloned directly with the
  correct layout (no post-clone transform). Update assertions to verify
  contained layout without transform messaging.
- `tests/manual/scenarios/clone/layout-prompt-skipped-for-daft-yml.yml` — same:
  remove `"Transforming"` assertions. The prompt is now suppressed (not fired
  then overridden). Verify that no prompt output appears and contained layout is
  used directly.

## Files Unchanged

- `src/hints.rs` — prompt function identical, just called later
- `src/core/layout/resolver.rs` — resolution chain unchanged
- `src/core/layout/transform.rs` — not used for fresh clones
