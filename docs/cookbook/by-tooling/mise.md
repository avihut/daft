---
title: daft + mise
description:
  Per-worktree tool versions and tasks via mise, automated by daft hooks.
pillars: [worktrees, hooks]
tooling: [mise]
languages: []
---

# daft + mise

> **Goal:** Each worktree boots with the exact tool versions declared in its
> `mise.toml`, automatically — no manual activation.

## Context

[mise](https://mise.jdx.dev) reads `mise.toml` to pin tool versions per
directory. With daft, each branch is a directory, so `mise.toml` becomes a
per-branch tool manifest. A worktree-post-create hook installs missing versions
on first creation; mise's shell hook activates them on `cd`.

## Prerequisites

- daft installed and shell integration enabled
- mise installed (`brew install mise` on macOS)
- mise's shell activation in your shell profile (`eval "$(mise activate bash)"`
  or equivalent)

## Steps

### 1. Add `mise.toml` to the repo

In the default-branch worktree:

```bash
cd ~/work/my-project/main
mise use node@22 python@3.13
git add mise.toml
git commit -m "chore: pin mise versions"
```

### 2. Add a `daft.yml` to install missing versions on worktree create

In the same worktree:

```yaml
# daft.yml
worktree-post-create:
  jobs:
    - name: install mise versions
      run: mise install
```

Trust the new `daft.yml`:

```bash
git add daft.yml
git commit -m "chore(daft): install mise versions on worktree create"
git daft-hooks trust
```

### 3. Create a worktree

```bash
daft start feat/upgrade-react
```

The hook fires; mise installs any missing versions. Your shell `cd`s into the
new worktree, and `mise activate` exposes the pinned tools on `PATH`.

## Verifying it works

```bash
node --version    # 22.x.x
python --version  # 3.13.x
which node        # ~/.local/share/mise/installs/node/22/bin/node (or similar)
```

## Variations

### Per-branch divergence

A feature branch can pin different versions. Edit `mise.toml` in that worktree,
commit, and the next time someone creates a worktree from that branch, the
post-create hook installs the new versions.

### mise tasks instead of `package.json` scripts

`mise.toml` `[tasks.*]` blocks let you run tasks via `mise run <name>`. This
works inside daft worktrees the same as anywhere else; no daft-specific
configuration needed.

## Troubleshooting

- **`mise install` fails with "no plugin found"** — run
  `mise plugin install <tool>` once to install the plugin globally; subsequent
  worktrees will reuse it.
- **`mise activate` not exposing tools** — confirm the shell hook is in your
  profile (after `eval "$(daft shell-init bash)"` is fine; before it works too).

## Where to next

- **[direnv](/cookbook/by-tooling/direnv)** — env vars per worktree (mise
  handles tool versions; direnv handles secrets)
- **[Hooks](/hooks/)** — what else can fire on worktree create
- **[Job orchestration](/hooks/job-orchestration)** — run `mise install` in
  parallel with other setup jobs
