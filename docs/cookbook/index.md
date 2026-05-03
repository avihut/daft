---
title: Cookbook
description:
  Recipes for adopting daft alongside your existing tooling, language, or
  scenario.
---

# Cookbook

Recipes for putting daft into practice. Each recipe is task-oriented (here's how
to do X), tagged with which **pillar(s)** it touches and which **tooling** /
**language** / **scenario** it's about.

## Find a recipe

### By tooling

How daft fits with the env-manager you already use.

- **[mise](/cookbook/by-tooling/mise)** — per-worktree tool versions and tasks
  via `mise.toml`
- **[direnv](/cookbook/by-tooling/direnv)** — per-worktree env vars via `.envrc`
- **[nvm](/cookbook/by-tooling/nvm)** — per-worktree Node versions via `.nvmrc`
- **[pyenv](/cookbook/by-tooling/pyenv)** — per-worktree Python versions via
  `.python-version`
- **[asdf](/cookbook/by-tooling/asdf)** — multi-language version management via
  `.tool-versions`

### By language

Per-language patterns for daft adoption.

- **[Node.js](/cookbook/by-language/node)** — `package.json`, `node_modules`,
  npm/pnpm/yarn
- **[Python](/cookbook/by-language/python)** — virtualenvs, requirements, `pip`
  vs `uv`
- **[Rust](/cookbook/by-language/rust)** — `target/` per worktree, `cargo`
  caches
- **[Go](/cookbook/by-language/go)** — `GOPATH`, modules, build cache

### By scenario

Patterns for specific workflow shapes.

- **[Monorepo](/cookbook/by-scenario/monorepo)** — daft in a multi-package
  monorepo
- **[Fork workflow](/cookbook/by-scenario/fork-workflow)** — daft + multi-remote
  for forks
- **[CI integration](/cookbook/by-scenario/ci-integration)** — running daft
  hooks in CI for parity

## Pillar tags

Every recipe lists the **pillar(s)** it touches in its frontmatter:

- `pillars: [worktrees]` — worktree workflow only
- `pillars: [worktrees, hooks]` — worktrees + automation via daft hooks
- `pillars: [hooks]` — hooks-only (rare today; will be common once
  [#468](https://github.com/avihut/daft/issues/468) ships)

## Contributing a recipe

Spot a missing recipe? See [Contributing](/about/contributing).
