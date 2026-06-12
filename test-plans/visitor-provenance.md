---
branch: daft-628/fix/visitor-provenance
---

# Visitor daft-file provenance

Seed-gated propagation for untracked daft files (issue #628): pristine copies
never overwrite or block anything; refined copies are consolidated three-way,
refused, or explicitly discarded to a stash. Prune verifies merge status before
deleting.

## Setup

Visitor repo: clone a test remote, commit a `.gitignore` covering `daft.yml` /
`daft.local.yml`, write an untracked `daft.yml` in the main worktree, then
`daft start feat/x` (seeds the copy).

## Removal

- [ ] Evolve main's `daft.yml` after branching; `daft remove -f feat/x` — main's
      file is untouched (no A→B→A revert), worktree gone
- [ ] Same setup, unforced `daft remove feat/x` — succeeds with no refusal
      (pristine-stale copies are frictionless)
- [ ] Refine feat/x's `daft.yml` (add a named job); unforced piped remove —
      refuses, message says "refined daft files", names `daft file merge` and
      `-f` (not `-D`)
- [ ] Same, in a real terminal — consolidation prompt appears with the key
      summary; answering `c` merges the job into main and removes the worktree;
      `d` stashes; `a`/Enter aborts
- [ ] `daft remove -f` on a refined copy — main untouched, file stashed at
      `.git/.daft/discarded/feat/x/daft.yml`, one "Discarded" line printed

## Merge

- [ ] Stale-pristine source: `daft merge feat/x` from main — main's `daft.yml`
      keeps its newer content, no consolidation announce
- [ ] Refined source: merge announces "Consolidated daft.yml … adopted N key(s)"
      and main gains the refinement; subsequent cleanup removal is silent (seed
      refreshed)
- [ ] Conflicting key (both sides changed the same job): piped merge aborts
      BEFORE the git merge with the key listed; real terminal prompts [s/t/A]

## Prune / sync

- [ ] Gone-but-unmerged branch: `daft prune` keeps worktree + branch with a "not
      merged" warning (sequential and TUI); `--force` removes
- [ ] Merged + stale-pristine daft.yml: prunes silently, main untouched
- [ ] Refined daft.yml: kept with end-of-run "Kept N worktree(s) …
      `daft file merge`" summary in both `daft prune` and `daft sync`; `--force`
      stashes and removes

## daft file merge

- [ ] Seeded source: preview lines ("will adopt: …"), backup at
      `.git/.daft/backups/file-merge/daft.yml`, target keeps its own changes and
      gains the source's refinement; source deleted (kept with `--keep-source`)
- [ ] Conflicting key: piped run exits non-zero listing the key; `-y` takes the
      source's values; real terminal prompts [s/t/A]
- [ ] `daft __dump-store visitor-seeds` lists seeds; rows disappear after the
      worktree is removed
