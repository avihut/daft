---
title: Adopting from lefthook
description:
  Try daft as your hooks manager without porting anything, then convert when
  you're sure — and what changes about your gates once you do.
pillars: [hooks]
kind: adoption
---

# Adopting from lefthook

## Starting state

The team has been on lefthook for a year. `lefthook.yml` is committed, everyone
has run `lefthook install`, and the gates work:

```yaml
# lefthook.yml
pre-commit:
  parallel: true
  jobs:
    - name: format
      run: prettier --write {staged_files}
      glob: "*.{ts,tsx,json,md}"
      stage_fixed: true
    - name: lint
      run: eslint {staged_files}
      glob: "*.{ts,tsx}"
    - name: typecheck
      run: tsc --noEmit
      glob: "*.{ts,tsx}"

pre-push:
  jobs:
    - name: test
      run: npm test
```

Nobody is unhappy with it. The reason to look at daft is that the team is
already using it for worktrees, and running two tools with two config files and
two mental models for "what gates my code" is the actual friction.

## Step 1 — let daft run your config, change nothing

```
daft hooks install
```

Daft notices `lefthook.yml`, takes over the hooks directory, and runs your
existing config unchanged. lefthook's hook files are moved aside; nothing is
added to the tracked tree.

```
Installed git hooks for 16 stages in .git/hooks
  · pre-commit moved aside to pre-commit.pre-daft (lefthook)
  · pre-push moved aside to pre-push.pre-daft (lefthook)

Stages run from lefthook.yml — daft is running your existing config as-is.
Nothing was written to the tracked tree.
  Next: daft hooks import converts it into daft.yml when you are ready.
Undo with daft hooks uninstall.
```

Commit something. The gates run, and now they report per job:

```
┌────────────────────────────────────────────┐
│ daft hooks v1.26.0  pre-commit  on: feature │
└────────────────────────────────────────────┘
┃  format ❯
┃  lint ❯
┃  typecheck ❯

────────────────────────────────────────
summary: (done in 2.1s)
  ✔ format (0.3s)
  ✔ lint (1.1s)
  ✔ typecheck (2.1s)
```

**This is fully reversible.** `daft hooks uninstall` puts lefthook's hook files
back byte for byte. Nothing in the repository changed, so there is nothing to
revert in git and nothing for a teammate to notice.

Run this way for a week. Your `LEFTHOOK=0` and `LEFTHOOK_EXCLUDE=job` habits
still work while daft is running a lefthook config.

## Step 2 — see what does not carry over

Install and each run report anything in the file daft will not do. The one that
matters is `remotes:`, which fetches hook definitions from another repository at
run time — daft will not do that, and says so every run rather than quietly
running a weaker gate than your config describes.

If you rely on `remotes:`, stop here: takeover is not faithful for you, and the
honest answer is to keep lefthook or inline the shared definitions first.

`min_version:` is reported and ignored (it constrains lefthook, not daft), and
`extends:` is not followed in takeover mode — though daft's own `extends:` does
work, so importing fixes that one.

## Step 3 — convert

```
daft hooks import --dry-run
daft hooks import
```

Your `daft.yml` gains the same definitions, appended below whatever was already
there. Comments and formatting survive — the import is a textual append that is
re-parsed and compared before it is written, not a serde round-trip.

```yaml
# daft.yml
layout: contained

hooks:
  worktree-post-create:
    jobs:
      - name: install
        run: npm ci

  # Imported from lefthook.yml by `daft hooks import`.
  pre-commit:
    parallel: true
    jobs:
      - name: format
        run: prettier --write {staged_files}
        glob: "*.{ts,tsx,json,md}"
        stage_fixed: true
      # …
```

One config file now describes every boundary the code crosses: worktree
creation, commit, push, merge, teardown.

The import takes effect immediately — a git stage in `daft.yml` wins over
`lefthook.yml` for every stage. `lefthook.yml` is now inert but not deleted;
`git rm lefthook.yml` when the team has agreed.

## Step 4 — use what you now have

Things worth adding once you are on daft's side:

**Gate the merge, not just the commit.** The checks your forge requires on a PR
can run locally before the merge lands:

```yaml
hooks:
  pre-merge:
    jobs:
      - name: test
        run: npm test
      - name: build
        run: npm run build
```

See [Merge gate parity](/recipes/merge-gate-parity) for keeping that list honest
against CI.

**Make CI refuse what it used to fix.** Locally, `stage_fixed: true` putting a
formatter's edits into the commit is convenient. In CI it is a gate that
rewrites the tree and passes — so it never tells you the code was wrong:

```yaml
hooks:
  pre-commit:
    fail_on_changes: true # in the CI overlay only
```

**Stop maintaining two copies of a command.** If the same flags appear in your
gate and your CI workflow, they will drift:

```yaml
templates:
  lint: eslint --max-warnings 0
```

## What your teammates have to do

Each person runs `daft hooks install` once in their clone — the same one-time
step `lefthook install` was. Until they do, their commits are ungated, exactly
as they would be with lefthook uninstalled.

`daft hooks status` says whether a clone is set up, which config its stages come
from, and whether the repository is trusted.

## The one new concept

Daft does not run hooks in a repository you have not trusted. In your own
repository that is one `daft hooks trust` and then never again — but it means a
clone of somebody else's project cannot run code at you on checkout.

An untrusted repository's stages **do not block**: the skip is reported and your
commit proceeds. A gate that refused every commit in an unfamiliar clone would
be an obstacle, not a safeguard.

## Reference

- [Git stages](/hooks/git-stages) — the full stage list, file placeholders, and
  what git passes each hook
- [Migrating from lefthook](/hooks/lefthook-migration) — the complete translates
  / does-not-translate table
