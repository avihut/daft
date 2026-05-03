---
title: daft + asdf
description: Multi-language version management via `.tool-versions`.
pillars: [worktrees, hooks]
tooling: []
languages: []
---

# daft + asdf

> **Status: stub.** This recipe is being written. See
> [#398](https://github.com/avihut/daft/issues/398) for status.

## What this recipe will cover

- Install asdf and add its shell integration to your profile
- Add a `.tool-versions` file to the repo listing all language versions for that
  branch (Node, Python, Ruby, Elixir, etc.)
- Add a `worktree-post-create` hook that runs `asdf install` to install all
  plugins and versions listed in `.tool-versions`
- Verify activation: `asdf current` shows the expected versions in a fresh
  worktree

## Why it matters

asdf manages multiple runtimes from a single tool. Teams that use more than one
language in a repo get per-worktree version isolation across all of them without
switching between nvm, pyenv, rbenv, and friends.

## Where to next

- [Cookbook home](/cookbook/)
- [Anchor recipe: mise](/cookbook/by-tooling/mise)
- [Anchor recipe: direnv](/cookbook/by-tooling/direnv)
- [Anchor recipe: monorepo](/cookbook/by-scenario/monorepo)
