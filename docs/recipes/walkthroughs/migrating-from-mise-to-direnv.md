---
title: Migrating from mise to direnv
description:
  The rarer reverse direction — replace mise.toml with .envrc plus a
  single-language version manager. daft.yml stays untouched.
pillars: [worktrees, hooks]
---

# Migrating from mise to direnv

This is the less common direction; most teams move toward mise. Reach for this
walkthrough if your team has decided mise's surface is too big, you want to
consolidate on direnv (which you may already use elsewhere), or a
single-language version manager (nvm, pyenv, rustup) handles everything you
need.

## Starting state

A project on mise. Tool versions and env defaults are declarative:

```
my-app/
├── mise.toml
├── .env.example
├── apps/
└── daft.yml
```

```toml
# mise.toml
[tools]
node = "22"
python = "3.11.7"

[env]
NODE_ENV = "development"
LOG_LEVEL = "debug"
_.file = ".env"
```

Every dev has `eval "$(mise activate zsh)"` in their shell rc. mise activates
tools and env on `cd`, and reads `.env` for secrets via `_.file`.

The team's reasons to switch vary, but the goal is the same: replace `mise.toml`
with `.envrc` and per-language version files. After the switch, the team uses
direnv for env-on-cd and a single-language version manager (nvm for Node, pyenv
for Python) for tool versions.

## Patterns we'll thread

This walkthrough is the inverse of
[Migrating from direnv to mise](/recipes/walkthroughs/migrating-from-direnv-to-mise).
The team-wide outcome is the
[Adopting from direnv](/recipes/adopting-from-direnv) steady state.

By the end:

- `.envrc` replaces `mise.toml`'s `[env]` block (export lines) and `_.file`
  directive (`dotenv_if_exists`)
- Per-language version files (`.nvmrc`, `.python-version`) replace `mise.toml`'s
  `[tools]`
- Tool installs handled by per-dev nvm / pyenv (or rustup, etc.)
- `daft.yml` stays untouched

## Step 1: install direnv team-wide

Each dev installs direnv once and adds the shell hook to their rc:

```bash
brew install direnv
echo 'eval "$(direnv hook zsh)"' >> ~/.zshrc
```

(Or the bash / fish equivalent.) If devs already have direnv from another
project, this step is a no-op for them.

Keep mise installed during the transition so the project still works on machines
that haven't switched yet.

## Step 2: convert tool versions to per-language files

Each version in `mise.toml`'s `[tools]` becomes its own file, read by the
matching version manager:

```
# .nvmrc
22
```

```
# .python-version
3.11.7
```

(`.python-version` is read by pyenv if pyenv is installed; otherwise it's
documentary.) Each dev needs the corresponding version manager on their machine
— nvm for Node, pyenv for Python, rustup for Rust. Document the prerequisites in
the README.

Trigger an install once on each machine to pick up the pinned versions:

```bash
nvm install   # reads .nvmrc
pyenv install --skip-existing  # reads .python-version
```

## Step 3: convert `[env]` to `.envrc` exports

Each `[env]` value becomes a `.envrc` export. Create `.envrc` at the root:

```bash
# .envrc
dotenv_if_exists .env
PATH_add bin

export NODE_ENV=development
export LOG_LEVEL=debug

[ -f .nvmrc ] && nvm use --silent 2>/dev/null
```

Note the additions:

- `dotenv_if_exists .env` replaces mise's `_.file = ".env"`
- The exports replace `[env]` entries
- The `nvm use` line ensures Node version is activated when the shell `cd`s in
  (mise was doing this implicitly via shell PATH)
- `PATH_add bin` is optional — add it if your project has a `bin/` directory the
  team relies on

Run `direnv allow` once per worktree:

```bash
direnv allow
```

## Step 4: delete `mise.toml`

After verifying the new direnv setup works on a couple of devs' machines, delete
`mise.toml`:

```bash
git rm mise.toml
git add .envrc .nvmrc .python-version
```

Optionally, devs can uninstall mise from their machines once nobody's relying on
it. Project-side, you're done.

If the team uses `mise.toml` `[tasks]` for on-demand workflows, you'll need a
replacement: `just`, `make`, or `npm run`-style scripts in `package.json`. Tasks
aren't in scope for this migration walkthrough — see
[Declarative envs](/recipes/declarative-envs#division-of-labor-declarative-vs-imperative)
for where they fit.

## Step 5: verify `daft.yml` still works

`daft.yml` likely needs no changes. Hooks invoke binaries by name; the parent
shell's `PATH` provides them (now via direnv + nvm/pyenv).

Test on a fresh worktree:

```bash
daft start feature/test-direnv-migration
# Verify install-deps, services-up, codegen complete
```

If a hook fails with "command not found," check that direnv has sourced
`nvm`/`pyenv` correctly. nvm specifically requires the `nvm.sh` shell script to
be sourced — most direnv setups source it in the user's `~/.zshrc` before the
direnv hook runs, but some custom shell configurations defer it.

## Final state

```
my-app/
├── .envrc             # exports + dotenv + version-manager activation
├── .nvmrc             # Node version
├── .python-version    # Python version (read by pyenv)
├── .env.example
├── apps/
└── daft.yml           # unchanged

DELETED:
- mise.toml
```

## What you got

Before:

- One file (`mise.toml`) handled tools, non-secret env, and `.env` loading
- mise activation in everyone's shell rc

After:

- `.envrc` for env, `.nvmrc` + `.python-version` for tool versions
- Direnv activation in shell rc; per-language tool managers handle installs
  (nvm, pyenv) per dev

The trade-off you accepted: more files, more tools per dev, but a smaller
surface for mise specifically. Whether that's worth it depends on team
preferences — most teams who move TO mise don't move back.

## Where to next

- **[Adopting from direnv](/recipes/adopting-from-direnv)** — the steady-state
  recipe for daft + direnv teams (which you now are).
- **[Env vars & secrets](/recipes/env-vars-and-secrets)** — for the
  vault-fetched patterns when `.env` doesn't suffice.
- **[Migrating from direnv to mise](/recipes/walkthroughs/migrating-from-direnv-to-mise)**
  — the inverse, if a future team decision flips back.
