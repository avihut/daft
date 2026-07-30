---
title: Hooks roadmap
description: Recently shipped hook stages, and where the pillar goes next.
---

# Hooks roadmap

Every stage in the [boundaries thesis](/hooks/) now ships. This page records
what landed most recently and what is still open.

## Recently shipped

- **Git stages** (`pre-commit`, `commit-msg`, `pre-push`, and thirteen more) —
  the progressive code-replication boundary, and with it daft as a full git
  hooks manager. See [Git stages](/hooks/git-stages), and
  [Migrating from lefthook](/hooks/lefthook-migration) if you are coming from
  one. ([#468](https://github.com/avihut/daft/issues/468))
- **Merge hooks** (`pre-merge` / `post-merge`) — the PR-check-parity boundary.
  See [Lifecycle hooks → Merge hooks](/hooks/lifecycle#merge-hooks).

## Known gaps

- **Batched sync pushes and staged gates.** `daft.sync.pushHookStrategy batched`
  sends every branch in one push, which cannot carry a per-ref staged gate. When
  daft manages `pre-push`, sync falls back to per-branch and says so.
- **Server-side and protocol hooks** are deliberately out of scope; see
  [Git stages](/hooks/git-stages) for the reasoning.
