---
title: daft merge — squash + cleanup refinement
date: 2026-04-25
status: design
supersedes:
  - 2026-04-24-daft-merge-design.md (sections "Squash merge" and "Cleanup flags
    `-r` / `-rb`")
---

# daft merge — squash + cleanup design

## Summary

`daft merge --squash` becomes commit-by-default with editor prepopulation,
matching how a regular `daft merge` and `git commit` already behave. The
combination `--squash -rb` becomes a fully supported cleanup flow with
transactional ordering and a justified force-delete, and is settable as a config
default. `--abort` / `--continue` learn to recognize the new
"squash-staged-but-not-committed" intermediate state. Cleanup is reordered to be
transactional — never partial.

This refinement supersedes the corresponding sections of the
[2026-04-24 daft merge design](2026-04-24-daft-merge-design.md). All other parts
of that spec stand.

## Motivation

The original design inherited git's historical wart that `git merge --squash`
stages but does not commit, and combined that with opt-in cleanup (`-r` /
`-rb`). The interaction is broken in two ways that surfaced immediately in real
use:

1. **`--squash` alone reports "Merge complete." after `git merge --squash`, even
   though no commit was made.** The result is a staged-but-uncommitted working
   tree on the target with a daft-printed success line that doesn't match
   reality.

2. **`--squash -rb` cannot complete safely under the current rules.** Even if
   the user manually commits the squash, git's safe `branch -d` will refuse to
   delete the source because the source's commits aren't reachable from the
   target. The original design accepted this and surfaced git's "not fully
   merged" error, but the cleanup ran in worktree-first / branch-second order,
   so a failure halfway through left the source worktree gone, the branch
   present, and the target dirty with no commit. Partial state with no clear
   recovery path.

The user wants `--squash -rb` to be a real, supported flow — including as a
configured default. The fix needs to:

- give `--squash` honest semantics that match the verb,
- make cleanup either complete or not happen at all, and
- give daft a sound basis for force-deleting the source after a squash that daft
  itself just committed.

## Behavior changes

### `--squash` always commits by default

When `daft merge <source> --squash` runs, daft now invokes `git commit` after
`git merge --squash` succeeds. Git auto-populates `.git/SQUASH_MSG` from the
squash, so the editor opens prepopulated with the canonical "Squashed commit of
the following:" message — same UX as `git commit` after a manual
`git merge --squash`.

Flag pass-throughs that affect commit composition are honored on this commit
step as well: `-m <msg>` / `-F <file>` skip the editor and use the supplied
message; `--no-edit` uses `SQUASH_MSG` verbatim; `--signoff` and `--gpg-sign`
flow through to the commit. `daft.merge.edit` already controls editor behavior
for regular merges; its semantics extend to the squash commit.

### Escape hatch: `--no-commit` / `daft.merge.commit = false`

Users who want git's historical "stage and stop" behavior — for the rare "squash
several things then craft one commit by hand" workflow — opt out explicitly:

- `--squash --no-commit` flag pair stages without committing.
- `daft.merge.commit = false` config makes that the default.

In either case daft prints a squash-aware status line and exits cleanly:

> `Squash staged on <target>. Commit when ready (e.g. \`git commit\`).`

### `--squash -rb` is a fully supported flow

With cleanup requested (`-r` or `-rb`), daft requires a commit (because
`branch -d`/`-D` operate on refs that exist independently of any staged state).
The interaction rules:

- `--squash --no-commit -r` and `--squash --no-commit -rb` are **rejected at
  parse time** with: "`--no-commit` is incompatible with `-r`/`-rb`; cleanup
  requires a commit." The user is explicitly opting out of the commit that
  cleanup needs.
- `--squash` defaults to commit, so `--squash -r` and `--squash -rb` "just work"
  — they squash, commit (editor or message-flag), then clean up.

### Settable as default

These three keys together make squash + cleanup the user's default `merge` verb:

```
daft.merge.squash = true
daft.merge.postMerge.removeSourceWorktree = true
daft.merge.postMerge.alsoRemoveSourceBranch = true
```

For non-interactive / CI use, add `daft.merge.edit = false` so the auto-
generated `SQUASH_MSG` is used verbatim without opening an editor.

### Cleanup is transactional

Cleanup never leaves partial state. Order:

1. Pre-validate every step that will mutate state:
   - source worktree removable (no unmergeable changes; the existing
     `requireCleanTarget` check already covered the _target_; this extends the
     same idea to the _source_ during cleanup);
   - source branch deletable under the rules of the current path (regular merge:
     `branch -d` would succeed; squash + commit just happened: `branch -D` is
     justified — see below).
2. If any pre-validation fails: error out before mutating anything, with a
   message naming the specific failure and a recovery hint.
3. If pre-validation passes: remove worktree, then delete branch. Each step
   prints a progress line first.

Worktree removal is no longer "do it and continue if it fails" — it's part of
the transactional plan. Either the cleanup plan succeeds end-to-end, or nothing
is mutated.

### `branch -D` is justified after a daft-driven squash + commit

The unsafety of `branch -D` is "it might silently lose unmerged work." In the
`--squash -rb` (or `--squash -r` plus an explicit later `-b`) path, daft has
direct first-party evidence of content equivalence:

1. **Source SHA captured** before any merge work begins.
2. **Squash + commit** lands on target, capturing the exact tree state of the
   source as of the captured SHA.
3. **Stability check** before cleanup: re-resolve the source ref. If the tip
   moved between step 1 and now (someone pushed; the user rebased the branch in
   another worktree during the editor session), abort cleanup with: "source
   `<branch>` moved during merge; refusing to delete to avoid losing work.
   Re-run cleanup manually if you've reconciled."
4. If the SHAs match, daft has proof that the squash captured everything
   currently on the source branch. The "unmerged work" `branch -D` would warn
   about is precisely the _separate commit history_ that the user explicitly
   chose to discard via `--squash`.

This is the precise scenario where `-D`'s safety check should be overridden:
daft has the equivalence proof that git's reachability heuristic lacks.

### `--abort` and `--continue` recognize squash-staged state

A new in-progress state is possible: `git merge --squash` succeeded, the commit
step is pending or was aborted from the editor. Detection: `SQUASH_MSG` exists,
`MERGE_HEAD` does **not** exist (regular merges set both; squash sets only
`SQUASH_MSG`), the index has staged changes.

- **`daft merge --abort`** in this state runs `git reset --merge` (resets the
  index to HEAD and discards `SQUASH_MSG`). No `MERGE_HEAD` to clear.
- **`daft merge --continue`** in this state re-opens the editor on the preserved
  `SQUASH_MSG` (effectively `git commit` with no `-m`/`--no-edit`). If the user
  supplies `-m`/`--no-edit`/`-F` on this `--continue` invocation, those win. If
  cleanup was originally requested (`-r`/`-rb`), the continuation runs cleanup
  after the commit succeeds — the in-progress state needs to record the original
  cleanup intent (see "Implementation notes" below).

`--quit` is unchanged: it discards in-progress state without running git
operations.

### Honest messaging

Replace the unconditional "Merge complete." with state-aware lines:

| State                                        | Line                                                                                                 |
| -------------------------------------------- | ---------------------------------------------------------------------------------------------------- |
| Regular merge succeeded                      | `Merge complete.`                                                                                    |
| FF only, ref advanced                        | `Fast-forwarded <target> to <sha>.`                                                                  |
| Squash + commit succeeded, no cleanup        | `Squash merged <source> into <target> as <sha>.`                                                     |
| Squash + commit + cleanup succeeded          | `Squash merged and cleaned up <source>.`                                                             |
| `--squash --no-commit` (or `commit = false`) | `Squash staged on <target>. Commit when ready.`                                                      |
| Editor aborted on squash commit              | `Commit aborted; squash changes are still staged on <target>. Cleanup skipped.` (plus recovery hint) |
| Conflict                                     | (existing conflict messaging)                                                                        |
| Already up to date                           | (existing AUTD messaging)                                                                            |

The "Merge complete." line is reserved for the path where a real merge commit
was created with `git merge` (no `--squash`).

### Progress on slow cleanup operations

Before each potentially-slow cleanup step, daft prints what it's about to do:

```
Removing worktree at /path/to/source...
Deleting branch source-branch...
```

Cleanup ops on real repositories with build artifacts can take seconds. Silence
is misleading.

### TTY guard

Any path that would open an editor (regular merge with `--edit`, squash commit
without `--no-edit`/`-m`/`-F`) checks for a TTY first. If stdin or stdout is not
a TTY and no message-supplying flag is set:

> No TTY available for the commit-message editor. Pass `--no-edit` to use the
> auto-generated message, `-m <msg>` for an explicit message, or `-F <file>` to
> read from a file.

Exit non-zero before any merge work runs. `-y` implies `--no-edit` for the
squash-commit step (consistent with `-y`'s "auto-accept prompts" semantics).

### post-merge `aborted` outcome

`DAFT_MERGE_RESULT` gains a fourth value: `aborted`. It fires when the squash
commit step is aborted (empty editor message, pre-commit hook fail, GPG-sign
fail, etc.). The `pre-merge` hook still fires before the squash runs — keeping
the pre/post pairing invariant — and `post-merge` fires with `RESULT=aborted`,
`COMMIT_SHA` empty. Cleanup is skipped.

Existing values continue to mean what they meant: `success`, `conflict`,
`already-up-to-date`.

## Safety reasoning summary

The conservative answer to "can we force-delete a squash-merged source" was "no,
git can't tell." Daft's answer is "we can tell, when it was _us_ that just made
the squash commit." The chain of evidence:

1. Daft captured `<source>`'s tip SHA at merge start.
2. Daft ran `git merge --squash <source>`, which staged exactly that tree.
3. Daft ran `git commit`, which landed that tree as a commit on `<target>`.
4. Before cleanup, daft re-checks `<source>`'s tip — if it equals the captured
   SHA, the squash captured everything currently on the branch.
5. Therefore force-deleting `<source>` loses no work that wasn't explicitly
   discarded by `--squash` itself.

If step 4 fails (branch tip moved during the editor session), daft refuses
cleanup. This narrows the unsafe-`-D` window to exactly the cases where daft has
both proof of equivalence and proof of stability.

## Hook interactions

- **`pre-merge`** fires unchanged — after pre-flight checks, before any merge
  operation. Failure aborts the merge.
- **`post-merge`** fires after every `pre-merge`, with one of:
  - `RESULT=success` after a real commit landed (regular merge or squash +
    commit)
  - `RESULT=conflict` after an unfinished merge with conflicts
  - `RESULT=already-up-to-date` (no merge work performed)
  - `RESULT=aborted` (new) when the commit step was aborted (editor empty,
    pre-commit hook fail, GPG-sign fail). `COMMIT_SHA` is empty in this case.

`post-merge` failures continue to log warnings without rolling back the merge —
same as before.

## Configuration interactions

| Key                                           | Behavior                                                                  |
| --------------------------------------------- | ------------------------------------------------------------------------- |
| `daft.merge.squash`                           | Default for `--squash`                                                    |
| `daft.merge.commit`                           | Default for `--commit`/`--no-commit`. With `false` and squash: stage only |
| `daft.merge.edit`                             | Default for `--edit`/`--no-edit`. Applies to the squash-commit step too   |
| `daft.merge.postMerge.removeSourceWorktree`   | Default for `-r`                                                          |
| `daft.merge.postMerge.alsoRemoveSourceBranch` | Default for `-b` (still requires effective `-r`)                          |

A user can set all three of `daft.merge.squash`, `…removeSourceWorktree`, and
`…alsoRemoveSourceBranch` to `true` and have `daft merge <source>` mean "squash,
commit, clean up." Add `daft.merge.edit = false` for non-interactive use. With
`daft.merge.commit = false` and `…alsoRemoveSourceBranch = true` set together,
daft will refuse to start — the combination is contradictory.

## Implementation notes

(High-level — exact mechanics belong in the implementation plan.)

- The merge-state file (`MERGE_HEAD` / `SQUASH_MSG` and any daft-specific
  marker) needs to record the **original cleanup intent** so `--continue` can
  resume the cleanup phase after a re-opened editor commit. Either daft writes
  its own marker file alongside `SQUASH_MSG`, or the in-progress state is
  rebuilt from clap args on the resume invocation. Plan-time decision.
- Source SHA capture must happen before `pre-merge` hook fires — the SHA becomes
  part of the hook env (`DAFT_MERGE_SOURCE_SHA`?) and is preserved for the
  stability check. (Optional — the env var is a nice-to-have; the capture itself
  is required.)
- TTY detection uses the existing `decide_adopt`-style `is_tty` helper for
  consistency.
- Progress prints go through the existing CLI presenter so `--quiet` /
  formatting flags work uniformly.

## Edge cases

| Case                                                           | Behavior                                                                                     |
| -------------------------------------------------------------- | -------------------------------------------------------------------------------------------- |
| `--squash` with octopus (multi-source)                         | Refused by git itself; surface git's error verbatim (existing)                               |
| `--squash --commit`                                            | Redundant under new defaults; pass through; git accepts                                      |
| Pre-commit hook fails during squash commit                     | Treat as editor-aborted: leave staged, skip cleanup, fire `post-merge` with `RESULT=aborted` |
| GPG-sign fail during squash commit                             | Same as above                                                                                |
| `SQUASH_MSG` already present from earlier attempt              | `git merge --squash` overwrites it; non-issue                                                |
| Source branch tip moved during editor session                  | Refuse cleanup with stability-check error; commit stays on target                            |
| `--abort` on squash-staged state                               | `git reset --merge`; clear in-progress marker                                                |
| `--continue` on squash-staged state                            | Re-open editor (or honor `-m`/`--no-edit`/`-F` from `--continue`); on success run cleanup    |
| `--squash` from a non-TTY without `--no-edit`/`-m`             | Refuse before merging                                                                        |
| `daft.merge.commit = false` + `…alsoRemoveSourceBranch = true` | Refuse to start: contradictory configuration                                                 |

## Non-goals

- **Squash-reachability detection for non-daft commits.** If the user performed
  a squash earlier in another tool and then wants `daft merge --abort` or `-rb`
  to recognize that history, it won't. The safety reasoning here only covers
  commits daft itself just made.
- **Auto-rolling back the squash commit on cleanup pre-validation failure.** The
  commit stays on target; only the cleanup is skipped. This preserves the user's
  reviewable commit and matches how the rest of daft's "merge result is never
  rolled back" rule works.
- **Scriptable retry of the editor abort path.** If the user aborts the editor,
  they recover manually (`git commit` then `daft prune` / `git branch -D`, or
  `git reset --merge`). daft does not invent a retry command beyond the existing
  `--continue`.

## Test changes (high-level — detailed in the plan)

- **Flip** `tests/manual/scenarios/merge/squash.yml` to assert that
  `--squash --no-edit` produces a real commit with the auto-generated message;
  add explicit assertions that `MERGE_HEAD` is gone and the staged set is empty
  after the commit.
- **Add** `squash-no-commit.yml` for the explicit stage-only opt-out.
- **Replace** `remove-unmerged-branch.yml` (which ratifies the broken
  partial-cleanup behavior). Split into:
  - `squash-rb.yml` — happy path: `--squash -rb` with `--no-edit` succeeds end
    to end; worktree gone, branch gone, commit on target.
  - `squash-rb-source-moved.yml` — source tip moves during merge → cleanup
    refused with stability-check error; commit stays on target.
  - `squash-no-commit-rb-refused.yml` — `--squash --no-commit -rb` rejected at
    parse time.
- **Add** `squash-no-tty.yml` — `--squash` from a non-TTY without
  `--no-edit`/`-m` refuses before merging.
- **Add** `squash-edit-aborted.yml` — empty editor message → squash staged,
  cleanup skipped, `post-merge` fires with `RESULT=aborted`.
- **Add** `cleanup-prevalidates.yml` — generic cleanup pre-validation refuses to
  touch state when any cleanup step would fail.
- **Add** `abort-squash-staged.yml`, `continue-squash-staged.yml` — finish
  commands work on the new in-progress state.
- **Add** unit tests for `execute_cleanup`:
  - validates before mutating
  - never partial state on failure
  - branch tip stability check
  - branch -D vs -d selection per path
- **Update** existing scenarios that match on "Merge complete." to expect the
  new state-aware lines where applicable.
