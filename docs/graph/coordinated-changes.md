---
title: Coordinated changes
description:
  Propagating one change across several repositories with relations, start
  --with-related, cross-repo go, and exec --related.
---

# Coordinated changes

A coordinated change is the same branch, carried through every repository the
change touches. This page walks the full loop for a service and its client; the
same shape scales to any set of related repos.

## 1. Declare the relation (once)

In the service's `daft.yml`:

```yaml
relations:
  - url: git@github.com:acme/api-client.git
    name: client
    kind: consumer
```

Commit it — the manifest is team-shared. Edges are directed; if the client's
team also drives coordinated changes from their side, they declare the service
in their `daft.yml` the same way.

Every related repo must be cloned locally (daft tells you the exact
`daft clone <url>` if one isn't).

## 2. Open the branch everywhere

```bash
daft start feat/rename-field --with-related
```

This creates `feat/rename-field` — worktree, branch, upstream — in the current
repo **and** in every related repo, each based on that repo's own default
branch. Rules that keep the fan-out safe:

- Resolution happens up front: a missing clone aborts before anything is
  created.
- `--carry` and `-x` apply only to the current repo.
- Hooks run in a related repo only when that repo is explicitly trusted — a
  fan-out never stops to ask.
- You land in the current repo's new worktree; per-repo failures are reported at
  the end rather than cascading.

When only one other repo needs the branch, skip the fan-out and target it
directly — `daft start <repo> <branch>` creates it over there (based on that
repo's default branch) and lands you in the new worktree:

```bash
daft start client feat/rename-field
```

Combining both forms roots the fan-out at the target:
`daft start client feat/rename-field --with-related` creates the branch in
`client` and in the repos _client's_ manifest declares.

## 3. Work across the set

Hop between the repos' worktrees as if they were one project:

```bash
daft go client                     # client's default-branch worktree
daft go client feat/rename-field   # the coordinated branch over there
daft go -                          # and back
```

Run anything across the branch's worktrees in every repo that has it:

```bash
daft exec --related -- pnpm test
daft exec --related -- git status -sb
```

`--related` follows your _current_ branch: it targets each related repo's
worktree for that branch and skips (with a notice) repos that don't carry it.
Output rows are labeled `repo:branch` so interleaved results stay readable.

## 4. Finish per repo

Coordinated changes end the ordinary way — each repo gets its own push, PR,
review, and merge. daft does not entangle git histories across repositories; the
graph coordinates your working state, not your remotes.

```bash
daft exec --related -- git push
```

Sequence the merges the way your dependency direction requires (for a
service+client pair: usually service first, then the client that consumes it).

## Variations

- **One-off cross-repo errand** — no manifest needed:
  `daft exec --repo client -- pnpm build`, or `daft go client fix/typo` to open
  a branch there directly.
- **Fleet-wide sweeps** — `--all-repos` targets every cataloged repo's
  default-branch worktree instead of following a branch:
  `daft exec --all-repos -- git fetch --prune`.

## Where to next

- [Repo catalog](/graph/repo-catalog) — the registry these commands resolve
  against
- [Graph concepts](/graph/concepts) — why relations are URL-keyed and directed
