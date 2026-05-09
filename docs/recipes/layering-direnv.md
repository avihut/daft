---
title: Layering direnv on daft
description:
  Add direnv to a daft-only project. direnv loads .env and any layout stanzas on
  cd; daft.yml's existing hooks remain unchanged.
pillars: [worktrees, hooks]
kind: adoption
---

# Layering direnv on daft

## Starting state

A team running daft alone. The `daft.yml` handles install, services, and
cleanup; secrets are loaded by hand — someone runs `set -a; source .env; set +a`
in their shell after every clone, or exports the vars they need each session.

```yaml
# daft.yml — abridged
hooks:
  worktree-post-create:
    jobs:
      - name: install-deps
        run: pnpm install --frozen-lockfile
      - name: services-up
        run: docker compose up -d --wait
        needs: [install-deps]
```

The README has a "Source `.env` before running anything" line that new
contributors miss. Mid-session, devs forget `DATABASE_URL` is loaded from `.env`
and run a one-off `psql` command against the wrong host.

The reach for daft + direnv: load `.env` automatically on every cd, so "did I
export the env vars?" becomes a non-question.

## What changes

A new `.envrc` at the root tells direnv what to load. Each dev installs direnv
once and adds the shell hook. direnv reads `.envrc` on cd and exports whatever
it specifies — `.env` contents, `bin/` on `PATH`, language-specific layout
stanzas.

`daft.yml` doesn't need to change. Hooks run with the worktree as cwd; direnv's
exports are at shell time, not hook time, but that's fine — the hook either
doesn't need shell-level env (build-time-only work) or sources `.env` itself
when it does.

## Recipe

Three things land.

1. `.envrc` at the repo root:

```bash
# .envrc
dotenv_if_exists .env
PATH_add bin
```

2. direnv installed and activated in each dev's shell rc (one-time, per dev):

```bash
brew install direnv
echo 'eval "$(direnv hook zsh)"' >> ~/.zshrc
```

3. `direnv allow` once per worktree (and again whenever `.envrc` changes):

```bash
direnv allow
```

A fresh `daft start feature/x` followed by `cd` into the worktree now loads
`.env` and adds `bin/` to `PATH` automatically. The "remember to source .env"
line in the README is gone.

## Variants

By **what `.envrc` does** for your project.

### Just `.env` loading

The minimal Recipe. `dotenv_if_exists .env` exports values from `.env` when
present; that's all. `.env` is gitignored; each dev fills in their own values
from a `.env.example` template:

```bash
# .envrc
dotenv_if_exists .env
```

### `.env` + project bin on `PATH`

Add `PATH_add bin`:

```bash
# .envrc
dotenv_if_exists .env
PATH_add bin
```

Project scripts in `bin/` now resolve as bare commands in any worktree shell —
no `./bin/foo` prefix needed at the shell prompt. Hooks still need the prefix
because they don't run inside a direnv-loaded shell.

### Per-language layouts (`layout python`, `layout ruby`)

If your project benefits from a per-directory virtualenv or gemset, direnv's
`layout` stanzas do the work:

```bash
# .envrc — Python project
dotenv_if_exists .env
layout python python3.11
```

direnv creates `.direnv/python-3.11/` and activates it on cd. Move
venv-population (`pip install -e .`) into a daft hook so it doesn't happen at
shell-load time — see [Adopting from direnv](/recipes/adopting-from-direnv) for
the layout-style hook job.

## Idempotency & safety

direnv re-evaluates `.envrc` whenever the file changes, re-prompting for trust
each time. Within a single worktree session, exports are stable until the next
cd-out and cd-back-in.

::: warning Don't seed `.envrc` from a daft hook

A daft hook that writes to `.envrc` triggers direnv's trust prompt every
worktree create. Either keep `.envrc` static (manage it by hand, commit changes)
or use mise's `_.file` for the per-worktree-derived case — see
[Layering mise on daft](/recipes/layering-mise).

:::

## Where to next

- **[Adopting from direnv](/recipes/adopting-from-direnv)** — the steady-state
  recipe for daft + direnv teams (which you now are).
- **[Env vars & secrets](/recipes/env-vars-and-secrets)** — for vault-fetched
  patterns when `.env` doesn't suffice.
- **[Layering mise on daft](/recipes/layering-mise)** — the alternative if you'd
  rather have mise than direnv.
