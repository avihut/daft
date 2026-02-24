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

## Description

Synchronizes all worktrees with the remote in a single command.

This is equivalent to running `daft prune` followed by `daft update --all`:

  1. **Prune**: fetches with `--prune`, removes worktrees and branches for
     deleted remote branches, executes lifecycle hooks for each removal.
  2. **Update**: pulls all remaining worktrees from their remote tracking
     branches.
  3. **Rebase** (`--rebase BRANCH`): rebases all remaining worktrees onto
     BRANCH. Best-effort: conflicts are immediately aborted and reported.

If you are currently inside a worktree that gets pruned, the shell is redirected
to a safe location (project root by default, or as configured via
`daft.prune.cdTarget`).

For fine-grained control over either phase, use `daft prune` and `daft update`
separately.

## Options

| Option | Description | Default |
|--------|-------------|---------|
| `-v, --verbose` | Be verbose; show detailed progress | |
| `-f, --force` | Force removal of worktrees with uncommitted changes | |
| `--rebase <BRANCH>` | Rebase all branches onto BRANCH after updating | |

## Global Options

| Option | Description |
|--------|-------------|
| `-h`, `--help` | Print help information |
| `-V`, `--version` | Print version information |

## Examples

```bash
# Prune stale worktrees and update all remaining ones
daft sync

# Sync with verbose output to see what happens in each phase
daft sync --verbose

# Sync and rebase all worktrees onto main
daft sync --rebase main

# Force sync even if worktrees have uncommitted changes
daft sync --force
```

## See Also

- [git sync](./git-sync.md) for the underlying git-native command
- [daft prune](./daft-prune.md) to prune stale worktrees only
- [daft update](./daft-update.md) to update branches only
