---
title: daft start
description: Create a new branch and worktree
---

# daft start

Create a new branch and worktree

## Usage

```
daft start [OPTIONS] <BRANCH_NAME> [BASE_BRANCH_NAME]
```

This is equivalent to `git worktree-checkout -b`. All options and arguments
are the same as `git worktree-checkout` with `-b` implied.

## Description

Creates a new branch and a corresponding worktree in a single operation.
The new branch is based on the current branch, or on <BASE_BRANCH_NAME>
if specified. After creating the branch locally, it is pushed to the remote
and upstream tracking is configured.

## Arguments

| Argument | Description | Required |
|----------|-------------|----------|
| `<BRANCH_NAME>` | Name of the new branch to create | Yes |
| `<BASE_BRANCH_NAME>` | Branch to use as the base; defaults to the current branch | No |

## See Also

- [daft go](./daft-go.md) to open an existing branch
- [git worktree-checkout](./git-worktree-checkout.md) for full options reference
