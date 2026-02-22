---
title: daft eject
description: Convert a worktree-based repository back to traditional layout
---

# daft eject

Convert a worktree-based repository back to traditional layout

## Usage

```
daft eject [OPTIONS] [REPOSITORY_PATH]
```

This is equivalent to `git worktree-flow-eject`. All options and arguments
are the same.

## Description

Converts your worktree-based repository back to a traditional Git layout.
This removes all worktrees except one, and moves that worktree's files
back to the repository root.

By default, the remote's default branch (main, master, etc.) is kept.
Use --branch to specify a different branch.

## See Also

- [daft adopt](./daft-adopt.md) to convert to worktree layout
- [git worktree-flow-eject](./git-worktree-flow-eject.md) for full options reference
