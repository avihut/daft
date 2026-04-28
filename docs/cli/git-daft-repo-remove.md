---
title: git-daft-repo-remove
description: Remove a Git repository and all its worktrees
---

# git daft-repo-remove

Remove a Git repository and all its worktrees

## Description

Removes a Git repository identified by <path> (or the current directory if no
path is given), including the bare git directory and every checked-out
worktree. For each worktree, the worktree-pre-remove and worktree-post-remove
lifecycle hooks are run when the repository is daft-managed and trusted.

Hook failures do not abort removal; failed hooks are summarized after the
operation completes. The repo is removed regardless.

Refuses to operate on paths that are not inside a Git repository.

## Usage

```
git daft-repo-remove [OPTIONS] [PATH]
```

## Arguments

| Argument | Description | Required |
|----------|-------------|----------|
| `<PATH>` | Path to the repo or any directory inside it (default: cwd) | No |

## Options

| Option | Description | Default |
|--------|-------------|----------|
| `-y, --force` | Skip the confirmation prompt |  |
| `--dry-run` | Print what would be removed without touching anything |  |
| `-v, --verbose` | Increase verbosity (-v hook details, -vv full sequential output) |  |

## Global Options

| Option | Description |
|--------|-------------|
| `-h`, `--help` | Print help information |
| `-V`, `--version` | Print version information |

