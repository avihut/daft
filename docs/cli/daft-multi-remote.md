---
title: daft-multi-remote
description: Manage multi-remote worktree organization
---

# daft multi-remote

Manage multi-remote worktree organization

## Description

Manages multi-remote mode, which organizes worktrees by remote when working
with multiple remotes (e.g., fork workflows with `origin` and `upstream`).

When multi-remote mode is disabled (default), worktrees are placed directly
under the project root:

    project/
    ├── .git/
    ├── main/
    └── feature/foo/

When multi-remote mode is enabled, worktrees are organized by remote:

    project/
    ├── .git/
    ├── origin/
    │   ├── main/
    │   └── feature/foo/
    └── upstream/
        └── main/

Use `git daft multi-remote enable` to migrate existing worktrees to the
multi-remote layout. Use `git daft multi-remote disable` to migrate back
to the flat layout.

## Usage

```
daft multi-remote
```

## Subcommands

### enable

Enable multi-remote mode and migrate existing worktrees

Enables multi-remote mode and migrates existing worktrees to the remote-prefixed
directory structure.

Each worktree is moved from `project/branch` to `project/remote/branch`, where
the remote is determined by the branch's upstream tracking configuration or
defaults to the specified default remote.

Use --dry-run to preview the migration without making changes.

```
daft multi-remote enable [OPTIONS]
```

#### Options

| Option | Description | Default |
|--------|-------------|----------|
| `--default <DEFAULT>` | Default remote for new branches (defaults to 'origin') |  |
| `--dry-run` | Preview changes without executing |  |
| `-f, --force` | Skip confirmation |  |

### disable

Disable multi-remote mode and flatten worktree structure

Disables multi-remote mode and migrates worktrees back to the flat directory
structure.

Each worktree is moved from `project/remote/branch` back to `project/branch`.
This command requires that only one remote is configured, as the flat structure
cannot distinguish between worktrees from different remotes.

Use --dry-run to preview the migration without making changes.

```
daft multi-remote disable [OPTIONS]
```

#### Options

| Option | Description | Default |
|--------|-------------|----------|
| `--dry-run` | Preview changes without executing |  |
| `-f, --force` | Skip confirmation |  |

### status

Show current multi-remote configuration

```
daft multi-remote status
```

### set-default

Change the default remote for new branches

```
daft multi-remote set-default <REMOTE>
```

#### Arguments

| Argument | Description | Required |
|----------|-------------|----------|
| `<REMOTE>` | Remote name to use as default | Yes |

### move

Move a worktree to a different remote folder

Moves a worktree from one remote folder to another. This is useful when:

- You forked a branch and want to organize it under a different remote
- You're transferring a feature branch from your fork to upstream
- You want to reorganize worktrees after adding a new remote

The worktree is physically moved on disk, and git's internal worktree
records are updated accordingly.

Options like --set-upstream can update the branch's tracking configuration
to match the new remote organization.

```
daft multi-remote move [OPTIONS] <BRANCH>
```

#### Arguments

| Argument | Description | Required |
|----------|-------------|----------|
| `<BRANCH>` | Branch name or worktree path to move | Yes |

#### Options

| Option | Description | Default |
|--------|-------------|----------|
| `--to <TO>` | Target remote folder |  |
| `--set-upstream` | Also update the branch's upstream tracking to the new remote |  |
| `--push` | Push the branch to the new remote |  |
| `--delete-old` | Delete the branch from the old remote after pushing |  |
| `--dry-run` | Preview changes without executing |  |
| `-f, --force` | Skip confirmation |  |

## Global Options

| Option | Description |
|--------|-------------|
| `-h`, `--help` | Print help information |
| `-V`, `--version` | Print version information |

