---
title: daft - Git Extensions Toolkit
description:
  Give each Git branch its own directory. No more stashing, no more context
  switching, no more waiting for builds to restart.
layout: home
hero:
  name: daft
  text: Parallel dev, by default
  tagline:
    Each branch in its own directory. Hooks at every code-evolution boundary.
    Coordinate across repos. (One of these is still in design.)
  actions:
    - theme: brand
      text: Get Started
      link: /getting-started/quick-start
    - theme: alt
      text: Why daft
      link: /about/why-daft
features:
  - title: Worktrees
    details:
      Every branch gets its own directory. Run feature-A and feature-B in
      different terminals at the same time — no stashing, no context switching.
      Merge across worktrees from anywhere with `daft merge`.
    link: /worktrees/
    linkText: Worktrees pillar
  - title: Hooks
    details:
      Boundaries at every code-evolution stage — the local-parallel-to-CI
      surface. Worktree lifecycle and PR-style merge gates today; the full
      git-hooks lifecycle is on the roadmap.
    link: /hooks/
    linkText: Hooks pillar
  - title: Recipes
    details:
      Patterns for adopting daft alongside your existing tooling — mise, direnv,
      asdf, monorepos, fork workflows, CI integration.
    link: /recipes/
    linkText: Recipes
---

# daft - Git Extensions Toolkit

> Stop switching branches. Work on multiple branches simultaneously.

**daft** gives each Git branch its own directory. No more stashing, no more
context switching, no more waiting for builds to restart.

```
my-project/
├── .git/                    # Shared Git data
├── main/                    # Stable branch
├── feature/auth/            # Your feature work
├── bugfix/login/            # Parallel bugfix
└── review/teammate-pr/      # Code review
```

## Quick Start

```bash
# Install (macOS)
brew install avihut/tap/daft

# Clone a repo
daft clone git@github.com:user/my-project.git

# Start a feature branch
daft start feature/auth
```

Each directory is a full working copy. Run different branches in different
terminals. Your IDE state, node_modules, build artifacts - all isolated per
branch.

## Why daft?

**Traditional Git workflow:**

```
$ git stash
$ git checkout feature-b
$ npm install        # wait...
$ npm run build      # wait...
# context lost, IDE state gone
$ git checkout feature-a
$ git stash pop
# where was I?
```

**With daft:**

```
Terminal 1 (feature-a/)     Terminal 2 (feature-b/)
┌───────────────────────┐   ┌───────────────────────┐
│ $ npm run dev         │   │ $ npm run dev         │
│ Server on :3000       │   │ Server on :3001       │
│ # full context        │   │ # full context        │
└───────────────────────┘   └───────────────────────┘
         ↓                           ↓
    Both running simultaneously, isolated environments
```

## Next Steps

- [Installation](./getting-started/installation.md) - Install daft on your
  system
- [Quick Start](./getting-started/quick-start.md) - Get up and running in
  minutes
- [Shell Integration](./getting-started/shell-integration.md) - Enable auto-cd
  into worktrees
- [Worktrees](./worktrees/index.md) - Understand the worktree-centric approach
