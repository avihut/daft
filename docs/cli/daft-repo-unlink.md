---
title: daft repo unlink
description: Remove a relation from this repo
---

# `daft repo unlink`

Removes a directed [relation](/graph/concepts#the-relations-manifest-committed-url-keyed-edges)
declared in the current worktree's `daft.yml` — the inverse of
[`daft repo link`](/reference/cli/daft-repo-link).

The target is matched against existing entries first by friendly label, then by
resolving it (catalog name, repo path, or remote URL) to a remote URL and
matching on that. So `unlink` accepts the same forms as `link`, plus the label
shown by [`daft repo info`](/reference/cli/daft-repo-info).

## Usage

    daft repo unlink <target>

| Argument   | Description                                                    |
| ---------- | ------------------------------------------------------------- |
| `<target>` | Relation label, catalog repo name, repo path, or remote URL.  |

Unlinking an edge that isn't declared is a friendly no-op, not an error. Only
the `relations:` block is touched; the rest of `daft.yml` is left intact.
Commit `daft.yml` to share the change with your team.

## Examples

    daft repo unlink client                          # by relation label
    daft repo unlink api-client                       # by catalog name
    daft repo unlink git@github.com:acme/api-client.git

## See also

- [Relations](/graph/concepts#the-relations-manifest-committed-url-keyed-edges) — the model
- [`daft repo link`](/reference/cli/daft-repo-link) — declare a relation
