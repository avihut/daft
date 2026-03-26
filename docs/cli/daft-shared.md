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

## Global Options

| Option | Description |
|--------|-------------|
| `-h`, `--help` | Print help information |
| `-V`, `--version` | Print version information |

