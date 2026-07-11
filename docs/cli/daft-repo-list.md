---
title: daft repo list
description: List repositories in the repo catalog
---

# `daft repo list`

Lists the repositories in daft's [repo catalog](/graph/repo-catalog): name,
worktree count, path, and remote. The repo you are standing in is marked
with `>`. Removed repositories keep a catalog entry (their job logs stay
addressable and `daft clone <name>` restores them); show them with `--all` —
they render dimmed with a `(removed)` note.

## Usage

    daft repo list [--all] [--sizes] [--format <fmt> | --template <tera>] [--no-headers]

| Flag                | Description                                                                                                                                                                              |
| ------------------- | ---------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------- |
| `-a`, `--all`       | Include removed repositories.                                                                                                                                                            |
| `--sizes`           | Add a disk-usage column with a total row. Walks every repository, so it is opt-in (like `daft list --columns +size`); on a terminal the sizes stream in live while the table renders immediately. |
| `--format <fmt>`    | Structured output: json, ndjson, tsv, csv, yaml, …                                                                                                                                       |
| `--template <tera>` | Custom Tera template output.                                                                                                                                                             |
| `--no-headers`      | Omit the header row (tsv/csv only).                                                                                                                                                      |

Structured output always includes the worktree count (`worktrees`) and the
recorded default branch; `--sizes` adds `size_bytes`.

## Examples

    daft repo list
    daft repo list --all
    daft repo list --sizes
    daft repo list --format json | jq '.[].name'

    $ daft repo list
      Name    Worktrees  Path              Remote
    > api     3          ~/src/api         git@github.com:acme/api.git
      webapp  2          ~/src/webapp      git@github.com:acme/webapp.git

## See also

- [Repo catalog](/graph/repo-catalog) — the full catalog guide
- [`daft repo add`](/reference/cli/daft-repo-add),
  [`daft repo info`](/reference/cli/daft-repo-info)
