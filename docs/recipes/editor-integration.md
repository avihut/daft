---
title: Editor integration
description:
  Per-worktree IDE settings that point at the worktree's own .venv, target/, and
  node_modules — so VS Code and IntelliJ pick up the right state without "Select
  Interpreter" rituals.
pillars: [worktrees]
---

# Editor integration

## Starting state

A Python data team using daft worktrees — several active branches per dev. One
worktree:

```
analytics/branches/feat-anomalies/
├── .venv/              # per-worktree, populated by `uv sync`
├── pyproject.toml
├── uv.lock
└── src/
```

`.vscode/` is gitignored; `.idea/` is gitignored. The team decided personal IDE
settings stay personal after committing them caused churn — keybinding profiles
drifted, debug configurations conflicted, half the team kept reverting the file
in their PRs.

The ritual: open a fresh worktree in VS Code; see the "Python interpreter not
selected" prompt at the bottom right; click "Select Interpreter"; navigate to
`.venv/bin/python`; close the prompt; start coding. New contributors miss the
prompt half the time, point at system Python by mistake, and chase import errors
that "work fine for me" gives no clue about. The team's `#py-help` channel has
had three threads about it.

The reach for daft: a `worktree-post-create` hook seeds `.vscode/settings.json`
with the worktree's own interpreter path. The IDE picks it up the first time the
worktree opens. No prompt; no system-Python confusion.

## What changes

A new daft hook job materializes `.vscode/settings.json` on worktree create. It
only writes the file if it doesn't already exist — devs who customize their IDE
settings keep their customizations; fresh worktrees get the team defaults.

The previous patterns made the worktree _filesystem_ correct (per-worktree venv,
`target/`, `node_modules/`). This pattern makes the IDE _consume_ that
filesystem correctly.

## Recipe

```yaml
# daft.yml
hooks:
  worktree-post-create:
    jobs:
      - name: seed-vscode-settings
        run: |
          if [ ! -f .vscode/settings.json ]; then
            mkdir -p .vscode
            cat > .vscode/settings.json <<'EOF'
            {
              "python.defaultInterpreterPath": "${workspaceFolder}/.venv/bin/python",
              "python.terminal.activateEnvironment": true,
              "search.exclude": {
                "**/.venv": true,
                "**/__pycache__": true
              },
              "files.watcherExclude": {
                "**/.venv/**": true
              }
            }
            EOF
          fi
```

The `if [ ! -f ... ]` guard is the idempotency — the hook re-runs safely and
existing files are left alone. `${workspaceFolder}` is a VS Code variable that
expands at runtime to the open folder, so the seeded file is portable; no
absolute paths get baked in.

After `daft start feature/x`, the new worktree opens in VS Code with the right
interpreter selected automatically. The Python language server resolves imports
against the worktree's `.venv`; "find symbol" returns symbols from this
worktree's code, not a sibling's. New VS Code terminals already have `.venv/bin`
on `PATH` — no manual `source .venv/bin/activate`.

## Variants

By **editor**.

### VS Code — other languages

The Recipe is Python-shaped; the same hook pattern works for any language.
Replace the JSON body:

**Rust:**

```json
{
  "rust-analyzer.cargo.targetDir": "${workspaceFolder}/target",
  "rust-analyzer.checkOnSave": true,
  "search.exclude": { "**/target": true }
}
```

The explicit `targetDir` matters when a Cargo workspace would otherwise inherit
a different default. The search exclude keeps "find references" from indexing
the build output.

**Node:**

```json
{
  "typescript.tsdk": "node_modules/typescript/lib",
  "search.exclude": {
    "**/node_modules": true,
    "**/.next": true,
    "**/dist": true
  }
}
```

`tsdk` pins TypeScript to the workspace's installed version, so two worktrees
with different TypeScript versions don't fight over a globally-installed one.

### IntelliJ / PyCharm

IntelliJ's per-project config lives in `.idea/`. The Python interpreter pointer
is in `.idea/misc.xml`; the SDK entry itself is in
`~/.config/JetBrains/<IDE>/options/jdk.table.xml` (a global file, not
per-worktree).

For Python, the cleanest path is to register the venv as a per-worktree SDK once
(IntelliJ "Add Interpreter" → "Existing environment" → `.venv/bin/python`) and
not try to seed `.idea/` from a hook. The `.idea/` XML format is brittle; small
mismatches fail silently and leave the project in an inconsistent state.

For Cargo projects, IntelliJ's Rust plugin auto-detects `Cargo.toml` and the
per-worktree `target/` — no per-worktree IDE config needed.

If automation matters more than safety, a one-time `bin/setup-ide.sh` script the
team runs after adopting daft is the safer move than a daft hook that writes to
`.idea/`.

### Helix / LSP-generic

For editors driven by the language server's own config (Helix, Neovim without
project-local settings), the LSP server reads its config from project-committed
files. For Python with pyright / basedpyright, drop a `[tool.pyright]` section
in `pyproject.toml`:

```toml
[tool.pyright]
venvPath = "."
venv = ".venv"
```

This is committed to the repo, not seeded by the hook — every worktree gets it
via `git checkout`. Helix and other LSP-driven editors pick it up on open. No
daft hook needed.

## Idempotency & safety

The Recipe's `if [ ! -f ... ]` guard ensures re-running the hook never clobbers
a user's customized settings. New worktrees get the defaults; existing worktrees
with edited settings keep them. If the team needs to roll out a settings update,
two paths: bump the seeded JSON _and_ delete the user's `.vscode/settings.json`
per worktree to take the new defaults, or accept that updates only land in fresh
worktrees going forward.

::: warning Don't seed secrets into editor configs

If a team commits `.vscode/settings.json` (some do, even gitignored ones sneak
in via PRs), the file ends up in git history. Never seed paths that include
credentials, hostnames, or values you wouldn't share — keep secrets in `.env`
(loaded via direnv or a hook) and reference env-var names, not values, in editor
settings.

:::

## Where to next

- **[Walkthroughs → Rust binary with debug warmup](/recipes/walkthroughs/rust-binary)**
  — where per-worktree `target/` matters most, and what rust-analyzer cares
  about beyond just the directory.
- **[Sharing caches across worktrees](/recipes/sharing-caches)** — what's safe
  to share across worktrees vs what needs per-worktree state, which informs what
  the editor should index vs ignore.
- **[Lifecycle hooks](/hooks/lifecycle)** — `worktree-post-create` timing
  relative to when the editor opens the folder.
