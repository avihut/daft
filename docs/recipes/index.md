---
title: Recipes
description:
  Patterns and walkthroughs for daft's lifecycle automation — toolchain
  bootstrap, services per worktree, secrets, cleanup on remove.
---

# Recipes

Recipes turn daft's idea — **lifecycle automation per worktree** — into
real-world setup. Three kinds of pages here:

- **Adoption recipes** — introducing daft into an existing project setup, or
  migrating off the manual rituals you have today.
- **Walkthroughs** — full project shapes threading patterns end-to-end.
- **Patterns** — atomic problems each solved once; combine as needed.

<script setup>
import { ref, computed, onMounted } from 'vue'
import { data as recipes } from '../.vitepress/data/recipes.data.ts'

const ADOPTION_LINKS = [
  '/recipes/adopting-from-direnv',
  '/recipes/adopting-from-mise',
  '/recipes/walkthroughs/migrating-from-setup-sh',
]

const pillar = ref(null)

onMounted(() => {
  const params = new URLSearchParams(window.location.search)
  pillar.value = params.get('pillar')
})

const visible = computed(() => {
  if (!pillar.value) return recipes
  return recipes.filter(r => r.pillars.includes(pillar.value))
})

const adoption = computed(() => visible.value.filter(r => ADOPTION_LINKS.includes(r.link)))
const walkthroughs = computed(() => visible.value.filter(r => r.kind === 'walkthrough' && !ADOPTION_LINKS.includes(r.link)))
const patterns = computed(() => visible.value.filter(r => r.kind === 'pattern' && !ADOPTION_LINKS.includes(r.link)))
const references = computed(() => visible.value.filter(r => r.kind === 'reference' || r.kind === 'anti-pattern'))
</script>

<template v-if="pillar">
  <p>Showing recipes tagged <code>{{ pillar }}</code>:</p>
</template>

## Adoption

Recipes for introducing daft into an existing setup, or for moving off the
manual rituals you have today. Start here if you're new to daft.

<ul>
  <li v-for="r in adoption" :key="r.link">
    <a :href="r.link">{{ r.title }}</a> — {{ r.description }}
  </li>
</ul>

## Walkthroughs

End-to-end recipes for full project shapes. Read the one closest to what you're
building, then drop into the patterns it cites for variants.

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
  <li v-for="r in patterns.filter(p => ['/recipes/toolchain-bootstrap', '/recipes/background-warmup', '/recipes/env-vars-and-secrets', '/recipes/services-with-ports', '/recipes/editor-integration'].includes(p.link))" :key="r.link">
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
