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
| `-v, --verbose` | Be verbose; show detailed progress |  |
| `-f, --force` | Force removal of worktrees with uncommitted changes |  |

## Global Options

| Option | Description |
|--------|-------------|
| `-h`, `--help` | Print help information |
| `-V`, `--version` | Print version information |

## See Also

- [git-worktree-prune](./git-worktree-prune.md)
- [git-worktree-fetch](./git-worktree-fetch.md)

