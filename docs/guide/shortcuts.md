---
title: Shortcuts
description: Short command aliases for frequently used daft commands
---

# Shortcuts

daft supports short aliases for frequently used commands. Instead of typing
`git worktree-checkout`, you can type `gwtco`.

## Shortcut Styles

Three styles are available. Enable the one that fits your preference.

### Git Style (default)

Prefix: `gwt` (Git Worktree)

| Shortcut   | Full Command                 |
| ---------- | ---------------------------- |
| `gwtclone` | `git-worktree-clone`         |
| `gwtinit`  | `git-worktree-init`          |
| `gwtco`    | `git-worktree-checkout`      |
| `gwtcb`    | `git-worktree-checkout -b`   |
| `gwtbd`    | `git-worktree-branch-delete` |
| `gwtprune` | `git-worktree-prune`         |
| `gwtcarry` | `git-worktree-carry`         |
| `gwtfetch` | `git-worktree-fetch`         |

### Shell Style

Prefix: `gw` (shorter, shell-friendly)

| Shortcut | Full Command               |
| -------- | -------------------------- |
| `gwco`   | `git-worktree-checkout`    |
| `gwcob`  | `git-worktree-checkout -b` |

### Legacy Style

From earlier versions of daft:

| Shortcut | Full Command               |
| -------- | -------------------------- |
| `gclone` | `git-worktree-clone`       |
| `gcw`    | `git-worktree-checkout`    |
| `gcbw`   | `git-worktree-checkout -b` |
| `gprune` | `git-worktree-prune`       |

### Default-Branch Shortcuts (shell-init only)

These shortcuts resolve the remote's default branch dynamically. They require
shell integration (`daft shell-init`) and are not available as symlinks.

| Shortcut | Style  | Description                                                    |
| -------- | ------ | -------------------------------------------------------------- |
| `gwtcm`  | Git    | Check out the default branch                                   |
| `gwtcbm` | Git    | Create branch from default branch (`git-worktree-checkout -b`) |
| `gwcobd` | Shell  | Create branch from default branch (`git-worktree-checkout -b`) |
| `gcbdw`  | Legacy | Create branch from default branch (`git-worktree-checkout -b`) |

## Managing Shortcuts

```bash
# List all available styles and mappings
daft setup shortcuts list

# Show which shortcuts are currently installed
daft setup shortcuts status

# Enable a style (creates symlinks)
daft setup shortcuts enable git

# Disable a style (removes symlinks)
daft setup shortcuts disable legacy

# Enable only one style (disable all others)
daft setup shortcuts only shell

# Preview changes without applying
daft setup shortcuts only git --dry-run
```

## How They Work

Shortcuts are implemented as symlinks that point to the `daft` binary. When the
binary starts, it inspects `argv[0]` (how it was invoked) and maps the shortcut
name to the corresponding full command.

For example, `gwtco` is a symlink to `daft`. When invoked, the binary sees
`argv[0] = "gwtco"`, resolves it to `git-worktree-checkout`, and runs that
command.

## Shell Integration Aliases

Alternatively, `daft shell-init` can generate shell aliases with the `--aliases`
flag:

```bash
eval "$(daft shell-init bash --aliases)"
```

This creates shell functions (not symlinks) for the shell-style shortcuts
(`gwco`, `gwcob`) with proper cd behavior built in. The `gwcob` alias maps to
`git-worktree-checkout -b`. Default-branch shortcuts (`gwtcm`, `gwtcbm`,
`gwcobd`, `gcbdw`) are always included in `daft shell-init` output regardless of
the `--aliases` flag.
