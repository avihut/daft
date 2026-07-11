---
title: git-daft-repo-list
description: List repositories in the repo catalog
---

# git daft-repo-list

List repositories in the repo catalog

## Description

Lists the repositories daft knows about. The catalog fills itself: cloning,
initializing, adopting, or running daft commands inside a repo registers it
automatically; `git daft repo add` registers one manually.

Removed repositories keep a catalog entry (so their job logs stay
addressable and `git daft clone <name>` can restore them); show them with
--all.

Use --columns to select which columns are shown and in what order.
  Replace mode:  --columns name,path,remote (exact set and order)
  Modifier mode: --columns -remote (remove from defaults)
  Add optional:  --columns +size,+layout,+branch (add to defaults)

The size column is not shown by default — it walks every repository, so it
is opt-in, same as the worktree commands. On a terminal the sizes stream in
live while the table renders immediately, with a total row summing them.
The recorded worktree layout (+layout) and default branch (+branch) are
likewise opt-in; structured output includes both by default.

## Usage

```
git daft-repo-list [OPTIONS]
```

## Options

| Option | Description | Default |
|--------|-------------|----------|
| `-a, --all` | Include removed repositories |  |
| `--columns <COLUMNS>` | Columns to display (comma-separated). Replace: name,path,remote. Modify defaults: +col,-col. Available: annotation, name, worktrees, layout, branch, path, size, remote |  |
| `--format <FORMAT>` | Output format. Mutually exclusive with --template |  |
| `--template <STR>` | Tera template string. Mutually exclusive with --format |  |
| `--no-headers` | Omit header row (tsv/csv only) |  |
| `-q, --quiet` | Suppress progress reporting |  |

## Global Options

| Option | Description |
|--------|-------------|
| `-h`, `--help` | Print help information |
| `-V`, `--version` | Print version information |

