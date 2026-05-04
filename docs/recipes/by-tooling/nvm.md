---
title: daft + nvm
description: Per-worktree Node versions via `.nvmrc`.
pillars: [worktrees, hooks]
tooling: []
languages: []
---

# daft + nvm

> **Status: stub.** This recipe is being written. See
> [#398](https://github.com/avihut/daft/issues/398) for status.

## What this recipe will cover

- Install nvm and add its shell activation to your profile
- Add a `.nvmrc` file to the repo pinning the Node version for that branch
- Add a `worktree-post-create` hook that runs `nvm install` to pull the pinned
  version automatically
- Verify activation: `node --version` matches `.nvmrc` in every new worktree

## Why it matters

When different branches require different Node versions (e.g. a `main` branch on
Node 20 while a feature branch experiments with Node 22), nvm + daft ensures
each worktree activates the right version without manual `nvm use` calls.

## Where to next

- [Recipes home](/recipes/)
- [Anchor recipe: mise](/recipes/by-tooling/mise)
- [Anchor recipe: direnv](/recipes/by-tooling/direnv)
- [Anchor recipe: monorepo](/recipes/by-scenario/monorepo)
