---
title: CI integration
description: Running daft hooks in CI for parity with local checks.
pillars: [hooks]
tooling: []
languages: []
---

# CI integration

> **Status: stub.** This recipe is being written. See
> [#398](https://github.com/avihut/daft/issues/398) for status.

## What this recipe will cover

- Running daft `worktree-post-create` hooks in GitHub Actions, CircleCI, and
  GitLab CI as part of a setup step, so CI performs the same environment
  bootstrapping as a local worktree creation
- Environment parity: ensuring CI sees the same hook output (tool versions, env
  vars, installed deps) as a developer's local worktree
- Caching hook trust state in CI: how to pre-trust `daft.yml` so hook runs are
  not blocked waiting for interactive confirmation
- When CI hooks should differ from local hooks: for example, skipping
  `direnv allow` or using CI-specific secret injection in the post-create job
- Debugging hook failures in CI: reading `git daft-hooks log show` output from
  CI artifact uploads

## Why it matters

Hooks encode your project's setup contract. Running them in CI catches
environment drift before it reaches production — and means onboarding a new
developer is as simple as creating a worktree.

## Where to next

- [Cookbook home](/cookbook/)
- [Anchor recipe: mise](/cookbook/by-tooling/mise)
- [Anchor recipe: direnv](/cookbook/by-tooling/direnv)
- [Anchor recipe: monorepo](/cookbook/by-scenario/monorepo)
