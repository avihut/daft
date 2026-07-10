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

## Usage

```
git daft-repo-list [OPTIONS]
```

## Options

| Option | Description | Default |
|--------|-------------|----------|
| `-a, --all` | Include removed repositories |  |
| `--format <FORMAT>` | Output format. Mutually exclusive with --template |  |
| `--template <STR>` | Tera template string. Mutually exclusive with --format |  |
| `--no-headers` | Omit header row (tsv/csv only) |  |
| `-q, --quiet` | Suppress progress reporting |  |

## Global Options

| Option | Description |
|--------|-------------|
| `-h`, `--help` | Print help information |
| `-V`, `--version` | Print version information |

