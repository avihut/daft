---
title: Toolchain bootstrap
description:
  Install dependencies idempotently when a worktree is created — Node, Python,
  Rust, Go.
pillars: [worktrees, hooks]
---

# Toolchain bootstrap

> A new worktree starts empty. No `node_modules`, no `.venv`, no fetched crates.
> Toolchain bootstrap is the `worktree-post-create` job that gets your worktree
> from "fresh checkout" to "ready to run a command" — every time, on every
> worktree, without you having to remember.

## When to reach for this

- Your project has a dependency manifest (`package.json`, `pyproject.toml`,
  `Cargo.toml`, `go.mod`) and lockfile, and the install step is the same every
  time.
- You want `daft start feature/x` to land you in a worktree where the next
  command (`pnpm test`, `cargo run`, `python -m mypkg`) just works.
- You're tired of typos in install commands and version drift between worktrees
  that "almost" set themselves up.

This is usually the first hook anyone writes. It pays for itself on the second
worktree.

## Minimal recipe

```yaml
# daft.yml
hooks:
  worktree-post-create:
    jobs:
      - name: install-deps
        run: pnpm install --frozen-lockfile
```

`worktree-post-create` runs from inside the new worktree, so `pnpm install`
finds the project's `package.json` automatically. With `--frozen-lockfile`, the
install fails fast if the lockfile is out of date — exactly the behavior you
want when bootstrapping a worktree from a known-good ref.

The default fail mode for `worktree-post-create` is `warn` (the worktree gets
created even if install fails); to make a failed install abort creation,
override per-hook:

```bash
git config daft.hooks.worktreePostCreate.failMode abort
```

## Variants

### Node — pnpm / npm / yarn / bun

```yaml
# pnpm — fast, content-addressable store shared across worktrees by default
- name: install-deps
  run: pnpm install --frozen-lockfile

# npm — use `ci`, never `install`, for reproducibility
- name: install-deps
  run: npm ci

# yarn (classic / berry) — `--immutable` is the v3+ name
- name: install-deps
  run: yarn install --immutable

# bun — also content-addressable; lockfile is `bun.lockb`
- name: install-deps
  run: bun install --frozen-lockfile
```

pnpm and bun share their package store across worktrees by default — no extra
wiring. npm and yarn copy `node_modules/` per worktree (the right default; see
[Anti-pattern: shared mutable state](/recipes/anti-patterns/shared-mutable-state)).

### Python — uv / pip / poetry

```yaml
# uv — fast, lockfile-based, creates per-worktree .venv automatically
- name: install-deps
  run: uv sync --frozen

# pip with venv — explicit venv creation
- name: install-deps
  run: |
    python -m venv .venv
    .venv/bin/pip install -r requirements.txt

# poetry — virtualenvs.in-project=true puts .venv inside the worktree
- name: install-deps
  run: poetry install --no-root --sync
```

For pip-based setups, prefer storing the venv inside the worktree (`./.venv/`)
so worktrees don't fight over a shared environment. uv does this automatically.

### Rust — cargo

```yaml
# Just fetch dependencies; don't build (that's a warmup job)
- name: fetch-deps
  run: cargo fetch --locked
```

`cargo fetch` populates the local registry cache with crates the workspace
needs. It does **not** build anything — that's
[Background warmup](/recipes/background-warmup)'s job. Mixing them makes
worktree creation feel slow.

`--locked` enforces `Cargo.lock` integrity, the analog of `--frozen-lockfile`.

### Go — modules

```yaml
- name: fetch-modules
  run: go mod download
```

`go mod download` fetches modules into the module cache (shared across worktrees
by default at `$GOPATH/pkg/mod`). Like Rust, this is fetch-only; build warmup is
a separate concern.

## Idempotency & safety

Most package managers are idempotent by design — running `pnpm install` twice
with the same lockfile is a near-no-op. But idempotency comes from **using the
lockfile-honoring command**, not the dev-friendly variant:

| Command                          | Idempotent? | Why                                          |
| -------------------------------- | ----------- | -------------------------------------------- |
| `pnpm install --frozen-lockfile` | ✓           | Refuses to mutate the lockfile               |
| `pnpm install`                   | ✗           | May rewrite lockfile, drift across worktrees |
| `npm ci`                         | ✓           | Wipes `node_modules`, installs from lockfile |
| `npm install`                    | ✗           | Mutates `package.json` if deps are missing   |
| `uv sync --frozen`               | ✓           | Refuses to update lockfile                   |
| `cargo fetch --locked`           | ✓           | Errors if `Cargo.lock` would change          |
| `go mod download`                | ✓           | Module cache is content-addressed            |

Always use the strict variants in hooks. Worktree creation should never silently
rewrite a lockfile.

::: warning Don't run `pnpm install` (without `--frozen-lockfile`) in a hook If
two worktrees create at roughly the same time and one has a slightly stale
lockfile, you can end up with divergent `pnpm-lock.yaml` states across
worktrees. Always pin to lockfile-strict mode in hooks. :::

## Composes well with

- **[Background warmup](/recipes/background-warmup)** — once deps are installed,
  kick off a warmup job (`cargo build`, Vite optimizer, Gradle daemon) in the
  background so the first user command is fast.
- **[Declarative envs](/recipes/declarative-envs)** — let mise/asdf install the
  toolchain itself (Node, Python, Rust versions) declaratively, then run
  `pnpm install` / `cargo fetch` from a daft hook. The two are complementary,
  not alternatives.
- **[Cleanup on remove](/recipes/cleanup-on-remove)** — if your install step
  touches state outside the worktree (a global lockfile, a registry daemon), the
  matching pre-remove cleanup goes there.

## Anti-patterns

- **[Shared `node_modules` across worktrees](/recipes/anti-patterns/shared-mutable-state)**
  — tempting (saves disk + time), breaks badly. Each worktree gets its own
  `node_modules`; share the package store instead (pnpm, bun do this natively).
- **`run: pnpm install`** without `--frozen-lockfile` — see the warning above.
- **`run: cargo build`** as the only post-create step — that's not a bootstrap,
  it's a warmup. Keep them separate so worktree creation doesn't block on a
  3-minute compile.

## See also

- **[Lifecycle hooks](/hooks/lifecycle)** — `worktree-post-create` reference,
  env vars, exit-code semantics
- **[Job orchestration](/hooks/job-orchestration)** — running install + warmup
  jobs in parallel, dependencies between them
- **[Sharing caches across worktrees](/recipes/sharing-caches)** — what's safe
  to share (package stores, build caches) vs not (`node_modules`, `target/`)
