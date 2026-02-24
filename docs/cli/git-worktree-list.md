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
including ahead/behind counts relative to the base branch, dirty status,
branch age, and last commit details.

Each worktree is shown with:
  - A `>` marker for the current worktree
  - Branch name (or "(detached)" for detached HEAD)
  - Relative path from the project root
  - Ahead/behind counts vs. the base branch (e.g. +3 -1)
  - A `*` dirty marker if there are uncommitted changes
  - Branch age since creation (e.g. 3d, 2w, 5mo)
  - Shorthand age of the last commit (e.g. 1h, 4d)
  - Subject line of the last commit (truncated to 40 chars)

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

