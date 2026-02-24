---
title: Worktree Workflow
description: Understanding the worktree-centric development approach
---

# Worktree Workflow

## What Is a Git Worktree?

A Git worktree is an additional working directory linked to the same repository.
Git 2.5+ supports multiple worktrees sharing a single `.git` directory, so each
branch can have its own files on disk simultaneously.

daft structures this into a consistent layout and provides commands to manage
the lifecycle.

## The daft Directory Layout

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

Key properties:

- `.git/` is a bare repository at the project root - it holds all shared Git
  data
- Each branch lives in its own directory as a sibling to `.git/`
- All worktrees share the same Git history, remotes, and configuration
- Each worktree has its own working files, index, and HEAD

## Why This Matters

### No Context Switching

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

### Full Isolation

Each worktree has its own:

- Working files and build artifacts (`node_modules/`, `target/`, etc.)
- IDE state and configuration (`.vscode/`, `.idea/`)
- Environment files (`.envrc`, `.env`)
- Running processes (dev servers, watchers)

### Parallel Development

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

## Daily Development Flow

### Starting a New Feature

```bash
# Creates branch + worktree, pushes to remote, sets upstream
daft start feature/user-auth

# Or using the git-native command
git worktree-checkout -b feature/user-auth
```

### Checking Out a PR for Review

```bash
# Creates worktree for existing remote branch
daft go feature/teammate-work

# Or using the git-native command
git worktree-checkout feature/teammate-work
```

### Toggling Between Worktrees

Quickly switch back and forth between two worktrees:

```bash
daft go -           # switch to the previous worktree
daft go -           # switch back
```

Each `daft go` or `daft start` records the source worktree, so `daft go -`
always takes you to the last one you came from.

### Branching from Default

When your current branch has diverged and you need a fresh start:

```bash
# Specify the base branch explicitly
daft start hotfix/critical-fix main

# Or with the gwtcbm shortcut (from shell integration)
gwtcbm hotfix/critical-fix
```

### Moving Uncommitted Work

Started work in the wrong branch? Move it:

```bash
daft carry feature/correct-branch
```

### Renaming a Branch

Rename a branch and its worktree directory in one step. The remote branch is
updated too:

```bash
daft rename feature/old-name feature/new-name
```

### Listing Worktrees

See all worktrees with status indicators, ahead/behind counts, and branch age:

```bash
daft list
```

Use `daft list --json` for machine-readable output.

### Cleaning Up

After branches are merged and deleted on the remote:

```bash
daft prune
```

### Syncing Everything

Prune stale worktrees and update all remaining ones in a single command:

```bash
daft sync
```

This is equivalent to `daft prune` followed by `daft update --all`. Use
`--rebase main` to also rebase all branches onto main after updating.

### Updating Branches

Pull the latest changes for specific worktrees or all at once:

```bash
# Update a specific worktree
daft update feature/auth

# Update all worktrees
daft update --all

# Reset a worktree to match a different remote branch
daft update master:test
```

## How Commands Find the Project Root

daft commands use `git rev-parse --git-common-dir` to locate the shared `.git`
directory regardless of which worktree you're in. This means you can run
commands from any directory within any worktree - they always find the project
root and create new worktrees as siblings.
