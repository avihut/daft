# Project History

This document preserves the history of daft's evolution from shell scripts to a
modern Rust implementation.

## Origins

daft started in 2024 as a collection of Bash shell scripts designed to solve the
friction of traditional Git branch switching. The core insight was simple:
instead of constantly switching branches and dealing with stashed changes, what
if each branch had its own persistent worktree?

## The Original Shell Scripts

The project began with six shell scripts, each handling a specific aspect of the
worktree workflow:

### git-worktree-clone

Cloned a repository into the structured layout with a bare `.git` directory at
the root and the default branch as the first worktree.

### git-worktree-init

Initialized a new local repository in the same structured layout, creating the
bare repository and initial worktree.

### git-worktree-checkout

Created a worktree from an existing local or remote branch, detecting whether to
use the local version or fetch from remote.

### git-worktree-checkout-branch

Created a new branch and its associated worktree in a single operation, pushing
to origin and setting up tracking.

### git-worktree-checkout-branch-from-default

Similar to checkout-branch but always branched from the remote's default branch,
useful when your current branch isn't what you want to base new work on.

### git-worktree-prune

Cleaned up local branches whose remote counterparts had been deleted, removing
the associated worktrees along with them.

## How They Worked

The shell scripts shared several key implementation patterns:

### Directory Structure

```
<repo-name>/
├── .git/           # Bare repository (shared Git metadata)
├── main/           # Default branch worktree
├── feature/auth/   # Feature branch worktree
└── bugfix/login/   # Bugfix branch worktree
```

This structure was created using `git clone --bare` followed by
`git worktree add` for each branch.

### Project Root Discovery

Scripts used `git rev-parse --git-common-dir` to locate the shared Git metadata
regardless of which worktree or subdirectory the user was in.

### Safe Error Recovery

Every script tracked the original directory with `original_dir=$(pwd)` and
included cleanup handlers to restore state if operations failed midway.

### Graceful Degradation

Optional features like direnv integration were designed to work when available
but not fail when absent:

```bash
if command -v direnv &> /dev/null; then
    if [[ -f ".envrc" ]]; then
        direnv allow .
    fi
fi
```

### Remote Default Branch Detection

Rather than assuming "main" or "master", scripts queried the remote directly:

```bash
git ls-remote --symref "$repo_url" HEAD | awk '/^ref:/ {sub("refs/heads/", ""); print $2}'
```

## Design Philosophy

The shell scripts embodied several core principles that carried forward to the
Rust implementation:

1. **One worktree per branch**: Eliminate context switching and stash juggling
2. **Run from anywhere**: Commands work from any directory within the repository
3. **Informative feedback**: Clear output about what operations are being
   performed
4. **Safe failure**: Clean up partial operations on failure to prevent
   inconsistent state

## Migration to Rust (2025)

While the shell scripts served the project well, several factors motivated the
migration to Rust:

### Limitations of Shell Scripts

- **Complex argument parsing**: Manual case statement parsing became unwieldy
- **Limited type safety**: Runtime errors for issues that could be caught at
  compile time
- **Testing challenges**: Shell script testing required external frameworks
- **Cross-platform concerns**: Differences between bash versions and Unix
  flavors

### Benefits of Rust

- **Type-safe argument parsing** with the clap library
- **Comprehensive error handling** with Result types and structured errors
- **Built-in testing** with cargo test
- **Single binary distribution** for easier installation
- **Shell completions** generated automatically
- **Performance**: Faster startup and execution

### Architectural Evolution

The migration introduced a **multicall binary architecture**:

- Single 589KB binary (`daft`) instead of six separate scripts
- Symlinks route commands to the main binary
- 83% reduction in total size compared to separate binaries
- Command detection via `argv[0]` for seamless Git integration

### Feature Parity and Enhancements

The Rust implementation maintains full compatibility with the original scripts
while adding:

- Professional `--help` output with detailed usage information
- Shell completion support (bash, zsh, fish)
- Better error messages with context and suggestions
- Shell integration for automatic directory changes
- The `daft setup` command for easy configuration

## Timeline

| Date    | Milestone                                          |
| ------- | -------------------------------------------------- |
| 2024    | Initial shell script implementation                |
| 2025-10 | Project renamed from git-worktree-workflow to daft |
| 2025-10 | Migration to single multicall Rust binary          |
| 2025-10 | Shell integration added for automatic cd           |
| 2025-10 | Legacy shell scripts deprecated and removed        |

## Legacy

The shell scripts served as the foundation and proof of concept for the worktree
workflow. Their patterns, error handling approaches, and user experience design
informed every aspect of the Rust implementation. While the scripts are no
longer part of the codebase, their influence remains in the project's philosophy
of safe, informative, and friction-free Git operations.
