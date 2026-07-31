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
    Coordinate across the repo graph.
  image:
    light: /brand/daft-donut-accent.svg
    dark: /brand/daft-donut-accent-white.svg
    alt: Donut, the daft dodo
  actions:
    - theme: brand
      text: Get Started
      link: /getting-started/quick-start
    - theme: alt
      text: Why daft
      link: /about/why-daft
features:
  - icon:
      light: /brand/glyph-worktrees.svg
      dark: /brand/glyph-worktrees-dark.svg
      width: 26
      height: 26
    title: Worktrees
    details:
      Every branch gets its own directory. Work on three things at once —
      nothing stashed, nothing rebuilt.
    link: /worktrees/
    linkText: Explore worktrees
  - icon:
      light: /brand/glyph-hooks.svg
      dark: /brand/glyph-hooks-dark.svg
      width: 26
      height: 26
    title: Hooks
    details:
      New worktrees boot ready to run — deps installed, env loaded, merge gates
      enforced. All declarative.
    link: /hooks/
    linkText: Explore hooks
  - icon:
      light: /brand/glyph-graph.svg
      dark: /brand/glyph-graph-dark.svg
      width: 26
      height: 26
    title: Graph
    details:
      Your repos, one connected set. Jump between them, open a branch
      everywhere, run commands across all of it.
    link: /graph/
    linkText: Explore the graph
  - icon:
      light: /brand/glyph-recipes.svg
      dark: /brand/glyph-recipes-dark.svg
      width: 26
      height: 26
    title: Recipes
    details:
      Proven setups for daft with mise, direnv, monorepos, forks, and CI. Copy,
      paste, adapt.
    link: /recipes/
    linkText: Browse recipes
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
