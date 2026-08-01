---
title: Troubleshooting
description: Common issues and how to fix them.
---

# Troubleshooting

This page covers general daft issues — install, layout, basic shell integration.
For symptoms specific to hooks and recipes (warmup hangs, trust-prompt loops,
env vars not propagating), see
[Troubleshooting recipes](/recipes/troubleshooting).

If your problem isn't listed here, run `daft doctor` first — it diagnoses common
configuration issues automatically.

## "command not found: daft"

`daft` is installed but not on `PATH`. Verify the install location
(`brew prefix avihut/tap/daft` on macOS) is in your shell's `PATH`.

## My shell doesn't `cd` into the new worktree

Shell integration isn't installed. See
[Shell integration](/getting-started/shell-integration) for the eval line to add
to your shell config.

## "Untrusted repo — N hooks not run: ..."

The repo defines hooks (`daft.yml` or `.daft/hooks/`) but hasn't been trusted,
so daft skipped them. Trust it, then replay the setup that was skipped:

```bash
git daft-hooks trust
# trust prints the exact replay commands, e.g.:
git daft-hooks run worktree-post-create   # inside each listed worktree
```

This is intentional — see [Trust & security](/hooks/trust-and-security) for why.

## My worktree is missing its hook side effects (env files, installs)

It was probably created before the repo was trusted. Run `git daft-hooks trust`
— it lists the worktrees whose setup hooks never ran and the
`git daft-hooks run ...` commands to replay them.

## Hooks fire but I don't see their output

Job stdout/stderr is captured to log files in `~/.local/state/daft/logs/` (XDG
state dir). Inspect with:

```bash
git daft-hooks log show
```

## Worktree creation fails with "fatal: <branch> is already checked out"

The branch is checked out in a different worktree. Either remove the other
worktree first (`daft remove <branch>`), or use a different branch name.

## `daft adopt` says my repo "looks like it's already adopted"

The repo already has the daft layout. Use `daft list` to see existing worktrees,
or `daft eject` to restore a single-working-tree layout if you want to start
over.

## I can't tell which worktree is which

`daft list` prints all worktrees. With `--format json` you get machine-readable
output.

## I removed a worktree but the disk space hasn't come back

That is expected, briefly. `daft remove` and `daft prune` move the worktree
aside and return as soon as the parts you are waiting on are done — the branch
is gone, git is consistent, and the path is free to reuse immediately. The
directory itself is deleted by a background process a moment later, which is why
removing a worktree full of `node_modules` takes about as long as removing an
empty one instead of tens of seconds.

If space still has not returned after a while, `daft doctor` reports any
worktrees that are still waiting, and `daft doctor --fix` reclaims them
immediately. Removals also sweep leftovers on their way through, so the next
`daft remove` or `daft prune` in that repo picks up anything stranded.

To make removal reclaim space before it returns — for a script that checks free
space straight afterwards, say — set `DAFT_NO_TRASH_REAP=1`.

## When in doubt

Run `daft doctor`. It diagnoses install, shell integration, layout health, and
hook trust state.
