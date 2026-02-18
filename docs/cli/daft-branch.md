---
title: daft-branch
description: Branch and worktree management operations
---

# daft branch

Branch and worktree management operations

## Description

Provides branch and worktree management operations, particularly useful in
multi-remote workflows.

The main subcommand is `move`, which moves a worktree to a different remote
folder when multi-remote mode is enabled.

## Usage

```
daft branch <SUBCOMMAND>
```

## Subcommands

### move

Move a worktree to a different remote folder.

```
daft branch move <BRANCH> --to <REMOTE> [OPTIONS]
```

Moves a worktree from one remote folder to another. This is useful when:

- You forked a branch and want to organize it under a different remote
- You're transferring a feature branch from your fork to upstream
- You want to reorganize worktrees after adding a new remote

The worktree is physically moved on disk, and git's internal worktree records
are updated accordingly.

Requires multi-remote mode to be enabled. See
[multi-remote](./daft-multi-remote.md).

| Argument / Option    | Description                                            | Required |
| -------------------- | ------------------------------------------------------ | -------- |
| `<BRANCH>`           | Branch name or worktree path to move                   | Yes      |
| `--to <REMOTE>`      | Target remote folder                                   | Yes      |
| `--set-upstream`     | Also update the branch's upstream tracking to the new remote |    |
| `--push`             | Push the branch to the new remote                      |          |
| `--delete-old`       | Delete the branch from the old remote after pushing    |          |
| `--dry-run`          | Preview changes without executing                      |          |
| `-f, --force`        | Skip confirmation                                      |          |

### Examples

```bash
# Move feature/auth from origin to upstream
daft branch move feature/auth --to upstream

# Move and update tracking + push
daft branch move feature/auth --to upstream --set-upstream --push

# Full transfer: move, push to new, delete from old
daft branch move feature/auth --to upstream --push --delete-old --force

# Preview what would happen
daft branch move feature/auth --to upstream --dry-run
```

## Global Options

| Option            | Description               |
| ----------------- | ------------------------- |
| `-h`, `--help`    | Print help information    |
| `-V`, `--version` | Print version information |

## See Also

- [daft-multi-remote](./daft-multi-remote.md)
- [Multi-Remote guide](../guide/multi-remote.md)
