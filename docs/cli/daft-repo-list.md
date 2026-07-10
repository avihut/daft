---
title: daft repo list
description: List repositories in the repo catalog
---

# `daft repo list`

Lists the repositories in daft's [repo catalog](/graph/repo-catalog): name,
default branch, and path. Removed repositories keep a catalog entry (their
job logs stay addressable and `daft clone <name>` restores them); show them
with `--all`.

## Usage

    daft repo list [--all] [--format <fmt> | --template <tera>] [--no-headers]

| Flag                | Description                                        |
| ------------------- | -------------------------------------------------- |
| `-a`, `--all`       | Include removed repositories.                      |
| `--format <fmt>`    | Structured output: json, ndjson, tsv, csv, yaml, … |
| `--template <tera>` | Custom Tera template output.                       |
| `--no-headers`      | Omit the header row (tsv/csv only).                |

## Examples

    daft repo list
    daft repo list --all
    daft repo list --format json | jq '.[].name'

## See also

- [Repo catalog](/graph/repo-catalog) — the full catalog guide
- [`daft repo add`](/reference/cli/daft-repo-add),
  [`daft repo info`](/reference/cli/daft-repo-info)
