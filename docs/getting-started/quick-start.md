---
title: Quick Start
description:
  Get up and running with daft in minutes — covers the worktree adoption arc.
---

# Quick Start

This guide walks you through the **worktree adoption arc** — three stages of
daft adoption depth. You can stop at any stage and still get value.

## Stage 1: Code isolation

Clone a repository into the worktree layout:

```bash
daft clone git@github.com:user/my-project.git
```

This creates a structured layout:

```
my-project/
├── .git/           # Shared Git metadata
└── main/           # Worktree for the default branch
    └── ... (project files)
```

You're automatically placed in `my-project/main/`.

::: tip This shows the **contained** layout. daft supports other layouts that
organize worktrees differently — see [Layouts](/worktrees/layouts) to explore
your options. :::

### Create a Feature Branch

From anywhere inside the repository:

```bash
daft start feature/auth
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

### Switch Between Branches

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
daft go bugfix/login-issue
```

### Branch From Default

When you need a fresh branch from `main` (regardless of where you are), use the
`gwtcbm` shortcut from shell integration:

```bash
daft start hotfix/critical-fix main

# Or with the gwtcbm shortcut (from shell integration)
gwtcbm hotfix/critical-fix
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

### Carry Changes Between Worktrees

Move uncommitted work to another worktree:

```bash
# Move changes from current worktree to feature/auth
daft carry feature/auth

# Or copy changes (keep them in both places)
daft carry --copy feature/auth main
```

### Clean Up Merged Branches

After branches are merged and deleted on the remote:

```bash
daft prune
```

This automatically:

- Fetches from remote and prunes stale tracking branches
- Identifies local branches whose remotes were deleted
- Removes associated worktrees
- Deletes local branches

### Adopt an Existing Repository

Already have a traditional repository? Convert it:

```bash
cd my-existing-project
daft adopt
```

This restructures your repo into the worktree layout. Uncommitted changes are
preserved.

::: tip Git-native commands Every daft command has a git-native equivalent
(e.g., `daft clone` = `git worktree-clone`). See the
[CLI Reference](/reference/cli/git-worktree-clone) for the full list. :::

That's stage 1: every branch in its own directory, no stashing, no swapping.

## Stage 2: Environment isolation

Worktrees give you code isolation. Real-world branches usually need different
runtime versions, env vars, or running services. Add a tool to handle that:

- **Tool versions**: see the [mise recipe](/cookbook/by-tooling/mise).
- **Env vars / secrets**: see the [direnv recipe](/cookbook/by-tooling/direnv).
- **Both**: combine the two recipes.

Each worktree boots with the right env on `cd`.

## Stage 3: Automation

Setting up the env per worktree gets repetitive — a great fit for
[daft hooks](/hooks/). Hooks fire on worktree create/remove (plus other
code-evolution boundaries; see the [boundaries thesis](/hooks/)). Two examples:

```yaml
# daft.yml
worktree-post-create:
  jobs:
    - name: install deps
      run: pnpm install --frozen-lockfile
    - name: copy envrc
      run:
        "[ ! -f .envrc ] && cp .envrc.example .envrc && direnv allow . || true"
```

Trust the new `daft.yml`:

```bash
git daft-hooks trust
```

Now every new worktree boots with deps installed and `.envrc` ready.

## Where to next

- **Pillar overview:** [Worktrees](/worktrees/), [Hooks](/hooks/)
- **Recipes:** [Cookbook](/cookbook/)
- **Why daft:** [About → Why daft](/about/why-daft)
