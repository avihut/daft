---
title: git-daft-repo-rename
description: Rename a repository's catalog entry
---

# git daft-repo-rename

Rename a repository's catalog entry

## Description

Changes the name a repository answers to in the repo catalog — the name taken
by `git daft go <repo>`, `git daft list <repo>` and `--repo <name>`.

Nothing on disk changes: the directory keeps its name and no worktree moves.
To relocate the repository itself, use `git daft repo move`, which can rename
the catalog entry in the same operation with --name.

This is the same operation as `git daft repo add --name <name>`.

Not to be confused with `git daft rename`, which renames a worktree and its
branch — the same way `git daft repo remove` sits beside `git daft remove`.

## Usage

```
git daft-repo-rename [OPTIONS] <REPO> <NEW_NAME>
```

## Arguments

| Argument | Description | Required |
|----------|-------------|----------|
| `<REPO>` | Repository to rename: a catalog name, uuid, or path | Yes |
| `<NEW_NAME>` | The name it should answer to | Yes |

## Options

| Option | Description | Default |
|--------|-------------|----------|
| `-q, --quiet` | Suppress progress reporting |  |
| `-v, --verbose` | Show detailed progress |  |

## Global Options

| Option | Description |
|--------|-------------|
| `-h`, `--help` | Print help information |
| `-V`, `--version` | Print version information |

