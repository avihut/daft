---
title: daft remove
description: Delete branches and their worktrees
---

# daft remove

Delete branches and their worktrees

## Usage

```
daft remove [OPTIONS] <BRANCHES>
```

This is equivalent to `git worktree-branch-delete`. All options and arguments
are the same.

## Description

Deletes one or more local branches along with their associated worktrees and
remote tracking branches in a single operation. Arguments can be branch names
or worktree paths.

Safety checks prevent accidental data loss. Use -D (--force) to override.

## See Also

- [git worktree-branch-delete](./git-worktree-branch-delete.md) for full options reference
