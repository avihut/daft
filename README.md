# daft - Git Extensions Toolkit

[![Docs](https://img.shields.io/badge/docs-daft.avihu.dev-blue)](https://daft.avihu.dev)
[![CI](https://github.com/avihut/daft/actions/workflows/test.yml/badge.svg)](https://github.com/avihut/daft/actions/workflows/test.yml)
[![Release](https://img.shields.io/github/v/release/avihut/daft?label=release)](https://github.com/avihut/daft/releases/latest)
[![License: MIT](https://img.shields.io/badge/License-MIT-blue.svg)](https://opensource.org/licenses/MIT)
[![Rust](https://img.shields.io/badge/rust-1.70%2B-orange.svg)](https://www.rust-lang.org/)
[![Homebrew](https://img.shields.io/badge/homebrew-avihut%2Ftap-blueviolet)](https://github.com/avihut/homebrew-tap)
[![macOS](https://img.shields.io/badge/macOS-supported-success)](https://github.com/avihut/daft/releases)
[![Linux](https://img.shields.io/badge/Linux-supported-success)](https://github.com/avihut/daft/releases)
[![Windows](<https://img.shields.io/badge/Windows-supported%20(WSL)-success>)](https://github.com/avihut/daft/releases)

> Stop switching branches. Work on multiple branches simultaneously.

![daft demo](https://github.com/user-attachments/assets/0ea922d5-6f01-4cdb-9b15-18d8a6112499)

**daft** gives each Git branch its own directory. No more stashing, no more
context switching, no more waiting for builds to restart.

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
brew install avihut/tap/daft

# Clone a repo (creates my-project/main/)
git worktree-clone git@github.com:user/my-project.git

# Start a feature branch (creates my-project/feature/auth/)
git worktree-checkout-branch feature/auth
```

Each directory is a full working copy. Run different branches in different
terminals. Your IDE state, node_modules, build artifacts - all isolated per
branch.

## Adopt Existing Repositories

Already have a repository? Convert it to the worktree workflow with one command:

```bash
cd my-existing-project
git worktree-flow-adopt
```

This restructures your repo:

```
my-project/                 my-project/
├── .git/                   ├── .git/        (bare repository)
├── src/           →        └── main/        (worktree)
└── README.md                   ├── src/
                                └── README.md
```

Your uncommitted changes are preserved. If you change your mind:

```bash
git worktree-flow-eject      # Converts back to traditional layout
```

## Why daft?

**Traditional Git workflow:**

```
┌─────────────────────────────────────────────────────────┐
│  $ git stash                                            │
│  $ git checkout feature-b                               │
│  $ npm install        # wait...                         │
│  $ npm run build      # wait...                         │
│  # context lost, IDE state gone                         │
│  $ git checkout feature-a                               │
│  $ git stash pop                                        │
│  # where was I?                                         │
└─────────────────────────────────────────────────────────┘
```

**With daft:**

```
Terminal 1 (feature-a/)     Terminal 2 (feature-b/)
┌───────────────────────┐   ┌───────────────────────┐
│ $ npm run dev         │   │ $ npm run dev         │
│ Server on :3000       │   │ Server on :3001       │
│ # full context        │   │ # full context        │
└───────────────────────┘   └───────────────────────┘
         ↓                           ↓
    Both running simultaneously, isolated environments
```

## Installation

### macOS (Homebrew)

```bash
brew install avihut/tap/daft
```

### Windows

```powershell
irm https://github.com/avihut/daft/releases/latest/download/daft-installer.ps1 | iex
```

### Linux / From Source

Download binaries from
[GitHub Releases](https://github.com/avihut/daft/releases/latest) or build from
source:

```bash
git clone https://github.com/avihut/daft.git
cd daft && cargo build --release
```

See [CONTRIBUTING.md](CONTRIBUTING.md) for detailed installation and symlink
setup.

### Shell Integration

Enable automatic cd into new worktrees:

```bash
# Add to ~/.bashrc or ~/.zshrc
eval "$(daft shell-init bash)"

# Or for fish (~/.config/fish/config.fish)
daft shell-init fish | source
```

## Commands

| Command                                      | Description                                     |
| -------------------------------------------- | ----------------------------------------------- |
| `git worktree-clone <url>`                   | Clone a repo into the worktree structure        |
| `git worktree-init <name>`                   | Initialize a new repo in the worktree structure |
| `git worktree-checkout <branch>`             | Create worktree from existing branch            |
| `git worktree-checkout-branch <branch>`      | Create new branch + worktree                    |
| `git worktree-checkout-branch <branch> main` | Create branch from a specific base              |
| `git worktree-prune`                         | Remove worktrees for deleted remote branches    |
| `git worktree-carry`                         | Carry uncommitted changes to other worktrees    |
| `git worktree-flow-adopt`                    | Convert traditional repo to worktree layout     |
| `git worktree-flow-eject`                    | Convert back to traditional layout              |

Run any command with `--help` for full options.

### Shortcuts

Enable short aliases like `gwtco`, `gwtcb`:

```bash
daft setup shortcuts enable git    # gwtco, gwtcb, gwtprune, etc.
daft setup shortcuts list          # See all available shortcuts
```

## Hooks

Automate worktree lifecycle events with a `daft.yml` configuration file:

| Hook                   | Trigger                 |
| ---------------------- | ----------------------- |
| `post-clone`           | After repository clone  |
| `worktree-post-create` | After worktree created  |
| `worktree-pre-remove`  | Before worktree removal |

**Example** - install dependencies and auto-allow direnv in new worktrees:

```yaml
# daft.yml
hooks:
  worktree-post-create:
    jobs:
      - name: install-deps
        run: npm install
      - name: direnv-allow
        run: direnv allow .
```

```bash
git daft hooks trust
```

Hooks require explicit trust for security. See the
[hooks guide](https://avihu.dev/daft/guide/hooks) for details.

## AI Agent Skill

daft ships an [Agent Skill](https://github.com/anthropics/agent-skills) that
teaches AI coding agents (Claude Code, Cursor, Windsurf, and others) the
worktree workflow -- commands, hooks, environment tooling, and worktree-aware
Git operations.

```bash
npx skills add avihut/daft
```

See the [Agent Skill guide](https://avihu.dev/daft/guide/claude-skill) for
manual installation options.

## Requirements

- **Git** 2.5+ (for worktree support)
- **Rust** 1.70+ (only for building from source)

## Contributing

See [CONTRIBUTING.md](CONTRIBUTING.md) for development setup and guidelines.

## License

MIT License - see [LICENSE](LICENSE) for details.

---

**Pro Tip**: This workflow is powerful for frontend development, code reviews,
hotfixes, and any project with complex build processes that benefit from
isolation.
