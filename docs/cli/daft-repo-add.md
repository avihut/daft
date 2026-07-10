---
title: daft repo add
description: Register a repository in the repo catalog
---

# `daft repo add`

Registers a repository in daft's [repo catalog](/graph/repo-catalog) — the
machine-local registry behind `daft go <repo>`, the `--repo`/`--all-repos`
flags, and clone-by-name.

The catalog normally maintains itself: cloning, initializing, or running daft
commands inside a repo keeps its entry current. Reach for `repo add` to
register a repository daft has never operated in, or to rename an entry.

## Usage

    daft repo add [<path>] [--name <name>] [-q] [-v]

| Argument / flag | Description                                                |
| --------------- | ---------------------------------------------------------- |
| `<path>`        | Repository to register (default: the repo around the cwd). |
| `--name <name>` | Catalog name; renames the entry when already registered.   |
| `-q`, `--quiet` | Suppress progress reporting.                                |
| `-v`, `--verbose` | Show detailed progress.                                   |

Names are unique among live entries. Automatic registration resolves
collisions by suffixing (`api-2`); an explicit `--name` that is already taken
is an error instead.

## Examples

    daft repo add                    # register the current repo
    daft repo add ~/code/legacy      # register another repo by path
    daft repo add --name api         # rename the current repo's entry

## See also

- [Repo catalog](/graph/repo-catalog) — the full catalog guide
- [`daft repo list`](/reference/cli/daft-repo-list),
  [`daft repo info`](/reference/cli/daft-repo-info)
