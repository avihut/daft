# CLAUDE.md

This file provides guidance to Claude Code (claude.ai/code) when working with code in this repository.

## Overview

This is a Git worktree workflow toolkit consisting of shell scripts designed to streamline a development workflow that heavily utilizes `git worktree`. The scripts are intended to be used as custom Git commands (e.g., `git worktree-clone`, `git worktree-checkout`).

## Key Concepts

- **Worktree-centric workflow**: One worktree per branch, with all worktrees for a repository organized under a common parent directory
- **Directory structure**: Uses `<repo-name>/.git` at root with worktrees at `<repo-name>/<branch-name>/`
- **`direnv` integration**: Automatically runs `direnv allow` when entering new worktrees that contain `.envrc` files
- **Dynamic branch detection**: Scripts query remote repositories to determine actual default branch (main, master, develop, etc.)

## Script Architecture

All scripts are located in the `scripts/` directory and follow these patterns:

### Core Scripts

- **`git-worktree-clone`**: Clones a repository into the structured layout (`<repo>/.git` + `<repo>/<default-branch>/`)
- **`git-worktree-checkout`**: Creates worktree from an existing local or remote branch
- **`git-worktree-checkout-branch`**: Creates new worktree + new branch from current or specified base branch
- **`git-worktree-checkout-branch-from-default`**: Creates new worktree + new branch from remote's default branch
- **`git-worktree-prune`**: Removes local branches whose remote counterparts are deleted, plus associated worktrees

### Script Patterns

- All scripts use `#!/bin/bash` and include comprehensive error handling
- Scripts that create worktrees change directory into the new worktree upon completion
- Remote name is configurable via `remote_name="origin"` variable
- Scripts use `git rev-parse --git-common-dir` to locate shared Git metadata
- Path resolution handles both absolute and relative paths robustly

## Usage

Scripts are installed by adding the `scripts/` directory to your `PATH`. Once installed, they can be executed as Git subcommands:

```bash
git worktree-clone <repository-url>
git worktree-checkout <existing-branch-name>
git worktree-checkout-branch <new-branch-name> [base-branch-name]
git worktree-checkout-branch-from-default <new-branch-name>
git worktree-prune
```

## Development Notes

- Scripts assume execution from within an existing worktree directory (except `git-worktree-clone`)
- New worktrees are created as siblings using relative paths (`../<branch-name>`)
- Scripts include optional `direnv` integration but silently skip if not available
- Error handling includes cleanup of partially created worktrees on failure
- All scripts include detailed usage documentation and examples in their headers

## Worktree Workflow

These scripts enable a complete worktree-based development workflow that eliminates traditional Git branch switching friction:

### Initial Setup

**Start with any Git repository:**
```bash
git worktree-clone git@github.com:user/my-project.git
```

This creates a structured layout:
```
my-project/
├── .git/           # Shared Git metadata
└── main/          # First worktree (default branch)
    └── ... (project files)
```

You're automatically placed in `my-project/main/` and ready to work.

### Daily Development Workflow

**Working on a new feature:**
```bash
# From inside my-project/main/
git worktree-checkout-branch feature/user-auth

# Creates: my-project/feature/user-auth/ alongside main/
# Automatically: creates branch, pushes to origin, sets upstream, runs direnv
```

**Switching to existing branch:**
```bash
git worktree-checkout bugfix/login-issue

# Creates: my-project/bugfix/login-issue/
# Checks out existing branch, sets upstream if remote exists
```

**Branching from default branch (not current):**
```bash
# From any worktree
git worktree-checkout-branch-from-default hotfix/critical-fix

# Always branches from origin's default branch (main/master/develop)
# Useful when current branch isn't what you want to base on
```

### The Resulting Workflow

Your directory structure becomes:
```
my-project/
├── .git/                    # Shared Git metadata
├── main/                    # Default branch worktree
├── feature/user-auth/       # Feature branch worktree
├── bugfix/login-issue/      # Bugfix branch worktree
└── hotfix/critical-fix/     # Hotfix branch worktree
```

**Key Benefits:**
- **No branch switching**: Each branch has its own directory
- **No stashing**: Work persists across branches
- **Parallel development**: Multiple branches can be worked on simultaneously
- **IDE context**: Each worktree maintains its own IDE settings/context
- **Environment isolation**: Each worktree can have its own `.envrc` file

### Cleanup Workflow

**When branches are merged and deleted remotely:**
```bash
git worktree-prune
```

This automatically:
- Fetches from origin and prunes stale remote branches
- Identifies local branches tracking deleted remotes
- Removes associated worktrees
- Deletes local branches

### Advanced Scenarios

**Working on multiple features simultaneously:**
```bash
# Terminal 1: working on authentication
cd my-project/feature/user-auth/
npm run dev

# Terminal 2: working on UI components  
cd my-project/feature/new-ui/
npm run storybook

# Terminal 3: testing a bugfix
cd my-project/bugfix/payment-error/
npm test
```

**Code reviews and testing:**
```bash
# Quickly check out a PR branch for review
git worktree-checkout feature/teammate-work

# Test runs in isolation without affecting your current work
cd my-project/feature/teammate-work/
npm test
```

This workflow eliminates the traditional friction of Git branch switching, stashing, and context loss, making it particularly powerful for projects where you frequently work on multiple branches or need to maintain different development environments per branch.

## Testing

This repository contains only shell scripts with no traditional build, test, or lint commands. Testing should be done manually by executing the scripts in various Git repository scenarios.