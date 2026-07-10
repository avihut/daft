---
title: git-daft-repo-info
description: Show a repository's catalog entry
---

# git daft-repo-info

Show a repository's catalog entry

## Description

Shows a repository's catalog entry: name, location, remote, default branch,
identity, and removed-state. The repository may be addressed by catalog
name, path, or uuid; with no argument the repo containing the current
directory is shown.

Removed repositories resolve too — their entries are retained so job logs
stay addressable and `git daft clone <name>` can restore them.

## Usage

```
git daft-repo-info [OPTIONS] [REPO]
```

## Arguments

| Argument | Description | Required |
|----------|-------------|----------|
| `<REPO>` | Catalog name, path, or uuid (default: the current repo) | No |

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

