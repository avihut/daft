---
title: Networking roadmap
description: Cross-repo coordination is daft's third pillar — in design.
---

# Networking roadmap

> **Status: in design.** Tracking issue:
> [#357](https://github.com/avihut/daft/issues/357).

The Networking pillar is the second half of daft's thesis (the first half —
parallel dev via isolation — is what worktrees + hooks deliver today):

> **Coordinate changes across repos.**

## The problem

Polyrepo development means a change often spans multiple repos: a service and
its client, a library and its consumers, a monorepo of microservices. Today, the
coordination is manual:

- You clone N repos by hand
- You apply N versions of a related change by hand
- You track N PRs across N repos in a spreadsheet
- You cherry-pick a refactor across N repos because there's no shared
  abstraction

Networking is daft's surface for that.

## The shape of the solution

Two pieces:

1. **A repo catalog** — a daft-managed registry of repos on your machine, with
   their layout, default branch, and identity.
2. **A relations manifest** — a per-repo declaration of "this repo depends on
   these others" / "this repo is a sibling of those others." Stored in
   `daft.yml` (or similar; design pending).

With those, daft can:

- Clone the closure of a repo and its declared dependencies
- Propagate a related change across repos (start matched feature branches, run
  merge gates across the closure)
- Surface "stale" repos in the catalog (haven't synced in N days)
- Coordinate releases across a service+client pair

## When this ships

This page goes away. Its content migrates to `networking/index.md` as the third
pillar's Overview, and the top nav adds a "Networking" entry between "Hooks" and
"Cookbook." See
[#398's coordination notes](https://github.com/avihut/daft/issues/398) for how
docs land alongside the feature.
