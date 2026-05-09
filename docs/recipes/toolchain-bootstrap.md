---
title: Toolchain bootstrap
description:
  Replace the bin/setup.sh ritual with a worktree-post-create hook that installs
  deps automatically — every worktree, every time.
pillars: [worktrees, hooks]
---

# Toolchain bootstrap

## Starting state

A Node monorepo. The repo has:

- `package.json` and `pnpm-lock.yaml` at the root
- `bin/setup.sh` that runs `pnpm install --frozen-lockfile` and copies
  `.env.example` to `.env` if one isn't there yet
- A README that opens with **"First time? Run `bin/setup.sh`."**

The ritual: `git checkout feature/x`, then `bin/setup.sh`. Sooner or later
someone forgets and hits a confusing missing-module error from a transitive dep
that yesterday's lockfile bump pulled in.

The reach for daft: stop having a setup ritual at all. **Worktree creation
should be the setup ritual.**

## What changes

- `bin/setup.sh` shrinks (most of its body moves into `daft.yml`) or deletes
  outright.
- `daft.yml` gains a `worktree-post-create` hook that does the install.
- The README loses its "first time, run setup.sh" line.

What you get for it:

- `daft start feature/x` lands you in a worktree where `pnpm test` works as the
  next command you type.
- One canonical description of "how this project sets up", in `daft.yml`, used
  by every dev and (later) by CI.

## Recipe

```yaml
# daft.yml
hooks:
  worktree-post-create:
    jobs:
      - name: install-deps
        run: pnpm install --frozen-lockfile
```

Commit and trust:

```bash
git add daft.yml
git commit -m "chore(daft): install deps on worktree create"
git daft-hooks trust
```

A fresh `daft start feature/x` now lands in a worktree with `node_modules/`
populated, `pnpm-lock.yaml` honored, and `pnpm test` ready to run. The README's
setup line — and the muscle-memory it required — is gone.

## Variants

By language and package manager. Each is a drop-in replacement for the
`install-deps` job's `run:` line.

### Node — pnpm / npm / yarn / bun

```yaml
# pnpm — content-addressable store shared across worktrees by default
- name: install-deps
  run: pnpm install --frozen-lockfile

# npm — use `ci`, never `install`, for reproducibility
- name: install-deps
  run: npm ci

# yarn (v3+) — `--immutable` is the modern name for frozen-lockfile
- name: install-deps
  run: yarn install --immutable

# bun — also content-addressable; lockfile is `bun.lockb`
- name: install-deps
  run: bun install --frozen-lockfile
```

pnpm and bun share their package store across worktrees by default — no extra
wiring. npm and yarn copy `node_modules/` per worktree, which is the right
default (the alternative is a corruption hazard — see
[Anti-pattern: shared mutable state](/recipes/anti-patterns/shared-mutable-state)).

### Python — uv / pip / poetry

```yaml
# uv — fast, lockfile-based, creates per-worktree .venv automatically
- name: install-deps
  run: uv sync --frozen

# pip — explicit venv creation, then install from a lockfile
- name: install-deps
  run: |
    python -m venv .venv
    .venv/bin/pip install -r requirements.txt

# poetry — virtualenvs.in-project=true puts .venv inside the worktree
- name: install-deps
  run: poetry install --no-root --sync
```

For pip-based setups, store the venv inside the worktree (`./.venv/`) so
worktrees don't fight over a shared environment. uv does this automatically.

### Rust — cargo

```yaml
- name: fetch-deps
  run: cargo fetch --locked
```

`cargo fetch` populates the local registry cache. It does **not** build anything
— that's [Background warmup](/recipes/background-warmup)'s job. Mixing them
makes worktree creation feel slow without need.

### Go — modules

```yaml
- name: fetch-modules
  run: go mod download
```

Like Rust, this is fetch-only. Build warmup is a separate concern.

## Idempotency & safety

Most package managers are idempotent — running install twice with the same
lockfile is a near-no-op. But idempotency comes from the **lockfile-honoring**
command, not the dev-friendly variant:

| Command                          | Idempotent? | Why                                          |
| -------------------------------- | ----------- | -------------------------------------------- |
| `pnpm install --frozen-lockfile` | yes         | Refuses to mutate the lockfile               |
| `pnpm install`                   | no          | May rewrite lockfile, drift across worktrees |
| `npm ci`                         | yes         | Wipes `node_modules`, installs from lockfile |
| `npm install`                    | no          | Mutates `package.json` if deps are missing   |
| `uv sync --frozen`               | yes         | Refuses to update lockfile                   |
| `cargo fetch --locked`           | yes         | Errors if `Cargo.lock` would change          |
| `go mod download`                | yes         | Module cache is content-addressed            |

In hooks, always reach for the strict variants. A worktree create that silently
rewrites a lockfile is a worktree create that destroys reproducibility — exactly
the thing daft hooks are meant to give you.

## Tuning the failure mode

By default, `worktree-post-create` failures **warn**: the worktree is created
even if install fails, leaving you with a half-set-up worktree to retry from. To
make a failed install abort creation instead:

```bash
git config daft.hooks.worktreePostCreate.failMode abort
```

The default is `warn` because flaky installs (registry timeouts, slow mirrors)
are usually recoverable by re-running, and you'd rather have a worktree to retry
from than no worktree at all.

## Where to next

- **[Background warmup](/recipes/background-warmup)** — once deps are installed,
  kick off a build (`cargo build`, `vite optimize`) in the background so the
  first command is fast.
- **[Declarative envs](/recipes/declarative-envs)** — when tool versions (Node
  22, Python 3.13, Rust 1.84) also need to be pinned per-worktree.
- **[Sharing caches across worktrees](/recipes/sharing-caches)** — the per-tool
  answer for what's safe to share (pnpm store, cargo registry) vs not
  (`node_modules/`, `target/`).
