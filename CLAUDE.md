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
- **`git-worktree-init`**: Initializes a new repository in the structured layout (`<repo>/.git` + `<repo>/<initial-branch>/`)
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
git worktree-init <repository-name>
git worktree-checkout <existing-branch-name>
git worktree-checkout-branch <new-branch-name> [base-branch-name]
git worktree-checkout-branch-from-default <new-branch-name>
git worktree-prune
```

## Development Notes

- Scripts can be executed from anywhere within the Git repository (including deep subdirectories)
- New worktrees are always created at the project root level (alongside the `.git` directory)
- Scripts use `git rev-parse --git-common-dir` to locate the project root regardless of execution location
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

**Start a new repository:**
```bash
git worktree-init my-new-project
```

This initializes a new repository in the structured layout:
```
my-new-project/
├── .git/           # Shared Git metadata
└── master/        # Initial worktree (default branch)
    └── ... (ready for project files)
```

You're automatically placed in `my-new-project/master/` and ready to start coding.

### Daily Development Workflow

**Working on a new feature:**
```bash
# From anywhere in the repository (main/, subdirectories, etc.)
git worktree-checkout-branch feature/user-auth

# Creates: my-project/feature/user-auth/ at project root level
# Automatically: creates branch, pushes to origin, sets upstream, runs direnv
```

**Switching to existing branch:**
```bash
# From anywhere in the repository
git worktree-checkout bugfix/login-issue

# Creates: my-project/bugfix/login-issue/ at project root level
# Checks out existing branch, sets upstream if remote exists
```

**Branching from default branch (not current):**
```bash
# From anywhere in the repository
git worktree-checkout-branch-from-default hotfix/critical-fix

# Creates: my-project/hotfix/critical-fix/ at project root level
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

The project has a comprehensive three-tier testing architecture that covers unit tests, legacy shell script tests, and Rust integration tests. All tests are fully integrated into GitHub Actions CI/CD workflows.

### Testing Architecture

#### 1. **Unit Tests** (`make test-unit`)
- Rust unit tests for library functions and utilities
- 16 tests covering:
  - Git command wrapper functionality
  - Directory and path utility functions
  - Branch/repository name validation
  - Direnv integration logic
  - Remote branch detection
- Run via `cargo test`

#### 2. **Legacy Tests** (`make test-legacy`)
- Tests for original shell script implementations in `tests/legacy/`
- Comprehensive test suites for each command:
  - `test_clone.sh`, `test_init.sh`, `test_checkout.sh`, etc.
- Uses `test_framework.sh` for consistent test infrastructure
- Ensures backward compatibility during Rust migration

#### 3. **Integration Tests** (`make test-integration`)
- End-to-end tests for Rust binaries in `tests/integration/`
- Mirrors legacy test structure but tests Rust implementations
- Key test files:
  - `test_checkout_direnv` - Tests direnv integration
  - `test_checkout_branch_workflow` - Tests development workflow scenarios
  - `test_checkout_branch_from_default_remote_updates` - Tests remote branch updates
  - `test_prune_multiple_deletions` - Tests cleanup operations
  - `test_integration_full_workflow` - Tests complete workflow scenarios
- Includes performance, security, and cross-platform compatibility tests

### Test Execution

**Run all tests:**
```bash
make test        # or make test-all
```

**Run specific test suites:**
```bash
make test-unit                    # Rust unit tests only
make test-legacy                  # Legacy shell script tests
make test-integration             # Rust integration tests
```

**Run individual integration test suites:**
```bash
make test-integration-clone
make test-integration-checkout
make test-integration-checkout-branch
make test-integration-checkout-branch-from-default
make test-integration-init
make test-integration-prune
```

### GitHub Actions Integration

The testing architecture is fully integrated into GitHub Actions via `.github/workflows/test.yml`:

1. **Multi-platform testing**: Runs on both `ubuntu-latest` and `macos-latest`
2. **Complete test coverage**:
   - Builds Rust binaries (`cargo build --release`)
   - Runs Rust unit tests (`cargo test`)
   - Runs Rust linting (`cargo clippy -- -D warnings`)
   - Checks code formatting (`cargo fmt -- --check`)
   - Executes legacy tests (`make test-legacy`)
   - Executes integration tests (`make test-integration`)
3. **Path configuration**: Automatically adds both legacy scripts and Rust binaries to PATH
4. **Dependency validation**: Verifies required tools (git, awk, basename) are available
5. **Test result artifacts**: Uploads test results for debugging failures

### Key Implementation Details

#### Remote Tracking Branch Handling
The Rust implementation includes sophisticated logic for handling remote tracking branches in bare repository setups:
- Ensures remote tracking branches are created with `git fetch origin +refs/heads/*:refs/remotes/origin/*`
- Intelligently chooses between local and remote branches based on commit history
- Prefers local branches when they have unpushed commits (development workflow)
- Prefers remote branches when they're ahead or equal (for getting latest changes)

#### Test Framework Features
- Isolated test environments using temporary directories
- Automatic cleanup after test completion
- Colored output for better readability
- Detailed error reporting with exit codes
- Support for verbose mode (`VERBOSE=1`)
- Parallel test execution support

### Makefile Integration

The Makefile provides convenient targets for all testing needs:
- `test-all` runs unit + legacy + integration tests
- Individual targets for granular testing
- Verbose modes for debugging (`test-verbose`, `test-legacy-verbose`, `test-integration-verbose`)
- Performance testing targets (`test-perf`, `test-perf-legacy`, `test-perf-integration`)
- CI simulation target (`make ci`) that mimics GitHub Actions workflow

### Test Maintenance

When adding new features:
1. Add unit tests for new Rust functions in the appropriate module
2. Add integration tests in `tests/integration/` following existing patterns
3. Ensure tests pass locally with `make test`
4. Verify CI passes on pull requests

## Language Migration Considerations

### Current State Assessment
The project is currently implemented as shell scripts, which has been appropriate for the core Git worktree operations. However, as the project grows in complexity (based on open GitHub issues #3-13), several factors suggest considering migration to a more robust language.

### Complexity Analysis of Planned Features
Analysis of open issues reveals a mix of complexities:
- **Simple features (4 issues)**: Command shortcuts, init command, clone flags, man pages
- **Medium features (4 issues)**: Brew packaging, shell completions, fetch commands, testing
- **Complex features (2 issues)**: Hooks system, uncommitted work copying

### Shell Script Limitations Emerging
1. **Argument parsing complexity**: Manual case statement parsing is becoming unwieldy with multiple options (`-n`, `-q`, `-a`) and will worsen with option forwarding
2. **Shell completions requirement**: Issue #5 requires dynamic completion generation, much easier in modern CLI frameworks
3. **Interactive features**: Planned features like branch selection and conflict resolution are cumbersome in shell
4. **Error handling**: Complex state management and rollback (Issue #10) is brittle in shell scripts
5. **Testing infrastructure**: Issue #13 requires robust testing, which is challenging for shell scripts

### Rust + Clap Migration Case
**Strong arguments for Rust migration:**
- **Argument parsing**: Clap provides automatic help text, shell completions, validation, and option forwarding
- **External command integration**: `std::process::Command` handles `direnv allow`, `git` commands excellently
- **Professional UX**: Better error messages, help formatting, type-safe arguments
- **Scalability**: As features grow, Rust will handle complexity better than shell scripts
- **Single binary distribution**: Easier than managing multiple shell scripts

**Rust advantages for this project:**
```rust
// Automatic completions, help text, validation
#[derive(Parser)]
#[command(name = "git-worktree-clone")]
struct Args {
    #[arg(short = 'n', long = "no-checkout")]
    no_checkout: bool,
    
    #[arg(short = 'q', long = "quiet")]
    quiet: bool,
    
    /// Forward to git clone
    #[arg(long = "depth")]
    depth: Option<u32>,
    
    repository: String,
}
```

### Migration Strategy
**Recommended approach:**
1. **Incremental migration**: Start with one complex command (e.g., `git-worktree-clone`)
2. **Hybrid approach**: Keep simple shell scripts, migrate complex features to Rust
3. **Unified tool**: Eventually consolidate into single Rust binary with subcommands

### Decision Factors
**Migrate to Rust if:**
- ✅ Multiple options per command (already present)
- ✅ Option forwarding needs (planned)
- ✅ Shell completion requirements (Issue #5)
- ✅ Interactive features planned
- ✅ Complex validation needs

**Current recommendation**: **Yes, migrate to Rust + clap**. The tipping point has been reached where shell scripts become limiting for the sophisticated CLI tool this project is becoming.

### External Command Integration
Running commands like `direnv allow` and `git` operations work excellently in Rust:
```rust
use std::process::Command;

fn run_direnv_allow() -> Result<(), Box<dyn std::error::Error>> {
    let output = Command::new("direnv")
        .args(&["allow", "."])
        .output()?;
    
    if output.status.success() {
        println!("direnv allow completed successfully");
    }
    
    Ok(())
}
```

This provides better error handling, type safety, and cross-platform compatibility than shell scripts.