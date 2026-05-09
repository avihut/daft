---
title: Hooks roadmap
description: Hook stages that are designed but not yet shipped.
---

# Hooks roadmap

One hook stage from the [boundaries thesis](/hooks/) is not yet shipped. It is
tracked as a feature issue, with its docs landing in the same PR as the feature
(per "docs and features enter together").

## Commit hooks (full git-hooks drop-in)

**Tracking:** [#468](https://github.com/avihut/daft/issues/468)

Lefthook-style drop-in: `pre-commit`, `commit-msg`, `prepare-commit-msg`,
`pre-push`, `post-commit`, `pre-rebase`. The "progressive code-replication
boundary" — format, lint, fast tests gate every commit.

When this ships, daft becomes a viable lefthook replacement. Recipes for the
migration will live under [Recipes → By tooling → lefthook → daft](/recipes/)
once written.

## Recently shipped

- **Merge hooks** (`pre-merge` / `post-merge`) — the PR-check-parity boundary.
  See [Lifecycle hooks → Merge hooks](/hooks/lifecycle#merge-hooks) for the full
  reference.

## Why commit hooks aren't shipped yet

The IA exists today (this pillar, this Overview, this roadmap page) so the
conceptual frame can be in place. The remaining commit-stage work is sequenced
after the IA itself stabilizes — see
[#398](https://github.com/avihut/daft/issues/398) for context.
