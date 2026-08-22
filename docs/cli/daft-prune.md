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

A branch is only in scope when something attests that it was on the remote:
git's own upstream tracking, or a publication daft recorded. Being absent from
the remote is not enough on its own, since that is equally true of a branch
that was just created and never pushed. A branch nothing attests to is left
alone, whatever `--force` says; discard those with
[`daft remove`](./daft-remove.md).

## Options

| Option | Description | Default |
|--------|-------------|---------|
| `-v, --verbose` | Increase verbosity (`-v` for hook details, `-vv` for full sequential output) | |
| `-f, --force` | Force removal of worktrees with uncommitted changes or untracked files | |
| `--stat <STAT>` | Statistics mode: `summary` or `lines` (default: from git config `daft.prune.stat`, or `summary`) | |
| `--columns <COLUMNS>` | Columns to display in the summary table (comma-separated). Replace mode: `name,path,age`. Modifier mode: `+col,-col`. The status column is always shown. | |
| `--sort <SORT>` | Sort order (comma-separated). `+col` ascending, `-col` descending. Sortable columns: `name`, `path`, `size`, `age`, `owner`, `activity`. Default: `daft.prune.sort` or `+name`. | |

## Global Options

| Option | Description |
|--------|-------------|
| `-h`, `--help` | Print help information |
| `-V`, `--version` | Print version information |

## See Also

- [git worktree-prune](./git-worktree-prune.md) for full options reference
