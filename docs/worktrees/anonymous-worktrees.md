---
title: Anonymous Worktrees
description:
  Detached-HEAD sandboxes and forks — visit points in history, fan out private
  worktrees for parallel work, and merge the results back.
---

# Anonymous Worktrees

Not every worktree needs a branch. An anonymous worktree is a detached-HEAD
checkout with the full daft treatment — hooks run, environment set up, visible
in `daft list` — that lives exactly as long as its directory. No branch, no
tracking, no push.

Two commands create them, split by intent: **`go` visits anything that exists;
`start` mints anything new.**

## Quick examples

```bash
daft go v1.18.0                              # visit a tag — sandbox created, hooks run, cd in
daft go $(git merge-base HEAD origin/master) # inspect a PR's "before" state
wt=$(daft start --fork)                      # mint a private fork of the current position
daft start --fork -n 3 -x './rebuild.sh'     # three forks, each built, paths on stdout
```

## Visit or mint?

- **Visit — `daft go <commit-ish>`** when you need to _look at_ a point in
  history and sharing is fine. Idempotent: the first visit materializes the
  canonical sandbox for that commit; every later visit — by any spelling that
  resolves to the same commit — lands in the same worktree, environment warm.
- **Mint — `daft start --fork [<base>]`** when you need a _private_ worktree
  that nothing else will collide with, or several at once. Always fresh: run it
  twice, get two. A fork is never matched by `go`'s resolution — it is reachable
  only by its printed name, which is what makes it safe for parallel agents.

The created path prints bare on stdout (one per line under `-n`), narration goes
to stderr — `wt=$(daft start --fork)` is the whole scripting integration. See
[`daft go`](/reference/cli/daft-go) and
[`daft start`](/reference/cli/daft-start) for the full surface.

## Fan out, then merge back

The loop that motivates forks: split a branch's current position into several
private worktrees, work in each, then adopt the results. From a worktree on
`feature-x`:

```bash
daft start --fork -n 3        # three forks of feature-x's HEAD; paths on stdout
```

Make commits in each fork as usual. Every worktree's HEAD is a git reachability
root, so the commits are protected for as long as the fork exists — no branch
required.

To merge work back, give each fork's position a branch name first. From inside
the fork:

```bash
git switch -c fork-a          # names this fork's HEAD, in place
```

Then adopt it from the target worktree:

```bash
# from the feature-x worktree
daft merge --rebase fork-a               # adopt one fork, linear history

# adopt two at once — octopus, one merge commit
daft merge fork-a fork-b

# adopt a single commit out of a fork instead of the whole thing
git cherry-pick <sha>
```

Naming the branch is what makes this work in a repository that commits
`merge: { source_worktree: clean }`: that gate certifies the source's **working
tree**, and a bare commit-ish — including git's `worktrees/<dirname>/HEAD`
spelling for another worktree's HEAD — gives it no worktree to inspect.
`git switch -c` costs nothing here, since the fork's own directory becomes the
new branch's worktree, already clean.

Merge hooks fire as on any other merge, with `DAFT_MERGE_SOURCE_SHAS` naming the
resolved fork positions — see [Merging across worktrees](/worktrees/merging).

## Keeping work: promotion

When a fork's work deserves a branch of its own — a PR, a push, a longer life —
promote it instead of merging: from inside the fork,

```bash
daft start real-branch-name   # new branch based on the fork's detached HEAD
```

mints a real branch at the fork's position, and from there it is ordinary branch
machinery.

## Cleanup and the pin guard

Remove anonymous worktrees by their directory name, one at a time or with a
wildcard pattern over the whole fleet:

```bash
daft remove feature-x-fork-3        # untouched fork: removes cleanly
daft remove feature-x-fork -f       # fork with new commits: -f required
daft remove 'feature-x-fork*'       # every fork in one sweep
```

Patterns (`*` and `?`) are expanded by daft against sandbox names — they never
match branches, and a pattern that matches nothing aborts. Quote the pattern so
your shell passes it through untouched.

Every anonymous worktree records the commit it was created at — its _pin_.
`daft remove` compares the worktree's HEAD to the pin:

| Fork state                     | Removal behavior                        |
| ------------------------------ | --------------------------------------- |
| HEAD still on the pin          | Removes without ceremony                |
| HEAD moved (commits were made) | Refuses — the commits die with the fork |
| HEAD moved, `-f` passed        | Removes; you have confirmed the loss    |

The guard is a plain HEAD-vs-pin comparison — it does not know whether the
commits were merged elsewhere. After a successful merge back, the target branch
reaches the fork's commits, so `-f` is the informed confirmation that nothing is
actually lost.

::: warning Never push from an anonymous worktree

There is no branch and no upstream. To publish work made in a fork, promote it
to a branch first or merge it into one.

:::

## Where to next

- **Merging:** [Merging across worktrees](/worktrees/merging) — styles,
  conflicts, hook gates
- **CLI reference:** [`daft go`](/reference/cli/daft-go),
  [`daft start`](/reference/cli/daft-start),
  [`daft remove`](/reference/cli/daft-remove)
- **Configuration:** `daft.start.forkNaming` in
  [Configuration](/reference/configuration)
