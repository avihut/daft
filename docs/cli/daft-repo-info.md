---
title: daft repo info
description: Show a repository's catalog entry
---

# `daft repo info`

Shows one [repo catalog](/graph/repo-catalog) entry in full: name, status,
location, remote, default branch, recorded worktree layout, the repo's
worktrees as a tree (branch and checkout path per line), and its resolved
[relations](/graph/concepts), each mapped to its local clone or flagged as
not cloned.

The repository may be addressed by catalog name, uuid, or a path. A path
resolves to the repo that encloses it — the repo root, a subdirectory, or any
of its worktrees all work — so `daft repo info .` shows the repo you are
standing in, the same as passing no argument. A catalog name still wins over a
like-named directory; the path form is the fallback. Removed repositories
resolve too.

Paths render relative to your working directory when that form is shorter
(same rule as `daft repo list`). Identity plumbing — uuid, git common dir,
raw canonical paths, registration timestamps — lives in structured output
only: `--format json` carries every recorded field, plus the worktrees as a
`{branch, path}` array (`branch: null` for a detached HEAD, `worktrees:
null` when the repo can't be opened).

## Usage

    daft repo info [<repo>] [--format <fmt> | --template <tera>]

| Argument / flag     | Description                                          |
| ------------------- | --------------------------------------------------- |
| `<repo>`            | Catalog name, uuid, or a repo path (default: cwd repo). |
| `--format <fmt>`    | Structured output: json, yaml, toon, …              |
| `--template <tera>` | Custom Tera template output.                         |

## Examples

    daft repo info
    daft repo info .
    daft repo info ../api-client
    daft repo info client
    daft repo info client --format json | jq .relations

    $ daft repo info api
    Name             api
    Status           live
    Path             ~/src/api
    Remote           git@github.com:acme/api.git
    Default branch   main
    Layout           contained
    Worktrees        3
      ├ main             ~/src/api/main
      ├ feat/login       ~/src/api/feat/login
      └ (detached)       ~/src/api/parked
    Relations
      webapp [service] → ~/src/webapp

## See also

- [Repo catalog](/graph/repo-catalog) — the full catalog guide
- [`daft repo add`](/reference/cli/daft-repo-add),
  [`daft repo list`](/reference/cli/daft-repo-list)
