---
title: Graph concepts
description:
  How daft models cross-repo relationships — the catalog's identity model, the
  relations manifest's URL keying, and how resolution binds them.
---

# Graph concepts

The graph is deliberately split into a machine-local half (the catalog) and a
committed half (the relations manifest). This page explains each model and how
resolution ties them together.

## The catalog: identity, location, lifecycle

Every repository daft touches gets a catalog entry:

| Field          | Meaning                                                              |
| -------------- | -------------------------------------------------------------------- |
| identity       | The repo's `daft-id` (a UUID created on first contact)               |
| name           | Short handle used by `daft go <repo>`, `--repo`, `daft clone <name>` |
| path           | Project root on this machine                                         |
| remote URL     | As configured; also stored in normalized form for matching           |
| default branch | Where bare `daft go <repo>` lands                                    |
| removed        | Tombstone timestamp — see below                                      |

Identity lives in the repository's git directory (`daft-id`), so it survives
moves and dies with the repo; the catalog is the index that survives _outside_
the repo. That asymmetry drives the lifecycle rules:

- **Registration is ambient.** Clone, init, adopt, and everyday commands running
  inside a repo upsert its entry. `daft repo add` exists only for repositories
  daft has never operated in, and for renaming (`--name`).
- **Names are unique among live entries.** Two clones that would derive the same
  name get suffixed (`api`, `api-2`) with a notice; rename with
  `daft repo add --name`.
- **Removal is a tombstone, not a deletion.** `daft repo remove` marks the entry
  removed and keeps it: the repo's hook-job logs stay addressable
  (`daft hooks jobs --repo <name>`) and `daft clone <name>` restores the repo
  from its recorded remote. Re-cloning at the same path creates a fresh identity
  that takes over the live name; the old identity stays as a removed entry.

## The relations manifest: committed, URL-keyed edges

Relations are declared in `daft.yml`, next to hooks:

```yaml
relations:
  - url: git@github.com:acme/api-client.git
    name: client # optional friendly label
    kind: consumer # optional, free-form
```

Three properties are load-bearing:

- **Keyed by remote URL, not by path or name.** Paths and catalog names are
  machine-local; the remote URL is the one identity every teammate shares. URL
  forms are normalized before matching, so `git@github.com:acme/x.git`,
  `ssh://git@github.com/acme/x`, and `https://github.com/acme/x` all name the
  same repo.
- **Directed.** Declaring the client in the service's manifest does not make the
  service appear in the client's. If both sides drive coordinated changes,
  declare both edges.
- **Open semantics.** `kind` is a free-form label (`client`, `library`,
  `deploy`, …) for humans and tooling to grow into; daft does not interpret it.

## Resolution: manifest → catalog → path

When a command needs a related repo (`daft exec --related`,
`daft start --with-related`, `daft repo info`), each manifest edge resolves in
one step: normalize the edge's URL and look it up among the catalog's live
entries' normalized remotes. A hit yields the local path — wherever that
teammate happened to clone the repo. A miss means "not cloned here", and daft
says so with the exact `daft clone <url>` to fix it.

The same live-first rule applies everywhere the catalog resolves anything: live
entries win over removed ones, and within `daft go`, anything resolvable in the
_current_ repository (worktrees, local branches, remote branches) always wins
over a catalog repo of the same name — `--repo` addresses a shadowed repo
explicitly.

## Where to next

- [Repo catalog](/graph/repo-catalog) — the day-to-day commands
- [Coordinated changes](/graph/coordinated-changes) — the workflow the model
  exists for
