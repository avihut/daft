---
title: daft remove
description: Delete branches and their worktrees
---

# daft remove

Delete branches and their worktrees

## Usage

```
daft remove [OPTIONS] <BRANCHES>
daft remove -f <BRANCHES>
```

This is equivalent to `git worktree-branch -d` (safe delete). Use `-f` to
force-delete branches regardless of merge status (`git worktree-branch -D`).

## Description

Deletes one or more local branches along with their associated worktrees in
a single operation. Arguments can be branch names or worktree paths.

By default, the remote branch is not deleted. To also delete the remote branch,
set `daft.branchDelete.remote true` or use `daft config remote-sync --on`. You
can also pass `--remote` to delete only the remote branch while keeping the
local worktree and branch, or `--local` to skip the remote entirely regardless
of config.

Safety checks prevent accidental data loss. Use `-f` (`--force`) to override.
For the default branch (e.g. main), `-f` removes its worktree only -- the
local branch ref and remote branch are always preserved.

## Options

| Option | Description | Default |
|--------|-------------|---------|
| `-f, --force` | Force deletion even if not fully merged | |
| `--local` | Delete only locally; do not touch the remote branch | |
| `--remote` | Delete only the remote branch; keep the local worktree and branch | |
| `-v, --verbose` | Show detailed progress | |
| `-q, --quiet` | Suppress non-error output | |

## Global Options

| Option | Description |
|--------|-------------|
| `-h`, `--help` | Print help information |
| `-V`, `--version` | Print version information |

## See Also

- [daft config](./daft-config.md) to configure remote sync behavior
- [git worktree-branch](./git-worktree-branch.md) for full options reference
