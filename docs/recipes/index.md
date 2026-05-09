---
title: Recipes
description:
  Patterns and walkthroughs for daft's lifecycle automation — toolchain
  bootstrap, services per worktree, secrets, cleanup on remove.
---

# Recipes

Recipes are how daft's idea — **lifecycle automation per worktree** — turns into
real-world setup. Patterns are atomic problems each solved once; walkthroughs
are real project shapes that thread patterns end-to-end. Pick the patterns you
need, or read a walkthrough that matches your stack.

## Walkthroughs

End-to-end recipes for full project shapes. Read the one closest to what you're
building, then drop into the patterns it cites for variants.

<script setup>
import { ref, computed, onMounted } from 'vue'
import { data as recipes } from '../.vitepress/data/recipes.data.ts'

const pillar = ref(null)

onMounted(() => {
  const params = new URLSearchParams(window.location.search)
  pillar.value = params.get('pillar')
})

const visible = computed(() => {
  if (!pillar.value) return recipes
  return recipes.filter(r => r.pillars.includes(pillar.value))
})

const walkthroughs = computed(() => visible.value.filter(r => r.kind === 'walkthrough'))
const patterns = computed(() => visible.value.filter(r => r.kind === 'pattern'))
const references = computed(() => visible.value.filter(r => r.kind === 'reference' || r.kind === 'anti-pattern'))
</script>

<template v-if="pillar">
  <p>Showing recipes tagged <code>{{ pillar }}</code>:</p>
</template>

<ul>
  <li v-for="r in walkthroughs" :key="r.link">
    <a :href="r.link">{{ r.title }}</a> — {{ r.description }}
  </li>
</ul>

## Patterns

Atomic problems, one recipe per problem. Combine them as your project needs.

### Setup

Lifecycle stage: `worktree-post-create`. Get a fresh worktree from "empty
checkout" to "ready to run a command."

<ul>
  <li v-for="r in patterns.filter(p => ['/recipes/toolchain-bootstrap', '/recipes/background-warmup', '/recipes/env-vars-and-secrets', '/recipes/services-with-ports', '/recipes/adopting-from-direnv', '/recipes/adopting-from-mise', '/recipes/editor-integration'].includes(p.link))" :key="r.link">
    <a :href="r.link">{{ r.title }}</a> — {{ r.description }}
  </li>
</ul>

### Steady state

Patterns that span the worktree's full life — declarative env activation on
`cd`, running the same hooks in CI for parity.

<ul>
  <li v-for="r in patterns.filter(p => ['/recipes/declarative-envs', '/recipes/ci-parity'].includes(p.link))" :key="r.link">
    <a :href="r.link">{{ r.title }}</a> — {{ r.description }}
  </li>
</ul>

### Teardown

Lifecycle stage: `worktree-pre-remove`. Anything the create hook brought into
existence needs a way back out.

<ul>
  <li v-for="r in patterns.filter(p => p.link === '/recipes/cleanup-on-remove')" :key="r.link">
    <a :href="r.link">{{ r.title }}</a> — {{ r.description }}
  </li>
</ul>

## References

Background reading and what-not-to-do — useful alongside the patterns.

<ul>
  <li v-for="r in references" :key="r.link">
    <a :href="r.link">{{ r.title }}</a> — {{ r.description }}
  </li>
</ul>

## Pillar tags

Every recipe lists the **pillar(s)** it touches in its frontmatter. Filter this
page with `?pillar=worktrees` or `?pillar=hooks` to see only recipes relevant to
one pillar.

- `pillars: [worktrees, hooks]` — the common case (most patterns combine
  worktree-aware setup with hook-driven automation).
- `pillars: [hooks]` — hook-shaped concerns that aren't worktree-specific (CI
  parity).

## Contributing a recipe

Spot a missing recipe? See [Contributing](/about/contributing).
