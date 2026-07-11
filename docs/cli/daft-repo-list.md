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

    daft repo list [--all] [--worktrees] [--columns <cols>] [--format <fmt> | --template <tera>] [--no-headers]

| Flag                | Description                                                                                                                                                                              |
| ------------------- | ---------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------- |
| `-a`, `--all`       | Include removed repositories.                                                                                                                                                            |
| `-w`, `--worktrees` | Expand each repository into a tree of its worktrees — branch and checkout path per line.                                                                                                 |
| `--columns <cols>`  | Columns to display, in the shared grammar: replace mode (`name,path,remote` — exact set and order) or modifier mode (`+size`, `-remote`). Available: annotation, name, worktrees, layout, branch, path, size, remote. |
| `--format <fmt>`    | Structured output: json, ndjson, tsv, csv, yaml, … (with `--worktrees`: json, yaml, toon, markdown).                                                                                     |
| `--template <tera>` | Custom Tera template output.                                                                                                                                                             |
| `--no-headers`      | Omit the header row (tsv/csv only).                                                                                                                                                      |

The size column is opt-in (`--columns +size`, same as `daft list`) because it
walks every repository; on a terminal the sizes stream in live with a total
row while the rest of the table renders immediately. The recorded worktree
layout (`+layout`) and default branch (`+branch`) are likewise opt-in; the
layout is the one recorded in daft's repo store at clone/adopt time (or via
`daft layout set`), shown as `-` for repositories daft never laid out.

With `--worktrees`, each repository expands into its worktrees, one tree line
per worktree: the branch in the Name column, the checkout path in the Path
column. From inside a worktree, the row highlight moves onto that worktree's
line (the repo row keeps the `>` marker). Structured output then nests a
`worktrees` array per repository — `{branch, path}` objects, `branch: null`
for a detached HEAD — in place of the count, which narrows the supported
formats to json, yaml, toon, and markdown.

Paths render relative to your working directory when that form is shorter
(same relativization as `daft list`), falling back to the `~`-abbreviated
absolute path for repositories far from where you stand. Structured output
always carries raw absolute paths.

By default, structured output includes the worktree count (`worktrees`), the
recorded layout, and the recorded default branch; a customized `--columns`
selection narrows the emitted fields to match, and `+size` adds `size_bytes`.

## Examples

    daft repo list
    daft repo list --all
    daft repo list --columns +size
    daft repo list --columns name,remote
    daft repo list --format json | jq '.[].name'

    $ daft repo list
      Name    Worktrees  Path              Remote
    > api     3          ~/src/api         git@github.com:acme/api.git
      webapp  2          ~/src/webapp      git@github.com:acme/webapp.git

    $ daft repo list --worktrees
      Name             Worktrees  Path                  Remote
    > api              3          ~/src/api             git@github.com:acme/api.git
      ├ main                      ~/src/api/main
      ├ feat/login                ~/src/api/feat/login
      └ fix/rate-limit            ~/src/api/fix/rate-limit
      webapp           2          ~/src/webapp          git@github.com:acme/webapp.git
      ├ main                      ~/src/webapp/main
      └ feat/nav                  ~/src/webapp/feat/nav

## See also

- [Repo catalog](/graph/repo-catalog) — the full catalog guide
- [`daft repo add`](/reference/cli/daft-repo-add),
  [`daft repo info`](/reference/cli/daft-repo-info)
