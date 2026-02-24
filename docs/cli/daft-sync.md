---
title: daft sync
description: Synchronize worktrees with remote (prune + update all)
---

# daft sync

Synchronize worktrees with remote (prune + update all)

## Usage

```
daft sync [OPTIONS]
```

This is equivalent to `git sync`. All options and arguments are
the same.

## Description

Synchronizes all worktrees with the remote in a single command.

This is equivalent to running `daft prune` followed by `daft update --all`:

  1. Prune: fetches with --prune, removes worktrees and branches for deleted
     remote branches, executes lifecycle hooks for each removal.
  2. Update: pulls all remaining worktrees from their remote tracking branches.
  3. Rebase (--rebase BRANCH): rebases all remaining worktrees onto BRANCH.
     Best-effort: conflicts are immediately aborted and reported.

If you are currently inside a worktree that gets pruned, the shell is redirected
to a safe location (project root by default, or as configured via
daft.prune.cdTarget).

For fine-grained control over either phase, use `daft prune` and `daft update`
separately.

## See Also

- [git sync](./git-sync.md) for full options reference
