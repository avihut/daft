---
title: git-daft-repo-info
description: Show a repository's catalog entry
---

# git daft-repo-info

Show a repository's catalog entry

## Description

Shows a repository's catalog entry: name, status, location, remote, default
branch, recorded worktree layout, its worktrees (branch and checkout path
per line), and any daft.yml relations resolved against the catalog. The
repository may be addressed by catalog name, uuid, or a path. A path may be
the repo root, a subdirectory, or any of its worktrees — daft resolves it to
the repo that encloses it, so `git daft repo info .` shows the repo you are
standing in. With no argument the repo containing the current directory is
shown.

Paths render relative to your working directory when that form is shorter
(same rule as `git daft repo list`). Identity plumbing lives in structured
output only: `--format json` carries every recorded field — uuid, git
common dir, raw canonical paths, registration timestamps — plus the
worktrees as a `{branch, path}` array.

Removed repositories resolve too — their entries are retained so job logs
stay addressable and `git daft clone <name>` can restore them.

## Usage

```
git daft-repo-info [OPTIONS] [REPO]
```

## Arguments

| Argument | Description | Required |
|----------|-------------|----------|
| `<REPO>` | Catalog name, uuid, or a repo path — including . or a subdirectory (default: the current repo) | No |

## Options

| Option | Description | Default |
|--------|-------------|----------|
| `--format <FORMAT>` | Output format. Mutually exclusive with --template |  |
| `--template <STR>` | Tera template string. Mutually exclusive with --format |  |
| `--no-headers` | Omit header row (tsv/csv only) |  |

## Global Options

| Option | Description |
|--------|-------------|
| `-h`, `--help` | Print help information |
| `-V`, `--version` | Print version information |

