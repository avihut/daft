---
title: Rust binary with debug warmup
description:
  End-to-end daft setup for a Rust binary project — install deps, prebuild the
  debug binary in the background, fast first-incremental-compile.
pillars: [worktrees, hooks]
---

# Rust binary with debug warmup

This walkthrough sets up a Rust binary project so that:

1. `daft start feature/x` returns immediately into a fresh, working worktree.
2. Crate dependencies are fetched synchronously so `cargo run` doesn't hit the
   network.
3. The debug binary is **prebuilt in the background** so the first `cargo run`
   after coding takes seconds, not minutes.

It's the smallest interesting daft setup — two patterns, one config file, one
team rule.

## What you're building

A Rust project with `Cargo.toml`, `Cargo.lock`, `src/main.rs`. Could be a
single-package binary or a Cargo workspace; the recipe handles both. Build times
are measured in minutes, the team is small, and you create several worktrees per
week.

## Patterns used

- **[Toolchain bootstrap](/recipes/toolchain-bootstrap)** — `cargo fetch`
  synchronously, so deps are local before the warmup needs them.
- **[Background warmup](/recipes/background-warmup)** — `cargo build` detached,
  so worktree creation doesn't wait but the cached compile does the slow work
  upfront.

## Step 1: scaffold `daft.yml`

In the default-branch worktree, create `daft.yml`:

```yaml
# daft.yml
hooks:
  worktree-post-create:
    jobs:
      - name: fetch-deps
        run: cargo fetch --locked
```

`cargo fetch --locked` populates `~/.cargo/registry/` with the crates this
workspace needs. `--locked` enforces `Cargo.lock` integrity so a worktree never
silently rewrites your lockfile.

Trust the new config:

```bash
git add daft.yml
git commit -m "chore(daft): fetch crates on worktree create"
git daft-hooks trust
```

Test it on a fresh worktree:

```bash
daft start feature/scratch
# In the new worktree:
cargo run    # No network — crates are local
```

You'd skip this step if your team already runs everything offline-cached or if
dependency churn is rare. Most projects benefit.

## Step 2: add the background warmup

`cargo run` works after step 1, but the first invocation still has to
**compile** every dependency. That can be 30 seconds to several minutes
depending on dep tree. Add a warmup:

```yaml
# daft.yml
hooks:
  worktree-post-create:
    jobs:
      - name: fetch-deps
        run: cargo fetch --locked

      - name: warmup-build
        run: cargo build
        background: true
        needs: [fetch-deps]
```

`background: true` detaches the build from worktree creation. You drop into the
new worktree immediately; `cargo build` keeps running. By the time you've opened
your editor and looked at what's changing, the debug binary is built. Your first
edit triggers a fast incremental compile.

`needs: [fetch-deps]` ensures `cargo build` doesn't race with the fetch job —
fetch must complete first or the build will try to download crates itself.

::: tip Verify the warmup is actually running After
`daft start feature/scratch`, run `daft hooks log show` from the new worktree.
The backgrounded `warmup-build` should appear with a `running` status until it
completes. :::

## Step 3: scope the warmup in a workspace

For a multi-package Cargo workspace, building everything by default is wasteful
— most worktrees touch one or two packages. Scope to the common targets:

```yaml
- name: warmup-build
  run: cargo build -p server -p worker
  background: true
  needs: [fetch-deps]
```

If different developers focus on different packages, drop the scoping and warm
everything:

```yaml
- name: warmup-build
  run: cargo build --workspace
  background: true
  needs: [fetch-deps]
```

CPU and battery cost is real for `--workspace` on a large repo — measure once
and pick what fits. The build cache is per-worktree, so this work doesn't share
with siblings (see
[Anti-pattern: shared mutable state](/recipes/anti-patterns/shared-mutable-state)).

## Step 4: optional — sccache for cross-worktree wins

If you create many worktrees and the warmup CPU adds up, `sccache` shares
compiled artifacts across all worktrees:

```bash
brew install sccache
```

```yaml
# daft.yml
- name: warmup-build
  run: cargo build
  background: true
  needs: [fetch-deps]
  env:
    RUSTC_WRAPPER: sccache
    SCCACHE_DIR: ${HOME}/.cache/sccache
```

The first warmup populates the sccache cache. Subsequent warmups in any worktree
pull cached artifacts. The bigger the dep tree, the more this pays.

To make sccache the default for all your `cargo` invocations (not just hooks),
add to your shell rc:

```bash
export RUSTC_WRAPPER=sccache
```

## Step 5: verify it works

```bash
# Start a fresh worktree
daft start feature/measure-warmup

# Confirm the warmup is running
daft hooks log show

# Make a small edit, then time the first build
echo "// touch" >> src/main.rs
time cargo build
```

A worktree without the warmup typically takes 30s–5min on this first build. With
the warmup completed in the background, the same build finishes in 2–10s.

## Final `daft.yml`

The complete config — copy and adapt:

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
ever creates.

## What you got

Before:

- Created a worktree → waited at the prompt while `cargo fetch` ran (or forgot
  to run it and hit a confusing offline error).
- First `cargo run` took several minutes.
- "Wait for the cold compile" was an accepted ritual.

After:

- `daft start feature/x` returns instantly. You're typing in the new worktree
  before the warmup is half done.
- The first `cargo run` after a real edit is a fast incremental compile.
- `sccache` lets the work pay for itself across multiple worktrees.

## Where to next

- **[Toolchain bootstrap](/recipes/toolchain-bootstrap)** — for the full set of
  variants (Node, Python, Go) and the idempotency story.
- **[Background warmup](/recipes/background-warmup)** — full reference for
  `background:`, `needs:`, cancellation, and cache-priming.
- **[Sharing caches across worktrees](/recipes/sharing-caches)** — the sccache
  deep-dive plus what's safe vs not (`target/` is **not**).
- **[Walkthroughs → Node monorepo with services](/recipes/walkthroughs/node-monorepo-services)**
  — the next-complexity walkthrough, adding services and per-worktree cleanup.
