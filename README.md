# daft - Git Extensions Toolkit

**daft** is a comprehensive toolkit that extends Git functionality to enhance developer workflows. Starting with powerful worktree management that enables a "one worktree per branch" approach, daft aims to provide a suite of Git extensions that eliminate friction and streamline modern development practices.

**Built with Rust**: Professional-grade Git extensions with type safety, comprehensive error handling, and excellent performance.

## Key Features

### Worktree Extensions (Current Focus)
- **Worktree-centric workflow**: One worktree per branch, organized under a common parent directory
- **Smart repository structure**: Uses `<repo-name>/.git` at root with worktrees at `<repo-name>/<branch-name>/`
- **Automatic branch detection**: Dynamically detects default branches (main, master, develop, etc.)
- **direnv integration**: Automatically runs `direnv allow` when entering new worktrees
- **Comprehensive error handling**: Robust cleanup of partial operations on failure
- **Works from anywhere**: Execute commands from any directory within the repository

### Future Extensions
daft is evolving beyond worktree management to provide additional Git workflow enhancements. Future extensions will focus on streamlining common Git operations and enabling advanced workflows that aren't well-supported by Git's core commands.

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
make install-completions

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
make gen-completions-bash

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
make test

# Run only Rust unit tests
make test-unit

# Run only integration tests
make test-integration

# Run specific test suites
make test-integration-clone
make test-integration-checkout
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
make test-verbose

# Run performance tests
make test-perf

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
│   ├── direnv.rs            # Direnv integration
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
- **Optional integrations**: Features like direnv work when available but don't break when absent
- **Atomic operations**: Failed operations are cleaned up automatically

## Requirements

- **Git**: Version 2.5+ (for worktree support)
- **Rust**: Version 1.70+ (for building from source)
- **direnv** (optional): For automatic environment setup

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
4. Run the test suite: `cargo test && make test`
5. Submit a pull request

### Local Development Setup

**Quick Start:**
```bash
# Build binary, create symlinks, and verify
make dev

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
make dev                           # Quick: build + verify
./target/release/git-worktree-clone --help

# 3. Run tests
cargo test --lib                   # Unit tests (fast)
make test                          # Full test suite

# 4. Quality checks
cargo clippy -- -D warnings        # Linting
cargo fmt                          # Formatting
```

**Useful Make Targets:**
- `make dev` - Build binary + create symlinks + verify (recommended)
- `make dev-test` - Full setup + run all tests
- `make dev-clean` - Remove symlinks (keeps binary)
- `make help` - Show all available targets

**Architecture Note:**
daft uses a single binary (589KB) with symlinks for all commands. When you run `make dev`, it creates symlinks in `target/release/` that point to the main `daft` binary. The binary detects how it was invoked (via argv[0]) and routes to the appropriate command.

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
