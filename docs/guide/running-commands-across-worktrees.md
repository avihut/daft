---
title: Running commands across worktrees
description:
  Use daft exec to run one or more commands against one or many worktrees
  without cd-ing into them.
---

# Running commands across worktrees

`daft exec` runs a command against one or more worktrees without changing your
current directory. It's the right tool when you want to:

- Run a test or build on a specific branch without switching to it
- Fan-out a lint or format pass across every branch in flight
- Execute a pipeline of setup commands across many worktrees at once

## The basics

```bash
daft exec feat/auth -- cargo test
daft exec --all -- pnpm lint
daft exec 'feat/*' -- npm test
```

Positional arguments can be branch names, worktree directory names, or globs
against branch names. `--all` expands to every worktree. The `--` separator
marks the boundary between daft's flags and the command you want to run —
everything after it is forwarded verbatim.

## Multiple commands

Pass `-x` one or more times to run a pipeline of commands sequentially per
worktree. If any command in the pipeline fails, that worktree's pipeline stops;
other worktrees are unaffected.

```bash
daft exec --all -x 'mise install' -x 'pnpm build' -x 'pnpm test'
```

`-x` and `--` are mutually exclusive. Use `-x` for pipelines, `--` for single
commands whose own flags would otherwise collide with daft's.

## Parallel vs sequential

By default, worktrees run in parallel. Use `--sequential` to run them one at a
time (stopping on first failure), or `--keep-going` to run every worktree even
after failures:

```bash
daft exec --all -- cargo test               # parallel, default
daft exec --all --sequential -- cargo test  # one at a time, stop on first fail
daft exec --all --keep-going -- cargo test  # one at a time, don't stop
```

## Single-target pass-through

When your selectors resolve to exactly one worktree, daft hands stdio through
directly. Interactive programs work:

```bash
daft exec feat/auth -- claude
daft exec feat/auth -- vim src/main.rs
```

No UI renders; the child's exit code is propagated verbatim.

## Viewing output

During a multi-worktree run, `daft exec` shows a live progress row per worktree
with a rolling tail of output beneath each. When a worktree finishes, its row
collapses to a single compact line that names the command inline. After all
worktrees complete, any that failed have their captured output dumped to stdout
for easy scrollback review.

Each finalized row shows the worktree name, the command that ran, and a
state-specific suffix:

- `✓ branch ❯ cmd (1.5s)` — command succeeded
- `✗ branch ❯ cmd (1.2s)` — command exited non-zero
- `⊘ branch ❯ cmd cancelled after 0.4s` — command was running when Ctrl-C fired
- `○ branch ❯ cmd skipped` — command never started (fail-fast upstream or cancel
  before launch)

## Relationship to other commands

| Use case                                          | Command                                         |
| ------------------------------------------------- | ----------------------------------------------- |
| Run once, ad-hoc, across many worktrees           | `daft exec`                                     |
| Run every time a worktree is created              | `daft.yml` `worktree-post-create` hook          |
| Run once per command invocation on a new worktree | `-x` flag on `daft clone` / `init` / `checkout` |

## See also

- [daft exec](../cli/daft-exec.md) /
  [git worktree-exec](../cli/git-worktree-exec.md) — CLI reference
- [Hooks](./hooks.md) — recurring per-worktree automation via `daft.yml`
