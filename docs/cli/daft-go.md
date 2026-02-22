---
title: daft go
description: Open an existing branch in a worktree
---

# daft go

Open an existing branch in a worktree

## Usage

```
daft go [OPTIONS] <BRANCH_NAME>
```

This is equivalent to `git worktree-checkout`. All options and arguments are
the same.

## Description

Creates a new worktree for an existing local or remote branch. The worktree
is placed at the project root level as a sibling to other worktrees, using
the branch name as the directory name.

If the branch exists only on the remote, a local tracking branch is created
automatically. If a worktree for the branch already exists, the working
directory is changed to it.

## See Also

- [daft start](./daft-start.md) to create a new branch
- [git worktree-checkout](./git-worktree-checkout.md) for full options reference
