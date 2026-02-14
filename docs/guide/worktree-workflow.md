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
git worktree-checkout-branch feature/user-auth
```

### Checking Out a PR for Review

```bash
# Creates worktree for existing remote branch
git worktree-checkout feature/teammate-work
```

### Branching from Default

When your current branch has diverged and you need a fresh start:

```bash
# Always branches from origin's default branch (main/master/develop)
git worktree-checkout-branch --from-default hotfix/critical-fix
```

### Moving Uncommitted Work

Started work in the wrong branch? Move it:

```bash
git worktree-carry feature/correct-branch
```

### Cleaning Up

After branches are merged and deleted on the remote:

```bash
git worktree-prune
```

## How Commands Find the Project Root

daft commands use `git rev-parse --git-common-dir` to locate the shared `.git`
directory regardless of which worktree you're in. This means you can run
commands from any directory within any worktree - they always find the project
root and create new worktrees as siblings.
