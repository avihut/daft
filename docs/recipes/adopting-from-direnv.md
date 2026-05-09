---
title: Adopting from direnv
description:
  Layer daft hooks underneath an existing direnv setup — keep what direnv does,
  add what it doesn't (install, services, warmup, cleanup).
pillars: [worktrees, hooks]
---

# Adopting from direnv

## Starting state

The team adopted direnv a while back. The repo has:

- `.envrc` at the root with `use mise`, `dotenv_if_exists .env`, and
  `PATH_add bin`
- A `mise.toml` pinning Node 22, Python 3.13, Rust 1.84 — activated by direnv's
  `use mise`
- A README that says "first clone? `direnv allow`, then `pnpm install`,
  `docker compose up -d`, and `scripts/codegen.sh`."

The ritual: clone, see direnv's "blocked" message, `direnv allow`, then work
through the rest of the README from memory. direnv solves the tools-and-env half
of setup — `node` resolves to the right version, the `.env` secrets are
exported, the project's `bin/` is on `PATH`. The other half — the slow stuff
(deps install, services up, codegen) — still relies on muscle memory.

That was tolerable for a single working tree. With daft worktrees, the slow
rituals fire dozens of times a month. Sooner or later someone runs `pnpm test`
before `pnpm install` finished, sees a missing-module error, and re-runs through
the README to figure out what they skipped.

The reach for daft: don't replace direnv — layer hooks underneath it. direnv
keeps managing what loads on `cd`; daft hooks pick up the rituals direnv was
never meant to handle.

## What changes

A new `daft.yml` adds the install / services / cleanup work to
`worktree-post-create` and `worktree-pre-remove`. **Nothing changes in
`.envrc`.** mise activation, the `dotenv` line, the `PATH_add` — all stay where
they are.

Hooks run with the worktree as cwd. They do not run inside a direnv-loaded shell
— `direnv exec` is not needed and is in fact counterproductive (see Idempotency
below). The hook's per-job `env:` is independent of direnv's exports; the two
layers coexist without conflict.

## Recipe

```yaml
# daft.yml
hooks:
  worktree-post-create:
    jobs:
      - name: install-deps
        run: pnpm install --frozen-lockfile

      - name: services-up
        run: docker compose up -d --wait
        needs: [install-deps]

      - name: codegen
        run: ./scripts/codegen.sh
        needs: [install-deps]
        background: true

  worktree-pre-remove:
    jobs:
      - name: services-down
        run: docker compose down -v --remove-orphans
```

Existing `.envrc` stays as-is:

```bash
# .envrc — unchanged
use mise
dotenv_if_exists .env
PATH_add bin
```

A fresh `daft start feature/x` now lands in a worktree with deps installed,
services running, codegen warming up in the background — and direnv's "tools +
env on cd" behavior intact when you actually `cd` in. The README's "after
`direnv allow`, also run pnpm install / compose up" muscle memory is gone.

## Variants

By **what direnv is already managing** in your project. Each variant names a
thing direnv covers and what — if anything — daft adds.

### direnv pins tool versions (`use mise`, `use node`, `use python`)

Skip an explicit `mise install` job. mise activation runs when the user `cd`s
into the worktree; the binaries resolve via shell PATH. If a pinned version
isn't on disk yet, mise prompts the user on the next cd — one-time, until
installed.

If you want eager install at worktree create (no surprise prompt later):

```yaml
- name: install-tool-versions
  run: mise install
```

### direnv loads secrets via `dotenv` / `dotenv_if_exists`

Most hook jobs don't need secrets — install, services up, codegen typically use
the build toolchain, not API keys. Leave secret loading to direnv at the shell
level; the hook stays free of `.env` reads.

For a hook job that does need a secret (e.g., a migration that reads
`DATABASE_URL`), source the same `.env` direnv reads:

```yaml
- name: migrate
  run: |
    set -a
    source .env
    set +a
    pnpm db:migrate
  needs: [services-up]
```

Don't seed secrets into `.envrc` from a hook — that round-trips through direnv's
trust prompt every worktree create. See
[Env vars & secrets](/recipes/env-vars-and-secrets) for vault-fetched patterns
where secrets shouldn't sit in a local file at all.

### direnv adds a project bin to PATH (`PATH_add bin`)

No daft change. Hooks run with the worktree as cwd, so `./bin/foo` resolves
directly. The hook doesn't see direnv's `PATH_add` because the hook isn't
running inside a direnv-loaded shell — and that's fine; just prefix with
`./bin/` in the hook's `run:` line.

## Idempotency & safety

Double-loading is the most common gotcha. If a hook's per-job `env:` exports the
same var direnv exports:

- **Inside the hook**, the per-job `env:` value wins (hook env overrides
  whatever the hook inherited from the parent shell).
- **Inside the shell, after the hook completes**, direnv wins — it's
  re-evaluated on every `cd`.

This is usually fine. The hook needs deterministic hook-local values; keep them
in `env:` blocks. The shell needs the values direnv computes; let direnv keep
computing them.

::: warning Don't `direnv exec . daft hooks run …`

Wrapping daft in `direnv exec` is unnecessary (the hook doesn't need direnv's
exports) and invites a slow `.envrc` re-evaluation that can deadlock against
direnv's trust prompt during a `daft start`. Run daft binaries directly.

:::

## Where to next

- **[Declarative envs](/recipes/declarative-envs)** — if you're considering
  migrating off direnv. mise's `[env]` block + `mise activate` covers the same
  job with one less tool in the chain.
- **[Env vars & secrets](/recipes/env-vars-and-secrets)** — deeper on hook-time
  vs shell-time env, especially when secrets need to come from a vault rather
  than a local `.env`.
- **[Lifecycle hooks](/hooks/lifecycle)** — when `worktree-post-create` fires
  relative to direnv's `cd`-time evaluation.
