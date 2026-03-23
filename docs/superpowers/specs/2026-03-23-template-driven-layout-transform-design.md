# Template-Driven Layout Transform Engine

## Overview

Redesign the layout transform engine to derive all operations from template
evaluation rather than hardcoded per-layout-pair logic. The engine models a
transform as a diff between two layout states (source and target), computes an
ordered plan of discrete operations with conflict-driven sequencing, then
executes the plan. This handles all current and future layout combinations
uniformly, including `contained-classic` and `contained-flat`.

## Motivation

The current transform engine uses a 2x2 matrix of
`(is_currently_bare, target_needs_bare)` with specialized functions for each
direction (`convert_to_bare`, `convert_to_non_bare`, `relocate_worktrees`). This
breaks when new layout categories are added — `contained-classic` (wrapped
non-bare) doesn't fit either column of the matrix. The template already encodes
everything needed: where worktrees go, where `.git` goes, and whether the repo
is bare. The transform engine should derive operations from this information
instead of discarding it in favor of a boolean.

## Data Model

### LayoutState

A snapshot of where everything is (source) or should be (target):

```rust
struct LayoutState {
    git_dir: PathBuf,
    is_bare: bool,
    default_branch: String,
    worktrees: Vec<WorktreeState>,
}

struct WorktreeState {
    branch: String,
    path: PathBuf,
}
```

**Source state** is read from the actual repo via
`git worktree list --porcelain` and `git config core.bare`.

**Target state** is computed by evaluating the target layout template for each
branch. The target `git_dir` and `is_bare` are derived from the layout's
`needs_bare()` and `needs_wrapper()` methods.

### Target git_dir derivation

The target `git_dir` location depends on the layout category:

| Layout category  | `git_dir` location                | Bare |
| ---------------- | --------------------------------- | ---- |
| Bare             | `<wrapper>/.git`                  | Yes  |
| Wrapped non-bare | `<wrapper>/<default_branch>/.git` | No   |
| Regular non-bare | `<project_root>/.git`             | No   |

For wrapped non-bare (`needs_wrapper()`): evaluate the template with
`branch = default_branch` to get the clone subdirectory, then append `/.git`.

For regular non-bare: `git_dir` is the `project_root/.git` — the repo root IS
the default branch checkout.

For bare: `git_dir` stays at `project_root/.git`.

### TransformPlan

An ordered list of `TransformOp`s. Computed entirely before any mutation.

```rust
struct TransformPlan {
    ops: Vec<TransformOp>,
    non_conforming: Vec<NonConformingWorktree>,
}

struct NonConformingWorktree {
    branch: String,
    current_path: PathBuf,
    template_path: PathBuf,
}
```

### TransformOp

Each operation is a discrete, reversible mutation:

```rust
enum TransformOp {
    StashChanges { branch: String, worktree_path: PathBuf },
    MoveWorktree { branch: String, from: PathBuf, to: PathBuf },
    MoveGitDir { from: PathBuf, to: PathBuf },
    SetBare(bool),
    RegisterWorktree { branch: String, path: PathBuf },
    UnregisterWorktree { branch: String },
    CollapseIntoRoot { worktree_path: PathBuf, root_path: PathBuf },
    NestFromRoot { root_path: PathBuf, subdir_path: PathBuf },
    InitWorktreeIndex { path: PathBuf },
    CreateDirectory { path: PathBuf },
    PopStash { branch: String, worktree_path: PathBuf },
    ValidateIntegrity,
}
```

**Operation semantics:**

- `MoveWorktree` — calls `git worktree move` for linked worktrees
- `MoveGitDir` — `fs::rename` of the `.git` directory + fixup of
  `.git/worktrees/*/gitdir` paths
- `SetBare` — `git config core.bare true/false` + index cleanup if going bare
- `RegisterWorktree` — writes `.git/worktrees/<branch>/gitdir` and the
  worktree's `.git` file
- `UnregisterWorktree` — removes `.git/worktrees/<branch>/` directory
- `CollapseIntoRoot` — moves all files from a subdirectory into its parent
  (default branch checkout becomes repo root)
- `NestFromRoot` — moves all files from a directory into a new subdirectory
  (repo root contents move into default branch subdir)
- `InitWorktreeIndex` — `git reset --mixed HEAD` to rebuild the index
- `StashChanges` / `PopStash` — per-worktree stash/pop to preserve dirty state
- `ValidateIntegrity` — `git fsck` or equivalent post-transform check

## Plan Builder

### Algorithm

1. **Read source state** — `git worktree list --porcelain`,
   `git config core.bare`, detect default branch
2. **Compute target state** — evaluate target template for each branch to get
   target paths; derive target `git_dir` and `is_bare`
3. **Classify worktrees** — compare each worktree's current path against the
   target template output. Conforming worktrees are relocated. Non-conforming
   worktrees (current path matches neither source nor target template) are
   skipped unless the user passes `--include <branch>` or `--include-all`
4. **Compute git_dir migration** — if source `git_dir` != target `git_dir`, add
   `MoveGitDir`
5. **Compute default branch handling** — if the default branch needs to collapse
   into root (`NestFromRoot` → bare, or reverse), add the appropriate op
6. **Build dependency graph** — analyze path overlaps between operations
7. **Topological sort** — determine safe execution order
8. **Break cycles** — if two operations need each other's paths, insert a move
   to a temp staging path
9. **Prepend stash ops** — for each dirty worktree that will be moved
10. **Append finalization** — `SetBare`, `InitWorktreeIndex`, `PopStash` ops,
    then `ValidateIntegrity`

### Conflict-Driven Sequencing

No hardcoded per-layout-pair logic. The sequencer analyzes path relationships:

**Vacate-before-occupy** — if operation A's target path is currently occupied by
something that operation B will move, B must execute before A.

**Collapse/nest dependencies** — if the default branch must collapse into a
directory that contains other worktrees, those worktrees must vacate first.

**Git-dir dependencies** — `.git` move must happen after worktree registrations
pointing to the old path are updated, but before new registrations need the new
path.

**Cycle breaking** — if A needs B's path and B needs A's path, move one to a
temp directory first, then proceed.

### Example: contained → sibling

Source: `.git` at `repo/.git` (bare), worktrees at `repo/main/`, `repo/develop/`

Target: `.git` at `repo/.git` (non-bare), default branch at `repo/`, worktree at
`repo.develop/`

Dependency analysis:

- `repo/develop/` must vacate before `repo/main/` can collapse into `repo/`
- `repo/main/` must collapse before bare can flip (files need to be at root)

Plan:

1. `StashChanges { develop, repo/develop/ }` (if dirty)
2. `MoveWorktree { develop, repo/develop/ → repo.develop/ }`
3. `StashChanges { main, repo/main/ }` (if dirty)
4. `CollapseIntoRoot { repo/main/ → repo/ }`
5. `UnregisterWorktree { main }`
6. `SetBare(false)`
7. `InitWorktreeIndex { repo/ }`
8. `PopStash { main, repo/ }` (if was dirty)
9. `PopStash { develop, repo.develop/ }` (if was dirty)
10. `ValidateIntegrity`

### Example: sibling → contained-classic

Source: `.git` at `repo/.git` (non-bare), worktree at `repo.develop/`

Target: `.git` at `repo/main/.git` (non-bare), worktrees at `repo/develop/`

Plan:

1. `StashChanges { main, repo/ }` (if dirty)
2. `MoveWorktree { develop, repo.develop/ → repo/develop/ }`
3. `NestFromRoot { repo/ → repo/main/ }`
4. `MoveGitDir { repo/.git → repo/main/.git }`
5. `InitWorktreeIndex { repo/main/ }`
6. `PopStash { main, repo/main/ }` (if was dirty)
7. `ValidateIntegrity`

### Example: contained → contained-classic

Source: `.git` at `repo/.git` (bare), worktrees at `repo/main/`, `repo/develop/`

Target: `.git` at `repo/main/.git` (non-bare), worktrees at `repo/develop/`

Plan:

1. `MoveGitDir { repo/.git → repo/main/.git }`
2. `SetBare(false)`
3. `InitWorktreeIndex { repo/main/ }`
4. `UnregisterWorktree { main }`
5. `ValidateIntegrity`

Note: `repo/develop/` is already at its target path — no move needed. The only
operations are moving `.git` into the default branch subdir and flipping bare.

## Non-Conforming Worktree Handling

A worktree is **non-conforming** if its current path does not match what the
target template computes for its branch. This is determined purely by evaluating
the template — no registry in repos.json.

**Default behavior:** non-conforming worktrees are left in place. They continue
to function because git's worktree tracking is path-based.

**Flags:**

- `--include <branch>` (repeatable) — relocate this specific non-conforming
  worktree to its template-computed path
- `--include-all` — relocate every non-conforming worktree

**Dry run output** clearly labels each worktree's disposition:

```
Plan for transform to 'sibling':
  Move 'develop':    repo/develop/    → repo.develop/     (conforming)
  Collapse 'main':   repo/main/       → repo/             (default branch)
  Skip 'experiment': ~/scratch/exp                        (non-conforming, use --include to relocate)
  Move .git:         repo/.git        → repo/.git         (bare → non-bare)
```

## Dry Run

`--dry-run` computes and prints the full plan without executing. This is the
same code path as a real transform minus the execution step — the plan builder
runs, conflict analysis runs, the plan is printed, then the engine stops.

Output includes:

- Each operation in execution order
- Non-conforming worktrees that will be skipped
- Dirty worktrees that would need stashing
- Any detected issues (path conflicts, missing directories)

## Safety

### Dirty worktree handling

Pre-flight scan detects uncommitted changes in all worktrees. Default behavior:
abort with a list of dirty worktrees. `--force` proceeds with per-worktree
stashing — each worktree's changes are stashed independently before it is moved,
and popped after it arrives at its target path.

### Integrity validation

After all operations complete, run `git fsck` (or a lighter check — verify each
worktree's `.git` file points to a valid gitdir, verify `core.bare` matches
expectations). Report any issues found.

### Rollback on failure

Each executed operation is pushed onto a rollback stack. If any operation fails:

1. Attempt to reverse the completed operations in reverse order
2. If rollback succeeds, report the original failure
3. If rollback also fails, print the current state, what was attempted, and
   manual recovery instructions

### Fixup after MoveGitDir

When `.git` moves, all `.git/worktrees/<name>/gitdir` files contain absolute
paths back to each worktree's `.git` file. These must be updated. Additionally,
each worktree's `.git` file (a text file containing `gitdir: <path>`) must be
updated to point to the new `.git/worktrees/<name>` location.

## CLI Interface

```
daft layout transform <LAYOUT> [OPTIONS]

Options:
    --force           Force transform even with uncommitted changes
    --dry-run         Show plan without executing
    --include <BRANCH>  Also relocate this non-conforming worktree (repeatable)
    --include-all     Relocate all non-conforming worktrees
```

## What Gets Replaced

| Current code                                | Replacement                                       |
| ------------------------------------------- | ------------------------------------------------- |
| `transform.rs` `convert_to_bare()`          | Plan builder + `SetBare` / `NestFromRoot` ops     |
| `transform.rs` `convert_to_non_bare()`      | Plan builder + `SetBare` / `CollapseIntoRoot` ops |
| `commands/layout.rs` 2x2 dispatch           | Plan builder (no dispatch matrix)                 |
| `commands/layout.rs` `relocate_worktrees()` | `MoveWorktree` ops in the plan                    |
| `transform.rs` `move_files_to_worktree()`   | `NestFromRoot` op                                 |
| `transform.rs` `move_files_from_worktree()` | `CollapseIntoRoot` op                             |

## What Stays

- `git worktree move` for relocating linked worktrees
- `git worktree list --porcelain` for reading current state
- Template evaluation via `Layout::worktree_path()` with `needs_wrapper()`
  adjustment
- `needs_bare()` and `needs_wrapper()` for deriving target git structure
- `cleanup_empty_parents()` utility
- Hook execution for worktree removal (when a worktree is being removed as part
  of transform, pre/post-remove hooks still fire)
