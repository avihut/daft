---
title: Declarative envs
description:
  mise / asdf / nvm / pyenv as the declarative half of worktree setup — tool
  versions, committed env defaults, and what daft hooks add on top.
pillars: [worktrees, hooks]
---

# Declarative envs

## Starting state

A polyglot service. Tool versions are pinned in three different files in three
different parts of the repo:

```
apps/api/.nvmrc                18
apps/etl/.python-version       3.11
apps/auth/Dockerfile           FROM rust:1.78  (the only place rust is pinned)
```

Three pinning files, three different mechanisms, three different runtimes that
may or may not pick them up. `.nvmrc` only matters if you remember `nvm use`.
`.python-version` only matters if pyenv is installed and `pyenv shell` is wired
into your shell rc. The Rust version is _implicit_ until the Docker build —
locally everyone runs whatever `rustc` they last installed.

Bugs that "only repro on Alex's machine" turn out to be version mismatches —
Node 20 vs 18, Rust 1.79 vs 1.78. The `.nvmrc` file exists; nobody ran `nvm use`
after `cd`-ing.

The reach for daft: stop relying on muscle memory. Tool versions should
_activate_ on cd, not when you remember to run a command.

## What changes

One `mise.toml` (committed) replaces the three pinning files. mise's shell hook
reads it on `cd` and prepends the right binaries to `PATH`, so `node`, `python`,
and `cargo` resolve to the pinned versions the moment you enter a worktree.

A daft hook handles the install half — making sure those pinned versions are
actually present on disk before the worktree is "ready."

What this gets you: every dev's `node --version` matches every other dev's,
every worktree, every time. "Only repros on my machine" stops being a Node
version mismatch.

## Recipe

```toml
# mise.toml
[tools]
node = "22"
python = "3.13"
rust = "1.84"
```

```yaml
# daft.yml
hooks:
  worktree-post-create:
    jobs:
      - name: install-tool-versions
        run: mise install
```

Activation is mise's job, not daft's. The parent shell's hook
(`eval "$(mise activate zsh)"` in your `~/.zshrc` or equivalent) detects
`mise.toml` on `cd` and switches versions for you. The daft hook only does the
install half — materializing missing versions into
`~/.local/share/mise/installs/` so activation can find them.

Prerequisites: [mise](https://mise.jdx.dev) installed (`brew install mise` on
macOS) and its shell activation loaded.

## Committed env defaults — `mise.toml` `[env]`

Tool versions are one half of "what every dev has when they cd into a worktree."
Non-secret env defaults are the other. mise's `[env]` block does both in one
file:

```toml
# mise.toml
[tools]
node = "22"
python = "3.13"

[env]
NODE_ENV = "development"
DATABASE_URL = "postgres://localhost/myapp_dev"
LOG_LEVEL = "debug"
```

When mise activates the worktree, the `[env]` values export. No daft hook needed
— `[env]` activation is part of the same shell-hook flow that switches tool
versions.

The trade-off: `mise.toml` is committed, so its `[env]` block is fine for
**non-secret defaults** (a placeholder DATABASE_URL, NODE_ENV, log levels) but
never for actual secrets. Anything that shouldn't be in git stays out of
`mise.toml`. For real secrets, see
[Env vars & secrets](/recipes/env-vars-and-secrets).

## Variants

By tool. mise is the recommended choice for new projects; the others are
documented for teams already using them.

### asdf

```yaml
# daft.yml
hooks:
  worktree-post-create:
    jobs:
      - name: install-tool-versions
        run: asdf install
```

```
# .tool-versions  (committed)
nodejs 22.11.0
python 3.13.0
```

asdf needs per-language plugins (`asdf plugin add nodejs`,
`asdf plugin add python`) installed once per machine. Plugin install isn't part
of `asdf install`; document it in your README or wire it into a one-time
`bin/setup-asdf.sh` for new contributors.

mise can read `.tool-versions` as a fallback, so a slow migration off asdf is
straightforward: install mise, leave the file in place, activate mise's shell
hook. The `mise.toml` migration follows when ready.

### nvm — Node only

```yaml
- name: install-node
  run: |
    source "$NVM_DIR/nvm.sh"
    nvm install
    nvm use
```

```
# .nvmrc  (committed)
22
```

nvm is bash-functions, not a binary, so the hook has to source `nvm.sh` before
calling `nvm`. Use it if Node is your only versioned tool. If you also need
Python or Rust, switching to mise is cleaner than running three single-language
tools.

### pyenv — Python only

```yaml
- name: install-python
  run: pyenv install --skip-existing
```

```
# .python-version  (committed)
3.13.0
```

`--skip-existing` makes the hook idempotent (already-installed versions are
silently skipped). Like nvm, single-language only — mise reads `.python-version`
as a fallback if you migrate later.

## Division of labor: declarative vs imperative

| Concern                                     | Where it goes                                                   |
| ------------------------------------------- | --------------------------------------------------------------- |
| Tool versions (Node, Python, Rust)          | `mise.toml` / `.tool-versions`                                  |
| Committed env-var defaults (non-secret)     | `mise.toml` `[env]`                                             |
| Secret env vars                             | daft hook ([Env vars & secrets](/recipes/env-vars-and-secrets)) |
| Install dependencies (`pnpm install`, etc.) | daft hook ([Toolchain bootstrap](/recipes/toolchain-bootstrap)) |
| Background warmup (`cargo build`, …)        | daft hook ([Background warmup](/recipes/background-warmup))     |
| Service orchestration (compose up)          | daft hook ([Services with ports](/recipes/services-with-ports)) |
| Cleanup on remove                           | daft hook ([Cleanup on remove](/recipes/cleanup-on-remove))     |
| Ad-hoc developer task ("run dev server")    | mise `[tasks]` _or_ `package.json` scripts                      |

The declarative tool **describes** what should be there; daft hooks **do** what
needs doing on lifecycle events.

::: warning Don't put long-running setup in `mise.toml` `[tasks]`

mise's `[tasks]` is a developer-convenience task runner, not a hook system. It
doesn't run on worktree create. Putting `pnpm install` in `[tasks.setup]` means
the user has to remember `mise run setup` after every worktree create — which is
exactly what you adopted daft to stop doing. Use `worktree-post-create` for
setup; reserve `[tasks]` for on-demand workflows like "start the dev server" or
"run the full test suite."

:::

## Where to next

- **[Toolchain bootstrap](/recipes/toolchain-bootstrap)** — the typical flow is
  `mise install` (this page) before `pnpm install` (that one). `needs:` between
  them makes the order explicit.
- **[Env vars & secrets](/recipes/env-vars-and-secrets)** — for the secret half
  of "what gets exported when you cd into a worktree."
- **[CI parity](/recipes/ci-parity)** — both `mise install` and `daft hooks run`
  work the same locally and in CI; one source of truth for "how this project
  sets up."
