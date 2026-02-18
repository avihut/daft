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
```

## Global Options

| Option            | Description               |
| ----------------- | ------------------------- |
| `-h`, `--help`    | Print help information    |
| `-V`, `--version` | Print version information |

## See Also

- [daft-branch](./daft-branch.md)
- [Multi-Remote guide](../guide/multi-remote.md)
- [Configuration](../guide/configuration.md)
