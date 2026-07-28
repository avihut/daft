---
title: daft remove
description: Delete branches and their worktrees
---

# daft remove

Delete branches and their worktrees

## Usage

```
daft remove [OPTIONS] <BRANCHES>
daft remove -f <BRANCHES>
```

This is equivalent to `git worktree-branch -d` (safe delete). Use `-f` to
force-delete branches regardless of merge status (`git worktree-branch -D`).

## Description

Deletes one or more local branches along with their associated worktrees in
a single operation. Arguments can be branch names, worktree paths, or
sandbox names.

### Sandboxes

Anonymous detached worktrees created by daft — `daft go <commit-ish>`
sandboxes and `daft start --fork` forks — are removed by their directory
name or path, exactly like branch worktrees:

```bash
daft remove v1.18.0          # a tag sandbox, by name
daft remove main-fork-2      # a fork, by name
daft remove ../brave-otter   # by path
daft remove 'main-fork*'     # a wildcard over sandbox names
```

A branch sharing a sandbox's spelling always wins the name — the sandbox
reading applies only when no such branch exists. There is no branch or
remote to delete, so only the worktree goes; pre/post-remove hooks still
run. Two safety checks replace the branch checks: the worktree must be
clean, and its HEAD must still sit at the commit the sandbox was pinned at.
Commits made on a detached HEAD exist nowhere else, so a moved HEAD refuses
removal — promote the work first with `daft start <new-branch>` from inside
the sandbox, or pass `-f` to discard it. A detached worktree daft did not
create (no identity record) keeps the historical refusal and is never
treated as a sandbox.

### Wildcards

Sandbox names may be given as wildcard patterns: `*` matches any run of
characters and `?` exactly one. `daft remove 'main-fork*'` sweeps every fork
minted off main in one command — the natural cleanup after a
`daft start --fork -n 3` fan-out. Each matched sandbox still passes the
safety checks above, and one refusal aborts the whole run (fix it or pass
`-f`).

Patterns match sandbox names only. They never expand to branches — removing
a branch also deletes its remote, and fleet-scale branch cleanup is
[daft prune](./git-worktree-prune.md)'s job — and never to paths. A pattern
that matches no live sandbox aborts the command rather than silently
removing nothing; if branches did match the pattern, the error names them so
you can spell them out deliberately.

Quote the pattern (`'main-fork*'`) so your shell passes it through: zsh
errors on globs that match nothing in the current directory, and an
unquoted glob that does match local files would rewrite your arguments
before daft sees them.

When invoked outside any git repository, `daft remove` accepts absolute or
relative worktree paths and discovers the owning repository from the path
itself, so worktrees can be cleaned up without first `cd`-ing into a sibling
worktree. All paths in a single invocation must belong to the same repository.

`--repo <name>` addresses another cataloged repository by name instead of by
path, from inside a different repository or from outside any repository:

```bash
daft remove --repo api feature-x
```

The resolved destination is announced before any work begins. Because the
removal happens elsewhere, your current directory stays valid and your shell
is never relocated. A `--repo` removal also runs non-interactively: a branch
whose refined daft files would normally raise a consolidation prompt aborts
instead, with the usual guidance to consolidate with `daft file merge` or pass
`-f` up front -- it never waits on a keypress about a repository you are not
standing in. Combining `--repo` with a worktree path is an error -- the path
already identifies its own repository. There is no `--all-repos` form; removing
one branch across every repository is rarely intended, and fleet-wide cleanup
is [daft prune](./git-worktree-prune.md)'s job.

Note that `--repo` is a flag rather than a positional. `daft remove api
feature-x` always means "remove the branches `api` and `feature-x` in the
current repository" -- the positional slot is a list of branches or paths and
is never reinterpreted as a repository name.

By default, the remote branch is not deleted. To also delete the remote branch,
set `daft.branchDelete.remote true` or use `daft config remote-sync --on`. You
can also pass `--remote` to delete only the remote branch while keeping the
local worktree and branch, or `--local` to skip the remote entirely regardless
of config.

The remote delete pushes no content, so the repo's pre-push hook is skipped by
default (configurable via `daft.pushVerify`; `--no-verify` skips it
unconditionally). `daft.pushVerify` is the base setting every daft push reads,
so it also affects the branch-creation upstream push; scope it to that push
alone with `daft.checkout.pushVerify`. See
[Git Hooks](/reference/configuration#git-hooks) for details.

Safety checks prevent accidental data loss. Use `-f` (`--force`) to override.
For the default branch (e.g. main), `-f` removes its worktree only -- the
local branch ref and remote branch are always preserved.

An unmerged branch does not need `-f` when it is identical to a remote branch
that this removal preserves: the commits stay reachable at the remote, and
`daft go <branch>` brings them back. Daft confirms that with the remote itself
rather than trusting `refs/remotes/<remote>/<branch>`, which is a local cache
that outlives a server-side delete. That check contacts the remote (as the
squash-merge check may already do, via `gh`/`glab`); it is bounded at 15
seconds, runs non-interactively, and refuses rather than assumes when the
remote cannot be reached. Enabling remote deletion
(`daft.branchDelete.remote`, or `--remote`) withdraws the allowance, because
then the remote copy does not survive.

## Options

| Option | Description | Default |
|--------|-------------|---------|
| `-f, --force` | Force deletion even if not fully merged | |
| `--local` | Delete only locally; do not touch the remote branch | |
| `--remote` | Delete only the remote branch; keep the local worktree and branch | |
| `--repo <REPO>` | Remove branches in another cataloged repository | |
| `-v, --verbose` | Show detailed progress | |
| `-q, --quiet` | Suppress non-error output | |

## Global Options

| Option | Description |
|--------|-------------|
| `-h`, `--help` | Print help information |
| `-V`, `--version` | Print version information |

## See Also

- [daft config](./daft-config.md) to configure remote sync behavior
- [git worktree-branch](./git-worktree-branch.md) for full options reference
