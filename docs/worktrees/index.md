---
title: Worktrees
description:
  Code isolation through per-branch directories — daft's Worktrees pillar.
---

# Worktrees

The Worktrees pillar gives every Git branch its own directory on disk. Run
different branches in different terminals with full isolation — no stashing, no
context switching, no waiting for builds to restart.

## The adoption arc

Worktree adoption deepens in three stages. You don't need all three to get
value; each stage stands on its own.

1. **Code isolation.** Each branch lives in its own directory. The Git metadata
   is shared (one `.git/`), but the working files are separate. You can edit
   `feature-A` and `feature-B` simultaneously without `git stash` or branch
   swapping.
2. **Environment isolation.** Different branches often need different runtime
   versions, env vars, or secrets. With per-worktree env management
   ([declarative envs](/recipes/declarative-envs) via mise/asdf/nvm/pyenv, plus
   [env vars & secrets](/recipes/env-vars-and-secrets) via direnv/sops), each
   worktree boots with the right environment.
3. **Automation.** Setting up env per worktree gets repetitive. The
   [Hooks pillar](/hooks/) automates it: declarative jobs that run when
   worktrees are created, removed, or merged.

This page covers stage 1. Stages 2 and 3 are covered by linked pages.

## What is a Git worktree?

A Git worktree is an additional working directory linked to the same repository.
Git 2.5+ supports multiple worktrees sharing a single `.git` directory, so each
branch can have its own files on disk simultaneously.

daft structures this into a consistent layout and provides commands to manage
the lifecycle.

## The daft directory layout

When you clone or init with daft, repositories use this structure:

```
my-project/
├── .git/                    # Shared Git metadata (bare repository)
├── main/                    # Worktree for the default branch
│   ├── src/
│   ├── package.json
│   └── ...
├── feature/auth/            # Worktree for feature branch
│   ├── src/
│   ├── package.json
│   └── ...
└── bugfix/login/            # Worktree for bugfix branch
    ├── src/
    ├── package.json
    └── ...
```

::: tip Other layouts available This page shows the **contained** layout, where
worktrees are subdirectories of the repo. daft supports several layouts that
control where worktrees are placed. See [Layouts](/worktrees/layouts) to explore
your options. :::

Key properties:

- `.git/` is a bare repository at the project root - it holds all shared Git
  data
- Each branch lives in its own directory as a sibling to `.git/`
- All worktrees share the same Git history, remotes, and configuration
- Each worktree has its own working files, index, and HEAD

## Why this matters

### No context switching

Traditional Git requires `git checkout` to switch branches, which replaces all
files in your working directory. With worktrees, branches coexist:

```bash
# Traditional: sequential, with context loss
git stash
git checkout feature-b
# ... work ...
git checkout feature-a
git stash pop

# Worktree: parallel, no context loss
cd ../feature-b/
# ... work ...
cd ../feature-a/
```

### Full isolation

Each worktree has its own:

- Working files and build artifacts (`node_modules/`, `target/`, etc.)
- IDE state and configuration (`.vscode/`, `.idea/`)
- Environment files (`.envrc`, `.env`)
- Running processes (dev servers, watchers)

### Parallel development

Run multiple branches simultaneously in separate terminals:

```bash
# Terminal 1: running dev server for feature work
cd my-project/feature/auth/
npm run dev  # http://localhost:3000

# Terminal 2: running tests for bugfix
cd my-project/bugfix/login/
npm test

# Terminal 3: code review
cd my-project/review/teammate-pr/
npm run lint
```

## Daily development flow

### Starting a new feature

```bash
# Creates branch + worktree, pushes to remote, sets upstream
daft start feature/user-auth

# Or using the git-native command
git worktree-checkout -b feature/user-auth
```

### Checking out a PR for review

```bash
# Creates worktree for existing remote branch
daft go feature/teammate-work

# Or using the git-native command
git worktree-checkout feature/teammate-work
```

### Toggling between worktrees

Quickly switch back and forth between two worktrees:

```bash
daft go -           # switch to the previous worktree
daft go -           # switch back
```

Each `daft go` or `daft start` records the source worktree, so `daft go -`
always takes you to the last one you came from.

### Branching from default

When your current branch has diverged and you need a fresh start:

```bash
# Specify the base branch explicitly
daft start hotfix/critical-fix main

# Or with the gwtcbm shortcut (from shell integration)
gwtcbm hotfix/critical-fix
```

### Moving uncommitted work

Started work in the wrong branch? Move it:

```bash
daft carry feature/correct-branch
```

### Renaming a branch

Rename a branch and its worktree directory in one step. The remote branch is
updated too:

```bash
daft rename feature/old-name feature/new-name
```

### Listing worktrees

See all worktrees with status indicators, ahead/behind counts, and branch age:

```bash
daft list
```

Use `daft list --format json` for machine-readable output.

### Cleaning up

After branches are merged and deleted on the remote:

```bash
daft prune
```

### Removing a repository

To tear down a daft-managed repository entirely — git dir, every worktree, trust
marker, and any `worktree-pre/post-remove` hooks — use `daft repo remove`:

```bash
# Remove the repo containing the current directory
daft repo remove

# Remove a repo by path (works from anywhere)
daft repo remove ~/code/old-project

# Preview what would happen first
daft repo remove --dry-run ~/code/old-project

# Skip the confirmation prompt
daft repo remove --force ~/code/old-project
```

When run from inside the repo being deleted, daft writes a safe redirect path to
`$DAFT_CD_FILE` so the shell wrapper `cd`s the user out of the now-deleted
directory. See [`daft repo remove`](/cli/daft-repo-remove) for full reference.

### Syncing everything

Prune stale worktrees and update all remaining ones in a single command:

```bash
daft sync
```

This is equivalent to `daft prune` followed by `daft update --all`. Use
`--rebase main` to also rebase all branches onto main after updating.

### Updating branches

Pull the latest changes for specific worktrees or all at once:

```bash
# Update a specific worktree
daft update feature/auth

# Update all worktrees
daft update --all

# Reset a worktree to match a different remote branch
daft update master:test
```

## How commands find the project root

daft commands use `git rev-parse --git-common-dir` to locate the shared `.git`
directory regardless of which worktree you're in or which
[layout](/worktrees/layouts) the repository uses. This means you can run
commands from any directory within any worktree — they always find the project
root and place new worktrees according to the active layout.

## Where to next

- **Geometry on disk:** [Layouts](/worktrees/layouts) — sibling, contained,
  nested, custom
- **Existing repos:**
  [Adopting existing repos](/worktrees/adopting-existing-repos) — convert a
  traditional repo to the worktree layout
- **Forks and mirrors:** [Multi-remote](/worktrees/multi-remote) — organize
  worktrees by remote
- **Run commands across worktrees:**
  [Running commands across worktrees](/worktrees/running-commands) — `daft exec`
- **Merge across worktrees:** [Merging across worktrees](/worktrees/merging) —
  `daft merge` from anywhere, octopus, ephemeral targets, PR-style hook gates
- **Faster typing:** [Shortcuts](/worktrees/shortcuts) — `gwt*` symlink aliases
- **Recipes:** [Recipes for Worktrees](/recipes/?pillar=worktrees)
- **Next pillar:** [Hooks](/hooks/) — automate the env-setup-per-worktree
  problem
