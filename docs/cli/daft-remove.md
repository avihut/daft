---
title: daft remove
description: Delete branches and their worktrees
---

# daft remove

Delete branches and their worktrees

## Usage

```
daft remove [OPTIONS] <BRANCHES>
daft remove -f <BRANCHES>
```

This is equivalent to `git worktree-branch -d` (safe delete). Use `-f` to
force-delete branches regardless of merge status (`git worktree-branch -D`).

## Description

Deletes one or more local branches along with their associated worktrees and
remote tracking branches in a single operation. Arguments can be branch names
or worktree paths.

Safety checks prevent accidental data loss. Use `-f` (`--force`) to override.

## See Also

- [git worktree-branch](./git-worktree-branch.md) for full options reference
