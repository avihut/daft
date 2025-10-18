# daft - Git Extensions Toolkit

**daft** is a comprehensive toolkit that extends Git functionality to enhance developer workflows. Starting with powerful worktree management that enables a "one worktree per branch" approach, daft aims to provide a suite of Git extensions that eliminate friction and streamline modern development practices.

**🦀 Built with Rust**: Professional-grade Git extensions with type safety, comprehensive error handling, and excellent performance.

## 🚀 Key Features

### Worktree Extensions (Current Focus)
- **Worktree-centric workflow**: One worktree per branch, organized under a common parent directory
- **Smart repository structure**: Uses `<repo-name>/.git` at root with worktrees at `<repo-name>/<branch-name>/`
- **Automatic branch detection**: Dynamically detects default branches (main, master, develop, etc.)
- **direnv integration**: Automatically runs `direnv allow` when entering new worktrees
- **Comprehensive error handling**: Robust cleanup of partial operations on failure
- **Works from anywhere**: Execute commands from any directory within the repository

### Future Extensions
daft is evolving beyond worktree management to provide additional Git workflow enhancements. Future extensions will focus on streamlining common Git operations and enabling advanced workflows that aren't well-supported by Git's core commands.

## 📦 Installation

### Option 1: Rust Binary (Recommended)

**daft uses a single multicall binary architecture** - one 589KB binary with symlinks for all commands.

#### Automated Installation (Recommended)

```bash
git clone https://github.com/avihut/daft.git
cd daft
./install.sh
```

The installation script will:
1. Build the optimized release binary
2. Create symlinks for all Git commands
3. Add daft to your PATH (via ~/.bashrc or ~/.zshrc)

#### Manual Installation

1. Clone and build:
```bash
git clone https://github.com/avihut/daft.git
cd daft
cargo build --release
```

2. Create symlinks and add to PATH:
```bash
# Create development symlinks (from project root)
make dev

# Add to your PATH
export PATH="/path/to/daft/target/release:$PATH"

# Or install to system location
sudo cp target/release/daft /usr/local/bin/
sudo ln -s /usr/local/bin/daft /usr/local/bin/git-worktree-clone
sudo ln -s /usr/local/bin/daft /usr/local/bin/git-worktree-checkout
sudo ln -s /usr/local/bin/daft /usr/local/bin/git-worktree-checkout-branch
sudo ln -s /usr/local/bin/daft /usr/local/bin/git-worktree-checkout-branch-from-default
sudo ln -s /usr/local/bin/daft /usr/local/bin/git-worktree-init
sudo ln -s /usr/local/bin/daft /usr/local/bin/git-worktree-prune
sudo ln -s /usr/local/bin/daft /usr/local/bin/git-daft
```

### Option 2: Shell Scripts (Legacy - Deprecated)

⚠️ **DEPRECATED**: The shell scripts are deprecated. Please use the Rust implementation.

The humble origins of this project.

1. Clone this repository (same as above)

2. Add the legacy scripts to your PATH:
```bash
# Add to your ~/.bashrc, ~/.zshrc, or similar
export PATH="/path/to/daft/src/legacy:$PATH"

# Or create symlinks to a directory already in your PATH
ln -s /path/to/daft/src/legacy/* /usr/local/bin/
```

### Verify Installation

```bash
git worktree-clone --help
```

## 🛠️ Commands

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

## 🔄 Workflow Examples

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

## 🧪 Testing

This project includes comprehensive test coverage with dual test suites for both legacy shell scripts and modern Rust implementations:

### Test Structure
```
tests/
├── legacy/              # Legacy shell script tests (37+ scenarios)
├── integration/         # Rust integration tests (80+ scenarios)
└── README.md           # Detailed testing documentation
```

### Quick Testing
```bash
# Run all tests (legacy + integration)
make test

# Run only legacy shell script tests
make test-legacy

# Run only Rust integration tests
make test-integration

# Run specific test suites
make test-legacy-init
make test-integration-clone
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

# Run tests with different shells
make test-bash
make test-zsh

# Run individual test files
cd tests/legacy && ./test_clone.sh
cd tests/integration && ./test_init.sh
```

### Test Coverage
The comprehensive test framework includes:
- **120+ test scenarios** across both implementations
- **Dual validation**: Legacy behavior + enhanced Rust features
- **Isolated test environments** with temporary directories
- **Mock remote repositories** for realistic testing
- **Security testing**: Path traversal prevention
- **Performance validation**: Timing and resource usage
- **Cross-platform compatibility** (Ubuntu, macOS)
- **Error handling** and edge case validation
- **CI/CD integration** with GitHub Actions

See `tests/README.md` for detailed testing documentation.

## 🏗️ Architecture

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
├── src/legacy/              # Legacy shell scripts (1,194 lines) - DEPRECATED
│   ├── git-worktree-clone              # 380 lines
│   ├── git-worktree-checkout           # 153 lines
│   ├── git-worktree-checkout-branch    # 165 lines
│   ├── git-worktree-checkout-branch-from-default  # 90 lines
│   ├── git-worktree-init               # 256 lines
│   ├── git-worktree-prune              # 150 lines
│   └── README.md                       # Deprecation notice
├── tests/                   # Comprehensive test suite
│   ├── legacy/              # Legacy shell script tests
│   ├── integration/         # Rust integration tests
│   └── README.md            # Testing documentation
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

- **Robust error handling**: All scripts include comprehensive error checking and cleanup
- **Path independence**: Commands work from any directory within the repository
- **Consistent behavior**: All scripts follow the same patterns and conventions
- **Optional integrations**: Features like direnv work when available but don't break when absent
- **Atomic operations**: Failed operations are cleaned up automatically

## 🔧 Requirements

### For Rust Binaries
- **Git**: Version 2.5+ (for worktree support)
- **Rust**: Version 1.70+ (for building from source)
- **direnv** (optional): For automatic environment setup

### For Shell Scripts (Legacy)
- **Git**: Version 2.5+ (for worktree support)
- **Bash**: Version 4.0+ 
- **Standard Unix tools**: `awk`, `basename`, `dirname`, `sed`, `cut`
- **direnv** (optional): For automatic environment setup

## 🦀 Rust Implementation Benefits

The Rust implementation provides significant advantages over the shell scripts:

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

## 🤝 Contributing

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
make test                          # Full test suite (147 tests)

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

- **Focus on Rust**: The shell scripts (`src/legacy/`) are deprecated
- **Add tests**: Include unit tests for new functionality
- **Run quality checks**: `cargo clippy` and `cargo fmt` before committing
- **Update docs**: Keep README.md and inline documentation current

## 📝 License

This project is licensed under the MIT License - see the LICENSE file for details.

## 🙏 Acknowledgments

- Built for developers who love Git worktrees
- Inspired by the need for friction-free branch switching
- Designed for modern parallel development workflows

---

**Pro Tip**: This workflow is particularly powerful for:
- Frontend development with multiple feature branches
- Code reviews that require testing different branches
- Hotfix development while feature work continues
- Projects with complex build processes that benefit from isolation
