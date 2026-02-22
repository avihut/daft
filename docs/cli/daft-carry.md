---
title: daft carry
description: Transfer uncommitted changes to other worktrees
---

# daft carry

Transfer uncommitted changes to other worktrees

## Usage

```
daft carry [OPTIONS] <TARGETS>
```

This is equivalent to `git worktree-carry`. All options and arguments are
the same.

## Description

Transfers uncommitted changes (staged, unstaged, and untracked files) from
the current worktree to one or more target worktrees.

When a single target is specified without --copy, changes are moved. When
--copy is specified or multiple targets are given, changes are copied while
remaining in the source worktree.

## See Also

- [git worktree-carry](./git-worktree-carry.md) for full options reference
