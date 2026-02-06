---
title: Multi-Remote Mode
description: Organize worktrees by remote for fork-based workflows
---

# Multi-Remote Mode

When working with multiple remotes (e.g., a fork workflow with `origin` and `upstream`), multi-remote mode organizes worktrees into remote-prefixed directories.

## Directory Layouts

### Standard Layout (default)

```
my-project/
├── .git/
├── main/
├── feature/auth/
└── bugfix/login/
```

### Multi-Remote Layout

```
my-project/
├── .git/
├── origin/
│   ├── main/
│   ├── feature/auth/
│   └── bugfix/login/
└── upstream/
    └── main/
```

## Enabling Multi-Remote Mode

```bash
# Enable for the current repository
git daft multi-remote enable

# Check current status
git daft multi-remote status

# Set the default remote
git daft multi-remote set-default origin
```

Enabling migrates existing worktrees into the remote-prefixed layout.

## Disabling Multi-Remote Mode

```bash
git daft multi-remote disable
```

This flattens the directory structure back to the standard layout.

## Using --remote on Commands

When multi-remote mode is enabled, you can specify which remote to organize under:

```bash
# Create worktree under origin/
git worktree-checkout-branch feature/auth --remote origin

# Create worktree under upstream/
git worktree-checkout upstream-branch --remote upstream
```

You can also enable multi-remote mode during clone:

```bash
git worktree-clone git@github.com:user/project.git --remote origin
```

## Configuration

| Key | Default | Description |
|-----|---------|-------------|
| `daft.multiRemote.enabled` | `false` | Enable multi-remote mode |
| `daft.multiRemote.defaultRemote` | `origin` | Default remote for new branches |
