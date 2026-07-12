---
title: git-daft-repo-add
description: Register a repository in the repo catalog
---

# git daft-repo-add

Register a repository in the repo catalog

## Description

Registers a repository in daft's repo catalog — the machine-local registry
behind cross-repo commands like `git daft go <repo>` and `git daft repo list`.

The catalog is normally maintained automatically: cloning, initializing, or
running any daft command inside a repo keeps its entry current. Reach for
`repo add` to register a repository daft has never operated in, or to rename
an entry with --name.

Names must be unique among live entries. Automatic registration resolves
collisions by suffixing (`api-2`); an explicit --name that is already taken
is an error instead.

## Usage

```
git daft-repo-add [OPTIONS] [PATH]
```

## Arguments

| Argument | Description | Required |
|----------|-------------|----------|
| `<PATH>` | Repository to register (default: the repo containing the current directory) | No |

## Options

| Option | Description | Default |
|--------|-------------|----------|
| `--name <NAME>` | Catalog name for the repo; renames it when already registered |  |
| `-q, --quiet` | Suppress progress reporting |  |
| `-v, --verbose` | Show detailed progress |  |

## Global Options

| Option | Description |
|--------|-------------|
| `-h`, `--help` | Print help information |
| `-V`, `--version` | Print version information |

