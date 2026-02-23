---
title: daft rename
description: Rename a branch and move its worktree
---

# daft rename

Rename a branch and move its worktree

## Usage

```
daft rename [OPTIONS] <SOURCE> <NEW_BRANCH>
```

This is equivalent to `git worktree-branch -m`. All options and arguments
are the same.

## Description

Renames a local branch and moves its associated worktree directory to match
the new name. Optionally renames the remote branch as well (push new name,
delete old name).

Arguments can be branch names or worktree paths. If run from inside the
worktree being renamed, the shell wrapper will cd to the new location.

## See Also

- [git worktree-branch](./git-worktree-branch.md) for full options reference
