# Git Worktree Workflow

A comprehensive toolkit of shell scripts that streamlines development workflows using Git worktrees. This project enables a "one worktree per branch" approach, eliminating the friction of traditional Git branch switching.

## 🚀 Key Features

- **Worktree-centric workflow**: One worktree per branch, organized under a common parent directory
- **Smart repository structure**: Uses `<repo-name>/.git` at root with worktrees at `<repo-name>/<branch-name>/`
- **Automatic branch detection**: Dynamically detects default branches (main, master, develop, etc.)
- **direnv integration**: Automatically runs `direnv allow` when entering new worktrees
- **Comprehensive error handling**: Robust cleanup of partial operations on failure
- **Works from anywhere**: Execute commands from any directory within the repository

## 📦 Installation

1. Clone this repository:
```bash
git clone https://github.com/user/git-worktree-workflow.git
cd git-worktree-workflow
```

2. Add the scripts to your PATH:
```bash
# Add to your ~/.bashrc, ~/.zshrc, or similar
export PATH="/path/to/git-worktree-workflow/scripts:$PATH"

# Or create symlinks to a directory already in your PATH
ln -s /path/to/git-worktree-workflow/scripts/* /usr/local/bin/
```

3. Verify installation:
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

This project includes a comprehensive test suite:

```bash
# Run all tests
make test

# Run specific test suites
make test-simple
make test-clone
make test-checkout
make test-init
make test-prune

# Run individual test files
bash tests/test_clone.sh
```

The test framework includes:
- **37+ test scenarios** covering all commands and edge cases
- **Isolated test environments** with temporary directories
- **Mock remote repositories** for realistic testing
- **Comprehensive assertions** for directory structure and Git state
- **Cross-platform compatibility** testing
- **Error handling** validation

## 🏗️ Architecture

### Directory Structure
```
git-worktree-workflow/
├── scripts/                 # Core shell scripts (1,194 lines)
│   ├── git-worktree-clone              # 380 lines
│   ├── git-worktree-checkout           # 153 lines
│   ├── git-worktree-checkout-branch    # 165 lines
│   ├── git-worktree-checkout-branch-from-default  # 90 lines
│   ├── git-worktree-init               # 256 lines
│   └── git-worktree-prune              # 150 lines
├── tests/                   # Comprehensive test suite
│   ├── test_framework.sh    # Test infrastructure
│   ├── test_*.sh           # Individual test files
│   └── Makefile            # Test automation
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

- **Git**: Version 2.5+ (for worktree support)
- **Bash**: Version 4.0+ 
- **Standard Unix tools**: `awk`, `basename`, `dirname`, `sed`, `cut`
- **direnv** (optional): For automatic environment setup

## 🚧 Future Development

This project is considering migration to Rust for enhanced features:

### Planned Enhancements
- **Advanced CLI**: Better argument parsing with `clap`
- **Shell completions**: Dynamic completion generation
- **Interactive features**: Branch selection menus
- **Hook system**: Custom workflow automation
- **Performance optimizations**: Faster operations on large repositories

### Migration Benefits
- Type safety and better error handling
- Single binary distribution
- Professional CLI experience
- Better cross-platform support
- Enhanced testing capabilities

## 🤝 Contributing

1. Fork the repository
2. Create a feature branch: `git worktree-checkout-branch feature/my-feature`
3. Make your changes and add tests
4. Run the test suite: `make test`
5. Submit a pull request

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