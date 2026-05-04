---
title: Hooks roadmap
description: Hook stages that are designed but not yet shipped.
---

# Hooks roadmap

Two hook stages are part of the [boundaries thesis](/hooks/) but not yet
shipped. They are tracked as feature issues, with their docs landing in the same
PR as the feature (per "docs and features enter together").

## Commit hooks (full git-hooks drop-in)

**Tracking:** [#468](https://github.com/avihut/daft/issues/468)

Lefthook-style drop-in: `pre-commit`, `commit-msg`, `prepare-commit-msg`,
`pre-push`, `post-commit`, `pre-rebase`. The "progressive code-replication
boundary" — format, lint, fast tests gate every commit.

When this ships, daft becomes a viable lefthook replacement. Recipes for the
migration will live under [Recipes → By tooling → lefthook → daft](/recipes/)
once written.

## Merge hooks

**Tracking:** [#330](https://github.com/avihut/daft/issues/330)

`pre-merge` and `post-merge` hooks fire around `daft merge` /
`daft worktree-merge`. The "PR-check-parity" boundary — full tests, integration,
security gates before code leaves an isolated branch.

This is the merge feature itself, currently in flight. Hook docs land in the
same PR.

## Why these aren't shipped yet

The IA exists today (this pillar, this Overview, this roadmap page) so the
conceptual frame can be in place. The features are sequenced after the IA itself
stabilizes — see [#398](https://github.com/avihut/daft/issues/398) for context.
