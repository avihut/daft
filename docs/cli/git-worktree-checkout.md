---
title: git-worktree-checkout
description: Create a worktree for an existing branch, or a new branch with -b
---

# git worktree-checkout

Create a worktree for an existing branch, or a new branch with -b

::: tip
This command is also available as `daft go` (existing branch) or `daft start`
(new branch with `-b`). See [daft go](./daft-go.md) and
[daft start](./daft-start.md).
:::

## Description

Creates a new worktree for an existing local or remote branch. The worktree
is placed at the project root level as a sibling to other worktrees, using
the branch name as the directory name.

If the branch exists only on the remote, a local tracking branch is created
automatically. If the branch exists both locally and on the remote, the local
branch is checked out and upstream tracking is configured.

With -b, creates a new branch and a corresponding worktree in a single
operation. The new branch is based on the current branch, or on <base-branch>
if specified. After creating the branch locally, it is pushed to the remote
and upstream tracking is configured.

With --start (or -s), if the specified branch does not exist locally or on the
remote, a new branch and worktree are created automatically, as if 'daft start'
had been called. This can also be enabled permanently with the daft.go.autoStart
git config option.

Use '-' as the branch name to switch to the previous worktree, similar to
'cd -'. Repeated 'daft go -' toggles between the two most recent worktrees.
Cannot be combined with -b/--create-branch.

This command can be run from anywhere within the repository. If a worktree
for the specified branch already exists, no new worktree is created; the
working directory is changed to the existing worktree instead.

Lifecycle hooks from .daft/hooks/ are executed if the repository is trusted.
See git-daft(1) for hook management.

## Usage

```
git worktree-checkout [OPTIONS] <BRANCH_NAME> [BASE_BRANCH_NAME]
```

## Arguments

| Argument | Description | Required |
|----------|-------------|----------|
| `<BRANCH_NAME>` | Name of the branch to check out (or create with -b); use '-' for previous worktree | Yes |
| `<BASE_BRANCH_NAME>` | Branch to use as the base for the new branch (only with -b); defaults to the current branch | No |

## Options

| Option | Description | Default |
|--------|-------------|----------|
| `-b, --create-branch` | Create a new branch instead of checking out an existing one |  |
| `-q, --quiet` | Operate quietly; suppress progress reporting |  |
| `-v, --verbose` | Be verbose; show detailed progress |  |
| `-c, --carry` | Apply uncommitted changes from the current worktree to the new one |  |
| `--no-carry` | Do not carry uncommitted changes |  |
| `-r, --remote <REMOTE>` | Remote for worktree organization (multi-remote mode) |  |
| `--no-cd` | Do not change directory to the new worktree |  |
| `-x, --exec <EXEC>` | Run a command in the worktree after setup completes (repeatable) |  |
| `-s, --start` | Create a new worktree if the branch does not exist |  |

## Global Options

| Option | Description |
|--------|-------------|
| `-h`, `--help` | Print help information |
| `-V`, `--version` | Print version information |

## See Also

- [git-worktree-carry](./git-worktree-carry.md)
- [git-worktree-branch](./git-worktree-branch.md)
- [git-worktree-rename](./git-worktree-rename.md)

