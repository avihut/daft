---
title: Background warmup
description:
  Detached prebuild work — Rust dev binary, Vite optimizer, Gradle daemon — that
  makes your first incremental command fast.
pillars: [worktrees, hooks]
---

# Background warmup

> Installing dependencies isn't enough. The first `cargo run`, `vite dev`, or
> `./gradlew test` in a fresh worktree is still slow — nothing is compiled, no
> caches are warm. Background warmup is a detached job that does the heavy work
> _while_ you start coding, so when you actually run the command, it's fast.

## When to reach for this

- Your project has a real build step (compiled languages, bundlers, long-warming
  daemons) and the first build after a fresh install takes meaningful time.
- You want `daft start feature/x` to feel instant, but you also want `cargo run`
  (or equivalent) to be fast when you get to it.
- You have CPU and battery to spare. Warmups consume both — they're a trade-off,
  not a free win.

If your first build is already fast (interpreted languages, tiny codebases,
projects with hot caches), skip this pattern.

## Minimal recipe

```yaml
# daft.yml
hooks:
  worktree-post-create:
    jobs:
      - name: install-deps
        run: cargo fetch --locked

      - name: warmup-build
        run: cargo build
        background: true
        needs: [install-deps]
```

`background: true` detaches the job from worktree creation. The hook returns as
soon as `install-deps` is done; `warmup-build` keeps running. You drop into the
new worktree immediately and the build progresses behind you.

`needs: [install-deps]` ensures the warmup waits for crates to be fetched.
Without that, you'd race.

The default fail mode for `worktree-post-create` is `warn`, so a failed warmup
never blocks worktree creation. That's the right default — a warmup is an
optimization, not a correctness step.

## Variants

### Rust — debug binary prebuild

```yaml
- name: warmup-build
  run: cargo build # debug profile, default workspace
  background: true
  needs: [install-deps]
```

Builds the debug binary into `target/debug/`. When you later run `cargo run` or
`cargo test`, only your delta is compiled — minutes turn into seconds. The
warmup is per-worktree; **don't** share `target/` across worktrees (see
[Anti-pattern: shared mutable state](/recipes/anti-patterns/shared-mutable-state)).

For multi-package workspaces, scope the warmup to the package(s) you work on
most:

```yaml
- name: warmup-build
  run: cargo build -p server -p worker
  background: true
  needs: [install-deps]
```

### Go — build cache priming

```yaml
- name: warmup-build
  run: go build ./...
  background: true
  needs: [fetch-modules]
```

`go build ./...` populates the build cache (`$GOCACHE`, shared across worktrees
by default). The compiled artifacts are content-addressed, so the cache survives
across worktrees and gives every worktree a head start.

### Vite / Next.js — dep optimizer warmup

```yaml
- name: warmup-vite
  run: pnpm exec vite optimize --force
  background: true
  needs: [install-deps]
  root: apps/web
```

Vite's dep optimizer is the slowest part of the first `vite dev`. Running
`vite optimize` ahead of time means the dev server starts hot. `root:` is a
per-job working directory — useful when the warmup target is a single app inside
a monorepo.

### Gradle — daemon + dep download

```yaml
- name: warmup-gradle
  run: ./gradlew --no-daemon dependencies
  background: true
  needs: [install-deps]
```

Gradle's biggest cold-start cost is dep resolution and the daemon spin-up.
`./gradlew dependencies` resolves the full graph and primes the configuration
cache.

### sccache / ccache — distributed cache prime

```yaml
- name: prime-cache
  run: |
    SCCACHE_DIR=$HOME/.cache/sccache cargo build
  background: true
  needs: [install-deps]
  env:
    RUSTC_WRAPPER: sccache
```

If you use sccache or ccache, the cache is shared across worktrees by design.
Priming it from a warmup means subsequent builds in **any** worktree benefit,
not just the current one.

## Idempotency & safety

Warmup jobs are usually idempotent — building twice is a no-op (the build system
sees no changes). But two concerns are worth being explicit about:

**Cancellation.** If you remove the worktree while a warmup is running, daft
sends `SIGTERM` to the job's process group. Most build tools handle this cleanly
(cargo, go, gradle all unwind partial work). If your warmup script holds
long-lived locks (a custom daemon, a database connection), trap the signal and
clean up explicitly.

**No critical work.** Anything truly required for correctness must run
synchronously, **not** in a warmup. The `background: true` job can fail, get
cancelled, or finish late; depending on it for correctness produces flaky
worktrees.

::: warning Don't run code generation in a warmup If your project generates
source files at build time (proto, GraphQL schema, OpenAPI client), that codegen
is **not** a warmup — code that imports the generated module breaks if it isn't
there. Run codegen synchronously in toolchain-bootstrap, or as a sequential step
after install. :::

## Composes well with

- **[Toolchain bootstrap](/recipes/toolchain-bootstrap)** — the install step a
  warmup `needs:`. Always upstream from warmup.
- **[Job orchestration](/hooks/job-orchestration)** — `parallel`, `needs`,
  `priority` are how you compose multi-step setups. Use `parallel: true` (the
  default) to let warmups run alongside other background work.
- **[Sharing caches across worktrees](/recipes/sharing-caches)** — warmups pay
  for themselves more when their cached output is shared (sccache, Go build
  cache, Vite dep cache directory).

## Anti-patterns

- **Codegen as a warmup** — see the warning above. Codegen is a correctness
  step.
- **Blocking install steps disguised as warmups** — if your warmup is required
  before the next user command can run, it isn't really a warmup. Move it back
  to a synchronous job.
- **Sharing `target/` across worktrees** to "make warmup faster" — this corrupts
  cache. See
  [Anti-pattern: shared mutable state](/recipes/anti-patterns/shared-mutable-state).
  Use sccache instead, which is designed for the sharing case.

## See also

- **[Lifecycle hooks](/hooks/lifecycle)** — `worktree-post-create` reference,
  exit-code semantics, env vars
- **[Job orchestration](/hooks/job-orchestration)** — full reference for
  `background`, `needs`, `parallel`, `priority`
