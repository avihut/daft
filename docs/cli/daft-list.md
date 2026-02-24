---
title: daft list
description: List all worktrees with status information
---

# daft list

List all worktrees with status information

## Usage

```
daft list [OPTIONS]
```

This is equivalent to `git worktree-list`. All options and arguments are
the same.

## Description

Lists all worktrees in the current project with enriched status information
including ahead/behind counts relative to the base branch, dirty status,
and last commit details.

Each worktree is shown with:

- A `>` marker for the current worktree
- Branch name (or "(detached)" for detached HEAD)
- Relative path from the project root
- Ahead/behind counts vs. the base branch (e.g. +3 -1)
- A `*` dirty marker if there are uncommitted changes
- Branch age since creation (e.g. 3d, 2w, 5mo)
- Shorthand age of the last commit (e.g. 1h, 4d)
- Subject line of the last commit (truncated to 40 chars)

Ages use shorthand notation: `<1m`, `Xm`, `Xh`, `Xd`, `Xw`, `Xmo`, `Xy`.

Use `--json` for machine-readable output suitable for scripting.

## See Also

- [git worktree-list](./git-worktree-list.md) for full options reference
