---
title: Sharing caches across worktrees
description:
  When to share build caches between worktrees and when not — pnpm store, cargo
  registry, npm cache, ccache, uv cache. Per-tool answers.
pillars: [worktrees, hooks]
---

# Sharing caches across worktrees

> Every recipe that installs deps or builds something raises the same question:
> should this cache be shared across worktrees, or per-worktree? The answer
> depends on what's in the cache. This page gives the per-tool answer.

The rule of thumb: **content-addressed caches are safe to share; content-mutable
directories are not**. A pnpm store keys by hash and never overwrites;
`node_modules/` is a flat tree of dependent modules that breaks when two
worktrees mutate it. Same shape applies to cargo, Go, ccache, etc.

For the unsafe-sharing failure modes, see
[Anti-pattern: shared mutable state](/recipes/anti-patterns/shared-mutable-state).
This page is the safe-sharing reference.

## By tool

### pnpm — store yes, `node_modules/` no

| What                      | Default location                    | Share across worktrees? |
| ------------------------- | ----------------------------------- | ----------------------- |
| Store (content-addressed) | `~/Library/pnpm/store/v10/` (macOS) | **Yes — recommended**   |
| `node_modules/`           | per-worktree                        | **No**                  |

pnpm's store is content-addressable — every file is keyed by hash and hardlinked
into per-worktree `node_modules/` trees. Sharing it means disk usage stays
roughly constant as you add worktrees. Configure once:

```bash
pnpm config set store-dir ~/.pnpm-store
```

### npm — cache yes, `node_modules/` no

| What            | Default location   | Share?                                 |
| --------------- | ------------------ | -------------------------------------- |
| Cache           | `~/.npm/_cacache/` | Yes (default; npm handles concurrency) |
| `node_modules/` | per-worktree       | **No**                                 |

npm's cache holds tarballs and metadata — also content-addressed, also safe to
share. The default behavior is correct; no config change needed.

### bun — store yes, `node_modules/` no

bun uses a global content-addressable cache by default
(`~/.bun/install/cache/`). Same story as pnpm — share the cache, not the install
directory.

### cargo — registry yes, `target/` no

| What                     | Default location     | Share?            |
| ------------------------ | -------------------- | ----------------- |
| Registry (cached crates) | `~/.cargo/registry/` | **Yes (default)** |
| Build output (`target/`) | per-worktree         | **No**            |

`~/.cargo/registry/` is content-addressed; cargo handles concurrent access.
`target/` is per-worktree by design — sharing it via `CARGO_TARGET_DIR` produces
silent corruption when two worktrees with the same dep tree but different
feature flags or rust versions overwrite each other's artifacts.

For sharing **compiled** output across worktrees, use
[sccache](https://github.com/mozilla/sccache) — it's designed for the sharing
case:

```bash
brew install sccache
echo 'export RUSTC_WRAPPER=sccache' >> ~/.zshrc
```

### Go — module cache yes, build cache yes

| What         | Default location                          | Share?            |
| ------------ | ----------------------------------------- | ----------------- |
| Module cache | `$GOPATH/pkg/mod/`                        | **Yes (default)** |
| Build cache  | `$GOCACHE` (`~/Library/Caches/go-build/`) | **Yes (default)** |

Both are content-addressed; Go has the cleanest concurrent-cache model of the
major toolchains. No config needed.

### Python — uv yes, pip cache yes, `.venv/` no

| What      | Default location | Share?            |
| --------- | ---------------- | ----------------- |
| uv cache  | `~/.cache/uv/`   | **Yes (default)** |
| pip cache | `~/.cache/pip/`  | **Yes (default)** |
| `.venv/`  | per-worktree     | **No**            |

`.venv/` is the equivalent of `node_modules/` — install is mutable, holds
compiled `.pyc` files, version-pinned. Per-worktree.

### ccache / sccache — yes, by design

ccache and sccache exist for this. Configure once, every worktree benefits:

```bash
brew install ccache sccache
mkdir -p "$HOME/.cache/ccache" "$HOME/.cache/sccache"
```

For `cc`/`g++` builds, set `CC="ccache cc"`. For Rust, set
`RUSTC_WRAPPER=sccache` (see cargo above).

### Vite / webpack / esbuild — usually safe, with caveats

Vite's optimizer cache (`node_modules/.vite/`) is per-worktree and should stay
that way. webpack 5's persistent cache lives in `node_modules/.cache/` — also
per-worktree.

esbuild has no cache; nothing to share.

## Quick reference table

| Tool             | Share                                  | Don't share                      |
| ---------------- | -------------------------------------- | -------------------------------- |
| pnpm             | store (`~/.pnpm-store`)                | `node_modules/`                  |
| npm              | cache (`~/.npm`)                       | `node_modules/`                  |
| bun              | install cache (`~/.bun/install/cache`) | `node_modules/`                  |
| cargo            | registry (`~/.cargo/registry`)         | `target/`                        |
| Go               | mod cache + build cache                | (nothing — Go shares everything) |
| pip / uv         | cache (`~/.cache/{pip,uv}`)            | `.venv/`                         |
| ccache / sccache | the cache itself                       | (nothing — designed to share)    |

## Composes well with

- **[Toolchain bootstrap](/recipes/toolchain-bootstrap)** — every pattern in
  there benefits from cache sharing. The store is shared; the install
  (`pnpm install`, `cargo fetch`) writes per-worktree.
- **[Background warmup](/recipes/background-warmup)** — sccache makes warmups
  pay for themselves across multiple worktrees.

## See also

- **[Anti-pattern: shared mutable state](/recipes/anti-patterns/shared-mutable-state)**
  — the failure modes when the unsafe sharing happens
