---
title: daft for Python
description: Patterns for virtualenvs, requirements, `pip` and `uv` under daft.
pillars: [worktrees, hooks]
tooling: []
languages: []
---

# daft for Python

> **Status: stub.** This recipe is being written. See
> [#398](https://github.com/avihut/daft/issues/398) for status.

## What this recipe will cover

- Creating a virtualenv per worktree (`.venv/` in the worktree root) so each
  branch has isolated Python dependencies
- Lockfile options: `requirements.txt`, `poetry.lock`, and `uv.lock` — how each
  maps to per-worktree installs
- Adding a `worktree-post-create` hook to create the virtualenv and install deps
  automatically on branch creation
- Activating the virtualenv in a new shell: sourcing `.venv/bin/activate` vs
  auto-activation via direnv or mise
- Pinning the Python interpreter version per branch — link to the pyenv recipe

## Why it matters

Python virtualenvs are directory-local by convention, which maps cleanly onto
daft worktrees. The common friction point is manual activation; hooks and direnv
eliminate it.

## Where to next

- [Recipes home](/recipes/)
- [Anchor recipe: mise](/recipes/by-tooling/mise)
- [Anchor recipe: direnv](/recipes/by-tooling/direnv)
- [Anchor recipe: monorepo](/recipes/by-scenario/monorepo)
