# CLAUDE.md

This file provides guidance when working with code in this repository.

## Recent Changes

**Single Binary Architecture Migration (2025-10-18)**
- Migrated from 6 separate binaries to single multicall binary
- Reduced total binary size from ~3.5MB to 589KB (83% reduction)
- All commands now route through single `daft` binary via symlinks
- Binary detects invocation name (argv[0]) and routes to appropriate command
- Development workflow streamlined with `make dev` target
- All 147 tests passing with new architecture

**Project Renamed from `git-worktree-workflow` to `daft` (2025-10-17)**
- GitHub repository: `https://github.com/avihut/daft` (was `git-worktree-workflow`)
- Project directory: `/Users/avihu/Projects/daft` (was `git-worktree-workflow`)
- Cargo package name: `daft` (was `git-worktree-workflow`)
- All command names remain unchanged (`git-worktree-*`)
- All functionality preserved, 147 tests passing
- Documentation and installation paths updated throughout

## Release Workflow and Git Branching Strategy

This project uses a multi-channel release workflow with `master` as the stable branch and `develop` as the integration branch.

### Branch Structure

| Branch | Purpose | Version Example |
|--------|---------|-----------------|
| `master` | Stable releases, published to Homebrew | v0.3.0 |
| `develop` | Next release development, canary/beta builds | v0.4.0 |

### Release Channels

| Channel | Trigger | Tag Format | Binaries | Homebrew |
|---------|---------|------------|----------|----------|
| **Stable** | Promote workflow | `v0.3.0` | ✅ Yes | ✅ Yes |
| **Canary** | Push to develop | `v0.4.0-canary.N` | ❌ No | ❌ No |
| **Beta** | Monthly/manual | `v0.4.0-beta.N` | ❌ No | ❌ No |

### Git Flow Rules (Important!)

To maintain flat history on `master`, follow these rules strictly:

#### Normal Development Flow
```
1. All features/changes → develop branch
2. Periodically promote develop → master (creates stable release)
```

#### Hotfix Flow (bugfix needed on stable while develop is ahead)
```
1. Create hotfix branch from master
2. Fix the bug, PR to master
3. Cherry-pick the fix commit to develop
4. When promoting, git rebase will skip the duplicate commit
```

**Example:**
```bash
# On master (v0.3.0), need to fix a bug while develop is at v0.4.0

# 1. Create and apply hotfix to master
git checkout master
git checkout -b hotfix/critical-bug
# ... fix the bug ...
git commit -m "fix: critical bug"
# PR and merge to master → triggers v0.3.1 release

# 2. Cherry-pick to develop
git checkout develop
git cherry-pick <hotfix-commit-sha>
git push origin develop

# 3. Later, when promoting v0.4.0, rebase will be clean
#    (cherry-picked commit is auto-skipped)
```

#### Before Promoting a Release
If develop and master have diverged (conflicts during promote):
```bash
# Manually rebase develop onto master
git checkout develop
git fetch origin master
git rebase origin/master
# Resolve any conflicts
git push origin develop --force-with-lease

# Now run promote workflow - will be a clean fast-forward
```

### What NOT to Do
- ❌ Don't backport changes from develop to master (creates divergence)
- ❌ Don't merge master into develop (creates merge commits)
- ❌ Don't apply the same fix separately to both branches (creates conflicts)

### Workflows

| Workflow | File | Trigger |
|----------|------|---------|
| Bump Version | `bump-version.yml` | Push to master |
| Release | `release.yml` | After bump-version |
| Canary | `canary.yml` | Push to develop |
| Beta | `beta.yml` | Monthly schedule or manual |
| Promote | `promote-release.yml` | Manual with next_version input |

## Commit Message Convention

This project uses [Conventional Commits](https://www.conventionalcommits.org/) for automatic changelog generation via git-cliff.

### Format

```
<type>[optional scope]: <description>

[optional body]

[optional footer - e.g., "Fixes #42"]
```

### Types

| Type | Description | Changelog Section |
|------|-------------|-------------------|
| `feat` | New feature | Features |
| `fix` | Bug fix | Bug Fixes |
| `docs` | Documentation changes | Documentation |
| `style` | Code style (no logic change) | Styling |
| `refactor` | Code refactoring | Refactoring |
| `perf` | Performance improvement | Performance |
| `test` | Adding/updating tests | Testing |
| `chore` | Maintenance tasks | Miscellaneous |
| `ci` | CI/CD changes | CI/CD |

### Examples

```bash
feat: add shell completion generation
fix(clone): handle repositories with special characters
docs: update installation instructions
refactor(git): extract common git operations
chore: upgrade dependencies
```

### Changelog Generation

- **Stable releases**: CHANGELOG.md is automatically updated when releasing to master
- **Canary/Beta**: Release notes are generated but CHANGELOG.md is not modified
- Configuration files: `cliff.toml` (stable), `cliff-prerelease.toml` (prereleases)

## Overview

This is **daft** - a comprehensive Git extensions toolkit built in Rust. While the project currently focuses on worktree workflow management with both Rust binaries and legacy shell scripts, the vision extends far beyond: daft aims to provide a suite of Git extensions that enhance modern development workflows.

The current worktree commands are intended to be used as custom Git commands (e.g., `git worktree-clone`, `git worktree-checkout`), and future extensions will follow the same pattern of seamlessly integrating with Git's command-line interface.

## Key Concepts

- **Worktree-centric workflow**: One worktree per branch, with all worktrees for a repository organized under a common parent directory
- **Directory structure**: Uses `<repo-name>/.git` at root with worktrees at `<repo-name>/<branch-name>/`
- **`direnv` integration**: Automatically runs `direnv allow` when entering new worktrees that contain `.envrc` files
- **Dynamic branch detection**: Scripts query remote repositories to determine actual default branch (main, master, develop, etc.)

## Architecture

### Single Binary Design

**daft uses a multicall binary architecture** for optimal size and maintainability:

#### How It Works
1. **Single Entry Point**: `src/main.rs` contains the main binary entry point
2. **Command Routing**: The binary examines `argv[0]` (how it was invoked) and routes to the appropriate command
3. **Command Modules**: Individual command implementations live in `src/commands/`
4. **Symlink Distribution**: During installation/development, symlinks are created:
   - `git-worktree-clone` → `daft`
   - `git-worktree-checkout` → `daft`
   - `git-worktree-checkout-branch` → `daft`
   - `git-worktree-checkout-branch-from-default` → `daft`
   - `git-worktree-init` → `daft`
   - `git-worktree-prune` → `daft`
   - `git-daft` → `daft`

#### Benefits
- **Size Efficiency**: 589KB single binary vs ~3.5MB for 6 separate binaries (83% reduction)
- **Code Sharing**: All commands share the same compiled code for common operations
- **Easier Distribution**: Only one binary to compile, package, and distribute
- **Simpler Development**: `make dev` builds once and creates all necessary symlinks
- **Git Integration**: Symlinks work seamlessly with Git's command discovery

#### Directory Structure
```
src/
├── main.rs              # Multicall binary entry point - routes based on argv[0]
├── commands/            # Command implementations
│   ├── mod.rs           # Command module exports
│   ├── clone.rs         # git-worktree-clone implementation
│   ├── checkout.rs      # git-worktree-checkout implementation
│   ├── checkout_branch.rs  # git-worktree-checkout-branch implementation
│   ├── checkout_branch_from_default.rs
│   ├── init.rs          # git-worktree-init implementation
│   └── prune.rs         # git-worktree-prune implementation
├── lib.rs               # Shared library code
├── git.rs               # Git operations wrapper
├── remote.rs            # Remote repository handling
├── direnv.rs            # Direnv integration
├── utils.rs             # Utility functions
└── config.rs            # Configuration handling
```

Legacy shell scripts remain in `src/legacy/` for backward compatibility but are deprecated.

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

**Rust binaries** are installed by adding `target/release/` to your `PATH`, or **legacy scripts** by adding `src/legacy/` to your `PATH`. Once installed, they can be executed as Git subcommands:

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

### Branch names

When working on project tickets, branch names should follow this convention daft-<issue number>/<shortened issue name>

### PRs

PR titles should follow the conventional commit format:

```
feat: add dark mode toggle
fix: resolve authentication timeout
docs: update API documentation
```

Issue references should be in the PR body or commit footer, not the title. Example:
```
Fixes #42
```

## Worktree Workflow

These scripts enable a complete worktree-based development workflow that eliminates traditional Git branch switching friction:

### Initial Setup

**Start with any Git repository:**
```bash
git worktree-clone git@github.com:user/my-project.git
# Or clone daft itself:
git worktree-clone git@github.com:avihut/daft.git
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

## Code Quality Checks

**IMPORTANT**: Always perform these quality checks at the end of every work session to ensure code meets project standards:

### Required End-of-Work Checks

1. **Rust Clippy Linting** - Run clippy to catch common issues and enforce best practices:
   ```bash
   cargo clippy -- -D warnings
   ```
   This must pass with zero warnings. Common issues to watch for:
   - Uninlined format arguments (use `println!("{var}")` instead of `println!("{}", var)`)
   - Using `.len() < 1` instead of `.is_empty()`
   - Empty lines after doc comments
   - Unused imports or variables

2. **Rust Code Formatting** - Ensure consistent code style:
   ```bash
   cargo fmt -- --check
   ```
   If this fails, run `cargo fmt` to fix formatting automatically, then verify with `--check` again.

3. **Unit Test Validation** - Confirm all tests still pass:
   ```bash
   make test-unit
   ```

### Quality Check Workflow

Before committing any Rust code changes:
```bash
# 1. Fix any formatting issues
cargo fmt

# 2. Check for linting issues
cargo clippy -- -D warnings

# 3. Verify tests pass
make test-unit

# 4. (Optional) Run full test suite if significant changes
make test
```

### Common Issues and Fixes

- **Trailing whitespace**: Use your editor's whitespace cleanup or `sed -i '' 's/[[:space:]]*$//' filename.rs`
- **Missing newlines at EOF**: Ensure all files end with a single newline
- **Clippy warnings**: Address each warning individually - don't suppress unless absolutely necessary
- **Formatting inconsistencies**: Always run `cargo fmt` before committing

### CI Integration

These same checks run in GitHub Actions, so passing them locally ensures CI will pass:
- `cargo clippy -- -D warnings` (fails CI on any warnings)
- `cargo fmt -- --check` (fails CI on formatting issues)
- `cargo test` (fails CI on test failures)

**Remember**: These checks are not optional - they are required for all Rust code contributions and must pass before work is considered complete.

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
use clap::Parser;
use daft::utils::*;

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

## Legacy Removal Plan

Once the Rust migration is complete and deemed stable, the legacy shell scripts can be cleanly removed. Here's the systematic plan:

### Phase 1: Preparation and Validation
1. **Ensure Feature Parity**: Verify all legacy script functionality is fully implemented in Rust
2. **Performance Benchmarking**: Confirm Rust versions meet or exceed shell script performance
3. **User Migration**: Provide migration guide for users switching from shell to Rust versions
4. **Documentation Update**: Update all references from shell scripts to Rust binaries

### Phase 2: Deprecation Period (Recommended 1-2 releases)
1. **Add deprecation warnings** to shell scripts:
   ```bash
   echo "WARNING: Shell scripts are deprecated. Use Rust binaries instead."
   echo "Legacy scripts will be removed in version X.Y.Z"
   ```
2. **Update installation docs** to recommend Rust binaries
3. **Add migration notices** in README and release notes

### Phase 3: Systematic Legacy Removal

#### Files to Remove:
```
src/legacy/                                    # Entire legacy directory
├── README.md
├── git-worktree-checkout
├── git-worktree-checkout-branch
├── git-worktree-checkout-branch-from-default
├── git-worktree-clone
├── git-worktree-init
└── git-worktree-prune

tests/legacy/                                  # Entire legacy test directory
├── test_all.sh
├── test_checkout.sh
├── test_checkout_branch.sh
├── test_checkout_branch_from_default.sh
├── test_clone.sh
├── test_framework.sh
├── test_init.sh
├── test_prune.sh
└── test_simple.sh
```

#### Makefile Targets to Remove:
```makefile
# Legacy test targets to remove:
test-legacy
test-legacy-framework
test-legacy-clone
test-legacy-checkout
test-legacy-checkout-branch
test-legacy-checkout-branch-from-default
test-legacy-init
test-legacy-prune
test-legacy-simple
test-legacy-verbose
test-perf-legacy

# Compatibility aliases to remove:
test-framework, test-clone, test-checkout, etc.
```

#### GitHub Actions Cleanup:
```yaml
# Remove from .github/workflows/test.yml:
- name: Run legacy tests
  run: make test-legacy

- name: Add scripts to PATH (shell version)
  run: |
    echo "${{ github.workspace }}/src/legacy" >> $GITHUB_PATH

# Legacy help command tests section
```

#### Documentation Cleanup:
- Remove shell script installation instructions from README.md
- Remove legacy script examples and usage patterns
- Update CLAUDE.md to remove legacy architecture details
- Remove shell script references from all documentation

### Phase 4: Update Project Structure

#### Update Makefile:
```makefile
# Simplify test targets to:
test: test-unit test-integration
test-all: test-unit test-integration

# Remove all legacy-specific targets
# Update help text to remove legacy references
```

#### Update GitHub Actions:
```yaml
# Simplify to focus only on Rust:
jobs:
  test:
    strategy:
      matrix:
        os: [ubuntu-latest, macos-latest]
    steps:
      - name: Run unit tests
        run: make test-unit
      - name: Run integration tests  
        run: make test-integration
```

#### Update README.md:
- Remove "Legacy vs Rust" comparison sections
- Simplify installation to Rust-only approach
- Update all examples to use Rust binaries
- Remove shell script PATH setup instructions

### Phase 5: Cleanup Git History (Optional)
For a clean repository:
```bash
# Remove legacy files from git history (DESTRUCTIVE)
git filter-branch --tree-filter 'rm -rf src/legacy tests/legacy' --prune-empty HEAD
git for-each-ref --format="%(refname)" refs/original/ | xargs -n 1 git update-ref -d
```

### Phase 6: Post-Removal Validation
1. **Test CI/CD Pipeline**: Ensure all workflows pass without legacy components
2. **Update Release Process**: Remove legacy binary building/packaging
3. **Documentation Review**: Verify no broken links or references to removed files
4. **User Communication**: Release notes clearly document the removal

### Migration Commands for Users
Provide clear migration paths:

```bash
# Old (shell scripts)
export PATH="$PATH:/path/to/scripts"
git worktree-clone repo.git

# New (Rust binaries)  
export PATH="$PATH:/path/to/target/release"
git-worktree-clone repo.git
# OR (if installed via package manager)
git worktree-clone repo.git
```

### Benefits of Removal
- **Simplified codebase**: ~50% reduction in test files and maintenance burden
- **Reduced CI time**: Eliminate duplicate legacy test runs
- **Cleaner documentation**: Single source of truth for usage patterns
- **Easier development**: Focus on single implementation path
- **Better user experience**: No confusion between shell vs Rust versions

### Risk Mitigation
- **Gradual rollout**: Use semantic versioning to signal breaking changes
- **Backup branches**: Tag legacy-complete version before removal
- **User survey**: Collect feedback during deprecation period
- **Rollback plan**: Keep ability to restore legacy if critical issues arise

This plan ensures a clean, methodical removal of legacy components while maintaining user confidence and project stability.
