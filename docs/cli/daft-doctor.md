---
title: daft-doctor
description: Diagnose daft installation and configuration issues
---

# daft doctor

Diagnose daft installation and configuration issues

## Description

Diagnose daft installation and configuration issues.

Runs health checks on your daft installation, repository setup,
and hooks configuration. Reports issues with actionable suggestions.

When run outside a git repository, only installation checks are performed.
Inside a daft-managed repository, repository and hooks checks run too.

The --fix flag auto-repairs: missing command symlinks, missing shortcut
symlinks for partially-installed styles, orphaned worktree entries,
incorrect fetch refspecs, missing remote HEAD, non-executable hooks,
and deprecated hook names. Issues requiring manual intervention (binary
not in PATH, git not installed, shell integration) show suggestions only.

Use --fix --dry-run to preview planned actions with pre-flight validation.
Each action shows whether it would succeed or fail (e.g., directory not
writable, conflicting files). Actions marked + would succeed; actions
marked x would fail, with the reason shown below.

## Usage

```
daft doctor [OPTIONS]
```

## Options

| Option | Description | Default |
|--------|-------------|----------|
| `-v, --verbose` | Show detailed output for each check |  |
| `--fix` | Auto-fix issues that can be resolved automatically |  |
| `--dry-run` | Preview fixes without applying them (use with --fix) |  |
| `-q, --quiet` | Only show warnings and errors |  |

## Global Options

| Option | Description |
|--------|-------------|
| `-h`, `--help` | Print help information |
| `-V`, `--version` | Print version information |

## See Also

- [git-worktree-clone](./git-worktree-clone.md)
- [git-worktree-init](./git-worktree-init.md)

