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

daft.yml is a per-worktree file, so install is repo-aware. Inside a worktree it
targets the worktree root (even from a subdirectory). At the bare container root
of a contained layout it installs across the repo's worktrees — writing the
starter into the default worktree and copying it into the others, like
`daft clone --install`. It refuses only outside a git repository. If a daft.yml
already exists it reports whether the file is tracked or a visitor config and
stops without modifying it.

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

