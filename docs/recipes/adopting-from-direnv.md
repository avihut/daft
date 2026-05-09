---
title: Adopting from direnv
description:
  Layer daft hooks underneath an existing direnv setup — direnv keeps loading
  env on cd; daft adds the install/services/cleanup rituals direnv was never
  meant to handle.
pillars: [worktrees, hooks]
---

# Adopting from direnv

## Starting state

The team adopted direnv a while back. The repo has an `.envrc` at the root and a
README pointing new contributors at the manual rituals:

```bash
# .envrc
dotenv_if_exists .env
PATH_add bin
```

The README's "Getting started" section reads: _"first clone? `direnv allow`,
then `pnpm install`, `docker compose up -d`, and `scripts/codegen.sh`."_

Tool versions are managed externally — devs run whatever Node and Python they
have installed locally. The README has a "Required versions" line that the team
ignores half the time.

The ritual: clone, see direnv's "blocked" message, `direnv allow`, then work
through the rest of the README from memory. direnv solves the secrets-and-PATH
half of setup — `.env` exports, the project's `bin/` on `PATH`. The other half —
the slow stuff (deps install, services up, codegen) — still relies on muscle
memory.

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
`.envrc`.** The `dotenv` line and `PATH_add` stay where they are.

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
dotenv_if_exists .env
PATH_add bin
```

A fresh `daft start feature/x` now lands in a worktree with deps installed,
services running, codegen warming up — and direnv's "`.env` + PATH on cd"
behavior intact when you `cd` in. The README's "after `direnv allow`, also run
pnpm install / compose up" muscle memory is gone.

## Variants

By **what direnv is already managing** in your project. Each variant names a
thing direnv covers and what — if anything — daft adds.

### direnv loads `.env` via `dotenv` / `dotenv_if_exists`

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

### direnv creates a per-directory env (`layout python`, `layout ruby`)

direnv's `layout` stanzas create a per-directory virtualenv (Python) or gemset
(Ruby) and activate it on cd. The daft hook just needs to materialize the env
and install dependencies; direnv's layout activates it on the next cd.

For `layout python python3.11`:

```yaml
- name: install-python-deps
  run: |
    if [ ! -d .direnv/python-3.11 ]; then
      python3.11 -m venv .direnv/python-3.11
    fi
    .direnv/python-3.11/bin/pip install -e .
```

The hook pre-creates the venv that direnv's `layout python` would otherwise
create on the user's first cd, and pre-installs the dependencies. The next `cd`
exports the already-activated venv — no install delay on first interactive use.

For `layout ruby`:

```yaml
- name: install-ruby-deps
  run: bundle install --path .direnv/ruby
```

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

- **[Adopting from mise](/recipes/adopting-from-mise)** — the companion recipe
  for teams using mise instead of direnv.
- **[Env vars & secrets](/recipes/env-vars-and-secrets)** — deeper on hook-time
  vs shell-time env, especially for secrets that should come from a vault rather
  than a local `.env`.
- **[Lifecycle hooks](/hooks/lifecycle)** — when `worktree-post-create` fires
  relative to direnv's cd-time evaluation.
