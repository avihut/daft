---
title: daft repo info
description: Show a repository's catalog entry
---

# `daft repo info`

Shows one [repo catalog](/graph/repo-catalog) entry in full: name, status,
location, remote, default branch, identity — plus the repo's resolved
[relations](/graph/concepts), each mapped to its local clone or flagged as
not cloned.

The repository may be addressed by catalog name, path, or uuid; with no
argument the repo containing the current directory is shown. Removed
repositories resolve too.

## Usage

    daft repo info [<repo>] [--format <fmt> | --template <tera>]

| Argument / flag     | Description                                       |
| ------------------- | ------------------------------------------------- |
| `<repo>`            | Catalog name, path, or uuid (default: cwd repo).  |
| `--format <fmt>`    | Structured output: json, yaml, toon, …            |
| `--template <tera>` | Custom Tera template output.                      |

## Examples

    daft repo info
    daft repo info client
    daft repo info client --format json | jq .relations

## See also

- [Repo catalog](/graph/repo-catalog) — the full catalog guide
- [`daft repo add`](/reference/cli/daft-repo-add),
  [`daft repo list`](/reference/cli/daft-repo-list)
