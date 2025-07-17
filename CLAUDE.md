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

## Testing

This repository contains only shell scripts with no traditional build, test, or lint commands. Testing should be done manually by executing the scripts in various Git repository scenarios.