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

## Usage

```
daft doctor [OPTIONS]
```

## Options

| Option | Description | Default |
|--------|-------------|----------|
| `-v, --verbose` | Show detailed output for each check |  |
| `--fix` | Auto-fix issues that can be resolved automatically |  |
| `-q, --quiet` | Only show warnings and errors |  |

## Global Options

| Option | Description |
|--------|-------------|
| `-h`, `--help` | Print help information |
| `-V`, `--version` | Print version information |

## See Also

- [git-worktree-clone](./git-worktree-clone.md)
- [git-worktree-init](./git-worktree-init.md)

