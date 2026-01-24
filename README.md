# daft - Git Extensions Toolkit

[![CI](https://github.com/avihut/daft/actions/workflows/test.yml/badge.svg)](https://github.com/avihut/daft/actions/workflows/test.yml)
[![Release](https://img.shields.io/github/v/release/avihut/daft?label=release)](https://github.com/avihut/daft/releases/latest)
[![License: MIT](https://img.shields.io/badge/License-MIT-blue.svg)](https://opensource.org/licenses/MIT)
[![Rust](https://img.shields.io/badge/rust-1.70%2B-orange.svg)](https://www.rust-lang.org/)
[![Homebrew](https://img.shields.io/badge/homebrew-avihut%2Fdaft-blueviolet)](https://github.com/avihut/homebrew-daft)
[![macOS](https://img.shields.io/badge/macOS-supported-success)](https://github.com/avihut/daft/releases)
[![Linux](https://img.shields.io/badge/Linux-supported-success)](https://github.com/avihut/daft/releases)
[![Windows](https://img.shields.io/badge/Windows-supported-success)](https://github.com/avihut/daft/releases)

> Stop switching branches. Work on multiple branches simultaneously.

**daft** gives each Git branch its own directory. No more stashing, no more context switching, no more waiting for builds to restart.

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
brew install avihut/daft

# Clone a repo (creates my-project/main/)
git worktree-clone git@github.com:user/my-project.git

# Start a feature branch (creates my-project/feature/auth/)
git worktree-checkout-branch feature/auth
```

Each directory is a full working copy. Run different branches in different terminals. Your IDE state, node_modules, build artifacts - all isolated per branch.

## Installation

**daft uses a single multicall binary architecture** - one 589KB binary with symlinks for all commands.

### macOS

#### Homebrew (Recommended)

```bash
brew install avihut/daft
```

This automatically:
- Installs the `daft` binary
- Creates symlinks for all `git-worktree-*` commands
- Installs shell completions (bash, zsh, fish)
- Adds commands to your PATH

#### From Source

```bash
git clone https://github.com/avihut/daft.git
cd daft
cargo build --release

# Install system-wide
sudo cp target/release/daft /usr/local/bin/
cd /usr/local/bin/
sudo ln -s daft git-worktree-clone
sudo ln -s daft git-worktree-checkout
sudo ln -s daft git-worktree-checkout-branch
sudo ln -s daft git-worktree-checkout-branch-from-default
sudo ln -s daft git-worktree-init
sudo ln -s daft git-worktree-prune
sudo ln -s daft git-daft
```

### Windows

#### PowerShell Installer (Recommended)

```powershell
irm https://github.com/avihut/daft/releases/latest/download/daft-installer.ps1 | iex
```

This automatically:
- Downloads the latest Windows binary
- Installs to your local bin directory
- Creates shims for all `git-worktree-*` commands
- Adds to your PATH

#### MSI Installer

Download the `.msi` installer from [GitHub Releases](https://github.com/avihut/daft/releases/latest) and run it.

#### Scoop (Coming Soon)

```powershell
scoop bucket add avihut https://github.com/avihut/daft
scoop install daft
```

### Linux

#### From Binary Release

Download the appropriate binary for your platform from [GitHub Releases](https://github.com/avihut/daft/releases/latest):

```bash
# Download (replace with actual version and architecture)
wget https://github.com/avihut/daft/releases/latest/download/daft-x86_64-unknown-linux-gnu.tar.xz

# Extract
tar -xf daft-x86_64-unknown-linux-gnu.tar.xz

# Install system-wide
sudo cp daft /usr/local/bin/
cd /usr/local/bin/
sudo ln -s daft git-worktree-clone
sudo ln -s daft git-worktree-checkout
sudo ln -s daft git-worktree-checkout-branch
sudo ln -s daft git-worktree-checkout-branch-from-default
sudo ln -s daft git-worktree-init
sudo ln -s daft git-worktree-prune
sudo ln -s daft git-daft
```

#### From Source

```bash
git clone https://github.com/avihut/daft.git
cd daft
cargo build --release

# Install (same as above)
sudo cp target/release/daft /usr/local/bin/
# ... create symlinks as above
```

### Verify Installation

```bash
# Check version
daft --version

# View documentation
git daft

# Test a command
git worktree-clone --help
```

### Shell Completions

daft provides intelligent shell completions for bash, zsh, and fish shells with dynamic branch name suggestions.

#### Quick Install (Recommended)

```bash
# After building/installing daft, install completions for all shells
just install-completions

# Or install manually using daft
daft completions bash --install
daft completions zsh --install
daft completions fish --install
```

#### Manual Installation by Shell

**Bash:**
```bash
# Generate completion file
daft completions bash --command=git-worktree-checkout > ~/.local/share/bash-completion/completions/git-worktree-checkout

# Repeat for other commands or generate all at once
just gen-completions-bash

# Add to ~/.bashrc (if not already present)
if [ -f ~/.local/share/bash-completion/bash_completion ]; then
    . ~/.local/share/bash-completion/bash_completion
fi
```

**Zsh:**
```bash
# Add completions directory to fpath (in ~/.zshrc)
fpath=(~/.zfunc $fpath)
autoload -Uz compinit && compinit

# Generate completion files
daft completions zsh --install
```

**Fish:**
```bash
# Fish automatically loads completions from the config directory
daft completions fish --install
```

#### Features

- **Tab completion** for all command names and options
- **Dynamic branch name suggestions** - type `git worktree-checkout fea<TAB>` to see feature branches
- **Context-aware** - only shows relevant branches for each command
- **Fast** - completion suggestions return in < 50ms even in large repositories
- **Pattern suggestions** - suggests `feature/`, `bugfix/`, `hotfix/`, etc. when creating new branches

### Shell Integration (cd into new worktrees)

By default, daft commands change directory internally but your shell stays in the original directory. Shell integration solves this by automatically cd'ing into newly created worktrees.

**Automatic setup (recommended):**
```bash
daft setup
```

This detects your shell, backs up your config, and adds the integration line automatically.

**Manual setup:**

**Bash** (`~/.bashrc`):
```bash
eval "$(daft shell-init bash)"
```

**Zsh** (`~/.zshrc`):
```zsh
eval "$(daft shell-init zsh)"
```

**Fish** (`~/.config/fish/config.fish`):
```fish
daft shell-init fish | source
```

**With short aliases** (gwco, gwcob, etc.):
```bash
eval "$(daft shell-init bash --aliases)"
```

After setting this up, both command forms will auto-cd into new worktrees:
```bash
git worktree-checkout feature/auth    # spaces - works with shell integration
git-worktree-checkout feature/auth    # hyphens - also works
```

### Command Shortcuts

daft provides short aliases for frequently used commands. Three styles are available:

| Style | Shortcuts | Description |
|-------|-----------|-------------|
| **Git** (default) | `gwtclone`, `gwtinit`, `gwtco`, `gwtcb`, `gwtcbm`, `gwtprune`, `gwtcarry`, `gwtfetch` | Git worktree focused |
| **Shell** | `gwco`, `gwcob`, `gwcobd` | Shell-friendly minimal |
| **Legacy** | `gclone`, `gcw`, `gcbw`, `gcbdw`, `gprune` | Older style aliases |

**Managing shortcuts:**
```bash
# List all shortcut styles and their mappings
daft setup shortcuts list

# Show currently installed shortcuts
daft setup shortcuts status

# Enable a specific style
daft setup shortcuts enable git      # Enable git-style shortcuts
daft setup shortcuts enable shell    # Enable shell-style shortcuts

# Disable a style
daft setup shortcuts disable legacy

# Use only one style (disable others)
daft setup shortcuts only shell

# Preview changes without modifying
daft setup shortcuts only git --dry-run
```

**Example usage with shortcuts:**
```bash
gwtco feature/auth           # Same as: git worktree-checkout feature/auth
gwtcb feature/new-feature    # Same as: git worktree-checkout-branch feature/new-feature
gwtprune                     # Same as: git worktree-prune
```

## Commands

### Core Commands

| Command | Description |
|---------|-------------|
| `git worktree-clone` | Clone a repository into the structured layout |
| `git worktree-init` | Initialize a new repository in the structured layout |
| `git worktree-checkout` | Create worktree from existing branch |
| `git worktree-checkout-branch` | Create new worktree + new branch |
| `git worktree-checkout-branch-from-default` | Create new branch from remote's default branch |
| `git worktree-prune` | Remove worktrees for deleted remote branches |
| `git daft` | Show daft documentation and available commands |

### Command Details

#### `git worktree-clone`
Clones a repository and sets up the worktree structure:

```bash
git worktree-clone <repository-url>
git worktree-clone --no-checkout <repository-url>  # Only clone, no worktree
git worktree-clone --quiet <repository-url>        # Silent operation
git worktree-clone --all-branches <repository-url> # Create worktrees for all branches
```

#### `git worktree-init`
Initialize a new repository with worktree structure:

```bash
git worktree-init <repository-name>
git worktree-init --initial-branch main <repository-name>
git worktree-init --bare <repository-name>  # Create bare repository only
```

#### `git worktree-checkout`
Create worktree from existing branch:

```bash
git worktree-checkout <branch-name>
```

#### `git worktree-checkout-branch`
Create new branch and worktree:

```bash
git worktree-checkout-branch <new-branch-name>
git worktree-checkout-branch <new-branch-name> <base-branch>
```

#### `git worktree-checkout-branch-from-default`
Create new branch from remote's default branch:

```bash
git worktree-checkout-branch-from-default <new-branch-name>
```

#### `git worktree-prune`
Clean up deleted remote branches:

```bash
git worktree-prune
```

## Workflow Examples

### Starting a New Project

```bash
# Clone existing repository
git worktree-clone git@github.com:user/my-project.git

# Result:
# my-project/
# ├── .git/           # Shared Git metadata
# └── main/          # First worktree (default branch)
#     └── ... (project files)

# You're automatically placed in my-project/main/
```

### Daily Development

```bash
# Create feature branch
git worktree-checkout-branch feature/user-auth

# Work on feature
cd my-project/feature/user-auth/
# Make changes, commit, etc.

# Switch to bugfix (in parallel)
git worktree-checkout-branch bugfix/login-issue

# Your directory structure:
# my-project/
# ├── .git/
# ├── main/
# ├── feature/user-auth/       # Feature work continues here
# └── bugfix/login-issue/      # Bugfix work here
```

### Parallel Development

```bash
# Terminal 1: Feature development
cd my-project/feature/user-auth/
npm run dev

# Terminal 2: Bug fixing
cd my-project/bugfix/login-issue/
npm test

# Terminal 3: Code review
git worktree-checkout feature/teammate-pr
cd my-project/feature/teammate-pr/
npm run build
```

### Cleanup

```bash
# After branches are merged and deleted remotely
git worktree-prune

# Automatically removes local branches and worktrees
# for deleted remote branches
```

## Hooks System

daft provides a flexible, project-managed hooks system for automating worktree lifecycle events.

### Hook Types

| Hook | Trigger | Use Case |
|------|---------|----------|
| `post-clone` | After repository clone | Initial setup, dependency install |
| `post-init` | After repository init | Initialize new project |
| `pre-create` | Before worktree creation | Validate environment, check resources |
| `post-create` | After worktree created | Environment setup, docker up |
| `pre-remove` | Before worktree removal | Cleanup, docker down |
| `post-remove` | After worktree removed | Notifications, logging |

### Setting Up Hooks

Create hooks in `.daft/hooks/` within your repository:

```bash
mkdir -p .daft/hooks

# Example: post-create hook for environment setup
cat > .daft/hooks/post-create << 'EOF'
#!/bin/bash
# Allow direnv in new worktrees
if [ -f ".envrc" ] && command -v direnv &>/dev/null; then
    direnv allow .
fi

# Start docker services (isolated per branch)
if [ -f "docker-compose.yml" ]; then
    export COMPOSE_PROJECT_NAME="myapp-${DAFT_BRANCH_NAME//\//-}"
    docker compose up -d
fi
EOF
chmod +x .daft/hooks/post-create

# Commit hooks to share with team
git add .daft/hooks/
git commit -m "Add daft hooks for environment setup"
```

### Trust Model

For security, hooks require explicit trust before execution:

```bash
# Trust current repository
git daft hooks trust

# Check trust status
git daft hooks status

# List all trusted repositories
git daft hooks list
```

When cloning, hooks are not executed by default:

```bash
# Clone without running hooks (safe default)
git worktree-clone https://github.com/user/repo.git

# Clone and trust hooks immediately
git worktree-clone https://github.com/user/repo.git --trust-hooks

# Clone and skip hooks without prompting
git worktree-clone https://github.com/user/repo.git --no-hooks
```

### Environment Variables

Hooks receive context about the operation:

| Variable | Description |
|----------|-------------|
| `DAFT_HOOK` | Hook type (e.g., `post-create`) |
| `DAFT_COMMAND` | Triggering command (e.g., `checkout-branch`) |
| `DAFT_PROJECT_ROOT` | Repository root path |
| `DAFT_WORKTREE_PATH` | Path to target worktree |
| `DAFT_BRANCH_NAME` | Branch name |
| `DAFT_IS_NEW_BRANCH` | `true` or `false` |
| `DAFT_BASE_BRANCH` | Base branch (if applicable) |

### Migration from direnv

If you were relying on daft's previous built-in direnv integration, create a `post-create` hook:

```bash
mkdir -p .daft/hooks
cat > .daft/hooks/post-create << 'EOF'
#!/bin/bash
[ -f ".envrc" ] && command -v direnv &>/dev/null && direnv allow .
EOF
chmod +x .daft/hooks/post-create
git daft hooks trust
```

## Testing

This project includes comprehensive test coverage:

### Test Structure
```
tests/
├── integration/         # End-to-end Rust integration tests (80+ scenarios)
└── README.md           # Detailed testing documentation
```

### Quick Testing
```bash
# Run all tests (unit + integration)
just test

# Run only Rust unit tests
just test-unit

# Run only integration tests
just test-integration

# Run specific test suites
just test-integration-clone
just test-integration-checkout
```

### Rust Unit Tests
```bash
# Run Rust unit tests
cargo test

# Check code formatting and linting
cargo fmt --check
cargo clippy -- -D warnings
```

### Advanced Testing
```bash
# Run with verbose output
just test-verbose

# Run performance tests
just test-perf

# Run individual test files
cd tests/integration && ./test_init.sh
```

### Test Coverage
The comprehensive test framework includes:
- **80+ test scenarios** covering all commands
- **Isolated test environments** with temporary directories
- **Mock remote repositories** for realistic testing
- **Security testing**: Path traversal prevention
- **Performance validation**: Timing and resource usage
- **Cross-platform compatibility** (Ubuntu, macOS)
- **Error handling** and edge case validation
- **CI/CD integration** with GitHub Actions

See `tests/README.md` for detailed testing documentation.

## Architecture

### Single Binary Design

**daft uses a multicall binary architecture** - a single 589KB executable that provides all commands:

- **One binary**: `target/release/daft` (589KB)
- **Multiple symlinks**: `git-worktree-clone`, `git-worktree-checkout`, etc. all point to `daft`
- **Intelligent routing**: The binary detects how it was invoked (via `argv[0]`) and routes to the appropriate command
- **83% size reduction**: Compared to 6 separate binaries (~3.5MB), the single binary approach saves ~2.9MB

### Directory Structure
```
daft/
├── src/                     # Rust source code
│   ├── commands/            # Command implementations
│   │   ├── clone.rs         # git-worktree-clone
│   │   ├── checkout.rs      # git-worktree-checkout
│   │   ├── checkout_branch.rs  # git-worktree-checkout-branch
│   │   ├── checkout_branch_from_default.rs
│   │   ├── init.rs          # git-worktree-init
│   │   ├── prune.rs         # git-worktree-prune
│   │   └── mod.rs           # Command module
│   ├── main.rs              # Multicall binary entry point
│   ├── lib.rs               # Shared library code
│   ├── git.rs               # Git operations
│   ├── remote.rs            # Remote repository handling
│   ├── hooks/               # Lifecycle hooks system
│   └── utils.rs             # Utility functions
├── tests/                   # Test suite
│   ├── integration/         # End-to-end integration tests
│   └── README.md            # Testing documentation
├── docs/                    # Documentation
│   └── HISTORY.md           # Project history and origins
├── target/release/          # Build artifacts (gitignored)
│   ├── daft                 # Main binary (589KB)
│   ├── git-worktree-clone → daft       # Symlinks
│   ├── git-worktree-checkout → daft
│   ├── git-worktree-checkout-branch → daft
│   ├── git-worktree-checkout-branch-from-default → daft
│   ├── git-worktree-init → daft
│   ├── git-worktree-prune → daft
│   └── git-daft → daft
├── Cargo.toml              # Rust project configuration
├── Cargo.lock              # Rust dependency lock file
├── CLAUDE.md               # Project documentation
└── README.md              # This file
```

### Design Principles

- **Robust error handling**: All commands include comprehensive error checking and cleanup
- **Path independence**: Commands work from any directory within the repository
- **Consistent behavior**: All commands follow the same patterns and conventions
- **Flexible automation**: Project-managed hooks for custom automation workflows
- **Atomic operations**: Failed operations are cleaned up automatically

## Requirements

- **Git**: Version 2.5+ (for worktree support)
- **Rust**: Version 1.70+ (for building from source)

## Rust Implementation Benefits

The Rust implementation provides significant advantages:

### Current Features
- **Type safety**: Compile-time error checking prevents runtime issues
- **Better error handling**: Comprehensive error messages and graceful failures
- **Advanced CLI**: Professional argument parsing with `clap`
- **Single binary**: Easy distribution and installation
- **Cross-platform**: Better Windows support and compatibility
- **Performance**: Faster startup and execution times

### Future Enhancements
- **Shell completions**: Dynamic completion generation
- **Interactive features**: Branch selection menus
- **Hook system**: Custom workflow automation
- **Configuration files**: User-defined settings and preferences
- **Enhanced testing**: Better unit test coverage and integration testing

## Contributing

1. Fork the repository
2. Create a feature branch: `git-worktree-checkout-branch feature/my-feature`
3. Make your changes and add tests
4. Run the test suite: `cargo test && just test`
5. Submit a pull request

### Local Development Setup

**Quick Start:**
```bash
# Build binary, create symlinks, and verify
just dev

# Add to PATH for testing Git commands
export PATH="$PWD/target/release:$PATH"

# Test it works
git daft
git worktree-clone --help
```

**Development Workflow:**
```bash
# 1. Make changes
vim src/commands/clone.rs

# 2. Rebuild and test
just dev                           # Quick: build + verify
./target/release/git-worktree-clone --help

# 3. Run tests
cargo test --lib                   # Unit tests (fast)
just test                          # Full test suite

# 4. Quality checks
cargo clippy -- -D warnings        # Linting
cargo fmt                          # Formatting
```

**Useful Recipes:**
- `just dev` - Build binary + create symlinks + verify (recommended)
- `just dev-test` - Full setup + run all tests
- `just dev-clean` - Remove symlinks (keeps binary)
- `just help` - Show all available recipes

**Architecture Note:**
daft uses a single binary (589KB) with symlinks for all commands. When you run `just dev`, it creates symlinks in `target/release/` that point to the main `daft` binary. The binary detects how it was invoked (via argv[0]) and routes to the appropriate command.

### Guidelines

- **Add tests**: Include unit tests for new functionality
- **Run quality checks**: `cargo clippy` and `cargo fmt` before committing
- **Update docs**: Keep README.md and inline documentation current

## Release Process

daft uses a multi-channel release system:

| Channel | Branch | Trigger | Version Pattern | Audience |
|---------|--------|---------|-----------------|----------|
| **Stable** | master | Every push | v1.0.X | All users |
| **Canary** | develop | Every push | v2.0.0-canary.X | Developers |
| **Beta** | develop | Monthly | v2.0.0-beta.X | Early adopters |

### Release Channels

- **Stable (master)**: Auto-patches on every push. Manual minor/major via Cargo.toml bump.
- **Canary (develop)**: Bleeding-edge builds on every commit. Use at your own risk.
- **Beta (develop)**: Monthly curated releases for testing upcoming versions.

### Promoting a Major/Minor Release

To release a new major or minor version:

1. Ensure `develop` branch has all features for the release
2. Go to Actions → "Promote to Stable Release"
3. Enter the next development version (e.g., `3.0.0`)
4. The workflow rebases develop onto master (flat history) and bumps develop

### Installing Pre-release Versions

```bash
# Install specific canary/beta from GitHub releases
# Download from: https://github.com/avihut/daft/releases

# Or build from develop branch
git clone -b develop https://github.com/avihut/daft.git
cd daft && cargo build --release
```

## License

This project is licensed under the MIT License - see the LICENSE file for details.

## Acknowledgments

- Built for developers who love Git worktrees
- Inspired by the need for friction-free branch switching
- Designed for modern parallel development workflows

---

**Pro Tip**: This workflow is particularly powerful for:
- Frontend development with multiple feature branches
- Code reviews that require testing different branches
- Hotfix development while feature work continues
- Projects with complex build processes that benefit from isolation

**Project History**: Interested in how daft evolved from shell scripts to Rust? See [docs/HISTORY.md](docs/HISTORY.md).
