---
title: daft for Go
description: Patterns for `GOPATH`, modules, and build cache under daft.
pillars: [worktrees, hooks]
tooling: []
languages: []
---

# daft for Go

> **Status: stub.** This recipe is being written. See
> [#398](https://github.com/avihut/daft/issues/398) for status.

## What this recipe will cover

- How Go modules (`go.mod`, `go.sum`) interact with daft worktrees — modules are
  per-worktree by default and each branch can pin different module versions
- `GOPATH` per worktree vs a single global `GOPATH` — most modern Go projects
  use modules and don't need a per-worktree `GOPATH`
- Build cache (`GOCACHE`): sharing `~/.cache/go-build` across worktrees is safe
  and recommended to avoid redundant compilation
- Pinning the Go version per branch using `go.toolchain` in `go.mod` — link to
  the mise recipe for toolchain management via `mise.toml`

## Why it matters

Go's module system is already branch-aware via `go.mod`. The main daft benefit
is parallel branch work without constant `GOFLAGS` juggling or cache collisions.

## Where to next

- [Cookbook home](/cookbook/)
- [Anchor recipe: mise](/cookbook/by-tooling/mise)
- [Anchor recipe: direnv](/cookbook/by-tooling/direnv)
- [Anchor recipe: monorepo](/cookbook/by-scenario/monorepo)
