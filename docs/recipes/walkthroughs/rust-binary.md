---
title: Rust binary with debug warmup
description:
  Threading toolchain-bootstrap and background-warmup into a real Rust workspace
  — so worktree creation returns instantly and the first cargo run is a fast
  incremental compile.
pillars: [worktrees, hooks]
---

# Rust binary with debug warmup

## Starting state

A Rust workspace at the repo root:

```
myapp/
├── Cargo.toml          # workspace = ["server", "worker", "shared"]
├── Cargo.lock
├── server/
│   └── src/main.rs     # HTTP API binary
├── worker/
│   └── src/main.rs     # background job binary
└── shared/
    └── src/lib.rs      # types both binaries import
```

From a fresh clone, `cargo fetch` takes 30 seconds and `cargo build --workspace`
takes about 4 minutes. The team rule today is "after `git checkout`, run
`cargo fetch && cargo build --workspace` and go get coffee." Most worktrees get
the fetch (they need to type a command soon, and a missing-crate error is
annoying). Fewer get the build, so the first `cargo run` after coding still hits
the slow path. Every. Single. Worktree.

This walkthrough threads two patterns into one `daft.yml`:

- **[Toolchain bootstrap](/recipes/toolchain-bootstrap)** — `cargo fetch`
  synchronously, so deps are local before anything else needs them.
- **[Background warmup](/recipes/background-warmup)** — `cargo build` detached,
  so worktree creation returns immediately while the slow compile happens in the
  background.

By the end you'll have a `daft start` that returns in seconds, leaves a build
running, and lands the first incremental compile in your editor in two-digit
seconds instead of four-digit.

## Step 1: cargo fetch on create

Apply the [Toolchain bootstrap](/recipes/toolchain-bootstrap) pattern in its
Rust shape:

```yaml
# daft.yml
hooks:
  worktree-post-create:
    jobs:
      - name: fetch-deps
        run: cargo fetch --locked
```

Why `cargo fetch` and not `cargo build` here: fetch is fast (~30s), build is
slow (4min). The pattern's separation between fetch and warmup is exactly so the
synchronous part stays cheap. See the pattern for the full why on `--locked`.

```bash
git add daft.yml
git commit -m "chore(daft): cargo fetch on worktree create"
git daft-hooks trust

# Verify on a fresh worktree
daft start feature/scratch
cargo run -p server   # no network — crates already local
```

## Step 2: background warmup

Apply the [Background warmup](/recipes/background-warmup) pattern. Add a
backgrounded `cargo build --workspace` that depends on fetch:

```yaml
# daft.yml
hooks:
  worktree-post-create:
    jobs:
      - name: fetch-deps
        run: cargo fetch --locked

      - name: warmup-build
        run: cargo build --workspace
        background: true
        needs: [fetch-deps]
```

`background: true` detaches the build from worktree creation — `daft start`
returns as soon as `fetch-deps` finishes, and `cargo build` keeps running. By
the time you've opened your editor, the debug binary is built; your first edit
triggers a fast incremental compile.

::: tip Verify the warmup is actually running After
`daft start feature/scratch`, run `daft hooks log show` from the new worktree.
The backgrounded `warmup-build` shows as `running` until it completes. :::

The trade-off — and where this walkthrough's choice diverges from the pattern's
other variants — is `--workspace` vs scoped (`-p server -p worker`). The pattern
documents both; for a three-crate workspace where both binaries are touched
regularly, building the whole workspace is the right default. If your team
mostly touches one crate, swap in the scoped form from the pattern's Variants
section.

## Step 3: sccache for cross-worktree wins

If you create multiple worktrees a week (you do — that's the whole point), the
warmup CPU adds up. `sccache` is content-addressed by source-and-flags, so a
build in worktree A primes the cache for B:

```bash
brew install sccache
```

```yaml
- name: warmup-build
  run: cargo build --workspace
  background: true
  needs: [fetch-deps]
  env:
    RUSTC_WRAPPER: sccache
    SCCACHE_DIR: ${HOME}/.cache/sccache
```

The first warmup populates the cache. Every subsequent warmup, in any worktree,
pulls cached artifacts. The bigger your dep tree, the more this pays off.

To make sccache the default for _all_ your `cargo` invocations (not just hooks),
add to your shell rc:

```bash
export RUSTC_WRAPPER=sccache
```

`target/` itself stays per-worktree — sharing it directly is a corruption
hazard. See
[Anti-pattern: shared mutable state](/recipes/anti-patterns/shared-mutable-state).

## Final `daft.yml`

```yaml
# daft.yml
hooks:
  worktree-post-create:
    jobs:
      - name: fetch-deps
        run: cargo fetch --locked

      - name: warmup-build
        run: cargo build --workspace
        background: true
        needs: [fetch-deps]
        env:
          RUSTC_WRAPPER: sccache
          SCCACHE_DIR: ${HOME}/.cache/sccache
```

Two jobs, four lines of meaningful logic, applies to every worktree the team
ever creates. Same shape as the [Background warmup](/recipes/background-warmup)
pattern's sccache variant, just with the `--workspace` choice locked in for this
project's three-crate shape.

## What you got

Before:

- `git checkout feature/x` → `cargo fetch && cargo build --workspace` → 4
  minutes of cursor-blink before you can do anything useful. Most worktrees
  skipped the build, so the first `cargo run` paid the cost later instead.
- "Wait for the cold compile" was an accepted ritual. The team had learned to
  start it before coffee.

After:

- `daft start feature/x` returns in seconds. You're typing in the new worktree
  before the warmup is half done.
- The first `cargo run` after a real edit is a 2–10s incremental compile.
- sccache makes the warmup work in worktree A pay for the warmup in worktree B —
  so the team-wide cost stops scaling with worktree count.

## Where to next

- **[Background warmup](/recipes/background-warmup)** — if you want to swap in
  scoped builds (`-p server -p worker`) or layer in a Vite/ Gradle warmup
  alongside.
- **[Sharing caches across worktrees](/recipes/sharing-caches)** — the per-tool
  guide for sccache, the cargo registry, and what stays per-worktree.
- **[Walkthroughs → Node monorepo with services](/recipes/walkthroughs/node-monorepo-services)**
  — the next-complexity walkthrough, layering services and per-worktree cleanup
  on top of the bootstrap pattern.
