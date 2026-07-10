---
title: Polyrepo, monorepo feel
description:
  Use the repo catalog and worktrees to navigate, update, and maintain a
  constellation of repos as if it were one.
pillars: [graph, worktrees]
---

# Polyrepo, monorepo feel

## Starting state

A constellation of repos under `~/code`, each in daft's worktree layout:

```
~/code/
├── api/
├── web/
├── worker/
└── infra/
```

The ritual: navigation is `cd ~/code/wor<Tab>` muscle memory, and staying
current is a shell loop —

```bash
for d in ~/code/*/main; do (cd "$d" && git pull); done
```

— that you run when you remember to. The pain has already happened: the loop
silently skipped `worker` (its default worktree is `master`, not `main`), so a
security bump everyone "pulled" never landed there, and the stale checkout
burned an afternoon. Meanwhile "do we have a repo for the billing prototype, and
where is it?" is a question you answer by reading `ls ~/code` and guessing.

The reach for daft: it already touches every one of these repos — let its
catalog be the fleet's index, and run navigation and maintenance against the
catalog instead of against a hand-rolled loop.

## What changes

Nothing to configure. Every repo daft clones (or runs inside) registers itself
in a machine-local catalog — identity, name, path, remote, default branch. On
top of that index:

- `daft go <repo>` jumps to any repo's default-branch worktree, from anywhere on
  the filesystem.
- `daft list --all-repos`, `daft update --all-repos`, and
  `daft exec --all-repos` sweep the whole fleet.
- `daft doctor` audits the catalog itself and reconciles it with `--fix`.

## Recipe

Clone the fleet — each clone registers itself:

```bash
cd ~/code
daft clone git@github.com:acme/api.git
daft clone git@github.com:acme/web.git
daft clone git@github.com:acme/worker.git
daft clone git@github.com:acme/infra.git
```

For a repo daft has never touched (an old plain clone), register it explicitly
from inside:

```bash
daft repo add --name legacy-billing
```

Then the daily surface:

```bash
# The fleet's index — names, default branches, locations.
daft repo list

# Jump anywhere, from anywhere. Lands on the repo's default-branch worktree.
daft go worker

# Monday morning, one command instead of the shell loop. Each repo's
# default branch is what updates — master, main, whatever it actually is.
daft update --all-repos

# Fleet-wide maintenance, run in each repo's default-branch worktree.
daft exec --all-repos -- npm audit fix

# Every worktree of every repo, one listing.
daft list --all-repos

# Audit the index itself: stale paths, identity drift, name collisions.
daft doctor --all-repos
```

Piece by piece:

1. **`daft repo list`** shows what the catalog knows: `NAME`, default `BRANCH`,
   `PATH`. `daft repo info <name>` shows one entry in full, including any
   declared relations.
2. **`daft go worker`** works from `~/`, from inside `api`, from anywhere — the
   catalog supplies the path, and daft lands you in the default-branch worktree.
   `daft go worker feat/retry` opens a specific branch's worktree there,
   creating it if the branch exists.
3. **`daft update --all-repos`** replaces the pull loop, and it can't make the
   `master`-vs-`main` mistake: each repo's recorded default branch is the
   target.
4. **`daft doctor`** owns catalog hygiene. Deleted a repo with plain `rm -rf`?
   Doctor flags the entry pointing at a missing path, and `daft doctor --fix`
   marks it removed.

Removed repos stay addressable on purpose — their entry (and their hook-run
history) survives, so restoring one is:

```bash
daft clone worker   # re-clones from the recorded remote URL
```

## Variants

By **how repos join the catalog**.

### All up front

The Recipe's shape: a one-time clone script (or docs page) that `daft clone`s
the whole fleet. Best when the set is small and everyone should have all of it.
The catalog mirrors the org list from day one.

### Lazily, as touched

Clone nothing in advance. Repos enter the catalog the first time daft operates
in them — a clone for a bug fix, a `daft list` inside an old checkout, an
explicit `daft repo add` for the odd pre-daft clone. Best for large orgs where
nobody holds the whole fleet: your catalog converges on exactly the repos you
actually work in, and `--all-repos` means _your_ fleet, not the org chart.

## Idempotency & safety

The catalog is machine-local state (under daft's data directory), never
committed, and self-healing: entries refresh whenever daft runs inside a repo,
so moves and re-clones converge without ceremony. Re-cloning over a removed
entry revives the name; the old identity is retired, not lost.

Names come from the repo's directory or remote and must be unique among live
entries — a second clone of `api` elsewhere registers as `api-2`, with a notice.
Rename anytime from inside the repo with `daft repo add --name <name>`.

::: warning Branch names shadow repo names

`daft go api` opens the _branch_ `api` if one exists in the current repo —
anything resolvable locally wins over the catalog. When a repo name is shadowed,
address it explicitly: `daft go --repo api`.

:::

## Where to next

- **[Coordinating a service and its client](/recipes/coordinating-service-and-client)**
  — the relations manifest: teaching repos in the fleet that they move together.
- **[daft repo list](/reference/cli/git-daft-repo-list)** — the catalog's CLI
  reference, including structured `--format` output for scripting.
- **[Run daft from anywhere](/worktrees/from-anywhere)** — the `-C` flag,
  `daft go`'s single-repo ancestor.
