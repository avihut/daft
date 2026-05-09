---
title: "Anti-pattern: shared mutable state across worktrees"
description:
  Why sharing node_modules, target/, docker volumes, or test DBs across
  worktrees breaks in subtle and unsubtle ways.
pillars: [worktrees, hooks]
---

# Anti-pattern: shared mutable state across worktrees

> Tempting because it saves disk and avoids a re-install. Breaks because two
> worktrees writing to the same place corrupts state — usually silently, usually
> at the worst time.

The pattern that gets hit over and over: a developer notices that two worktrees
both have a 2 GB `node_modules/`, decides to symlink them together, and a week
later spends a day debugging mystery test failures that turn out to be
feature-A's React 18 fighting feature-B's React 19 in the same
`node_modules/.cache/`.

There's a safe sharing story for everything covered here — see
[Sharing caches across worktrees](/recipes/sharing-caches). This page is about
the **unsafe** sharing.

## What people try

### Shared `node_modules/` (npm/yarn/pnpm)

```bash
# In feature-B worktree
ln -s ../feature-a/node_modules .
```

Or via `package.json` config that points node_modules at a parent directory.

### Shared `target/` (cargo)

```bash
# Either of:
export CARGO_TARGET_DIR=~/work/myrepo/target
echo 'target-dir = "/shared/target"' >> .cargo/config.toml
```

### Shared docker volumes

```yaml
# compose.yaml
services:
  postgres:
    volumes:
      - shared-pgdata:/var/lib/postgresql/data
volumes:
  shared-pgdata:
    external: true
    name: my-shared-pgdata
```

### Shared `.venv/`

```bash
# In feature-B worktree
ln -s ../feature-a/.venv .
```

## Why it breaks

**`node_modules/` is mutable, version-pinned, and not concurrency-safe.**
Different worktrees pin different dep versions during `git pull`. Two worktrees
running `pnpm install` (even with `--frozen-lockfile`) at overlapping times can
produce a `node_modules/` that matches neither lockfile. Hot module reload,
Vite's `.vite/` cache, webpack's persistent cache — all assume single-writer.

Symptoms: `Cannot find module` for a package that's clearly installed. Tests
that pass in isolation, fail in CI. "Did I forget to run install?" becomes a
reflex.

**`target/` corruption is silent and hard to diagnose.** Two cargo invocations
with different feature flags — or different `cargo` versions — produce the same
artifact paths but incompatible content. Cargo's incremental compilation reuses
stale artifacts; you get linker errors or, worse, a build that "succeeds" but
the binary segfaults in production.

**Shared docker volumes are race-prone and lose data.** Two worktrees both
running `docker compose up` against an external pgdata volume: both Postgres
containers initialize, both hold WAL locks, both write to disk. Best case: one
container fails to start. Worst case: pg_data corruption that requires a restore
from backup.

**`.venv/` shares broken sys.path between Python processes.** Different
worktrees may have different `pyproject.toml` deps. Python imports the first
matching package; you'll get strange errors when feature-A's code runs against
feature-B's pinned version of a dep.

## What to do instead

Each tool has a **safe** sharing point — see
[Sharing caches across worktrees](/recipes/sharing-caches) for the full table.
The short version:

| Instead of sharing... | Share this                             | Result                                                                    |
| --------------------- | -------------------------------------- | ------------------------------------------------------------------------- |
| `node_modules/`       | pnpm store (`~/.pnpm-store`)           | Per-worktree node_modules, hardlinked into shared content-addressed store |
| `target/`             | sccache (`RUSTC_WRAPPER=sccache`)      | Per-worktree target/, but compilation work is cached across worktrees     |
| `pgdata` volume       | nothing — boot a fresh DB per worktree | Per-worktree state, no contention                                         |
| `.venv/`              | uv cache (`~/.cache/uv`)               | Per-worktree venv, fast install from shared cache                         |

For the postgres case specifically — if you genuinely need a "shared dev
database" and don't want each worktree to have its own state — keep the database
on the host (or in a separate, intentionally-shared container) and connect to it
via `DATABASE_URL`. That's not sharing a volume; it's sharing a service. Fine.
The anti-pattern is sharing the **volume** between two compose stacks.

## Composes well with

- **[Sharing caches across worktrees](/recipes/sharing-caches)** — the positive
  flip side: per-tool answers for safe sharing.
- **[Services with ports](/recipes/services-with-ports)** — the right way to do
  per-worktree services without volume contention.

## See also

- **[Toolchain bootstrap](/recipes/toolchain-bootstrap)** — install patterns
  that avoid the temptation
- **[Background warmup](/recipes/background-warmup)** — sccache recipe (the
  right way to share compiled artifacts)
