---
title: git-worktree-checkout-branch
description: Create a worktree with a new branch
---

# git worktree-checkout-branch

Create a worktree with a new branch

## Description

Creates a new branch and a corresponding worktree in a single operation. The
new branch is based on the current branch, or on <base-branch> if specified.
The worktree is placed at the project root level as a sibling to other
worktrees.

After creating the branch locally, this command pushes it to the remote and
configures upstream tracking. By default, uncommitted changes from the current
worktree are carried to the new worktree; use --no-carry to disable this.

This command can be run from anywhere within the repository. Lifecycle hooks
from .daft/hooks/ are executed if the repository is trusted. See git-daft(1)
for hook management.

## Usage

```
git worktree-checkout-branch [OPTIONS] <NEW_BRANCH_NAME> [BASE_BRANCH_NAME]
```

## Arguments

| Argument | Description | Required |
|----------|-------------|----------|
| `<NEW_BRANCH_NAME>` | Name for the new branch (also used as the worktree directory name) | Yes |
| `<BASE_BRANCH_NAME>` | Branch to use as the base for the new branch; defaults to the current branch | No |

## Options

| Option | Description | Default |
|--------|-------------|----------|
| `-q, --quiet` | Operate quietly; suppress progress reporting |  |
| `-v, --verbose` | Be verbose; show detailed progress |  |
| `-c, --carry` | Apply uncommitted changes to the new worktree (this is the default) |  |
| `--no-carry` | Do not carry uncommitted changes to the new worktree |  |
| `-r, --remote <REMOTE>` | Remote for worktree organization (multi-remote mode) |  |
| `--no-cd` | Do not change directory to the new worktree |  |

## Global Options

| Option | Description |
|--------|-------------|
| `-h`, `--help` | Print help information |
| `-V`, `--version` | Print version information |

## See Also

- [git-worktree-checkout](./git-worktree-checkout.md)
- [git-worktree-carry](./git-worktree-carry.md)
- [git-worktree-branch-delete](./git-worktree-branch-delete.md)

