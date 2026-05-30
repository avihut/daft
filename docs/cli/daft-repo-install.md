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

    daft repo install [--quiet | -q] [--verbose | -v] [--git-exclude]

| Argument / flag   | Description                                                                                                |
| ----------------- | --------------------------------------------------------------------------------------------------------- |
| `--quiet`, `-q`   | Suppress progress reporting. Without `--git-exclude`, also skips the git-exclude check (no prompt, no hint). |
| `--verbose`, `-v` | Show detailed progress.                                                                                   |
| `--git-exclude`   | Add `/daft.yml` to `.git/info/exclude` without prompting (keeps it private locally). Takes precedence over `--quiet`: the entry is still added, just silently. |

## Behavior

- Writes `daft.yml` at the current worktree root. The template is a fully
  commented skeleton — daft reads nothing from it until you uncomment a
  section.
- If `daft.yml` already exists, the command refuses without modifying anything.
  Edit the existing file with your editor instead.
- **Offers to keep it private (visitor mode):** after writing `daft.yml`, daft
  checks whether git already ignores it. If not, it offers to add `/daft.yml` to
  `.git/info/exclude` — a local, per-clone exclude that is **never committed**,
  so a visitor config stays invisible to teammates.
  - Interactive (a TTY): you are prompted (default No).
  - `--git-exclude`: adds the entry without prompting. This takes precedence
    over `--quiet` — the entry is still added, just without the confirmation
    message.
  - Non-interactive (no TTY, scripts, hooks): nothing is changed; daft prints a
    copy-pasteable hint instead.
  - `--quiet` (without `--git-exclude`): the check is skipped entirely — no
    prompt, no hint, no mutation.
- daft **never** touches the tracked `.gitignore`. For a team baseline you would
  commit `daft.yml` instead of excluding it.

## Examples

    # Bootstrap a starter daft.yml in the current worktree
    daft repo install

    # Bootstrap a private (visitor) config and exclude it in one step
    daft repo install --git-exclude

    # Same thing via the top-level alias
    daft install
