# CLAUDE.md

This file provides guidance when working with code in this repository.

## Recent Changes

**Man Pages Added (2025-01)**
- Added man page generation using `clap_mangen`
- Man pages are auto-generated from clap command definitions
- Generate with `just gen-man` or install with `just install-man`
- See "Documentation Requirements" section for best practices

**Hooks System Added (2025-01)**
- Added flexible, project-managed hooks system for worktree lifecycle events
- Replaces hardcoded direnv integration with configurable hooks
- Six hook types: post-clone, post-init, pre-create, post-create, pre-remove, post-remove
- Trust-based security model with per-repository trust levels
- Trust management via `git daft hooks` subcommand
- Hooks stored in `.daft/hooks/` within repositories
- See `docs/PLAN-hooks-system.md` for full specification

**Legacy Scripts Removed (2025-10)**
- Removed deprecated shell scripts from `src/legacy/`
- Removed legacy tests from `tests/legacy/`
- Project is now Rust-only
- See `docs/HISTORY.md` for project origins and shell script history

**Single Binary Architecture Migration (2025-10-18)**
- Migrated from 6 separate binaries to single multicall binary
- Reduced total binary size from ~3.5MB to 589KB (83% reduction)
- All commands now route through single `daft` binary via symlinks
- Binary detects invocation name (argv[0]) and routes to appropriate command
- Development workflow streamlined with `just dev` recipe

**Project Renamed from `git-worktree-workflow` to `daft` (2025-10-17)**
- GitHub repository: `https://github.com/avihut/daft` (was `git-worktree-workflow`)
- Project directory: `/Users/avihu/Projects/daft` (was `git-worktree-workflow`)
- Cargo package name: `daft` (was `git-worktree-workflow`)
- All command names remain unchanged (`git-worktree-*`)
- All functionality preserved
- Documentation and installation paths updated throughout

## Critical Development Rules

**DO NOT** violate these rules under any circumstances:

1. **Never modify global git config** - Do not change the global git user name, email, or any other global settings. This applies to both manual work and automated tests. If existing tests modify global config, they must be fixed to use local config instead.

2. **Never use this repository for testing** - This project's own git repository must never be used as a test subject for the worktree commands. Tests must always create isolated temporary repositories. Using this repo for testing could corrupt the project's version control state.

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
| **Stable** | Promote workflow | `v0.3.0` | Yes | Yes |
| **Canary** | Push to develop | `v0.4.0-canary.N` | No | No |
| **Beta** | Monthly/manual | `v0.4.0-beta.N` | No | No |

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
- Don't backport changes from develop to master (creates divergence)
- Don't merge master into develop (creates merge commits)
- Don't apply the same fix separately to both branches (creates conflicts)

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

This is **daft** - a comprehensive Git extensions toolkit built in Rust. The project currently focuses on worktree workflow management, with the vision of providing a suite of Git extensions that enhance modern development workflows.

The worktree commands are intended to be used as custom Git commands (e.g., `git worktree-clone`, `git worktree-checkout`), and future extensions will follow the same pattern of seamlessly integrating with Git's command-line interface.

## Key Concepts

- **Worktree-centric workflow**: One worktree per branch, with all worktrees for a repository organized under a common parent directory
- **Directory structure**: Uses `<repo-name>/.git` at root with worktrees at `<repo-name>/<branch-name>/`
- **Hooks system**: Project-managed lifecycle hooks for automation (post-clone, pre-create, post-create, pre-remove, etc.)
- **Dynamic branch detection**: Commands query remote repositories to determine actual default branch (main, master, develop, etc.)

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
- **Simpler Development**: `just dev` builds once and creates all necessary symlinks
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
│   ├── prune.rs         # git-worktree-prune implementation
│   ├── carry.rs         # git-worktree-carry implementation
│   ├── hooks.rs         # git-daft hooks subcommand
│   ├── man.rs           # daft man - man page generation
│   ├── shell_init.rs    # daft shell-init implementation
│   └── shortcuts.rs     # daft setup shortcuts - shortcut management
├── hooks/               # Hooks system
│   ├── mod.rs           # Hook types and configuration
│   ├── executor.rs      # Hook execution logic
│   ├── trust.rs         # Trust management and storage
│   └── environment.rs   # Environment variable builder
├── lib.rs               # Shared library code (includes output_cd_path for shell integration)
├── git.rs               # Git operations wrapper
├── remote.rs            # Remote repository handling
├── shortcuts.rs         # Shortcut aliases and resolution
├── utils.rs             # Utility functions
└── config.rs            # Configuration handling
```

### Core Commands

- **`git-worktree-clone`**: Clones a repository into the structured layout (`<repo>/.git` + `<repo>/<default-branch>/`)
- **`git-worktree-init`**: Initializes a new repository in the structured layout (`<repo>/.git` + `<repo>/<initial-branch>/`)
- **`git-worktree-checkout`**: Creates worktree from an existing local or remote branch
- **`git-worktree-checkout-branch`**: Creates new worktree + new branch from current or specified base branch
- **`git-worktree-checkout-branch-from-default`**: Creates new worktree + new branch from remote's default branch
- **`git-worktree-prune`**: Removes local branches whose remote counterparts are deleted, plus associated worktrees
- **`git-worktree-carry`**: Carries uncommitted changes to one or more existing worktrees

### Shell Integration

The `daft shell-init` command generates shell wrapper functions that enable automatic cd into new worktrees. This solves the problem where the Rust binary changes directory internally but the parent shell stays in the original directory.

**How it works:**
1. Wrappers set `DAFT_SHELL_WRAPPER=1` before calling the underlying command
2. When this env var is set, commands output a `__DAFT_CD__:/path/to/worktree` marker
3. Wrappers parse this marker and use the shell's builtin `cd` to change directory

**Usage:**
```bash
# Bash/Zsh: Add to ~/.bashrc or ~/.zshrc
eval "$(daft shell-init bash)"

# Fish: Add to ~/.config/fish/config.fish
daft shell-init fish | source

# With short aliases (gwco, gwcob, etc.)
eval "$(daft shell-init bash --aliases)"
```

### Command Shortcuts

daft supports three shortcut styles for frequently used commands:

| Style | Shortcuts | Description |
|-------|-----------|-------------|
| **Git** (default) | `gwtclone`, `gwtinit`, `gwtco`, `gwtcb`, `gwtcbm`, `gwtprune`, `gwtcarry`, `gwtfetch` | Git worktree focused |
| **Shell** | `gwco`, `gwcob`, `gwcobd` | Shell-friendly minimal |
| **Legacy** | `gclone`, `gcw`, `gcbw`, `gcbdw`, `gprune` | Older style aliases |

**How it works:**
1. Shortcuts are resolved in `src/shortcuts.rs` before command routing in `main.rs`
2. The binary detects the invocation name and maps shortcuts to their full command names
3. Symlinks are managed via `daft setup shortcuts`

**Managing shortcuts:**
```bash
daft setup shortcuts list            # List all styles and mappings
daft setup shortcuts status          # Show installed shortcuts
daft setup shortcuts enable git      # Enable git-style shortcuts
daft setup shortcuts disable legacy  # Disable legacy shortcuts
daft setup shortcuts only shell      # Enable only shell shortcuts
daft setup shortcuts only git --dry-run  # Preview changes
```

**Complete shortcut mapping:**

| Full Command | Git Style | Shell Style | Legacy Style |
|--------------|-----------|-------------|--------------|
| `git-worktree-clone` | `gwtclone` | - | `gclone` |
| `git-worktree-init` | `gwtinit` | - | - |
| `git-worktree-checkout` | `gwtco` | `gwco` | `gcw` |
| `git-worktree-checkout-branch` | `gwtcb` | `gwcob` | `gcbw` |
| `git-worktree-checkout-branch-from-default` | `gwtcbm` | `gwcobd` | `gcbdw` |
| `git-worktree-prune` | `gwtprune` | - | `gprune` |
| `git-worktree-carry` | `gwtcarry` | - | - |
| `git-worktree-fetch` | `gwtfetch` | - | - |

## Usage

Install by adding `target/release/` to your `PATH`. Commands can be executed as Git subcommands:

```bash
git worktree-clone <repository-url>
git worktree-init <repository-name>
git worktree-checkout <existing-branch-name>
git worktree-checkout-branch <new-branch-name> [base-branch-name]
git worktree-checkout-branch-from-default <new-branch-name>
git worktree-prune
```

## Development Notes

- Commands can be executed from anywhere within the Git repository (including deep subdirectories)
- New worktrees are always created at the project root level (alongside the `.git` directory)
- Commands use `git rev-parse --git-common-dir` to locate the project root regardless of execution location
- Commands execute lifecycle hooks from `.daft/hooks/` (requires trust for untrusted repositories)
- Error handling includes cleanup of partially created worktrees on failure

### Branch names

When working on project tickets, branch names should follow this convention daft-<issue number>/<shortened issue name>

### PRs

**Target branch**: All PRs should be opened against `develop`, not `master`. The `master` branch only receives changes through the promote workflow (rebase from develop).

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

These commands enable a complete worktree-based development workflow that eliminates traditional Git branch switching friction:

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
# Automatically: creates branch, pushes to origin, sets upstream, runs hooks
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

The project has a comprehensive two-tier testing architecture covering unit tests and integration tests. All tests are fully integrated into GitHub Actions CI/CD workflows.

### Testing Architecture

#### 1. **Unit Tests** (`just test-unit`)
- Rust unit tests for library functions and utilities
- Tests covering:
  - Git command wrapper functionality
  - Directory and path utility functions
  - Branch/repository name validation
  - Hooks system logic
  - Remote branch detection
- Run via `cargo test`

#### 2. **Integration Tests** (`just test-integration`)
- End-to-end tests for Rust binaries in `tests/integration/`
- Key test files:
  - `test_checkout_branch_workflow` - Tests development workflow scenarios
  - `test_checkout_branch_from_default_remote_updates` - Tests remote branch updates
  - `test_prune_multiple_deletions` - Tests cleanup operations
  - `test_integration_full_workflow` - Tests complete workflow scenarios
- Includes performance, security, and cross-platform compatibility tests

### Test Execution

**Run all tests:**
```bash
just test        # or just test-all
```

**Run specific test suites:**
```bash
just test-unit                    # Rust unit tests only
just test-integration             # Rust integration tests
```

**Run individual integration test suites:**
```bash
just test-integration-clone
just test-integration-checkout
just test-integration-checkout-branch
just test-integration-checkout-branch-from-default
just test-integration-init
just test-integration-prune
```

### GitHub Actions Integration

The testing architecture is fully integrated into GitHub Actions via `.github/workflows/test.yml`:

1. **Multi-platform testing**: Runs on both `ubuntu-latest` and `macos-latest`
2. **Complete test coverage**:
   - Builds Rust binaries (`cargo build --release`)
   - Runs Rust unit tests (`cargo test`)
   - Runs Rust linting (`cargo clippy -- -D warnings`)
   - Checks code formatting (`cargo fmt -- --check`)
   - Executes integration tests (`just test-integration`)
3. **Path configuration**: Automatically adds Rust binaries to PATH
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

### Justfile Integration

The justfile provides convenient recipes for all testing needs:
- `test-all` runs unit + integration tests
- Individual recipes for granular testing
- Verbose modes for debugging (`test-verbose`, `test-integration-verbose`)
- Performance testing recipes (`test-perf`, `test-perf-integration`)
- CI simulation recipe (`just ci`) that mimics GitHub Actions workflow

### Test Maintenance

When adding new features:
1. Add unit tests for new Rust functions in the appropriate module
2. Add integration tests in `tests/integration/` following existing patterns
3. Ensure tests pass locally with `just test`
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
   just test-unit
   ```

### Quality Check Workflow

Before committing any Rust code changes:
```bash
# 1. Fix any formatting issues
cargo fmt

# 2. Check for linting issues
cargo clippy -- -D warnings

# 3. Verify tests pass
just test-unit

# 4. (Optional) Run full test suite if significant changes
just test
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

## Documentation Requirements

### Man Pages

Man pages are **auto-generated from clap command definitions** using `clap_mangen`. This means:

1. **No separate man page files to maintain** - Man pages are derived directly from the `#[command]` and `#[arg]` attributes in each command's `Args` struct.

2. **Write descriptive help text** - When adding or modifying commands, ensure you provide:
   - `#[command(about = "...")]` - Short one-line description (shown in man page NAME section)
   - `#[command(long_about = "...")]` - Detailed description (shown in DESCRIPTION section)
   - `#[arg(help = "...")]` - Help text for each argument (shown in OPTIONS section)

3. **Example of well-documented command:**
   ```rust
   #[derive(Parser)]
   #[command(name = "git-worktree-example")]
   #[command(about = "Short description for the command")]
   #[command(long_about = r#"
   Detailed multi-line description explaining what the command does,
   when to use it, and any important behavior notes.
   "#)]
   pub struct Args {
       #[arg(help = "Description of what this argument does")]
       target: String,

       #[arg(short, long, help = "Enable verbose output")]
       verbose: bool,
   }
   ```

4. **Generation commands:**
   ```bash
   just gen-man       # Generate to man/ directory
   just install-man   # Install to ~/.local/share/man/man1/
   daft man --help    # See all options
   ```

5. **When adding new commands:**
   - Add the command to the `COMMANDS` array in `src/commands/man.rs`
   - Add the command mapping in `get_command_for_name()` function
   - Ensure the command has proper `#[command]` and `#[arg]` documentation

## Project History

For information about the project's origins as shell scripts and its evolution to Rust, see [docs/HISTORY.md](docs/HISTORY.md).
