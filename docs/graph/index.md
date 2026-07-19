---
title: Graph
description:
  daft's repo graph coordinates changes across repositories — a catalog of every
  repo on your machine plus a committed manifest of how they relate.
---

# Graph

> **Modern work rarely fits in one repository. The Graph pillar gives daft a
> model of your whole constellation of repos — where each one lives, and how
> they relate — so a change that touches three services can be driven as one
> coherent piece of work.**

Worktrees parallelize development _within_ a repository: every branch gets its
own directory, so nothing blocks on anything else. The graph extends the same
idea _across_ repositories. It has two halves:

- **The repo catalog** — a machine-local registry of every repository daft has
  touched: name, location, remote, default branch, identity. It fills itself:
  cloning, initializing, or running daft commands inside a repo keeps its entry
  current. You never maintain it by hand.
- **The relations manifest** — a committed `relations:` section in `daft.yml`
  declaring which repos this one moves with (a service and its client, a library
  and its consumers). Edges are keyed by remote URL, so the manifest is
  portable: each teammate's daft resolves the same edge to wherever _they_
  cloned that repo.

The catalog answers "where is that repo on this machine"; the manifest answers
"which repos move together". Together they make cross-repo work feel like
single-repo work:

```bash
daft go client                 # jump to another repo's default-branch worktree
daft go client feat/login      # open a specific branch there
daft start client feat/login   # create a new branch over there
daft start feat/login --with-related   # open the same branch across related repos
daft exec --related -- pnpm test       # run a command across that branch everywhere
daft list client               # another repo's worktrees without leaving this one
daft list --all-repos          # one view of every repo's worktrees
```

## How the graph complements worktrees

Worktrees give each branch an isolated directory; the graph decides _which
repositories_ participate in a change. A coordinated change is just the same
branch name checked out as a worktree in each related repo — created together,
navigated with `daft go`, exercised with `daft exec --related`, and finished
with ordinary per-repo pushes. Nothing about git's model changes: repos stay
independent, history stays per-repo, and any repo can be worked on alone.

## What lives where

| Piece              | Location                               | Shared with the team?             |
| ------------------ | -------------------------------------- | --------------------------------- |
| Repo catalog       | daft's data dir (`catalog/catalog.db`) | No — machine-local, self-updating |
| Relations manifest | `relations:` in `daft.yml`             | Yes — committed to the repo       |

## Where to next

- [Concepts](/graph/concepts) — how the catalog and the relations manifest model
  cross-repo dependencies
- [Repo catalog](/graph/repo-catalog) — managing the catalog: `daft repo add`,
  `list`, `info`, removed repos, naming
- [Coordinated changes](/graph/coordinated-changes) — propagating one change
  across several repos, end to end
