---
title: daft + pyenv
description: Per-worktree Python versions via `.python-version`.
pillars: [worktrees, hooks]
tooling: []
languages: []
---

# daft + pyenv

> **Status: stub.** This recipe is being written. See
> [#398](https://github.com/avihut/daft/issues/398) for status.

## What this recipe will cover

- Install pyenv and add its shell integration to your profile
- Add a `.python-version` file to the repo pinning the Python version per branch
- Add a `worktree-post-create` hook that runs `pyenv install` to fetch the
  pinned version when a new worktree is created
- Verify activation: `python --version` reflects `.python-version` in each
  worktree

## Why it matters

Python projects often pin interpreter versions to match production environments.
With pyenv + daft, each branch carries its `.python-version` file and the hook
ensures the interpreter is present before the developer writes a line of code.

## Where to next

- [Recipes home](/recipes/)
- [Anchor recipe: mise](/recipes/by-tooling/mise)
- [Anchor recipe: direnv](/recipes/by-tooling/direnv)
- [Anchor recipe: monorepo](/recipes/by-scenario/monorepo)
