---
title: FAQ
description: Frequently asked questions about daft.
---

# FAQ

## Does daft replace `git`?

No. daft sits next to git. Every daft command either calls into git or wraps a
git operation. You can mix `git` and `daft` commands freely in the same repo.

## Does daft work with monorepos?

Yes. See [Recipes → By scenario → Monorepo](/recipes/by-scenario/monorepo) for
the recommended pattern.

## Does daft work on Windows?

Yes. The binary is shipped for Windows and tested in CI. Shell integration works
in PowerShell, Git Bash, WSL, and Cmd (limited). See
[Shell integration](/getting-started/shell-integration).

## Does daft replace lefthook?

Today: no — daft hooks are scoped to worktree lifecycle. The lefthook drop-in is
on the roadmap ([#468](https://github.com/avihut/daft/issues/468)).

## Does daft replace GitHub Actions?

No. daft hooks are _local_ CI — they run on developer machines, before code
reaches the central repo. GitHub Actions runs _centrally_, after code arrives.
They're complementary: shift fast checks left into daft hooks; keep
slow/secrets-bound checks in Actions.

## How do I migrate an existing repo to daft?

`daft adopt`. See [Adopting existing repos](/worktrees/adopting-existing-repos).

## How do I uninstall daft from a repo?

`daft eject`. The repo is restored to a single-working-tree layout.

## Is daft safe for collaborators who don't use it?

Yes. daft writes to `.git/` and a single `daft.yml` (if you use hooks).
Collaborators using plain `git` see normal behavior; they don't need to adopt
daft.

## How does daft handle uncommitted changes when removing a worktree?

`daft remove` prompts before destroying uncommitted work. Use `--force` to
bypass.

## Does daft modify global git config?

No, ever. daft only writes repo-local config and its own files. Your global git
config is untouched.

## Where are hooks trusted?

In your XDG state directory — by default `~/.local/state/daft/trust.toml` on
Linux, `~/Library/Application Support/daft/trust.toml` on macOS,
`%LOCALAPPDATA%\daft\trust.toml` on Windows.
