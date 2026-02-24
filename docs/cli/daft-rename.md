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

## Description

Renames a local branch and moves its associated worktree directory to match
the new name. If the branch has a remote tracking branch, the remote branch
is also renamed (push new name, delete old name) unless `--no-remote` is
specified.

The source can be specified as a branch name or a path to an existing
worktree (absolute or relative). If you are currently inside the worktree
being renamed, the shell wrapper will cd to the new location after the
rename completes.

Empty parent directories left behind by the move are automatically cleaned up.

## Arguments

| Argument | Description | Required |
|----------|-------------|----------|
| `<SOURCE>` | Branch name or worktree path to rename | Yes |
| `<NEW_BRANCH>` | New branch name | Yes |

## Options

| Option | Description | Default |
|--------|-------------|---------|
| `--no-remote` | Skip remote branch rename | |
| `--dry-run` | Preview changes without executing | |
| `-q, --quiet` | Suppress non-error output | |
| `-v, --verbose` | Be verbose; show detailed progress | |

## Global Options

| Option | Description |
|--------|-------------|
| `-h`, `--help` | Print help information |
| `-V`, `--version` | Print version information |

## Examples

```bash
# Rename a branch (local + remote + worktree directory)
daft rename feature/old-name feature/new-name

# Rename without updating the remote branch
daft rename feature/old-name feature/new-name --no-remote

# Preview what would happen
daft rename feature/old-name feature/new-name --dry-run

# Rename from inside the worktree being renamed
cd feature/old-name
daft rename . feature/new-name
```

## See Also

- [git worktree-branch](./git-worktree-branch.md) for the underlying git-native command (rename mode with `-m`)
- [daft remove](./daft-remove.md) to delete branches instead of renaming
