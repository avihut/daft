---
title: Repo catalog
description:
  Managing daft's repo catalog — automatic registration, add/list/info,
  cross-repo navigation, removed repos, and fleet-wide commands.
---

# Repo catalog

The catalog is daft's machine-local registry of repositories. It powers
`daft go <repo>`, the `--repo`/`--all-repos` flags, clone-by-name, and relations
resolution. This page is the working reference for it.

## You mostly don't manage it

The catalog fills itself. These all register (or refresh) the repo they touch:

- `daft clone` / `daft init` / `daft adopt` / `daft eject`
- any of `daft go`, `list`, `exec`, `update`, `prune` running inside a repo

Explicit management exists for the gaps:

```bash
daft repo add                  # register the current repo (one daft never touched)
daft repo add ~/code/legacy    # register a repo by path
daft repo add --name api       # rename the current repo's entry
daft repo list                 # live entries: name, worktrees, path, remote
daft repo list --all           # include removed entries (dimmed)
daft repo list --columns +size # add a disk-usage column (sizes stream in live)
daft repo list --worktrees     # expand each repo into a tree of its worktrees
daft repo info client          # one entry in full, with resolved relations
```

`repo list` and `repo info` support `--format json|tsv|…` and `--template` for
scripting.

### Names

A repo's default name comes from its remote URL (falling back to its directory
name). Names are unique among live entries; a collision during automatic
registration auto-suffixes (`api-2`) and prints a notice, while an explicit
`daft repo add --name` with a taken name refuses instead. Rename at any time
with `daft repo add --name <new>` from inside the repo.

## Navigating with the catalog

`daft go` consults the catalog whenever the current repository can't satisfy a
name — and works entirely from the catalog outside any repository:

```bash
daft go client                 # its default-branch worktree
daft go client feat/login      # a specific branch's worktree (created if needed)
daft go --repo client          # explicit form, for names shadowed by branches
daft go --repo client -b feat/x main   # create a branch over there
daft go -                      # after a cross-repo hop: back where you came from
```

Precedence is strict and predictable: an existing worktree, local branch, or
remote branch in the current repo always beats a catalog repo of the same name;
a catalog match beats `daft.go.autoStart` branch creation; `--start` forces
creation. Tab completion offers repo names after your branches, and
`daft go <repo> <Tab>` completes the target repo's branches.

## Fleet commands

Every catalog-aware command accepts `--repo <name>` (act on one repo from
anywhere) and `--all-repos` (sweep every live entry). `daft list` — read-only,
with a free argument slot — additionally takes the repo as a positional, like
`daft go`:

```bash
daft list api                  # another repo's worktrees (sugar for --repo api)
daft list --all-repos          # every repo's worktrees, sectioned per repo
daft update --all-repos        # fetch/update the whole fleet
daft prune --all-repos         # prune everywhere (current repo last)
daft doctor --all-repos        # health-check every repo
daft exec --repo api -- pnpm build
daft exec --all-repos -- git status -sb
```

`daft doctor` also audits the catalog itself — live entries whose paths
vanished, identity drift, duplicate names — and `--fix` reconciles them.

## Removed repos

`daft repo remove` tombstones the catalog entry instead of forgetting it:

```bash
daft repo remove -y ./client       # repo gone; entry marked removed
daft repo remove --repo client -y  # same, addressed by catalog name
daft repo list --all               # …still visible here
daft hooks jobs --repo client      # its hook-job history is still addressable
daft clone client                  # restored from the recorded remote URL
```

Re-cloning (by name or URL) at any path brings the repo back as a fresh live
entry; the old identity remains as a removed record so its logs stay reachable.

To drop the catalog entry while keeping the repository on disk, pass
`--keep-files`: nothing is deleted, no hooks run, and no confirmation is asked.
The interactive prompt offers the same choice as `k` whenever the repo being
removed is cataloged. Because registration is ambient, the entry returns the
next time daft runs inside the kept repo — `--keep-files` is for repos leaving
daft's orbit, and for retiring a stale entry whose directory is already gone
(`daft repo remove --keep-files --repo old-name`; `daft doctor --fix` does the
same for every stale entry at once).

## Storage

One SQLite file at `<data-dir>/catalog/catalog.db` (XDG data directory). It is
an index, not a source of truth: entries for live repos are rebuilt by simply
running daft inside them. The relations manifest is not stored here — it lives
in each repo's committed `daft.yml`.

## Where to next

- [Coordinated changes](/graph/coordinated-changes) — using the catalog and
  relations together
- [Concepts](/graph/concepts) — the model behind these rules
