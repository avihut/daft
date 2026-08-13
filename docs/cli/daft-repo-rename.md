---
title: daft repo rename
description: Rename a repository's catalog entry
---

# `daft repo rename`

Changes the name a repository answers to in the
[repo catalog](/graph/repo-catalog) — the name taken by `daft go <repo>`,
`daft list <repo>` and `--repo <name>`.

Nothing on disk changes: the directory keeps its name and no worktree moves.
To relocate the repository itself, use [`daft repo move`](/cli/daft-repo-move),
which can rename the catalog entry in the same operation with `--name`.

Keeping the two verbs separate is the point. Renaming a catalog entry is
metadata and costs nothing; renaming a directory invalidates your shell, your
editor windows, and anything holding the old absolute path.

## Usage

    daft repo rename <repo> <new-name> [-q | -v]

| Argument / flag | Description                                          |
| --------------- | ---------------------------------------------------- |
| `<repo>`        | Repository to rename: a catalog name, uuid, or path. |
| `<new-name>`    | The name it should answer to.                        |
| `-q`            | Suppress progress reporting.                         |
| `-v`            | Show detailed progress.                              |

## Behavior

- The repository has to be in the catalog: renaming is a catalog operation, so
  unlike `daft repo move` there is no filesystem fallback.
- Names must be unique among live entries. A name another repository already
  holds is refused, with a pointer at renaming that one first.
- This is the same operation as `daft repo add --name <name>`, which is where
  it lived before. Nobody looks for a rename under `add`, which is why this
  verb exists.

Not to be confused with [`daft rename`](/cli/daft-rename), which renames a
*worktree* and its branch — the same way `daft repo remove` sits beside
`daft remove`.

## Examples

    # Rename a cataloged repository
    daft repo rename api api-gateway

    # Rename the repository containing the current directory
    daft repo rename . backend
