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
including uncommitted changes, ahead/behind counts vs. both the base branch
and the remote tracking branch, branch age, and last commit details.

Each worktree is shown with:
  - A `>` marker for the current worktree
  - Branch name, with `✦` for the default branch
  - Relative path from the current directory
  - Ahead/behind counts vs. the base branch (e.g. +3 -1)
  - File status: +N staged, -N unstaged, ?N untracked
  - Remote tracking status: ⇡N unpushed, ⇣N unpulled
  - Branch age since creation (e.g. 3d, 2w, 5mo)
  - Last commit: shorthand age + subject (e.g. 1h fix login bug)

Ages use shorthand notation: <1m, Xm, Xh, Xd, Xw, Xmo, Xy.

Use -b / --branches to also show local branches without a worktree.
Use -r / --remotes to also show remote tracking branches.
Use -a / --all to show both (equivalent to -b -r).

Non-worktree branches are shown with dimmed styling and blank Path/Changes columns.

Use --stat lines to show line-level change counts (insertions and deletions)
instead of the default summary (commit counts for base/remote, file counts for
changes). This is slower as it requires computing diffs for each worktree.

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
| `-b, --branches` | Also show local branches without a worktree |  |
| `-r, --remotes` | Also show remote tracking branches |  |
| `-a, --all` | Show all branches (equivalent to -b -r) |  |
| `--stat <STAT>` | Statistics mode: summary or lines (default: from git config daft.list.stat, or summary) |  |

## Global Options

| Option | Description |
|--------|-------------|
| `-h`, `--help` | Print help information |
| `-V`, `--version` | Print version information |

## See Also

- [git-worktree-checkout](./git-worktree-checkout.md)
- [git-worktree-prune](./git-worktree-prune.md)
- [git-worktree-branch](./git-worktree-branch.md)

