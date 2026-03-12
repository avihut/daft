---
title: git-sync
description: Synchronize worktrees with remote (prune + update all)
---

# git sync

Synchronize worktrees with remote (prune + update all)

::: tip
This command is also available as `daft sync`. See [daft sync](./daft-sync.md).
:::

## Description

Synchronizes all worktrees with the remote in a single command.

This is equivalent to running `daft prune` followed by `daft update --all`:

  1. Prune: fetches with --prune, removes worktrees and branches for deleted
     remote branches, executes lifecycle hooks for each removal.
  2. Update: pulls all remaining worktrees from their remote tracking branches.
  3. Rebase (--rebase BRANCH): rebases all remaining worktrees onto BRANCH.
     Best-effort: conflicts are immediately aborted and reported.
  4. Push (--push): pushes all branches to their remote tracking branches.
     Branches without an upstream are skipped. Push failures are reported as
     warnings; they do not cause sync to fail. Use --force-with-lease with
     --push to force-push rebased branches.

If you are currently inside a worktree that gets pruned, the shell is redirected
to a safe location (project root by default, or as configured via
daft.prune.cdTarget).

For fine-grained control over either phase, use `daft prune` and `daft update`
separately.

## Usage

```
git sync [OPTIONS]
```

## Options

| Option | Description | Default |
|--------|-------------|----------|
| `-v, --verbose` | Increase verbosity (-v for hook details, -vv for full sequential output) |  |
| `-f, --prune-dirty` | Force removal of worktrees with uncommitted changes |  |
| `--force` | Hidden deprecated alias for --prune-dirty |  |
| `--rebase <BRANCH>` | Rebase all branches onto BRANCH after updating |  |
| `--autostash` | Automatically stash and unstash uncommitted changes before/after rebase |  |
| `--push` | Push all branches to their remotes after syncing |  |
| `--force-with-lease` | Use --force-with-lease when pushing (requires --push) |  |
| `--stat <STAT>` | Statistics mode: summary or lines (default: from git config daft.sync.stat, or summary) |  |

## Global Options

| Option | Description |
|--------|-------------|
| `-h`, `--help` | Print help information |
| `-V`, `--version` | Print version information |

## See Also

- [git-worktree-prune](./git-worktree-prune.md)
- [git-worktree-fetch](./git-worktree-fetch.md)

