---
title: daft-multi-remote
description: Manage multi-remote worktree organization
---

# daft multi-remote

Manage multi-remote worktree organization

## Description

Manages multi-remote mode, which organizes worktrees by remote when working with
multiple remotes (e.g., fork workflows with `origin` and `upstream`).

When multi-remote mode is disabled (default), worktrees are placed directly
under the project root:

```
project/
├── .git/
├── main/
└── feature/foo/
```

When multi-remote mode is enabled, worktrees are organized by remote:

```
project/
├── .git/
├── origin/
│   ├── main/
│   └── feature/foo/
└── upstream/
    └── main/
```

Without a subcommand, shows the current multi-remote status.

## Usage

```
daft multi-remote [SUBCOMMAND]
```

## Subcommands

### enable

Enable multi-remote mode and migrate existing worktrees.

```
daft multi-remote enable [OPTIONS]
```

Each worktree is moved from `project/branch` to `project/remote/branch`, where
the remote is determined by the branch's upstream tracking configuration or
defaults to the specified default remote.

| Option                  | Description                                          |
| ----------------------- | ---------------------------------------------------- |
| `--default <REMOTE>`    | Default remote for new branches (defaults to 'origin') |
| `--dry-run`             | Preview changes without executing                    |
| `-f, --force`           | Skip confirmation                                    |

### disable

Disable multi-remote mode and flatten worktree structure.

```
daft multi-remote disable [OPTIONS]
```

Each worktree is moved from `project/remote/branch` back to `project/branch`.

| Option        | Description                       |
| ------------- | --------------------------------- |
| `--dry-run`   | Preview changes without executing |
| `-f, --force` | Skip confirmation                 |

### status

Show current multi-remote configuration (default when no subcommand given).

```
daft multi-remote status
```

Displays:

- Whether multi-remote mode is enabled or disabled
- The default remote
- Configured remotes
- Current worktrees and their organization

### set-default

Change the default remote for new branches.

```
daft multi-remote set-default <REMOTE>
```

| Argument   | Description                    | Required |
| ---------- | ------------------------------ | -------- |
| `<REMOTE>` | Remote name to use as default  | Yes      |

### move

Move a worktree to a different remote folder.

```
daft multi-remote move <BRANCH> --to <REMOTE> [OPTIONS]
```

Moves a worktree from one remote folder to another. This is useful when:

- You forked a branch and want to organize it under a different remote
- You're transferring a feature branch from your fork to upstream
- You want to reorganize worktrees after adding a new remote

The worktree is physically moved on disk, and git's internal worktree records
are updated accordingly.

| Argument / Option    | Description                                            | Required |
| -------------------- | ------------------------------------------------------ | -------- |
| `<BRANCH>`           | Branch name or worktree path to move                   | Yes      |
| `--to <REMOTE>`      | Target remote folder                                   | Yes      |
| `--set-upstream`     | Also update the branch's upstream tracking to the new remote |    |
| `--push`             | Push the branch to the new remote                      |          |
| `--delete-old`       | Delete the branch from the old remote after pushing    |          |
| `--dry-run`          | Preview changes without executing                      |          |
| `-f, --force`        | Skip confirmation                                      |          |

## Configuration

Multi-remote mode uses these git config keys:

| Key                              | Default    | Description                                |
| -------------------------------- | ---------- | ------------------------------------------ |
| `daft.multiRemote.enabled`       | `false`    | Enable multi-remote directory organization |
| `daft.multiRemote.defaultRemote` | `"origin"` | Default remote for new branches            |

## Examples

```bash
# Check current status
daft multi-remote

# Enable with default settings
daft multi-remote enable

# Enable with upstream as default
daft multi-remote enable --default upstream

# Preview migration without making changes
daft multi-remote enable --dry-run

# Change default remote
daft multi-remote set-default upstream

# Disable and flatten back
daft multi-remote disable

# Move feature/auth from origin to upstream
daft multi-remote move feature/auth --to upstream

# Move and update tracking + push
daft multi-remote move feature/auth --to upstream --set-upstream --push

# Full transfer: move, push to new, delete from old
daft multi-remote move feature/auth --to upstream --push --delete-old --force

# Preview what would happen
daft multi-remote move feature/auth --to upstream --dry-run
```

## Global Options

| Option            | Description               |
| ----------------- | ------------------------- |
| `-h`, `--help`    | Print help information    |
| `-V`, `--version` | Print version information |

## See Also

- [Multi-Remote guide](../guide/multi-remote.md)
- [Configuration](../guide/configuration.md)
