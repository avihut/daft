---
title: git-worktree-rename
description: Rename a branch and move its worktree
---

# git worktree-rename

Rename a branch and move its worktree

::: tip
This command is also available as `daft rename`. See [daft rename](./daft-rename.md).
:::

## Description

Renames a local branch and moves its associated worktree directory to match
the new branch name. If the branch has a remote tracking branch, the remote
branch is also renamed (push new name, delete old name) unless --no-remote
is specified.

The source can be specified as a branch name or a path to an existing
worktree (absolute or relative).

If you are currently inside the worktree being renamed, the shell is
redirected to the new worktree location after the rename completes.

Empty parent directories left behind by the move are automatically cleaned up.

## Usage

```
git worktree-rename [OPTIONS] <SOURCE> <NEW_BRANCH>
```

## Arguments

| Argument | Description | Required |
|----------|-------------|----------|
| `<SOURCE>` | Source branch name or worktree path | Yes |
| `<NEW_BRANCH>` | New branch name | Yes |

## Options

| Option | Description | Default |
|--------|-------------|----------|
| `--no-remote` | Skip remote branch rename |  |
| `--dry-run` | Preview changes without executing |  |
| `-q, --quiet` | Suppress progress output |  |
| `-v, --verbose` | Show detailed progress |  |

## Global Options

| Option | Description |
|--------|-------------|
| `-h`, `--help` | Print help information |
| `-V`, `--version` | Print version information |

## See Also

- [git-worktree-branch](./git-worktree-branch.md)
- [git-worktree-checkout](./git-worktree-checkout.md)

