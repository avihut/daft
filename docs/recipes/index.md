---
title: Recipes
description:
  Patterns for adopting daft alongside your existing tooling, language, or
  scenario.
---

# Recipes

Patterns for putting daft into practice. Each recipe is task-oriented (here's
how to do X), tagged with which **pillar(s)** it touches and which **tooling** /
**language** / **scenario** it's about.

## Find a recipe

### By tooling

How daft fits with the env-manager you already use.

- **[mise](/recipes/by-tooling/mise)** — per-worktree tool versions and tasks
  via `mise.toml`
- **[direnv](/recipes/by-tooling/direnv)** — per-worktree env vars via `.envrc`
- **[nvm](/recipes/by-tooling/nvm)** — per-worktree Node versions via `.nvmrc`
- **[pyenv](/recipes/by-tooling/pyenv)** — per-worktree Python versions via
  `.python-version`
- **[asdf](/recipes/by-tooling/asdf)** — multi-language version management via
  `.tool-versions`

### By language

Per-language patterns for daft adoption.

- **[Node.js](/recipes/by-language/node)** — `package.json`, `node_modules`,
  npm/pnpm/yarn
- **[Python](/recipes/by-language/python)** — virtualenvs, requirements, `pip`
  vs `uv`
- **[Rust](/recipes/by-language/rust)** — `target/` per worktree, `cargo` caches
- **[Go](/recipes/by-language/go)** — `GOPATH`, modules, build cache

### By scenario

Patterns for specific workflow shapes.

- **[Monorepo](/recipes/by-scenario/monorepo)** — daft in a multi-package
  monorepo
- **[Fork workflow](/recipes/by-scenario/fork-workflow)** — daft + multi-remote
  for forks
- **[CI integration](/recipes/by-scenario/ci-integration)** — running daft hooks
  in CI for parity

## Pillar tags

Every recipe lists the **pillar(s)** it touches in its frontmatter:

- `pillars: [worktrees]` — worktree workflow only
- `pillars: [worktrees, hooks]` — worktrees + automation via daft hooks
- `pillars: [hooks]` — hooks-only (rare today; will be common once
  [#468](https://github.com/avihut/daft/issues/468) ships)

## Contributing a recipe

Spot a missing recipe? See [Contributing](/about/contributing).

## Filtered recipes

<script setup>
import { ref, computed, onMounted } from 'vue'
import { data as recipes } from '../.vitepress/data/recipes.data.ts'

const pillar = ref(null)

onMounted(() => {
  const params = new URLSearchParams(window.location.search)
  pillar.value = params.get('pillar')
})

const filtered = computed(() => {
  if (!pillar.value) return []
  return recipes.filter(r => r.pillars.includes(pillar.value))
})
</script>

<template v-if="pillar">
  <p>Showing recipes tagged <code>{{ pillar }}</code>:</p>
  <ul>
    <li v-for="r in filtered" :key="r.link">
      <a :href="r.link">{{ r.title }}</a> — {{ r.description }}
    </li>
  </ul>
  <p v-if="filtered.length === 0">No recipes yet for this pillar.</p>
</template>
