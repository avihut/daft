---
title: daft - Git Extensions Toolkit
description:
  Give each Git branch its own directory. No more stashing, no more context
  switching, no more waiting for builds to restart.
layout: home
hero:
  name: daft
  text: Git Extensions Toolkit
  tagline:
    Give each Git branch its own directory. No more stashing, no more context
    switching.
  actions:
    - theme: brand
      text: Get Started
      link: /getting-started/installation
    - theme: alt
      text: View on GitHub
      link: https://github.com/avihut/daft
features:
  - title: One Branch, One Directory
    details:
      Each branch lives in its own directory. Run different branches in
      different terminals with full isolation.
  - title: Zero Context Switching
    details:
      No stashing, no rebuilding, no lost IDE state. Switch branches by
      switching directories.
  - title: Seamless Git Integration
    details:
      Works as native Git subcommands. Your existing Git knowledge applies -
      just add worktree power.
  - title: Lifecycle Hooks
    details:
      YAML-configured hooks automate setup for each new worktree — parallel
      jobs, dependencies, skip conditions, and more.
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
git worktree-clone git@github.com:user/my-project.git

# Start a feature branch
git worktree-checkout-branch feature/auth
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
- [Worktree Workflow Guide](./guide/worktree-workflow.md) - Understand the
  worktree-centric approach
