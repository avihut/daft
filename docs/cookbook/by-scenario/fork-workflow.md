---
title: Fork workflow
description: daft + multi-remote for fork-based workflows (origin + upstream).
pillars: [worktrees]
tooling: []
languages: []
---

# Fork workflow

> **Status: stub.** This recipe is being written. See
> [#398](https://github.com/avihut/daft/issues/398) for status.

## What this recipe will cover

- Setting up a multi-remote layout with `origin` (your fork) and `upstream` (the
  source project) so daft can create worktrees from either remote's branches
- Fetching upstream branches and creating worktrees off them without making them
  part of your fork's history
- Keeping a `main` worktree synced with upstream while feature worktrees track
  `origin`
- Syncing upstream changes into an active feature branch worktree without
  leaving the worktree
- Using `daft multi-remote` to manage the two-remote layout and inspect which
  remote each worktree tracks

## Why it matters

Fork-based workflows (common for open-source contribution) require juggling two
remotes. daft's multi-remote support lets you keep upstream and origin branches
isolated as separate worktrees — no more stashing, no more branch confusion.

## Where to next

- [Cookbook home](/cookbook/)
- [Anchor recipe: mise](/cookbook/by-tooling/mise)
- [Anchor recipe: direnv](/cookbook/by-tooling/direnv)
- [Anchor recipe: monorepo](/cookbook/by-scenario/monorepo)
