---
title: git-worktree-checkout
description: Create a worktree for an existing branch
---

# git worktree-checkout

Create a worktree for an existing branch

## Description

Creates a new worktree for an existing local or remote branch. The worktree
is placed at the project root level as a sibling to other worktrees, using
the branch name as the directory name.

If the branch exists only on the remote, a local tracking branch is created
automatically. If the branch exists both locally and on the remote, the local
branch is checked out and upstream tracking is configured.

This command can be run from anywhere within the repository. If a worktree
for the specified branch already exists, no new worktree is created; the
working directory is changed to the existing worktree instead.

Lifecycle hooks from .daft/hooks/ are executed if the repository is trusted.
See git-daft(1) for hook management.

## Usage

```
git worktree-checkout [OPTIONS] <BRANCH_NAME>
```

## Arguments

| Argument | Description | Required |
|----------|-------------|----------|
| `<BRANCH_NAME>` | Name of the branch to check out | Yes |

## Options

| Option | Description | Default |
|--------|-------------|----------|
| `-v, --verbose` | Be verbose; show detailed progress |  |
| `-c, --carry` | Apply uncommitted changes from the current worktree to the new one |  |
| `--no-carry` | Do not carry uncommitted changes (this is the default) |  |
| `-r, --remote <REMOTE>` | Remote for worktree organization (multi-remote mode) |  |
| `--no-cd` | Do not change directory to the new worktree |  |

## Global Options

| Option | Description |
|--------|-------------|
| `-h`, `--help` | Print help information |
| `-V`, `--version` | Print version information |

## See Also

- [git-worktree-checkout-branch](./git-worktree-checkout-branch.md)
- [git-worktree-checkout-branch-from-default](./git-worktree-checkout-branch-from-default.md)
- [git-worktree-carry](./git-worktree-carry.md)

