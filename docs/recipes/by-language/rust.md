---
title: daft for Rust
description:
  Patterns for `target/`, `cargo` caches, and incremental builds under daft.
pillars: [worktrees, hooks]
tooling: []
languages: []
---

# daft for Rust

> **Status: stub.** This recipe is being written. See
> [#398](https://github.com/avihut/daft/issues/398) for status.

## What this recipe will cover

- Per-worktree `target/` directory (the default): each branch builds into its
  own `target/` for full isolation, at the cost of more disk usage
- Shared `target/` via `CARGO_TARGET_DIR`: point all worktrees at a single
  directory to share compiled artifacts; trade-off is that switching branches
  can invalidate the shared cache
- Sharing the Cargo registry and git caches (`~/.cargo/registry`,
  `~/.cargo/git`) across worktrees — these are always safe to share
- Incremental builds: how Rust's incremental compilation interacts with
  per-worktree `target/` directories
- Pinning the Rust toolchain per branch via `rust-toolchain.toml` — link to the
  mise recipe for toolchain management

## Why it matters

Rust compile times are significant. Understanding the `target/` isolation
trade-off (disk vs build speed) is the main decision point for daft + Rust
projects.

## Where to next

- [Recipes home](/recipes/)
- [Anchor recipe: mise](/recipes/by-tooling/mise)
- [Anchor recipe: direnv](/recipes/by-tooling/direnv)
- [Anchor recipe: monorepo](/recipes/by-scenario/monorepo)
