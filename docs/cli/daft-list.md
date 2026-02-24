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
including uncommitted changes, ahead/behind counts vs. both the base branch
and the remote tracking branch, branch age, and last commit details.

Each worktree is shown with:

- A `>` marker for the current worktree
- Branch name, with `◉` for the default branch
- Relative path from the current directory
- Ahead/behind counts vs. the base branch (e.g. +3 -1)
- File status: +N staged, -N unstaged, ?N untracked
- Remote tracking status: ⇡N unpushed, ⇣N unpulled
- Branch age since creation (e.g. 3d, 2w, 5mo)
- Last commit: shorthand age + subject (e.g. 1h fix login bug)

Ages use shorthand notation: `<1m`, `Xm`, `Xh`, `Xd`, `Xw`, `Xmo`, `Xy`.

Use `--json` for machine-readable output suitable for scripting.

## See Also

- [git worktree-list](./git-worktree-list.md) for full options reference
