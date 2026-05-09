---
title: Merging Across Worktrees
description:
  Merge any branch into any worktree without checking out — cross-worktree
  merges, octopus, ephemeral targets, and PR-style hook gates.
---

# Merging Across Worktrees

`daft merge` lets you merge from anywhere into anywhere — no `cd`, no checkout,
no losing your place. The shell stays put; the merge runs in the target
worktree.

## Quick examples

```bash
# Merge feature/api into main without leaving your current worktree
daft merge feature/api --into main

# Octopus merge: three branches into main, in one commit
daft merge feature/a feature/b feature/c --into main

# Merge and clean up the source branch on success
daft merge feature/done --into main -r
```

`daft merge` mirrors `git merge` on the target worktree — all the standard flags
(`--ff-only`, `--squash`, `-s`/`--strategy`, `--signoff`,
`--allow-unrelated-histories`, etc.) pass through. The full surface is
documented in the [`daft merge` reference](/reference/cli/daft-merge).

## Merge styles

`--merge` (default), `--squash`, `--rebase`, and `--rebase-merge` pick the shape
of the resulting history:

| Flag             | History shape                                           |
| ---------------- | ------------------------------------------------------- |
| `--merge`        | Always create a merge commit (no FF)                    |
| `--squash`       | Single squash commit on the target; source is unchanged |
| `--rebase`       | Linear: rebase source onto target, then fast-forward    |
| `--rebase-merge` | Rebase source onto target, then create a merge commit   |

Set a default via `git config daft.merge.style <style>` (see the
[Merge Settings](/reference/configuration#merge-settings) reference).

## Conflicts

When the merge stops on a conflict, daft reports the worktree path where the
merge is in progress and exits non-zero. Resolve there, then continue from
anywhere:

```bash
# From any worktree:
daft merge --continue  # picks up the in-progress merge automatically

# Or abort:
daft merge --abort
```

`daft list --merging` shows worktrees that have an in-progress merge.

## Cleanup

`-r` removes the source branch on success. To also remove the source worktree,
or to override the default behavior repo-wide, set
`git config daft.merge.cleanup remove-branch`. The configured default is applied
unless you explicitly pass `--keep-branch`.

Cleanup runs only on a successful merge — a conflicted or aborted merge never
removes your source branch silently.

## Ephemeral targets

If the target branch has no worktree, `--adopt-target` spins up a temporary one
for the merge:

```bash
daft merge feature/hotfix --into release/1.2 --adopt-target
```

On success the ephemeral worktree is promoted to a permanent worktree at the
configured layout. On conflict it stays put so you can resolve in place.

## Hook gates

`pre-merge` and `post-merge` hooks fire around the operation, with
`DAFT_MERGE_*` env vars covering sources, target, mode, strategy, and result. A
`pre-merge` hook that exits non-zero aborts the merge — full tests, integration
checks, or security gates can run before code leaves the branch.

This is the [PR-check-parity boundary](/hooks/) of the hooks-as-boundaries
thesis. See [Lifecycle hooks → Merge hooks](/hooks/lifecycle#merge-hooks) for
env vars, fail modes, and config.

## Where to next

- **CLI reference:** [`daft merge`](/reference/cli/daft-merge),
  [`git worktree-merge`](/reference/cli/git-worktree-merge)
- **Configuration:** [Merge Settings](/reference/configuration#merge-settings)
- **Hooks:** [Merge hooks](/hooks/lifecycle#merge-hooks)
- **Recipes:** [Recipes for Worktrees](/recipes/?pillar=worktrees)
