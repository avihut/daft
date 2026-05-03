---
title: daft in a monorepo
description:
  Pattern for using daft inside a multi-package monorepo (Nx, Turborepo, pnpm
  workspaces, etc.).
pillars: [worktrees, hooks]
tooling: []
languages: []
---

# daft in a monorepo

> **Goal:** Multiple feature branches active simultaneously inside a monorepo,
> each with its own caches and node_modules / target / venv per branch.

## Context

Monorepos amplify daft's value: a typical "feature" touches one or two packages
out of dozens, and switching branches in a single working tree triggers full
re-installs and cache invalidations. With daft, each branch keeps its own state.

The catch: monorepo caches (`node_modules/`, `pnpm-store/`, `target/`, etc.)
don't fit in `.git/`. They're either per-worktree (more disk, faster swaps) or
shared (less disk, slower invalidations). This recipe walks both.

## Prerequisites

- daft installed; shell integration enabled
- A monorepo using one of: pnpm workspaces, Turborepo, Nx, Bazel, Cargo
  workspaces

## Steps

### 1. Pick a layout

For monorepos, the **contained** layout is usually right — worktrees live as
siblings under a shared parent that holds the `.git/` and any shared tooling.

```bash
daft layout set contained
```

### 2. Decide caches: per-worktree vs shared

**Per-worktree (recommended starting point)** — each worktree has its own
`node_modules/`, `target/`, `.venv/`. Fast branch swaps, more disk usage. No
special config needed; daft's default is per-worktree.

**Shared cache** — single cache that all worktrees use. Less disk usage, but
cache invalidations take down all branches simultaneously. Configure via env
vars or symlinks per the cache's documentation.

### 3. Add a `daft.yml` to install workspace deps on worktree create

For a pnpm workspace:

```yaml
# daft.yml
worktree-post-create:
  jobs:
    - name: install workspace deps
      run: pnpm install --frozen-lockfile
```

For a Turborepo:

```yaml
- name: install + warm
  run: |
    pnpm install --frozen-lockfile
    pnpm turbo run build --filter=...[origin/main] --cache-dir=.turbo-cache
```

(Adjust per your monorepo's tooling.)

Trust:

```bash
git add daft.yml
git commit -m "chore(daft): install workspace deps on worktree create"
git daft-hooks trust
```

### 4. Create a feature branch worktree

```bash
daft start feat/billing
```

The hook installs deps. `cd ~/work/my-project/feat/billing` and start working —
independent of any other branches you have open.

## Verifying it works

```bash
ls node_modules    # exists, populated
pnpm test          # works against this worktree's deps
```

In a sibling worktree (`cd ~/work/my-project/main`), the deps are independent —
installs in one don't affect the other.

## Variations

### Shared `pnpm-store` across worktrees

pnpm uses a content-addressable store; sharing it across worktrees is safe and
saves disk:

```bash
pnpm config set store-dir ~/.pnpm-store
```

Each worktree still has its own `node_modules/`, but the underlying packages are
shared.

### Sparse checkout per worktree

If your monorepo is huge, [#336](https://github.com/avihut/daft/issues/336)
tracks sparse-checkout profile support — define which packages a worktree
includes.

## Troubleshooting

- **Disk fills up fast** — switch to a shared content-addressable store (pnpm)
  or a shared cache (Cargo with `CARGO_TARGET_DIR`).
- **`pnpm install` is slow on every worktree create** — reuse the global pnpm
  store (variation above) and pnpm reuses already-fetched packages.

## Where to next

- **[mise](/cookbook/by-tooling/mise)** — pin Node/pnpm versions in `mise.toml`
- **[Layouts](/worktrees/layouts)** — the contained layout in detail
- **Sparse checkout** — [#336](https://github.com/avihut/daft/issues/336)
  (planned)
