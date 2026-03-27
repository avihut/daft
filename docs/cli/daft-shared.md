---
title: daft-shared
description: Manage shared files across worktrees
---

# daft shared

Manage shared files across worktrees

## Description

Centralize untracked configuration files (.env, .idea/, .vscode/, etc.)
so they are shared across worktrees via symlinks.

Files are stored in .git/.daft/shared/ and symlinked into each worktree.
Use 'materialize' to make a worktree-local copy, and 'link' to rejoin
the shared version.

## Usage

```
daft shared
```

## Subcommands

### add

Collect file/dir from current worktree into shared storage

```
daft shared add [OPTIONS] <PATHS>
```

#### Arguments

| Argument | Description | Required |
|----------|-------------|----------|
| `<PATHS>` | Paths to share (relative to worktree root) | Yes |

#### Options

| Option | Description | Default |
|--------|-------------|----------|
| `--declare` | Only declare the path in daft.yml without collecting (file need not exist) |  |

### remove

Stop sharing a file (materialize everywhere, then remove)

```
daft shared remove [OPTIONS] <PATHS>
```

#### Arguments

| Argument | Description | Required |
|----------|-------------|----------|
| `<PATHS>` | Paths to stop sharing | Yes |

#### Options

| Option | Description | Default |
|--------|-------------|----------|
| `--delete` | Delete shared file and all symlinks instead of materializing |  |

### materialize

Replace symlink with a local copy in current worktree

```
daft shared materialize [OPTIONS] <PATH> [WORKTREE]
```

#### Arguments

| Argument | Description | Required |
|----------|-------------|----------|
| `<PATH>` | Shared file path to materialize | Yes |
| `<WORKTREE>` | Target worktree name or path (defaults to current worktree) | No |

#### Options

| Option | Description | Default |
|--------|-------------|----------|
| `--override` | Force materialization even if a non-shared file exists |  |

### link

Replace local copy with symlink to shared version

```
daft shared link [OPTIONS] <PATH> [WORKTREE]
```

#### Arguments

| Argument | Description | Required |
|----------|-------------|----------|
| `<PATH>` | Shared file path to link back to shared version | Yes |
| `<WORKTREE>` | Target worktree name or path (defaults to current worktree) | No |

#### Options

| Option | Description | Default |
|--------|-------------|----------|
| `--override` | Replace local file even if it differs from shared version |  |

### status

Show shared files and per-worktree state

```
daft shared status
```

### sync

Ensure all worktrees have symlinks for declared shared files

```
daft shared sync
```

## Global Options

| Option | Description |
|--------|-------------|
| `-h`, `--help` | Print help information |
| `-V`, `--version` | Print version information |

