---
title: Adopting from mise
description:
  Layer daft hooks underneath an existing mise setup — mise keeps managing tool
  versions and env defaults; daft adds the install/services/cleanup rituals mise
  was never meant to handle.
pillars: [worktrees, hooks]
---

# Adopting from mise

::: tip daft pairs with mise

For environment management, daft recommends pairing with mise or direnv rather
than trying to cover env management with daft alone — both are more
comprehensive than vanilla daft's per-job `env:` blocks. This guide is the mise
side of that pairing (you already have mise; you're adding daft). If you're
going the other direction — adding mise to a daft-only setup — see
[Layering mise on daft](/recipes/layering-mise).

:::

## Starting state

The team adopted mise a while back. The repo's `mise.toml` pins tool versions
and exports a few non-secret env defaults:

```toml
# mise.toml
[tools]
node = "22"
python = "3.13"
rust = "1.84"

[env]
NODE_ENV = "development"
LOG_LEVEL = "debug"
```

Every dev has `eval "$(mise activate zsh)"` (or the bash/fish equivalent) in
their shell rc, so versions and env switch automatically when they `cd` into the
project.

The README's "Getting started" section reads: _"first clone? `mise install`,
then `pnpm install`, `docker compose up -d`, and `scripts/codegen.sh`."_

The ritual: clone, run `mise install` (mise prompts if any pinned versions
aren't on disk yet), then work through the rest of the README from memory. mise
solves the tools-and-env half of setup — `node` resolves to 22, `NODE_ENV` is
exported. The other half — the slow stuff (deps install, services up, codegen) —
still relies on muscle memory.

That was tolerable for a single working tree. With daft worktrees, the slow
rituals fire dozens of times a month. Sooner or later someone runs `pnpm test`
before `pnpm install` finished and chases a missing-module error.

The reach for daft: don't replace mise — layer hooks underneath it. mise keeps
managing what activates on `cd`; daft hooks pick up the rituals mise was never
meant to handle.

## What changes

A new `daft.yml` adds the install / services / cleanup work to
`worktree-post-create` and `worktree-pre-remove`. **Nothing changes in
`mise.toml`.** Tool versions, the `[env]` block, mise tasks — all stay where
they are.

Hooks run with the worktree as cwd. They don't run inside an mise-activated
shell — but that's fine; hooks invoke binaries by name and pick them up via the
parent shell's `PATH` (which mise has set up if daft was invoked from a
mise-activated shell). Per-job `env:` covers any extra context the hook needs.

## Recipe

```yaml
# daft.yml
hooks:
  worktree-post-create:
    jobs:
      - name: install-tool-versions
        run: mise install

      - name: install-deps
        run: pnpm install --frozen-lockfile
        needs: [install-tool-versions]

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

Existing `mise.toml` stays as-is.

A fresh `daft start feature/x` now lands in a worktree with tool versions
installed, deps installed, services running, codegen warming up — and mise's
"tools + env on cd" behavior intact when you `cd` in. The README's "after
`mise install`, also run pnpm install / compose up" muscle memory is gone.

## Variants

By **what mise is already managing** in your project. Each variant names a thing
mise covers and what — if anything — daft adds.

### mise pins tool versions via `[tools]`

mise activation runs when the user `cd`s into the worktree; the binaries resolve
via shell `PATH`. If a pinned version isn't on disk yet, mise prompts the user
on the next cd — one-time, until installed.

The Recipe includes `mise install` as the first job to install missing versions
eagerly, so there's no surprise prompt later. The cost is ~5–30s per worktree
create, depending on how many versions are missing. If you'd rather defer that
to the user's first cd, drop the `install-tool-versions` job:

```yaml
# daft.yml — drop install-tool-versions, let mise prompt on cd
- name: install-deps
  run: pnpm install --frozen-lockfile
```

### mise exports env via `[env]`

Skip an env-exporting job in `daft.yml`. mise's activation already exports the
`[env]` values when the user `cd`s into the worktree. Hook jobs that need the
same values can either re-export them in per-job `env:` (best for determinism)
or rely on the parent shell having activated mise (if daft was invoked from an
mise-activated shell):

```yaml
- name: migrate
  run: pnpm db:migrate
  env:
    DATABASE_URL: postgres://localhost/myapp_dev # re-exported for the hook
```

For secrets: `mise.toml` is committed (it's in the repo). Don't put real secrets
in `[env]`. Use a per-worktree `.env` (gitignored, optionally loaded via mise's
`_.file` setting or via direnv layered on top), or fetch from a vault — see
[Env vars & secrets](/recipes/env-vars-and-secrets).

### mise has `[tasks]`

mise's `[tasks]` is a developer-convenience task runner, not a hook system. It
doesn't run on worktree create. Don't try to reuse `[tasks]` entries in
`daft.yml` — keep the boundaries clean:

- `[tasks]` is for **on-demand** workflows: "start the dev server," "run the
  full test suite," "build the release artifact."
- `daft.yml` is for **lifecycle automation**: "what runs when a worktree is
  created or removed."

If your `[tasks.setup]` does the same thing as your `daft.yml`
`worktree-post-create`, that's a duplication you should resolve — usually in
favor of `daft.yml`, since it runs automatically on worktree create rather than
waiting for the user to remember `mise run setup`.

## Idempotency & safety

`mise install` is idempotent — already-installed versions are silently skipped.
mise's shell activation is idempotent the same way. Re-running the daft hook on
an existing worktree is safe; both mise's part and daft's part tolerate
already-done state.

::: warning Don't seed `mise.toml` from a daft hook

Resist the urge to have a daft hook write to `mise.toml`. The file is committed;
rewriting it from a hook would mean the hook contributes to git history (noisy
diffs) and trust prompts fire on every worktree create (changed file content).
Manage `mise.toml` by hand or via `mise use` from your shell.

:::

## Where to next

- **[Adopting from direnv](/recipes/adopting-from-direnv)** — the companion
  recipe for teams using direnv instead of mise.
- **[Declarative envs](/recipes/declarative-envs)** — deeper on mise's `[tools]`
  / `[env]` / `[tasks]` semantics; where this page positions mise as a peer to
  daft, declarative-envs makes mise the primary subject.
- **[Env vars & secrets](/recipes/env-vars-and-secrets)** — for hook-time vs
  shell-time env, and for secrets that shouldn't sit in `mise.toml`.
