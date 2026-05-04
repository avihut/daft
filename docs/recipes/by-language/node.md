---
title: daft for Node.js
description:
  Patterns for `package.json`, `node_modules`, npm/pnpm/yarn under daft.
pillars: [worktrees, hooks]
tooling: []
languages: []
---

# daft for Node.js

> **Status: stub.** This recipe is being written. See
> [#398](https://github.com/avihut/daft/issues/398) for status.

## What this recipe will cover

- Picking a package manager (npm, pnpm, yarn) and how each interacts with
  per-worktree `node_modules/`
- Lockfile-per-worktree: how `package-lock.json`, `pnpm-lock.yaml`, and
  `yarn.lock` stay independent across branches
- Per-worktree `node_modules/` vs a shared content-addressable store (pnpm
  global store, yarn PnP)
- Running npm/pnpm/yarn install via a `worktree-post-create` hook so each
  worktree is ready on creation
- Pinning Node versions per branch via a link to the mise or nvm recipe

## Why it matters

JavaScript tooling is sensitive to `node_modules/` state. With daft, each branch
has a fresh, isolated install — no more "it works on main but not on my feature
branch" caused by a stale or partially-upgraded `node_modules/`.

## Where to next

- [Recipes home](/recipes/)
- [Anchor recipe: mise](/recipes/by-tooling/mise)
- [Anchor recipe: direnv](/recipes/by-tooling/direnv)
- [Anchor recipe: monorepo](/recipes/by-scenario/monorepo)
