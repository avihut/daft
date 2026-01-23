# Daft Hooks System - Implementation Plan

## Overview

A flexible, project-managed hooks system for worktree lifecycle events, replacing the hardcoded direnv integration with a secure, configurable solution.

---

## 1. Hook Types

| Hook | Trigger | Source of Hook File | Use Cases |
|------|---------|---------------------|-----------|
| `post-clone` | After `git worktree-clone` completes | New default branch worktree | Initial project setup, dependency install |
| `post-init` | After `git worktree-init` completes | New initial worktree | Initialize new project |
| `pre-create` | Before `git worktree add` | Source worktree (where command runs) | Validate environment, check resources |
| `post-create` | After worktree created | New worktree | direnv allow, npm install, docker up |
| `pre-remove` | Before `git worktree remove` | Worktree being removed | docker down, cleanup, backup |
| `post-remove` | After worktree removed | Current worktree (where prune runs) | Notifications, logging |

---

## 2. Directory Structure

### Project Hooks (Tracked in Git)

```
<worktree>/
├── .daft/
│   └── hooks/
│       ├── post-clone      # Initial clone setup
│       ├── post-init       # New repo initialization
│       ├── pre-create      # Before worktree creation
│       ├── post-create     # After worktree creation (replaces direnv)
│       ├── pre-remove      # Before worktree removal
│       └── post-remove     # After worktree removal
├── .github/
│   └── workflows/          # (Similar pattern to GitHub Actions)
└── src/
```

### User-Global Hooks (Personal Automation)

```
~/.config/daft/
├── hooks/
│   ├── post-create         # Runs AFTER project hooks
│   └── pre-remove          # Runs AFTER project hooks
└── trust.json              # Repository trust database
```

---

## 3. Environment Variables

All hooks receive these environment variables:

### Universal Variables

| Variable | Description | Example |
|----------|-------------|---------|
| `DAFT_HOOK` | Current hook type | `post-create` |
| `DAFT_COMMAND` | Triggering command | `checkout-branch` |
| `DAFT_PROJECT_ROOT` | Repository root (parent of .git) | `/Users/avihu/Projects/daft` |
| `DAFT_GIT_DIR` | Path to .git directory | `/Users/avihu/Projects/daft/.git` |
| `DAFT_REMOTE` | Remote name | `origin` |
| `DAFT_SOURCE_WORKTREE` | Worktree where command was invoked | `/project/main` |

### Creation Hooks (`post-clone`, `post-init`, `pre-create`, `post-create`)

| Variable | Description | Example |
|----------|-------------|---------|
| `DAFT_WORKTREE_PATH` | Path to new/target worktree | `/project/feature/hooks` |
| `DAFT_BRANCH_NAME` | Branch name | `feature/hooks` |
| `DAFT_IS_NEW_BRANCH` | Whether branch is newly created | `true` or `false` |
| `DAFT_BASE_BRANCH` | Base branch (if applicable) | `main` |

### Clone-Specific (`post-clone`)

| Variable | Description | Example |
|----------|-------------|---------|
| `DAFT_REPOSITORY_URL` | Cloned repository URL | `git@github.com:user/repo.git` |
| `DAFT_DEFAULT_BRANCH` | Detected default branch | `main` |

### Removal Hooks (`pre-remove`, `post-remove`)

| Variable | Description | Example |
|----------|-------------|---------|
| `DAFT_WORKTREE_PATH` | Path to worktree being removed | `/project/feature/old` |
| `DAFT_BRANCH_NAME` | Branch being removed | `feature/old` |
| `DAFT_REMOVAL_REASON` | Why worktree is being removed | `remote-deleted` or `manual` |

---

## 4. Security & Permission Model

### Trust Levels

| Level | Behavior | Use Case |
|-------|----------|----------|
| `deny` | Never run hooks, no prompts | Untrusted/unknown repos |
| `prompt` | Ask before each hook execution | Semi-trusted repos |
| `allow` | Run all hooks without prompting | Fully trusted repos |

### Default Behavior

- **Unknown repositories**: `deny` (hooks silently skipped with warning)
- **User must explicitly grant trust** before hooks will run
- **Trust is per-repository**, stored in user config (not in repo)

### Trust Storage

**File: `~/.config/daft/trust.json`**

```json
{
  "version": 1,
  "default_level": "deny",
  "repositories": {
    "/Users/avihu/Projects/daft/.git": {
      "level": "allow",
      "granted_at": "2024-01-15T10:30:00Z",
      "granted_by": "user"
    },
    "/Users/avihu/Projects/work-project/.git": {
      "level": "prompt",
      "granted_at": "2024-01-20T14:00:00Z",
      "granted_by": "user"
    }
  },
  "patterns": [
    {
      "pattern": "/Users/avihu/Projects/trusted-org/*/.git",
      "level": "allow",
      "comment": "All repos in trusted-org directory"
    }
  ]
}
```

### Git Config Integration

```bash
# Global default (in ~/.gitconfig)
daft.hooks.defaultTrust = deny    # deny | prompt | allow

# Per-repository override (in repo's .git/config)
# Note: This is LOCAL config, not tracked - set by user
daft.hooks.trust = allow
```

### Trust Management Commands

New subcommand: `git-daft hooks` (or `daft hooks`)

```bash
# Trust current repository
git daft hooks trust [--level=allow|prompt]

# Revoke trust for current repository
git daft hooks untrust

# Show trust status
git daft hooks status

# List all trusted repositories
git daft hooks list

# Trust by pattern (for organizations/directories)
git daft hooks trust-pattern "/path/to/org/*" --level=allow
```

### Clone Behavior

For `git worktree-clone`, hooks present special security considerations since the repo is brand new:

1. **Default**: `post-clone` hook is NOT run automatically
2. **Prompt**: Show message about hooks and ask user
3. **Flag**: `--trust-hooks` to explicitly run hooks on clone
4. **Flag**: `--no-hooks` to explicitly skip (and not prompt)

```bash
# Clone without hooks (default, safe)
git worktree-clone https://github.com/user/repo.git

# Clone and trust hooks
git worktree-clone https://github.com/user/repo.git --trust-hooks

# Clone and explicitly skip hooks (no prompt)
git worktree-clone https://github.com/user/repo.git --no-hooks
```

Output when hooks are detected but not trusted:
```
Cloning into 'repo'...
✓ Created worktree at repo/main

⚠ This repository contains hooks in .daft/hooks/:
  - post-clone
  - post-create

Hooks were NOT executed. To enable hooks for this repository:
  cd repo/main && git daft hooks trust
```

---

## 5. Configuration Schema

### Git Config Keys

```bash
# ===== Global Settings =====
daft.hooks.enabled              # bool, default: true (master switch)
daft.hooks.defaultTrust         # deny|prompt|allow, default: deny
daft.hooks.userDirectory        # string, default: ~/.config/daft/hooks

# ===== Per-Hook Settings =====
# Pattern: daft.hooks.<hookName>.<setting>

daft.hooks.postClone.enabled    # bool, default: true
daft.hooks.postClone.failMode   # abort|warn, default: warn

daft.hooks.postInit.enabled     # bool, default: true
daft.hooks.postInit.failMode    # abort|warn, default: warn

daft.hooks.preCreate.enabled    # bool, default: true
daft.hooks.preCreate.failMode   # abort|warn, default: abort

daft.hooks.postCreate.enabled   # bool, default: true
daft.hooks.postCreate.failMode  # abort|warn, default: warn

daft.hooks.preRemove.enabled    # bool, default: true
daft.hooks.preRemove.failMode   # abort|warn, default: warn

daft.hooks.postRemove.enabled   # bool, default: true
daft.hooks.postRemove.failMode  # abort|warn, default: warn

# ===== Timeout =====
daft.hooks.timeout              # int (seconds), default: 300

# ===== Trust (per-repo, local config only) =====
daft.hooks.trust                # deny|prompt|allow (overrides global)
```

### Settings Struct Addition

```rust
// src/settings.rs additions

pub struct HookSettings {
    pub enabled: bool,
    pub fail_mode: FailMode,
}

pub enum FailMode {
    Abort,
    Warn,
}

pub enum TrustLevel {
    Deny,
    Prompt,
    Allow,
}

pub struct HooksConfig {
    pub enabled: bool,
    pub default_trust: TrustLevel,
    pub user_directory: PathBuf,
    pub timeout_seconds: u32,

    pub post_clone: HookSettings,
    pub post_init: HookSettings,
    pub pre_create: HookSettings,
    pub post_create: HookSettings,
    pub pre_remove: HookSettings,
    pub post_remove: HookSettings,
}
```

---

## 6. Execution Flow

### Hook Execution Order

1. Check if hooks are globally enabled (`daft.hooks.enabled`)
2. Check if specific hook is enabled (`daft.hooks.<hookName>.enabled`)
3. Determine trust level for repository
4. If `deny`: Skip with debug message
5. If `prompt`: Ask user for permission
6. If `allow`: Proceed to execution
7. Locate hook file (project hooks, then user hooks)
8. Set environment variables
9. Execute hook with timeout
10. Handle exit code based on `failMode`

### Hook Discovery

```rust
fn find_hook(hook_name: &str, worktree_path: &Path, user_hooks_dir: &Path) -> Vec<PathBuf> {
    let mut hooks = Vec::new();

    // 1. Project hook
    let project_hook = worktree_path.join(".daft/hooks").join(hook_name);
    if project_hook.exists() && is_executable(&project_hook) {
        hooks.push(project_hook);
    }

    // 2. User hook (runs after project hook)
    let user_hook = user_hooks_dir.join(hook_name);
    if user_hook.exists() && is_executable(&user_hook) {
        hooks.push(user_hook);
    }

    hooks
}
```

---

## 7. Implementation Tasks

### Phase 1: Core Infrastructure

- [ ] **Task 1.1**: Create `src/hooks/mod.rs` - Hook types and configuration
- [ ] **Task 1.2**: Create `src/hooks/executor.rs` - Hook execution logic
- [ ] **Task 1.3**: Create `src/hooks/trust.rs` - Trust management and storage
- [ ] **Task 1.4**: Create `src/hooks/environment.rs` - Environment variable builder
- [ ] **Task 1.5**: Update `src/settings.rs` - Add hook configuration loading

### Phase 2: Trust Management Command

- [ ] **Task 2.1**: Create `src/commands/hooks.rs` - `git-daft hooks` subcommand
- [ ] **Task 2.2**: Implement `trust` subcommand
- [ ] **Task 2.3**: Implement `untrust` subcommand
- [ ] **Task 2.4**: Implement `status` subcommand
- [ ] **Task 2.5**: Implement `list` subcommand
- [ ] **Task 2.6**: Add to main.rs routing

### Phase 3: Integration with Existing Commands

- [ ] **Task 3.1**: Update `clone.rs` - Add post-clone hook, --trust-hooks/--no-hooks flags
- [ ] **Task 3.2**: Update `init.rs` - Add post-init hook
- [ ] **Task 3.3**: Update `checkout.rs` - Add pre-create/post-create hooks
- [ ] **Task 3.4**: Update `checkout_branch.rs` - Add pre-create/post-create hooks
- [ ] **Task 3.5**: Update `checkout_branch_from_default.rs` - Add pre-create/post-create hooks
- [ ] **Task 3.6**: Update `prune.rs` - Add pre-remove/post-remove hooks

### Phase 4: Cleanup & Migration

- [ ] **Task 4.1**: Remove `src/direnv.rs`
- [ ] **Task 4.2**: Remove direnv calls from all commands
- [ ] **Task 4.3**: Update README.md with hooks documentation
- [ ] **Task 4.4**: Update CLAUDE.md with hooks information

### Phase 5: Testing

- [ ] **Task 5.1**: Unit tests for hook discovery
- [ ] **Task 5.2**: Unit tests for trust management
- [ ] **Task 5.3**: Unit tests for environment variable building
- [ ] **Task 5.4**: Integration tests for hook execution
- [ ] **Task 5.5**: Integration tests for trust commands
- [ ] **Task 5.6**: Integration tests for clone with hooks

---

## 8. Example Hook Files

### `.daft/hooks/post-clone` (Initial Setup)

```bash
#!/bin/bash
set -e

echo "🔧 Running post-clone setup..."

# Install dependencies
if [ -f "package.json" ]; then
    echo "  Installing npm dependencies..."
    npm install
fi

if [ -f "Cargo.toml" ]; then
    echo "  Building Rust project..."
    cargo build
fi

# Setup environment
if [ -f ".envrc" ] && command -v direnv &>/dev/null; then
    echo "  Allowing direnv..."
    direnv allow .
fi

echo "✓ Post-clone setup complete"
```

### `.daft/hooks/post-create` (Worktree Setup)

```bash
#!/bin/bash
# Allow direnv in new worktrees (replaces built-in direnv)

if [ -f ".envrc" ] && command -v direnv &>/dev/null; then
    direnv allow .
fi

# Start worktree-specific docker services
if [ -f "docker-compose.yml" ]; then
    # Use branch name as project name for isolation
    export COMPOSE_PROJECT_NAME="myapp-${DAFT_BRANCH_NAME//\//-}"
    docker compose up -d
fi
```

### `.daft/hooks/pre-remove` (Worktree Cleanup)

```bash
#!/bin/bash
# Stop docker services before removing worktree

if [ -f "docker-compose.yml" ]; then
    export COMPOSE_PROJECT_NAME="myapp-${DAFT_BRANCH_NAME//\//-}"
    echo "Stopping docker services for $DAFT_BRANCH_NAME..."
    docker compose down -v
fi

# Cleanup any worktree-specific temp files
rm -rf .cache .tmp 2>/dev/null || true
```

---

## 9. User Experience

### First Clone of Untrusted Repo

```
$ git worktree-clone https://github.com/unknown/repo.git

Cloning into 'repo'...
✓ Cloned repository
✓ Created worktree at repo/main

⚠ Hooks detected in .daft/hooks/:
    post-clone, post-create

  Hooks are not trusted and were skipped.
  To review and trust hooks: git daft hooks status
  To trust and run hooks:    git daft hooks trust
```

### Trusting a Repository

```
$ git daft hooks trust

Repository: /Users/avihu/Projects/repo/.git

Hooks found in .daft/hooks/:
  ├── post-clone   (1.2 KB)
  ├── post-create  (0.8 KB)
  └── pre-remove   (0.5 KB)

⚠ Trusting hooks allows this repository to run arbitrary
  scripts during worktree operations.

Trust this repository? [y/N]: y

✓ Repository trusted with level: allow
  Hooks will now run automatically.
```

### Hook Execution Output

```
$ git worktree-checkout-branch feature/new-feature

● Running pre-create hook...
✓ Pre-create hook completed

Creating worktree for 'feature/new-feature'...
✓ Created worktree at /project/feature/new-feature

● Running post-create hook...
  Allowing direnv...
  Starting docker services...
✓ Post-create hook completed

✓ Switched to /project/feature/new-feature
```

---

## 10. File Structure After Implementation

```
src/
├── main.rs
├── lib.rs
├── commands/
│   ├── mod.rs
│   ├── clone.rs          # Updated: post-clone hook
│   ├── init.rs           # Updated: post-init hook
│   ├── checkout.rs       # Updated: pre/post-create hooks
│   ├── checkout_branch.rs
│   ├── checkout_branch_from_default.rs
│   ├── prune.rs          # Updated: pre/post-remove hooks
│   ├── hooks.rs          # NEW: git-daft hooks command
│   └── shell_init.rs
├── hooks/                 # NEW: Hooks module
│   ├── mod.rs            # Hook types, HookRunner
│   ├── executor.rs       # Execution logic
│   ├── trust.rs          # Trust management
│   └── environment.rs    # Env var building
├── config.rs
├── settings.rs           # Updated: HooksConfig
├── git.rs
├── output/
├── remote.rs
├── utils.rs
└── logging.rs

# Removed:
# ├── direnv.rs           # DELETED
```

---

## 11. Migration Path

### For Existing direnv Users

When upgrading, show one-time notice:

```
╭─────────────────────────────────────────────────────────────╮
│ Daft Hooks System                                           │
│                                                             │
│ Built-in direnv integration has been replaced with a        │
│ flexible hooks system.                                      │
│                                                             │
│ To restore direnv behavior, create .daft/hooks/post-create: │
│                                                             │
│   mkdir -p .daft/hooks                                      │
│   cat > .daft/hooks/post-create << 'EOF'                    │
│   #!/bin/bash                                               │
│   [ -f ".envrc" ] && command -v direnv &>/dev/null \        │
│     && direnv allow .                                       │
│   EOF                                                       │
│   chmod +x .daft/hooks/post-create                          │
│   git add .daft/hooks/post-create                           │
│   git daft hooks trust                                      │
│                                                             │
│ Learn more: https://github.com/avihut/daft#hooks            │
╰─────────────────────────────────────────────────────────────╯
```

---

## 12. Future Considerations

1. **Hook Templates**: `git daft hooks init` to create common hook files
2. **`.d/` Directories**: Multiple hooks per event (e.g., `post-create.d/`)
3. **Hook Conditions**: Run only for specific branches/patterns
4. **Dry Run**: `--dry-run` to see what hooks would execute
5. **Hook Signing**: GPG-signed hooks for enterprise security
6. **Remote Hook Repos**: Share hooks across projects (like direnv stdlib)
