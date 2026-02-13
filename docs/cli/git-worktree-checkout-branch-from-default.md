---
title: git-worktree-checkout-branch-from-default
description:
  Create a worktree with a new branch based on the remote's default branch
---

# git worktree-checkout-branch-from-default

Create a worktree with a new branch based on the remote's default branch

## Description

Creates a new branch based on the remote's default branch (typically main or
master) and a corresponding worktree. This is equivalent to running
git-worktree-checkout-branch(1) with the default branch as the base.

The default branch is determined by querying the remote's HEAD reference. This
command is useful when the current branch has diverged from the mainline and a
fresh starting point is needed.

By default, uncommitted changes from the current worktree are carried to the new
worktree; use --no-carry to disable this. The worktree is placed at the project
root level as a sibling to other worktrees.

## Usage

```
git worktree-checkout-branch-from-default [OPTIONS] <NEW_BRANCH_NAME>
```

## Arguments

| Argument            | Description                                                        | Required |
| ------------------- | ------------------------------------------------------------------ | -------- |
| `<NEW_BRANCH_NAME>` | Name for the new branch (also used as the worktree directory name) | Yes      |

## Options

| Option                  | Description                                                         | Default |
| ----------------------- | ------------------------------------------------------------------- | ------- |
| `-v, --verbose`         | Be verbose; show detailed progress                                  |         |
| `-c, --carry`           | Apply uncommitted changes to the new worktree (this is the default) |         |
| `--no-carry`            | Do not carry uncommitted changes to the new worktree                |         |
| `-r, --remote <REMOTE>` | Remote for worktree organization (multi-remote mode)                |         |
| `--no-cd`               | Do not change directory to the new worktree                         |         |

## Global Options

| Option            | Description               |
| ----------------- | ------------------------- |
| `-h`, `--help`    | Print help information    |
| `-V`, `--version` | Print version information |

## See Also

- [git-worktree-checkout](./git-worktree-checkout.md)
- [git-worktree-checkout-branch](./git-worktree-checkout-branch.md)
- [git-worktree-carry](./git-worktree-carry.md)
