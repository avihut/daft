---
title: Quick Start
description: Get up and running with daft in minutes
---

# Quick Start

This guide walks you through the core daft workflow: cloning a repo, creating
branches, and cleaning up.

## 1. Clone a Repository

```bash
git worktree-clone git@github.com:user/my-project.git
```

This creates a structured layout:

```
my-project/
├── .git/           # Shared Git metadata
└── main/           # Worktree for the default branch
    └── ... (project files)
```

You're automatically placed in `my-project/main/`.

## 2. Create a Feature Branch

From anywhere inside the repository:

```bash
git worktree-checkout -b feature/auth
```

This creates a new branch and worktree in one step:

```
my-project/
├── .git/
├── main/               # Default branch (untouched)
└── feature/auth/       # New branch with its own worktree
    └── ... (project files)
```

You're now in `my-project/feature/auth/`. Your `main/` directory is completely
untouched.

## 3. Switch Between Branches

Each branch is just a directory. Open different terminals:

```bash
# Terminal 1 - working on feature
cd my-project/feature/auth/
npm run dev

# Terminal 2 - checking main
cd my-project/main/
npm test
```

To check out an existing branch:

```bash
git worktree-checkout bugfix/login-issue
```

## 4. Branch From Default

When you need a fresh branch from `main` (regardless of where you are), use the
`gwtcbm` shortcut from shell integration:

```bash
# With shell integration (daft shell-init)
gwtcbm hotfix/critical-fix

# Or specify the base branch explicitly
git worktree-checkout -b hotfix/critical-fix main
```

Your directory structure becomes:

```
my-project/
├── .git/
├── main/
├── feature/auth/
├── bugfix/login-issue/
└── hotfix/critical-fix/    # Branched from main, not current branch
```

## 5. Carry Changes Between Worktrees

Move uncommitted work to another worktree:

```bash
# Move changes from current worktree to feature/auth
git worktree-carry feature/auth

# Or copy changes (keep them in both places)
git worktree-carry --copy feature/auth main
```

## 6. Clean Up Merged Branches

After branches are merged and deleted on the remote:

```bash
git worktree-prune
```

This automatically:

- Fetches from remote and prunes stale tracking branches
- Identifies local branches whose remotes were deleted
- Removes associated worktrees
- Deletes local branches

## 7. Adopt an Existing Repository

Already have a traditional repository? Convert it:

```bash
cd my-existing-project
git worktree-flow-adopt
```

This restructures your repo into the worktree layout. Uncommitted changes are
preserved.

## What's Next

- [Shell Integration](./shell-integration.md) - Enable auto-cd into new
  worktrees
- [Worktree Workflow](../guide/worktree-workflow.md) - Deep dive into the
  worktree-centric approach
- [Hooks](../guide/hooks.md) - Automate worktree lifecycle events
- [Shortcuts](../guide/shortcuts.md) - Enable short command aliases
- [Configuration](../guide/configuration.md) - Customize daft's behavior
