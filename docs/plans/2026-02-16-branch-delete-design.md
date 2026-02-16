# Design: git-worktree-branch-delete

**Date:** 2026-02-16 **Issue:** #166 **Shortcut:** `gwtbd`

## Summary

A command that fully deletes a branch — its worktree, local branch, and remote
tracking branch — as a single atomic-feeling operation. Behaves like
`git branch -d` with safety checks that prevent data loss, and a `--force`/`-D`
override to bypass them.

## Command Interface

```
git worktree-branch-delete [-D | --force] [-q | --quiet] [-v | --verbose] <branch>...
```

**Arguments & flags:**

- `<branch>...` — one or more branch names to delete (required, positional)
- `-D` / `--force` — skip safety checks (uncommitted changes, unmerged, out of
  sync)
- `-q` / `--quiet` — suppress informational output, only show errors
- `-v` / `--verbose` — show detailed steps (git commands, hook execution)

## Architecture: Validate-Then-Execute

The command runs in two phases. All safety checks run for every branch before
any deletions begin. If any branch fails validation (without `--force`), the
entire command aborts with all errors listed. No partial state.

### Phase 1: Validation

For each branch, these checks run in order:

1. **Branch exists locally** — error if the branch name doesn't match a local
   branch
2. **Branch is not the default branch** — refuse to delete main/master (even
   with `--force`)
3. **Worktree has no uncommitted changes** — `git status --porcelain` on the
   worktree is clean (staged, unstaged, and untracked files)
4. **Branch is merged into the default branch** — all commits on the branch are
   reachable from the default branch, either by ancestry or squash merge
   detection
5. **Local and remote are in sync** — if the branch tracks a remote, local and
   remote point to the same commit (no unpushed or unpulled commits)

With `--force`/`-D`: checks 3, 4, and 5 are skipped. Checks 1 and 2 always
apply.

**Error output (no force):**

```
error: cannot delete 'feature/foo': has uncommitted changes in worktree
error: cannot delete 'feature/bar': not merged into 'main' (use -D to force)
Aborting: 2 of 3 branches failed validation. No branches were deleted.
```

### Phase 2: Execution

After all branches pass validation, deletions execute per branch in this order:

1. **Run `worktree-pre-remove` hook** — with `RemovalReason::Manual`. If hook
   fails with `fail_mode: Abort`, skip this branch and report error
2. **Delete remote branch** — `git push <remote> --delete <branch>`. Done first
   because it's the hardest to recreate if something fails later
3. **Remove worktree directory** — `git worktree remove <path>`. Uses `--force`
   if our `--force` is set
4. **Delete local branch** — `git branch -d <branch>` (or `-D` if forced)
5. **Clean up empty parent directories** — reuse existing
   `cleanup_empty_parent_dirs()` pattern from prune
6. **Run `worktree-post-remove` hook** — with `RemovalReason::Manual`

**Current worktree handling:** If one of the target branches belongs to the
worktree the user is currently in, it gets deferred to last. Before deleting it,
the CD target is resolved using the existing `daft.prune.cdTarget` setting and
`__DAFT_CD__` is emitted so the shell wrapper moves the user out.

**Partial failure during execution:** If a step fails for a branch (e.g., no
push access for remote deletion), the error is reported for that branch but
remaining branches continue. Summary at the end shows what succeeded and what
failed.

**Success output:**

```
Deleted feature/foo (worktree, local branch, remote branch)
Deleted feature/bar (worktree, local branch, remote branch)
```

**Partial failure output:**

```
Deleted feature/foo (worktree, local branch, remote branch)
error: failed to delete remote branch 'feature/bar' on origin: permission denied
Deleted feature/bar (worktree, local branch only)
```

## Edge Cases

**No remote tracking branch:** Remote deletion step is silently skipped. Note in
verbose output only.

**No worktree:** If the branch exists locally but has no associated worktree
(e.g., a plain local branch in a bare repo), the worktree removal step is
skipped. Only the branch and remote are deleted.

**Remote detection:** Uses track-based detection. Deletes from whatever remote
the branch tracks (`origin`, `upstream`, etc.). Consistent with daft's
multi-remote support.

## Squash Merge Detection

Standard `git branch -d` only checks commit ancestry, which fails for
squash-merged branches.

**Strategy:**

1. Check ancestry first — `git merge-base --is-ancestor`. If the branch is an
   ancestor of the default branch, it was regular-merged. Done.
2. If not an ancestor, use `git cherry <default> <branch>` to check if
   equivalent patches exist upstream. If all lines are prefixed with `-`, the
   patches are all upstream (squash or cherry-picked).
3. If detection is inconclusive (e.g., squash commit combined multiple PRs), the
   check fails conservatively — user needs `--force`. False negatives (allowing
   deletion of unmerged work) are worse than false positives (requiring
   `--force`).

## Hooks

Uses existing `worktree-pre-remove` and `worktree-post-remove` hooks with
`RemovalReason::Manual`.

**Environment variables:**

- `DAFT_REMOVAL_REASON=manual`
- Standard hook context: `worktree_path`, `branch_name`, `project_root`,
  `git_dir`, `remote`, `source_worktree`

## Integration Points

- **Shortcut:** `gwtbd` added to git-style shortcuts in `src/shortcuts.rs`
- **Routing:** `git-worktree-branch-delete` added to `src/main.rs`
- **Module:** `src/commands/branch_delete.rs`
- **xtask:** Added to `COMMANDS` array and `get_command_for_name()`
- **Docs:** Help output in `src/commands/docs.rs`, man page via
  `mise run gen-man`
- **CLI reference:** `docs/cli/daft-worktree-branch-delete.md`
