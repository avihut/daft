---
title: daft start
description: Create a new branch and worktree
---

# daft start

Create a new branch and worktree

## Usage

```
daft start [OPTIONS] <BRANCH_NAME> [BASE_OR_BRANCH] [BASE]

daft start <branch> [<base>]                # local
daft start <repo> <branch> [<base>]         # in another cataloged repo
daft start --repo <repo> <branch> [<base>]  # explicit / script-safe
```

The local form is equivalent to `git worktree-checkout -b`. All flags are the
same as `git worktree-checkout` with `-b` implied.

## Description

Creates a new branch and a corresponding worktree in a single operation.
The new branch is based on the current branch, or on the base branch
if specified.

By default, daft does not push the new branch to the remote. To enable pushing
and upstream tracking, set `daft.checkout.push true` or use
`daft config remote-sync --on`. You can also pass `--local` to skip remote
operations for a single invocation regardless of config.

When the push is enabled, the repo's `pre-push` hook runs only when the push
introduces new commits; a ref-only push of already-pushed commits skips it
(configurable via `daft.checkout.pushVerify`: `auto`, `always`, or `never` —
see [Git Hooks](/reference/configuration#git-hooks)).

## Creating in another repo

`daft start` takes a leading [repo catalog](/graph/repo-catalog) target,
mirroring `daft go <repo> <branch>`. How the names are read:

| Form | Meaning |
| --- | --- |
| `daft start A` | Always local: new branch `A` (a lone repo name is never a target) |
| `daft start A B` | Decided local-first, in order — see below |
| `daft start A B C` | Always cross-repo: branch `B` in repo `A`, based on `C` (a catalog miss is a hard error) |
| `daft start --repo A B [C]` | Explicit cross-repo — for repo names shadowed by local branches, and for scripts |

Two names is the only ambiguous arity. It resolves in this order, and the
first rule that matches wins:

1. **`A` is an existing local branch** — the local reading is kept and fails
   fast as "already exists" (with a `--repo` hint when `A` is also cataloged).
2. **`B` resolves to a commit here** (branch, remote-tracking ref, tag, SHA) —
   this is the ordinary `<branch> <base>` form. A repo-shaped `A` does not
   hijack it: `daft start api release-2` creates local branch `api` off
   `release-2`.
3. **`A` names the repo you are standing in** — a redundant qualifier, not a
   hop: the branch is created here on the usual local base (the current
   branch), and `-c` is not refused.
4. **`A` is a live cataloged repo** — create `B` over there.
5. Otherwise local: branch `A` based on `B`.

Anything meaningful in the current repository wins over a catalog match, so a
new branch name that happens to equal a repo name never silently retargets.
The escape when you really do want a cross-repo branch whose name collides
with a local ref is `--repo`. The guess matches names only — paths (and
UUIDs) work in the explicit forms.

A repo that is cataloged but whose directory moved, or that was removed with
`daft repo remove`, is reported the same way `daft go` reports it — it is
never quietly reinterpreted as a local branch name.

Cross-repo semantics:

- The resolved destination is announced before any work:
  `Creating branch 'X' in 'repo' (path) — based on 'base'` (`--quiet` opts
  out).
- Without a base, the branch is based on the **target repo's default branch**
  (never whatever happens to be checked out there). An explicit base must
  exist in the target repo.
- The shell lands in the new worktree over there, and `daft go -` hops back.
- The target repo's hooks run under **its** trust; an untrusted target skips
  them with a notice.
- `-c`/`--carry` cannot cross repositories (hard error); `-x` runs in the
  target repo's new worktree; a relative `--at` resolves inside the target
  repo.

## Coordinated fan-out (--with-related)

`daft start <branch> --with-related` also creates the branch in every repo the
current repo's `daft.yml` `relations:` manifest points at — see
[Coordinated changes](/graph/coordinated-changes). Combined with a repo
target, the fan-out is **rooted at the target**:
`daft start api feat/x --with-related` creates `feat/x` in `api` and in the
repos *api's* manifest declares, and the shell lands in api's new worktree.

## Arguments

| Argument | Description | Required |
|----------|-------------|----------|
| `<BRANCH_NAME>` | Name of the new branch — or a cataloged repo to create it in, when two or more names are given | Yes |
| `[BASE_OR_BRANCH]` | Base branch (defaults to the current branch) — or, when it names no ref here, the new branch inside the repo | No |
| `[BASE]` | Base branch inside the repo (three-name form); must exist there | No |

## Options

| Option | Description | Default |
|--------|-------------|---------|
| `--repo <REPO>` | Create the branch in a repository from the catalog (for repo names shadowed by local branches) | |
| `--with-related` | Also create the branch in every related repo (relations manifest), each based on its own default branch | |
| `--local` | Skip all remote operations (no fetch, no push) for this invocation | |
| `--skip-hooks <SELECTOR>` | Skip hooks this run (`all` \| a hook name like `worktree-post-create` \| `tag:<tag>` \| `<job>`); repeatable/comma-separated | |
| `-c, --carry` | Apply uncommitted changes from the current worktree to the new one | |
| `--no-carry` | Do not carry uncommitted changes | |
| `-x, --exec <EXEC>` | Run a command in the worktree after setup completes (repeatable) | |
| `--no-cd` | Do not change directory to the new worktree | |
| `-v, --verbose` | Show detailed progress | |
| `-q, --quiet` | Suppress non-error output | |

## Global Options

| Option | Description |
|--------|-------------|
| `-h`, `--help` | Print help information |
| `-V`, `--version` | Print version information |

## See Also

- [daft go](./daft-go.md) to open an existing branch
- [daft config](./daft-config.md) to configure remote sync behavior
- [git worktree-checkout](./git-worktree-checkout.md) for full options reference
