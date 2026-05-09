---
title: Declarative envs
description:
  mise / asdf / nvm / pyenv as the declarative alternative to imperative hooks.
  Layering, when to pick which, and division of labor with daft hooks.
pillars: [worktrees, hooks]
---

# Declarative envs

> Imperative hooks ("run `pnpm install`") describe steps. Declarative env tools
> (mise, asdf, nvm, pyenv) describe a target state — "this worktree uses Node 22
> and Python 3.13" — and let the tool figure out how to get there. They activate
> on `cd`, no daft involvement needed for activation. The two compose well:
> declarative for tool versions and committed env defaults, imperative hooks for
> installs and per-worktree dynamism.

## When to reach for this

- You want tool versions pinned per worktree, and you want them to work without
  anyone running an explicit "activate this env" command.
- Your team has a mix of OSes/architectures and a single "install Node 22" line
  in a hook isn't enough — different developers need different install paths.
- You'd rather declare versions in a TOML/YAML/text file than maintain an
  install script.

## Minimal recipe — mise

```toml
# mise.toml  (committed at repo root)
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

What happens: a fresh worktree triggers `mise install`, which materializes any
missing versions into `~/.local/share/mise/installs/...`. mise's shell hook
(`eval "$(mise activate zsh)"` in your shell rc) detects `mise.toml` on `cd` and
prepends the right binaries to `PATH`. By the time you're typing in the
worktree, `node --version` reports 22.

The daft hook does only the install half. Activation is mise's job, not daft's —
the parent shell's hook handles that on `cd`.

Prerequisites: [mise](https://mise.jdx.dev) installed (`brew install mise` on
macOS) and its shell activation loaded.

## Variants

### asdf — the precursor

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

asdf is the elder cousin of mise. The plugin ecosystem is broader (more obscure
tools have asdf plugins), but version resolution and install speed are slower
than mise. If you don't have a reason to use asdf specifically, prefer mise.

mise can read `.tool-versions` directly, so a slow migration off asdf is
straightforward: install mise, leave the file in place, mise picks it up.

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

nvm is single-language. Use it if Node is your only versioned tool and you don't
want the larger mise/asdf surface. The shell-source dance in the hook is awkward
but unavoidable — nvm is bash-functions, not a binary.

If you also need a Python or Rust version, switch to mise.

### pyenv — Python only

```yaml
- name: install-python
  run: pyenv install --skip-existing
```

```
# .python-version  (committed)
3.13.0
```

Like nvm, single-language. mise reads `.python-version` files too, so the
upgrade path is painless if you outgrow pyenv.

### mise's `[env]` for committed defaults

mise also handles env vars (see
[Env vars & secrets → mise's `[env]`](/recipes/env-vars-and-secrets#mise-s-env-section-declarative-no-hook)).
Combining tool versions and env in one declarative file is the closest to "the
worktree's whole config is visible at a glance":

```toml
# mise.toml
[tools]
node = "22"

[env]
DATABASE_URL = "postgres://localhost/myapp_dev"
NODE_ENV = "development"

[tasks.dev]
description = "Run the dev server"
run = "pnpm dev"
```

`mise.toml` even has a `[tasks]` block that's an mini-task-runner. That overlaps
with `daft.yml`'s job system — see the division-of-labor section below.

### devbox / nix-direnv — heavier declarative envs

For projects that need OS-level tools (databases, image libraries, build
toolchains), [devbox](https://www.jetify.com/devbox) and
[nix-direnv](https://github.com/nix-community/nix-direnv) provide Nix-based
declarative environments. The daft hook becomes a `devbox install` or
`direnv allow` step:

```yaml
- name: install-devbox
  run: devbox install
```

These are heavier than mise (Nix store, longer first install) but genuinely
reproducible across machines.

## Division of labor: declarative vs imperative

Use the declarative tool for what it's good at; use daft hooks for the rest:

| Concern                                              | Where it goes                                                                          |
| ---------------------------------------------------- | -------------------------------------------------------------------------------------- |
| Tool versions (Node, Python, Rust)                   | `mise.toml` / `.tool-versions`                                                         |
| Committed env-var defaults                           | `mise.toml` `[env]`                                                                    |
| Secret env vars                                      | daft hook (vault/sops fetch — see [Env vars & secrets](/recipes/env-vars-and-secrets)) |
| Install dependencies (`pnpm install`, `cargo fetch`) | daft hook (`worktree-post-create`)                                                     |
| Background warmup (cargo build, vite optimize)       | daft hook with `background: true`                                                      |
| Service orchestration (compose up)                   | daft hook                                                                              |
| Cleanup on remove                                    | daft hook (`worktree-pre-remove`)                                                      |
| Ad-hoc developer task ("run dev server")             | mise `[tasks]` _or_ `package.json` scripts — outside daft                              |

The declarative tool **describes** what's there; daft hooks **do** what needs
doing on lifecycle events.

::: warning Don't put long-running setup in `mise.toml` `[tasks]` mise's
`[tasks]` block is a developer convenience, not a hook system. It doesn't run on
worktree create. Putting `pnpm install` there as a "setup task" means the user
has to remember to run `mise run setup` after creating a worktree — which is
exactly what you're trying to avoid by adopting daft hooks. :::

## Composes well with

- **[Toolchain bootstrap](/recipes/toolchain-bootstrap)** — the typical flow is
  mise installs the **toolchain** (Node, Python), then a daft hook runs the
  **dependency install** (`pnpm install`). `needs:` makes the order explicit.
- **[Env vars & secrets](/recipes/env-vars-and-secrets)** — committed defaults
  in `mise.toml` `[env]`; secrets via a daft hook.
- **[CI parity](/recipes/ci-parity)** — both mise and daft.yml run in CI; the
  same `mise install && pnpm install --frozen-lockfile` pattern works locally
  and in CI.

## Anti-patterns

- **Duplicating tool installs** — `mise.toml` lists Node 22 _and_ `daft.yml`
  runs `nvm install 22`. Pick one source of truth. (mise wins for new projects.)
- **Imperative installs disguised as declarative** —
  `[tasks.setup] run = "pnpm install"`. Tasks aren't hooks; this still requires
  the user to know to run them.
- **Per-worktree mise overrides committed to a feature branch** — fine if the
  version bump is part of the feature, problematic if it's only for one
  developer's machine. For machine-specific overrides, use `mise.local.toml` and
  gitignore it.

## See also

- **[Lifecycle hooks](/hooks/lifecycle)** — `worktree-post-create` is the
  primary surface declarative envs interact with
- **[Job orchestration](/hooks/job-orchestration)** — `needs:` for ordering
  install-tools before install-deps
- **[mise documentation](https://mise.jdx.dev)** — the canonical reference for
  the tool itself
