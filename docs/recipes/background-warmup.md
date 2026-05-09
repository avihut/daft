---
title: Background warmup
description:
  A detached prebuild job that finishes while you're still opening the editor —
  so the first incremental compile after your edit is fast.
pillars: [worktrees, hooks]
---

# Background warmup

## Starting state

A Rust workspace — three crates. From a fresh `cargo fetch`, the first
`cargo build --workspace` on your laptop takes about 4 minutes. You've already
adopted [Toolchain bootstrap](/recipes/toolchain-bootstrap), so
`daft start feature/x` populates the registry cache before returning. That
part's good.

What's still bad: the _first_ `cargo run` (or `cargo test`) in a fresh worktree
still pays those 4 minutes. Every worktree. You started the worktree because you
wanted to context-switch fast, and now you're watching the cursor blink for as
long as it would have taken to deal with the original task. By the time it's
done, you've already lost the thread.

The reach for daft: do the slow work _while_ you're still opening the editor. By
the time your first edit is ready, the cache is warm and the incremental compile
is a few seconds.

## What changes

`daft.yml` gains a second job — backgrounded, downstream of the install.
Worktree creation still returns as soon as the install finishes; `cargo build`
keeps running, detached, after the daft command exits. You drop into the new
worktree right away.

What you don't get: a guarantee. Background warmups are an optimization, not a
correctness contract. The job can fail, get cancelled, or simply be slower than
your first edit. None of that breaks anything; the worst case is that the first
build still pays the slow path. Background warmup is the kind of automation you
can add and forget about.

## Recipe

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

`background: true` detaches the job from worktree creation. The hook returns as
soon as `fetch-deps` finishes; `warmup-build` keeps running. You're typing in
the new worktree while the compiler grinds.

`needs: [fetch-deps]` ensures the build doesn't race the fetch. Without it,
`cargo build` would try to download crates that the parallel fetch is already
pulling.

The default fail mode for `worktree-post-create` is `warn`, so a failed warmup
never blocks worktree creation. That's the right default — a warmup is an
optimization, and an optimization that occasionally fails is still a net win.

## Variants

By tool. The shape is the same — `background: true`, `needs:` the install — only
the `run:` line changes.

### Rust — debug binary, scoped or workspace

```yaml
# Whole workspace — simplest, slowest
- name: warmup-build
  run: cargo build --workspace
  background: true
  needs: [fetch-deps]

# Scoped to packages you work on most — faster, less complete
- name: warmup-build
  run: cargo build -p server -p worker
  background: true
  needs: [fetch-deps]
```

`--workspace` is the right default for small/medium repos. For a big multi-crate
workspace where each developer focuses on a couple of packages, scoping pays off
— it cuts CPU time and battery drain at the cost of cold-compiling whatever you
didn't pre-build.

The build cache (`target/`) is per-worktree by design — sharing it silently
corrupts artifacts. For sharing **compiled** output, use `sccache` (next
variant), not `CARGO_TARGET_DIR`. The full failure modes are at
[Anti-pattern: shared mutable state](/recipes/anti-patterns/shared-mutable-state).

### Go — build cache priming

```yaml
- name: warmup-build
  run: go build ./...
  background: true
  needs: [fetch-modules]
```

Go's build cache (`$GOCACHE`) is shared across worktrees by default and
content-addressed, so a warmup in one worktree pre-compiles for every other one
too. The cleanest cache model of any major toolchain.

### Vite / Next.js — dep optimizer prime

```yaml
- name: warmup-vite
  run: pnpm exec vite optimize --force
  background: true
  needs: [install-deps]
  root: apps/web
```

Vite's dep optimizer is the slow part of the first `vite dev`. Running
`vite optimize` ahead of time means the dev server starts hot. `root:` is a
per-job working directory — useful when the warmup target is a single app inside
a monorepo.

### Gradle — daemon spin-up

```yaml
- name: warmup-gradle
  run: ./gradlew --no-daemon dependencies
  background: true
  needs: [install-deps]
```

Gradle's biggest cold-start cost is dep resolution and configuration.
`./gradlew dependencies` resolves the full graph and primes the configuration
cache.

### sccache — share the compile work, not the artifacts

```yaml
- name: warmup-build
  run: cargo build --workspace
  background: true
  needs: [fetch-deps]
  env:
    RUSTC_WRAPPER: sccache
    SCCACHE_DIR: ${HOME}/.cache/sccache
```

`sccache` is content-addressed by source-and-flags, so a warmup in worktree A
primes the cache for worktree B. The `target/` directories stay per-worktree
(correct), but the actual compile work runs once. The bigger your workspace, the
more this pays off.

## Idempotency & safety

Warmups are idempotent by construction — building twice with no source changes
is a near-no-op. Two specific concerns are worth being explicit about:

**Cancellation.** Removing the worktree while a warmup is running sends SIGTERM
to the job's process group. cargo, go, and gradle all unwind partial work
cleanly. If your warmup is a custom script that holds long-lived locks (a
daemon, a database connection), trap the signal and clean up explicitly.

**No critical work in a warmup.** Anything required for correctness must run
synchronously. The `background: true` job can fail, get cancelled, or finish
late — depending on it for correctness produces flaky worktrees. The most common
mistake is putting code generation here.

::: warning Don't run code generation in a warmup If your project generates
source files at build time (proto, GraphQL schema, OpenAPI client), code that
imports the generated module breaks if the codegen isn't done. Run codegen
synchronously as part of the install, not as a backgrounded warmup. :::

## Where to next

- **[Toolchain bootstrap](/recipes/toolchain-bootstrap)** — the install job a
  warmup `needs:`. Always upstream from warmup; can't skip it.
- **[Sharing caches across worktrees](/recipes/sharing-caches)** — sccache,
  ccache, the Go build cache, and what makes them safe to share when `target/`
  and `node_modules/` aren't.
- **[Job orchestration](/hooks/job-orchestration)** — `background`, `needs`,
  `parallel`, `priority` reference for composing multi-step hooks.
