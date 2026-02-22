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

## See Also

- [git worktree-prune](./git-worktree-prune.md) for full options reference
