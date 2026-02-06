---
title: Hooks
description: Automate worktree lifecycle events with project-managed hooks
---

# Hooks

daft provides a hooks system that runs scripts at worktree lifecycle events. Hooks are stored in the repository and shared with your team, with a trust-based security model.

## Overview

Hooks are executable scripts placed in `.daft/hooks/` within your repository. They run automatically when worktrees are created, removed, or cloned.

```
my-project/
├── .daft/
│   └── hooks/
│       ├── post-clone            # Runs after cloning the repo
│       ├── worktree-post-create  # Runs after creating a worktree
│       └── worktree-pre-remove   # Runs before removing a worktree
└── src/
```

## Hook Types

| Hook | Trigger | Runs From |
|------|---------|-----------|
| `post-clone` | After `git worktree-clone` completes | New default branch worktree |
| `post-init` | After `git worktree-init` completes | New initial worktree |
| `worktree-pre-create` | Before new worktree is added | Source worktree (where command runs) |
| `worktree-post-create` | After new worktree is created | New worktree |
| `worktree-pre-remove` | Before worktree is removed | Worktree being removed |
| `worktree-post-remove` | After worktree is removed | Current worktree (where prune runs) |

## Trust Model

For security, hooks from untrusted repositories don't run automatically. Trust is managed per-repository.

### Trust Levels

| Level | Behavior |
|-------|----------|
| `deny` (default) | Hooks are never executed |
| `prompt` | User is prompted before each hook execution |
| `allow` | Hooks run without prompting |

### Managing Trust

```bash
# Trust the current repository
git daft hooks trust

# Prompt before running hooks
git daft hooks prompt

# Revoke trust
git daft hooks deny

# Check current status
git daft hooks status

# List all trusted repositories
git daft hooks list

# Clear all trust settings
git daft hooks reset-trust
```

## Writing a Hook

Hooks are executable scripts. They can be written in any language.

### Example: Auto-allow direnv

```bash
#!/bin/bash
# .daft/hooks/worktree-post-create
if [ -f ".envrc" ] && command -v direnv &>/dev/null; then
    direnv allow .
fi
```

### Example: Install dependencies

```bash
#!/bin/bash
# .daft/hooks/worktree-post-create
if [ -f "package.json" ]; then
    npm install
elif [ -f "Gemfile" ]; then
    bundle install
elif [ -f "requirements.txt" ]; then
    pip install -r requirements.txt
fi
```

### Example: Use correct Node version

```bash
#!/bin/bash
# .daft/hooks/worktree-post-create
if [ -f ".nvmrc" ] && command -v nvm &>/dev/null; then
    nvm use
fi
```

Make hooks executable:

```bash
chmod +x .daft/hooks/worktree-post-create
```

## Environment Variables

Hooks receive context via environment variables:

### Universal (all hooks)

| Variable | Description |
|----------|-------------|
| `DAFT_HOOK` | Hook type (e.g., `worktree-post-create`) |
| `DAFT_COMMAND` | Command that triggered the hook (e.g., `checkout-branch`) |
| `DAFT_PROJECT_ROOT` | Repository root (parent of `.git` directory) |
| `DAFT_GIT_DIR` | Path to the `.git` directory |
| `DAFT_REMOTE` | Remote name (usually `origin`) |
| `DAFT_SOURCE_WORKTREE` | Worktree where the command was invoked |

### Worktree (creation and removal hooks)

| Variable | Description |
|----------|-------------|
| `DAFT_WORKTREE_PATH` | Path to the target worktree |
| `DAFT_BRANCH_NAME` | Branch name for the target worktree |

### Creation (create hooks only)

| Variable | Description |
|----------|-------------|
| `DAFT_IS_NEW_BRANCH` | `true` if the branch was newly created, `false` otherwise |
| `DAFT_BASE_BRANCH` | Base branch (for `checkout-branch` commands) |

### Clone (post-clone only)

| Variable | Description |
|----------|-------------|
| `DAFT_REPOSITORY_URL` | The cloned repository URL |
| `DAFT_DEFAULT_BRANCH` | The remote's default branch |

### Removal (remove hooks only)

| Variable | Description |
|----------|-------------|
| `DAFT_REMOVAL_REASON` | Why the worktree is being removed: `remote-deleted`, `manual`, or `ejecting` |

## Fail Modes

Each hook type has a default fail mode that determines what happens when a hook exits with a non-zero status:

| Hook | Default Fail Mode | Behavior |
|------|--------------------|----------|
| `worktree-pre-create` | `abort` | Operation is cancelled |
| All others | `warn` | Warning is shown, operation continues |

Override per-hook:

```bash
# Make post-create hooks abort on failure
git config daft.hooks.worktreePostCreate.failMode abort

# Make pre-create hooks just warn
git config daft.hooks.worktreePreCreate.failMode warn
```

## User-Global Hooks

Place hooks in `~/.config/daft/hooks/` to run them for all repositories. Global hooks run after project hooks.

Customize the directory:

```bash
git config --global daft.hooks.userDirectory ~/my-daft-hooks
```

## Configuration

| Key | Default | Description |
|-----|---------|-------------|
| `daft.hooks.enabled` | `true` | Master switch for all hooks |
| `daft.hooks.defaultTrust` | `deny` | Default trust level for unknown repos |
| `daft.hooks.userDirectory` | `~/.config/daft/hooks/` | Path to user-global hooks |
| `daft.hooks.timeout` | `300` | Hook execution timeout in seconds |
| `daft.hooks.<hookName>.enabled` | `true` | Enable/disable a specific hook type |
| `daft.hooks.<hookName>.failMode` | varies | `abort` or `warn` on hook failure |

Hook name config keys use camelCase: `postClone`, `postInit`, `worktreePreCreate`, `worktreePostCreate`, `worktreePreRemove`, `worktreePostRemove`.

## Migration from Deprecated Names

In earlier versions, worktree hooks used shorter names (`pre-create`, `post-create`, `pre-remove`, `post-remove`). These were renamed with a `worktree-` prefix for clarity.

Old names still work with deprecation warnings until v2.0.0. To migrate:

```bash
git daft hooks migrate
```

This renames hook files in the current worktree from old names to new names.
