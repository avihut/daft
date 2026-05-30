---
title: git-daft-repo-install
description: Install a starter daft.yml in the current worktree
---

# git daft-repo-install

Install a starter daft.yml in the current worktree

## Description

Creates a starter daft.yml at the worktree root with a commented skeleton
covering the major sections (hooks, shared, layout). Modeled on
`lefthook install`.

This is the canonical name for the bootstrap; `daft install` is a top-level
alias that runs the same thing (so lefthook-style discovery keeps working).

daft.yml is a per-worktree file, so install is repo-aware. Run it inside a
worktree: from a subdirectory it targets the worktree root, and it refuses
outside a git repository or at the bare container root of a contained layout
(where a daft.yml would be inert). If a daft.yml already exists it reports
whether that file is tracked (a team baseline) or a visitor config (untracked)
and stops without modifying it.

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

