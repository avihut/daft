---
title: Why daft
description:
  daft helps you parallelize development through isolation, and coordinate
  changes across the repo graph.
---

# Why daft

daft is built on one thesis:

> **Parallelize development through isolation; coordinate across the repo
> graph.**

The first half — parallelize through isolation — is worktrees and hooks. The
second half — coordinate across repos — is the [Graph pillar](/graph/): a
self-maintaining repo catalog plus a committed manifest of cross-repo
relationships.

## The problem

Modern dev work is often blocked by serialization that doesn't have to exist:

- You can't work on feature A and feature B simultaneously because they share a
  working tree
- Switching branches restarts builds, dev servers, file watchers
- `git stash` is a sharp tool that loses work when used carelessly
- A bug fix can't share a working tree with the feature you were working on
- Different branches need different env vars, runtime versions, secrets — and
  `.envrc` doesn't follow your branch

These are all symptoms of one root cause: a single working directory that flips
between branches.

## The shape of the solution

Three pillars, each idempotent — you can adopt one without the others.

- **[Worktrees](/worktrees/)**: every branch gets its own directory. No
  flipping. No stashing. Run `feature-A` and `feature-B` in different terminals
  at the same time.
- **[Hooks](/hooks/)**: declarative automation at every code-evolution boundary.
  Local equivalent of GitHub Actions, but enforced before code leaves your
  machine.
- **[Graph](/graph/)**: coordinate changes across the repo graph. A repo catalog
  plus a manifest of cross-repo relationships, so a change that touches three
  services can be propagated coherently.

The pillars are loosely coupled. A user who only wants worktrees never has to
learn hooks. A user who only wants hooks doesn't need to adopt worktrees (once
the [full git-hooks drop-in](https://github.com/avihut/daft/issues/468) ships).

## When daft is the right tool

- You frequently switch contexts and lose flow because of it
- Your branches need different env vars, runtime versions, or services running
  locally
- You want CI-style gates running before code leaves your machine, not after
- You work in a polyrepo where changes naturally span multiple repos

## When daft is not the right tool

- You only ever work on one branch at a time and never context-switch (rare in
  practice — but if it's you, daft adds setup cost without value)
- You need worktree-aware features inside an IDE that doesn't support multi-root
  projects (technically still usable but rougher)
- You need to deploy on Windows-only environments where shell-integration is
  awkward (works, but the ergonomics are rougher)

## Where to start

- **[Quick Start](/getting-started/quick-start)** — a Tutorial that walks the
  worktree adoption arc
- **[Worktrees](/worktrees/)** — the foundation pillar
- **[Recipes](/recipes/)** — recipes for adopting daft alongside your existing
  tooling
