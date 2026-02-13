---
title: git-worktree-flow-adopt
description: Convert a traditional repository to worktree-based layout
---

# git worktree-flow-adopt

Convert a traditional repository to worktree-based layout

## Description

WHAT THIS COMMAND DOES

Converts your existing Git repository from the traditional layout to daft's
worktree-based layout. After conversion:

Before: After: my-project/ my-project/ ├── .git/ ├── .git/ (bare repository) ├──
src/ └── main/ (worktree) └── README.md ├── src/ └── README.md

Your uncommitted changes (staged and unstaged) are preserved in the new
worktree. The command is safe to run - if anything fails, your repository is
restored to its original state.

ABOUT THE WORKTREE WORKFLOW

The worktree workflow eliminates Git branch switching friction by giving each
branch its own directory. Instead of switching branches within a single
directory, you navigate between directories - each containing a different
branch.

BENEFITS

- No more stashing: Each branch has its own working directory
- Parallel development: Work on multiple branches simultaneously
- Persistent context: Each worktree keeps its own IDE state, terminal history,
  and environment (.envrc, node_modules, etc.)
- Instant switching: Just cd to another directory
- Safe experimentation: Changes in one worktree never affect another

HOW TO WORK WITH IT

After adopting, use these commands:

git worktree-checkout <branch> Check out an existing branch into a new worktree

git worktree-checkout-branch <new-branch> Create a new branch and worktree from
current branch

git worktree-checkout-branch-from-default <new-branch> Create a new branch from
the remote's default branch

git worktree-prune Clean up worktrees for merged/deleted branches

Your directory structure grows as you work:

my-project/ ├── .git/ ├── main/ # Default branch ├── feature/auth/ # Feature
branch └── bugfix/login/ # Bugfix branch

REVERTING

To convert back to a traditional layout, use git-worktree-flow-eject(1).

## Usage

```
git worktree-flow-adopt [OPTIONS] [REPOSITORY_PATH]
```

## Arguments

| Argument            | Description                                                       | Required |
| ------------------- | ----------------------------------------------------------------- | -------- |
| `<REPOSITORY_PATH>` | Path to the repository to convert (defaults to current directory) | No       |

## Options

| Option          | Description                                                   | Default |
| --------------- | ------------------------------------------------------------- | ------- |
| `-q, --quiet`   | Operate quietly; suppress progress reporting                  |         |
| `-v, --verbose` | Be verbose; show detailed progress                            |         |
| `--trust-hooks` | Trust the repository and allow hooks to run without prompting |         |
| `--no-hooks`    | Do not run any hooks from the repository                      |         |
| `--dry-run`     | Show what would be done without making any changes            |         |

## Global Options

| Option            | Description               |
| ----------------- | ------------------------- |
| `-h`, `--help`    | Print help information    |
| `-V`, `--version` | Print version information |

## See Also

- [git-worktree-clone](./git-worktree-clone.md)
- [git-worktree-init](./git-worktree-init.md)
- [git-worktree-flow-eject](./git-worktree-flow-eject.md)
