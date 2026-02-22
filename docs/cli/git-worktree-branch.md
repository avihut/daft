---
title: git-worktree-branch
description: Delete branches and their worktrees
---

# git worktree-branch

Delete branches and their worktrees

::: tip
This command is also available as `daft remove` (safe delete with `-d`).
See [daft remove](./daft-remove.md).
:::

## Description

Deletes one or more local branches along with their associated worktrees and
remote tracking branches in a single operation. This is the inverse of
git-worktree-checkout(1) -b.

Use -d for a safe delete that checks whether each branch has been merged.
Use -D to force-delete branches regardless of merge status. One of -d or -D
is required.

Arguments can be branch names or worktree paths. When a path is given
(absolute, relative, or "."), the branch checked out in that worktree is
resolved automatically. This is convenient when you are inside a worktree
and want to delete it without remembering the branch name.

Safety checks (with -d) prevent accidental data loss. The command refuses to
delete a branch that:

  - has uncommitted changes in its worktree
  - has not been merged (or squash-merged) into the default branch
  - is out of sync with its remote tracking branch

Use -D to override these safety checks. The command always refuses to delete
the repository's default branch (e.g. main), even with -D.

All targeted branches are validated before any deletions begin. If any branch
fails validation without -D, the entire command aborts and no branches are
deleted.

Pre-remove and post-remove lifecycle hooks are executed for each worktree
removal if the repository is trusted. See git-daft(1) for hook management.

## Usage

```
git worktree-branch [OPTIONS] [BRANCHES]
```

## Arguments

| Argument | Description | Required |
|----------|-------------|----------|
| `<BRANCHES>` | Branches to delete (names or worktree paths) | No |

## Options

| Option | Description | Default |
|--------|-------------|----------|
| `-d, --delete` | Delete branches (safe mode) |  |
| `-D, --force` | Force deletion even if not fully merged |  |
| `-q, --quiet` | Operate quietly; suppress progress reporting |  |
| `-v, --verbose` | Be verbose; show detailed progress |  |

## Global Options

| Option | Description |
|--------|-------------|
| `-h`, `--help` | Print help information |
| `-V`, `--version` | Print version information |

## See Also

- [git-worktree-prune](./git-worktree-prune.md)
- [git-worktree-checkout](./git-worktree-checkout.md)

