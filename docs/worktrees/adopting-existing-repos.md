---
title: Adopting Existing Repositories
description: Move a traditional repository into a worktree-based layout
---

# Adopting Existing Repositories

Already have a traditional Git repository? Move it into a worktree layout
without losing any work — `daft layout transform` restructures the repository
you point it at, and carries the working tree exactly as it is.

## What the transform does

`daft layout transform contained` restructures a traditional repository into
daft's default worktree layout:

**Before:**

```
my-project/
├── .git/            # Regular git directory
├── src/
├── package.json
└── README.md
```

**After:**

```
my-project/
├── .git/            # Bare repository
└── main/            # Worktree for the branch that was checked out
    ├── src/
    ├── package.json
    └── README.md
```

::: tip `contained` is daft's default layout. Any other layout works the same
way — `daft layout transform sibling`, `nested`, `centralized`,
`contained-flat`, `contained-classic` — and you can move between them at any
time. See [Layouts](/worktrees/layouts). :::

The transform follows the branch that is actually checked out in the repository
— not the default branch. Adopt a clone in the middle of a feature and the
worktree you get is that feature's; if the default branch has no worktree, it
stays that way (the transform says so before it starts).

## Running it

```bash
cd my-existing-project
daft layout transform contained
```

### Preview first

Use `--dry-run` to see the plan without making changes. It also reports anything
that would stop the transform, so "can this repo transform?" is answerable
without attempting it:

```bash
daft layout transform contained --dry-run
```

## Your working tree comes along

Modified files, staged hunks, untracked files, ignored build output, even
unresolved conflict entries — everything in the working tree is carried across
the transform untouched. `git status` reads the same before and after; nothing
is stashed, and no `--force` is needed.

The only things a transform will not carry are operations git itself is in the
middle of: a paused rebase, merge, cherry-pick, revert, or bisect in the working
tree that changes role. Those are refused up front — every one of them in a
single report, each with the exact commands to finish or abort it and the
command to retry — rather than carried half-way. See
[Layouts → What is refused](/worktrees/layouts#what-is-refused-and-how-to-settle-it).

## Reverting

If you decide the worktree layout isn't for you, transform back:

```bash
daft layout transform sibling
```

This restores the main working tree at the repository root:

**Before:**

```
my-project/
├── .git/            # Bare repository
├── main/
│   ├── src/
│   └── README.md
└── feature/auth/
    ├── src/
    └── README.md
```

**After:**

```
my-project/            # Regular git directory again
├── .git/
├── src/
└── README.md
my-project.feature-auth/
├── src/
└── README.md
```

Other worktrees are kept and relocated to where the target layout puts them —
nothing is deleted. Remove the ones you no longer need with
`daft remove <branch>`.

## When to transform vs clone fresh

**Use `daft layout transform`** when:

- You have an existing local repository with work in progress
- You want to try the worktree workflow without re-cloning
- You have local branches or stashes you want to preserve

**Use `daft clone`** when:

- Starting fresh from a remote repository
- Setting up a new development environment
- The repository has no local-only work to preserve
- You want to choose a specific [layout](/worktrees/layouts) with `--layout`
