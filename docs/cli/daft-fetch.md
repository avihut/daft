---
title: daft fetch
description: Pull updates into worktree branches
---

# daft fetch

Pull updates into worktree branches

## Usage

```
daft fetch [OPTIONS] [TARGETS] [PULL_ARGS]
```

This is equivalent to `git worktree-fetch`. All options and arguments are
the same.

## Description

Updates worktree branches by pulling from their remote tracking branches.
By default, only fast-forward updates are allowed. If no targets are specified
and --all is not used, the current worktree is updated.

Worktrees with uncommitted changes are skipped unless --force is specified.

## See Also

- [git worktree-fetch](./git-worktree-fetch.md) for full options reference
