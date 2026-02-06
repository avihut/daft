---
title: Adopting Existing Repositories
description: Convert traditional repositories to the worktree-based layout
---

# Adopting Existing Repositories

Already have a traditional Git repository? You can convert it to the worktree layout without losing any work.

## What flow-adopt Does

`git worktree-flow-adopt` restructures a traditional repository into daft's worktree layout:

**Before:**
```
my-project/
├── .git/            # Regular git directory
├── src/
├── package.json
└── README.md
```

**After:**
```
my-project/
├── .git/            # Bare repository
└── main/            # Worktree for current branch
    ├── src/
    ├── package.json
    └── README.md
```

## Running It

```bash
cd my-existing-project
git worktree-flow-adopt
```

Or specify a path:

```bash
git worktree-flow-adopt /path/to/my-project
```

### Preview First

Use `--dry-run` to see what would happen without making changes:

```bash
git worktree-flow-adopt --dry-run
```

## Uncommitted Changes Are Preserved

Any staged, unstaged, or untracked changes in your working directory are carried into the new worktree. You won't lose any work.

## Reverting with flow-eject

If you decide the worktree layout isn't for you:

```bash
git worktree-flow-eject
```

This converts back to a traditional repository layout:

**Before:**
```
my-project/
├── .git/            # Bare repository
├── main/
│   ├── src/
│   └── README.md
└── feature/auth/
    ├── src/
    └── README.md
```

**After:**
```
my-project/
├── .git/            # Regular git directory
├── src/
└── README.md
```

By default, `flow-eject` keeps the remote's default branch. Use `--branch` to specify a different one:

```bash
git worktree-flow-eject --branch feature/auth
```

Other worktrees are removed. If any have uncommitted changes, the command fails unless you pass `--force`.

## When to Adopt vs Clone Fresh

**Use `flow-adopt`** when:
- You have an existing local repository with work in progress
- You want to try the worktree workflow without re-cloning
- You have local branches or stashes you want to preserve

**Use `worktree-clone`** when:
- Starting fresh from a remote repository
- Setting up a new development environment
- The repository has no local-only work to preserve
