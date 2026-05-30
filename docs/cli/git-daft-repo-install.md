---
title: git-daft-repo-install
description: Install a starter daft.yml in the current worktree
---

# git daft-repo-install

Install a starter daft.yml in the current worktree

## Description

Creates a starter daft.yml at the current worktree root with a commented
skeleton covering the major sections (hooks, shared, layout). Modeled on
`lefthook install`.

This is the canonical name for the bootstrap; `daft install` is a top-level
alias that runs the same thing (so lefthook-style discovery keeps working).

If daft.yml already exists, the command refuses without modifying anything;
edit the existing file with your editor or a future `daft config` TUI.

After writing daft.yml, daft checks whether git already ignores it. If not, it
offers to add `/daft.yml` to .git/info/exclude — a local, per-clone exclude
that is never committed, so a visitor config stays invisible to teammates. On a
terminal it prompts (default No); --git-exclude adds it without prompting; a
non-interactive run only prints a hint and changes nothing. Without
--git-exclude, --quiet skips the check entirely. daft never touches the tracked
.gitignore.

## Usage

```
git daft-repo-install [OPTIONS]
```

## Options

| Option | Description | Default |
|--------|-------------|----------|
| `-q, --quiet` | Suppress progress reporting |  |
| `-v, --verbose` | Show detailed progress |  |
| `--git-exclude` | Add /daft.yml to .git/info/exclude without prompting (keeps it private to this clone) |  |

## Global Options

| Option | Description |
|--------|-------------|
| `-h`, `--help` | Print help information |
| `-V`, `--version` | Print version information |

