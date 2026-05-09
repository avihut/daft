---
title: Migrating from direnv to mise
description:
  Replace scattered tool-version files and direnv layout stanzas with one
  mise.toml. mise.toml becomes the source of truth for tools and non-secret env;
  daft.yml stays untouched.
pillars: [worktrees, hooks]
---

# Migrating from direnv to mise

## Starting state

A polyglot project on direnv. Tool versions live in three places, and `.envrc`
ties them together with a few exports:

```
my-app/
├── .envrc
├── .nvmrc
├── .python-version
├── .env.example
├── apps/
└── daft.yml
```

```bash
# .envrc
use_python_venv 3.11
dotenv_if_exists .env
PATH_add bin
export NODE_ENV=development
export LOG_LEVEL=debug
[ -f .nvmrc ] && nvm use --silent 2>/dev/null
```

```
# .nvmrc
22
```

```
# .python-version
3.11.7
```

The team's been running this for a while. It works, but:

- Three separate version files, each read by a different tool. Half the team has
  nvm; some have asdf-direnv; some have pyenv. Each handles `.nvmrc` /
  `.python-version` differently.
- direnv requires `direnv allow` per worktree, and re-allow after every `.envrc`
  edit. With daft worktrees that's a per-worktree ritual that adds up.
- Tool version drift between devs is a recurring "works on my machine" source.

The goal: replace direnv with mise. Tools and env become declarative in one file
(`mise.toml`). Activation is consistent across the team. `daft.yml` stays
untouched.

## Patterns we'll thread

This walkthrough applies the [Declarative envs](/recipes/declarative-envs)
pattern (mise variant), and the team-wide outcome is the
[Adopting from mise](/recipes/adopting-from-mise) steady state.

By the end:

- `mise.toml` replaces `.nvmrc`, `.python-version`, and the export lines from
  `.envrc`
- mise handles tool installs (replacing per-dev nvm / pyenv)
- The `[env]` block in `mise.toml` exports non-secret defaults on cd
- A separate `.env` (gitignored) handles real secrets, loaded by mise's optional
  dotenv support _or_ by a trimmed-to-one-line `.envrc`

## Step 1: install mise team-wide

Each dev installs mise once and adds activation to their shell rc:

```bash
brew install mise
echo 'eval "$(mise activate zsh)"' >> ~/.zshrc
```

(Or the bash / fish equivalent.) Until everyone's done this, keep direnv
installed alongside — they coexist as long as they manage different vars.

## Step 2: convert tool versions to `mise.toml`

Create `mise.toml` at the repo root with the versions from the old files:

```toml
# mise.toml
[tools]
node = "22"
python = "3.11.7"
```

Install the versions on disk:

```bash
mise install
```

Then delete the old version files:

```bash
git rm .nvmrc .python-version
git add mise.toml
```

mise's activation now switches versions on `cd`, replacing per-dev nvm / pyenv
invocations.

## Step 3: convert env defaults to `[env]`

The `.envrc` had shell exports for non-secret defaults. Move them to
`mise.toml`'s `[env]` block:

```toml
# mise.toml — adding [env]
[tools]
node = "22"
python = "3.11.7"

[env]
NODE_ENV = "development"
LOG_LEVEL = "debug"
```

Delete the matching exports from `.envrc`:

```bash
# .envrc — exports gone
use_python_venv 3.11
dotenv_if_exists .env
PATH_add bin
[ -f .nvmrc ] && nvm use --silent 2>/dev/null  # also delete this
```

Result so far in `.envrc`:

```bash
# .envrc — partial
use_python_venv 3.11
dotenv_if_exists .env
PATH_add bin
```

## Step 4: handle secrets

`dotenv_if_exists .env` was your secret-loading mechanism. mise's `[env]` is
committed and isn't appropriate for secrets. Two options keep secret loading
working without direnv as a separate tool:

**Option A — keep direnv just for `.env`.**

`.envrc` shrinks to one line:

```bash
# .envrc — secrets only; mise handles tools + non-secret env
dotenv_if_exists .env
```

direnv and mise coexist: mise owns tools and `[env]`; direnv owns `.env`
loading. They don't conflict as long as they manage different vars.

**Option B — use mise's dotenv support.**

mise can load `.env` directly:

```toml
# mise.toml
[env]
_.file = ".env"
NODE_ENV = "development"
LOG_LEVEL = "debug"
```

mise reads `.env` on activation. One less tool in the dependency chain.

**Decision rule:** pick A if your `.envrc` has other stanzas you still want
(`PATH_add`, `layout python`) — keep direnv for those. Pick B if `.env` loading
is the only thing left in `.envrc`.

## Step 5: handle remaining `.envrc` stanzas

Two remaining stanzas from the original `.envrc`:

- `use_python_venv 3.11` — direnv-specific virtualenv activation. Not directly
  portable to mise; mise activates Python via `[tools] python = "3.11.7"` but
  doesn't auto-create a project venv. Move the venv creation into a `daft.yml`
  hook instead (pre-create the `.venv/` so any tool that expects it finds it):

  ```yaml
  # daft.yml — append to worktree-post-create
  - name: install-python-venv
    run: |
      if [ ! -d .venv ]; then python -m venv .venv; fi
      .venv/bin/pip install -e .
    needs: [install-tool-versions]
  ```

  And drop `use_python_venv` from `.envrc`.

- `PATH_add bin` — direnv-specific. mise has no direct equivalent; scripts in
  `bin/` should be invoked with `./bin/` prefix in hooks or bound to `[tasks]`
  in `mise.toml`. Drop the line if no shell workflow depends on `bin/` being on
  `PATH`; otherwise keep direnv (Option A above).

## Step 6: simplify or remove `.envrc`

If you went with Option B (no direnv) and removed all stanzas, delete `.envrc`:

```bash
git rm .envrc
```

If Option A, `.envrc` is one or two lines. Commit the simplified version. Devs
run `direnv allow` once per existing worktree to refresh trust.

## Step 7: verify `daft.yml` still works

`daft.yml` likely needs no structural change. Hooks invoke binaries by name;
mise activates them at cd time, and daft hooks running with the worktree as cwd
inherit the parent shell's `PATH` (which mise has set up if daft was invoked
from a mise-activated shell).

Test on a fresh worktree:

```bash
daft start feature/test-mise-migration
# Verify install-deps, services-up, codegen complete
```

If a hook fails with "command not found," the parent shell didn't have mise
activation. Run `mise install` once and ensure `eval "$(mise activate zsh)"` is
in your shell rc.

## Final state

```
my-app/
├── mise.toml          # tools + env defaults (+ _.file=.env if Option B)
├── .envrc             # Option A only: one dotenv line
├── .env.example
├── apps/
└── daft.yml           # appended Python venv install if it had use_python_venv

DELETED:
- .nvmrc
- .python-version
- (Option B) .envrc
```

## What you got

Before:

- Three tool-version files (`.nvmrc`, `.python-version`, possibly `Dockerfile`
  Rust pin)
- `.envrc` with `use_python_venv`, `dotenv`, `PATH_add`, `nvm use`, and shell
  exports
- Each dev runs whatever tool they have (nvm, asdf, pyenv) — drift is routine
- `direnv allow` required per worktree, re-allow after every edit

After:

- One file (`mise.toml`) for tools and non-secret env defaults
- Optional `.envrc` (one line) for secrets if you kept direnv (Option A), or no
  `.envrc` at all (Option B)
- mise activation is consistent across the team
- No `direnv allow` per worktree (unless Option A, in which case once per
  worktree for the trimmed file)

## Where to next

- **[Adopting from mise](/recipes/adopting-from-mise)** — the steady- state
  recipe for daft + mise teams (which you now are).
- **[Declarative envs](/recipes/declarative-envs)** — deeper on mise's `[tools]`
  / `[env]` / `[tasks]` semantics.
- **[Migrating from mise to direnv](/recipes/walkthroughs/migrating-from-mise-to-direnv)**
  — the reverse direction, if a future team decision flips back.
