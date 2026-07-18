---
title: daft push
description: Push a branch with pre-push hooks running in its own worktree
---

# daft push

Push a branch with the repository's shared `pre-push` hook running in the
pushed branch's own worktree. `daft push` is the short verb form of
`git worktree-push`: all flags and behavior are identical, but invoking
through `daft` integrates with daft's shell wrappers.

## Usage

```
daft push [OPTIONS] [BRANCH]
```

This command is equivalent to `git worktree-push`. See
[git worktree-push](./git-worktree-push.md) for the generated flag reference;
this page focuses on why the command exists and the surrounding workflow.

## Description

In a worktree layout the `pre-push` hook is shared: git resolves it from the
common git directory (or `core.hooksPath`, the mechanism used by lefthook,
husky, and pre-commit), and fires it exactly once per push — with the working
directory of whatever worktree you invoked `git push` from. Pushing branch B
while sitting in worktree A therefore runs the hook against A's tree: a hook
that runs the test suite, lints the working tree, or reads worktree-local
configuration (`.env`, mise, direnv) silently validates the wrong source.

`daft push <branch>` exists to fix exactly that. It resolves the branch to
its worktree and runs the push from there, so the hook always sees the tree
it is guarding. That is the only thing it adds over `git push`:

- **Remote:** the push targets `daft.remote` (default: `origin`), the same
  remote `daft sync` and `daft start` use.
- **Upstream:** a branch with no upstream is pushed with `--set-upstream`,
  so tracking gets configured as a side effect.
- **No worktree:** a branch with no checked-out worktree is pushed by
  refname from the current directory — plain `git push` behavior, not an
  error.
- **No argument:** pushes the current worktree's branch; resolution is a
  no-op and the command behaves like plain `git push`.
- **Single-branch only:** git fires `pre-push` once with one working
  directory, so worktree-correct hook context is only well-defined for one
  branch. A second positional is rejected at parse time. To push every
  branch you own, use `daft sync --push`.

On an interactive terminal the run renders as a plan-then-execute rail: the
resolved worktree, the embedded `pre-push hooks` section with the hook's
output, and the push result. A failing hook blocks the push, surfaces the
hook's output, and exits non-zero.

## Key Options

| Option               | Description                                                          |
| -------------------- | -------------------------------------------------------------------- |
| `--no-verify`        | Skip the repo's `pre-push` hook once (passes `--no-verify` to git).  |
| `--force-with-lease` | Passthrough of `git push --force-with-lease` (e.g. after an amend).  |
| `-v, --verbose`      | Thread the hook's full output under its rail row.                    |
| `-q, --quiet`        | Suppress non-essential output.                                       |

## Examples

### Push another worktree's branch, hooks running in its tree

```bash
# In ~/code/acme/main, pushing feature-b (worktree at ../feature-b):
daft push feature-b
# The shared pre-push hook runs with cwd = ../feature-b
```

### Push the current branch

```bash
daft push
```

### Bypass a failing hook once

```bash
daft push --no-verify feature-b
```

### Push an amended branch

```bash
daft push --force-with-lease feature-b
```

## Configuration

| Key                        | Effect                                                          |
| -------------------------- | --------------------------------------------------------------- |
| `daft.remote`              | Remote the push targets (default `origin`).                     |
| `daft.hooks.output.*`      | Hook output density (`verbose`, `quiet`, `tailLines`).          |

## Related

- [daft sync](./daft-sync.md) — push every owned branch (`--push`), each from
  its own worktree
- [daft start](./daft-start.md) — the automatic upstream push on branch
  creation honors the same hook machinery
- [Hooks: daft and your existing git hooks](/hooks/#daft-and-your-existing-git-hooks)
