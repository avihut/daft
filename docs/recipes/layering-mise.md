---
title: Layering mise on daft
description:
  Add mise to a daft-only project. mise pins tool versions and exports
  non-secret env on cd; daft.yml's existing hooks pick up the consistent
  toolchain.
pillars: [worktrees, hooks]
kind: adoption
---

# Layering mise on daft

## Starting state

A team running daft alone. The `daft.yml` handles install, services, and
cleanup; tool versions are managed externally — devs use system Node and Python
from Homebrew, or whatever single-language manager they happen to have (one team
member uses nvm, another rbenv, a third does nothing and runs whatever
`node --version` returns).

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

The README has a "Required versions" line ("Node 22, Python 3.13") that the team
ignores half the time. Bugs that "only repro on Alex's machine" have started
turning out to be Node 22 vs 20 mismatches.

The reach for daft + mise: pin tool versions declaratively in one file that
activates on cd, so "what version of Node does this project use" becomes a
non-question.

## What changes

A new `mise.toml` at the root pins tools and (optionally) exports non-secret env
defaults. Each dev installs mise once and adds shell activation. mise's
activation switches versions on cd; the daft hook inherits the parent shell's
`PATH`, so binaries like `pnpm` resolve to the pinned versions.

`daft.yml` doesn't strictly need to change. Adding `mise install` as the first
job is a small upgrade: missing versions install eagerly on worktree create
rather than prompting on the user's first cd.

## Recipe

Three things land.

1. `mise.toml` at the repo root:

```toml
# mise.toml
[tools]
node = "22"
python = "3.13"
```

2. mise installed and activated in each dev's shell rc (one-time, per dev):

```bash
brew install mise
echo 'eval "$(mise activate zsh)"' >> ~/.zshrc
```

(Or the bash / fish equivalent.)

3. (Optional) prepend `mise install` to `daft.yml` so missing versions install
   eagerly on worktree create:

```yaml
# daft.yml — prepend to worktree-post-create
- name: install-tool-versions
  run: mise install

- name: install-deps
  run: pnpm install --frozen-lockfile
  needs: [install-tool-versions]
```

A fresh `daft start feature/x` now lands in a worktree where `node` and `python`
resolve to the pinned versions. Existing daft hooks consume them transparently.
The README's "Required versions" line goes away — `mise.toml` is the source of
truth.

## Variants

By **what mise covers** in the new setup.

### Just tool versions (`[tools]` only)

The minimal Recipe. mise pins versions; nothing else. Non-secret env defaults
stay in your shell rc or in a `.env` you load some other way.

### Tools + non-secret env defaults (`[tools]` + `[env]`)

Add an `[env]` block for committed defaults:

```toml
# mise.toml
[tools]
node = "22"
python = "3.13"

[env]
NODE_ENV = "development"
LOG_LEVEL = "debug"
DATABASE_URL = "postgres://localhost/myapp_dev"
```

mise activates these on cd. Because `mise.toml` is committed, only non-secret
values belong here — placeholders, log levels, development-environment URLs.

### Tools + env + dotenv (`_.file = ".env"`)

For real per-dev secrets, point mise at a gitignored `.env`:

```toml
# mise.toml
[env]
_.file = ".env"
NODE_ENV = "development"
```

Each dev fills in their own `.env`; mise loads it on cd alongside the committed
`[env]` defaults.

## Idempotency & safety

mise activation and `mise install` are both idempotent — re-running either is a
near-no-op when state is already correct.

::: warning Don't put real secrets in `mise.toml`

`mise.toml` is committed. Anything in `[env]` ships to the repo and into git
history. Use `_.file = ".env"` (with `.env` gitignored) or a vault fetch
pattern. See [Env vars & secrets](/recipes/env-vars-and-secrets).

:::

## Where to next

- **[Adopting from mise](/recipes/adopting-from-mise)** — the steady-state
  recipe for daft + mise teams (which you now are).
- **[Declarative envs](/recipes/declarative-envs)** — deeper on mise's `[tools]`
  / `[env]` / `[tasks]` semantics.
- **[Layering direnv on daft](/recipes/layering-direnv)** — the alternative if
  you'd rather have direnv than mise.
