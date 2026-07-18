---
title: daft repo link
description: Declare a relation from this repo to another
---

# `daft repo link`

Declares a directed [relation](/graph/concepts#the-relations-manifest-committed-url-keyed-edges)
from the current repo to another, writing a well-formed entry into the current
worktree's `daft.yml` `relations:` list. Relations power
[`daft exec --related`](/reference/cli/daft-exec),
`daft start --with-related`, and the
[`daft repo info`](/reference/cli/daft-repo-info) Relations section.

The target is resolved to a remote URL — the portable key relations match on —
in this order: a catalog repo name (or a repo path daft has cataloged), then a
path to a git repo on disk, then a remote URL used as-is. A URL that isn't
cloned yet is fine; the edge resolves as "not cloned" until it is.

## Usage

    daft repo link <target> [--name <label>] [--kind <kind>]

| Argument / flag   | Description                                                       |
| ----------------- | ---------------------------------------------------------------- |
| `<target>`        | Catalog repo name, a repo path, or a remote URL to link to.      |
| `--name <label>`  | Friendly label for the edge (defaults to the URL's last segment). |
| `--kind <kind>`   | Free-form relationship kind (e.g. `consumer`, `library`).        |

Linking is idempotent: re-linking an existing edge is a no-op, and `--name` or
`--kind` update that edge in place. Edges are deduped by normalized URL, so
`git@…`, `https://…`, and `.git`/no-`.git` forms collapse to one. Only the
`relations:` block is edited — comments and formatting elsewhere in `daft.yml`
are preserved. Linking a repo to itself is refused. The manifest is committed;
commit `daft.yml` to share the relation with your team.

## Examples

    daft repo link api-client                       # by catalog name
    daft repo link ../api-client                     # by repo path
    daft repo link git@github.com:acme/api-client.git --name client --kind consumer

## See also

- [Relations](/graph/concepts#the-relations-manifest-committed-url-keyed-edges) — the model
- [`daft repo unlink`](/reference/cli/daft-repo-unlink) — remove a relation
- [Coordinating a service and its client](/recipes/coordinating-service-and-client)
