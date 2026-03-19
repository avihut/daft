---
title: daft prune
description: Remove worktrees for deleted remote branches
---

# daft prune

Remove worktrees for deleted remote branches

## Usage

```
daft prune [OPTIONS]
```

This is equivalent to `git worktree-prune`. All options and arguments are
the same.

## Description

Removes local branches whose corresponding remote tracking branches have been
deleted, along with any associated worktrees. This is useful for cleaning up
after branches have been merged and deleted on the remote.

## Options

| Option | Description | Default |
|--------|-------------|---------|
| `-v, --verbose` | Increase verbosity (`-v` for hook details, `-vv` for full sequential output) | |
| `-f, --force` | Force removal of worktrees with uncommitted changes or untracked files | |
| `--stat <STAT>` | Statistics mode: `summary` or `lines` (default: from git config `daft.prune.stat`, or `summary`) | |
| `--columns <COLUMNS>` | Columns to display in the summary table (comma-separated). Replace mode: `branch,path,age`. Modifier mode: `+col,-col`. The status column is always shown. | |
| `--sort <SORT>` | Sort order (comma-separated). `+col` ascending, `-col` descending. Sortable columns: `branch`, `path`, `size`, `age`, `owner`, `activity`. Default: `daft.prune.sort` or `+branch`. | |

## Global Options

| Option | Description |
|--------|-------------|
| `-h`, `--help` | Print help information |
| `-V`, `--version` | Print version information |

## See Also

- [git worktree-prune](./git-worktree-prune.md) for full options reference
