---
title: git-worktree-list
description: List all worktrees with status information
---

# git worktree-list

List all worktrees with status information

::: tip
This command is also available as `daft list`. See [daft list](./daft-list.md).
:::

## Description

Lists all worktrees in the current project with enriched status information
including HEAD SHA, ahead/behind counts, remote tracking, dirty status,
branch age, and last commit details.

Each worktree is shown with:
  - A `>` marker for the current worktree
  - Branch name (or "(detached)" for detached HEAD)
  - Relative path from the current directory
  - Short HEAD commit SHA
  - Ahead/behind counts vs. the base branch (e.g. +3 -1)
  - A `*` dirty marker if there are uncommitted changes
  - Remote tracking branch (e.g. origin/main)
  - Branch age since creation (e.g. 3d, 2w, 5mo)
  - Last commit: shorthand age + subject (e.g. 1h fix login bug)

Ages use shorthand notation: <1m, Xm, Xh, Xd, Xw, Xmo, Xy.

Use --json for machine-readable output suitable for scripting.

## Usage

```
git worktree-list [OPTIONS]
```

## Options

| Option | Description | Default |
|--------|-------------|----------|
| `--json` | Output in JSON format |  |
| `-v, --verbose` | Be verbose; show detailed progress |  |

## Global Options

| Option | Description |
|--------|-------------|
| `-h`, `--help` | Print help information |
| `-V`, `--version` | Print version information |

## See Also

- [git-worktree-checkout](./git-worktree-checkout.md)
- [git-worktree-prune](./git-worktree-prune.md)
- [git-worktree-branch](./git-worktree-branch.md)

