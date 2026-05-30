---
title: daft repo install
description: Install a starter daft.yml in the current worktree
---

# `daft repo install`

Writes a starter `daft.yml` at the current worktree root — a commented skeleton
covering the major sections (hooks, shared, layout) so you can uncomment what
you need. Modeled on `lefthook install`.

This is the canonical name. `daft install` is a top-level alias that runs the
exact same thing; the alias is kept so lefthook-style discovery (`daft install`)
keeps working.

## Usage

    daft repo install [--quiet | -q] [--verbose | -v]

| Argument / flag   | Description                  |
| ----------------- | ---------------------------- |
| `--quiet`, `-q`   | Suppress progress reporting. |
| `--verbose`, `-v` | Show detailed progress.      |

## Behavior

- Writes `daft.yml` at the current worktree root. The template is a fully
  commented skeleton — daft reads nothing from it until you uncomment a
  section.
- If `daft.yml` already exists, the command refuses without modifying anything.
  Edit the existing file with your editor instead.
- **No git side effects:** daft does not touch `.gitignore` or
  `.git/info/exclude`. Whether the file is tracked (a team baseline) or
  gitignored (your personal visitor config) is your call — see the comments in
  the generated file.

## Examples

    # Bootstrap a starter daft.yml in the current worktree
    daft repo install

    # Same thing via the top-level alias
    daft install
