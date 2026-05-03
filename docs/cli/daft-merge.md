---
title: daft merge
description: Merge branches across worktrees (cross-worktree, squash, rebase, cleanup)
---

# daft merge

Merge branches across worktrees, without the "switch, merge, switch back"
dance. `daft merge` is the short verb form of `git worktree-merge`: all flags
and behavior are identical, but invoking through `daft` integrates with daft's
auto-cd and shell wrappers.

## Usage

```
daft merge [OPTIONS] [SOURCE...]              # start a merge
daft merge --abort    [<worktree|branch>]     # abort an in-progress merge
daft merge --continue [<worktree|branch>]     # continue after conflict resolution
daft merge --quit     [<worktree|branch>]     # quit without resetting the index
```

This command is equivalent to `git worktree-merge`. See
[git worktree-merge](./git-worktree-merge.md) for the exhaustive flag
reference; this page focuses on daft-specific behavior, common recipes, and
the surrounding workflow (configuration keys, hooks, related commands).

## Description

Merges one or more source branches into a target worktree's branch. Unlike
`git merge`, which requires you to `git switch` to the target branch first,
`daft merge` can operate on any worktree:

- **No `--into`:** target is the current worktree's branch (mirrors `git merge`).
- **`--into <target>`:** target is another worktree, named by branch name,
  worktree path, or relative path. The working directory doesn't change, and
  the command returns you to it once the merge finishes.
- **Multiple sources:** triggers git's octopus strategy; the command announces
  this explicitly so there are no surprises.
- **Ephemeral target worktree:** when the target branch exists but has no
  worktree, daft can spin up a temporary worktree just for the merge, then
  promote it to a permanent worktree on success.

On conflict, daft does not switch your shell into the target worktree. It
reports the conflicted files and the exact `--continue` / `--abort` commands
to resolve the merge, so you stay where you are and decide how to proceed.
This is the **report-and-stay** policy: a conflict never hijacks your
working directory.

## Key Options

The full flag surface mirrors `git merge` and is documented in
[git worktree-merge](./git-worktree-merge.md). The flags below are the ones
that are unique to daft merge or that shape the cross-worktree workflow.

| Option                    | Description                                                                                                                                                        |
| ------------------------- | ------------------------------------------------------------------------------------------------------------------------------------------------------------------ |
| `--into <TARGET>`         | Target worktree/branch; omit to merge into the current worktree.                                                                                                   |
| `--abort`                 | Abort an in-progress merge (or squash-staged state) in the named worktree (default: CWD).                                                                          |
| `--continue`              | Continue after resolving conflicts, or resume a squash-staged commit.                                                                                              |
| `--quit`                  | Quit a merge without resetting the index.                                                                                                                          |
| `--merge`                 | Merge style: always create a merge commit (the default). Use to cancel a config-set style.                                                                         |
| `--squash`                | Squash style: collapse source commits into one commit on the target. Editor opens for the message; use `--no-edit` or `-m` to skip it.                             |
| `--rebase`                | Rebase style: rebase source onto target, then fast-forward. Produces linear history.                                                                               |
| `--rebase-merge`          | Rebase-merge style: rebase source onto target, then create a merge commit.                                                                                         |
| `-r, --remove-branch`     | Remove the source worktree **and** delete the source branch after a successful merge. Local/remote behavior follows `branch.deleteRemote` (default: local-only).   |
| `--keep-branch`           | Explicit keep — cancels a config-set `merge.cleanup = remove-branch`.                                                                                              |
| `--set-default`           | Write the resolved style/cleanup choices to `git config --local` after the merge succeeds.                                                                         |
| `--adopt-target`          | When the target has no worktree, create an ephemeral worktree and run the merge there — no prompt.                                                                  |
| `--no-adopt-target`       | Refuse instead of prompting when the target has no worktree.                                                                                                       |
| `-y, --yes`               | Auto-accept interactive prompts; implies `--adopt-target` unless overridden.                                                                                       |
| `--no-commit`             | After `--squash`, stage the changes without creating a commit. Incompatible with `-r`.                                                                             |
| `-s, --strategy <STRAT>`  | Merge strategy (`recursive`, `ours`, `octopus`, etc.).                                                                                                             |
| `-X, --strategy-option`   | Strategy-specific option (repeatable).                                                                                                                             |

## Examples

### Basic merge from the current worktree

```bash
# Merge feature/api into the current worktree's branch
daft merge feature/api
```

Equivalent to `git merge feature/api`, but hooks fire (`pre-merge`,
`post-merge`) and progress uses daft's output style.

### Cross-worktree merge

```bash
# From any worktree: merge feature/api into the `main` worktree
daft merge feature/api --into main
```

No `cd` required. Your shell stays in the current worktree throughout.

### Octopus merge

```bash
# Merge three feature branches into main in one commit
daft merge feature/a feature/b feature/c --into main
```

daft announces the octopus strategy before running it. Any conflict aborts
the whole merge (octopus merges don't allow resolving conflicts mid-flight).

### Squash merge

```bash
# Squash feature/api's commits into one commit on the current branch.
# An editor opens pre-populated with a "Squashed commit of the following:"
# message; save and close to create the commit.
daft merge --squash feature/api

# Skip the editor and use the auto-generated message verbatim:
daft merge --squash --no-edit feature/api

# Supply an explicit message (no editor):
daft merge --squash -m "feat: squash feature/api" feature/api

# Opt out of the automatic commit — stage only, commit by hand:
daft merge --squash --no-commit feature/api
```

By default `--squash` creates a real commit on the target after staging the
squashed changes. The editor opens pre-populated from `.git/SQUASH_MSG` so
you can review and adjust the message before committing. Pass `--no-edit` or
`-m <msg>` to skip the editor. Pass `--no-commit` to restore git's historical
"stage only" behavior (incompatible with `-r`; see [Cleanup](#cleanup-r)).

When no TTY is available (e.g. piped in CI), daft refuses to open an editor
and exits with a clear hint to pass `--no-edit` or `-m`.

### Rebase merge (linear history)

```bash
# Rebase feature/api onto the current branch, then fast-forward.
# HEAD ends up with 1 parent — no merge commit.
daft merge --rebase feature/api

# Rebase onto another branch:
daft merge --rebase feature/api --into main
```

The source branch is rebased onto the target, then the target fast-forwards
to the rebased tip. This produces linear history equivalent to
`git rebase && git merge --ff-only`. Conflicts stop the rebase mid-flight;
resolve them, `git rebase --continue`, then re-run without `--rebase` to
finish.

### Rebase-merge (rebase + merge commit)

```bash
# Rebase feature/api onto the current branch, then create a merge commit.
# HEAD ends up with 2 parents.
daft merge --rebase-merge feature/api
```

Like `--rebase` but appends a merge commit on top, preserving the rebase in
the reflog while still recording an explicit merge in history.

### Abort a conflicted merge or squash-staged state

```bash
# In the worktree where the merge is in progress:
daft merge --abort

# Or from anywhere, naming the worktree/branch:
daft merge --abort main
```

`--abort` handles two in-progress states:

- **Regular merge conflict** — runs `git merge --abort`; restores the index
  and working tree to the pre-merge state.
- **Squash staged, commit pending** — runs `git reset --merge`; resets the
  index to HEAD, discards `SQUASH_MSG`. This state arises when the commit
  editor was closed without saving, or when `--squash --no-commit` was used.

### Continue after resolving conflicts or resume a squash commit

```bash
# After resolving conflict files, `git add` them, then:
daft merge --continue

# Or from anywhere:
daft merge --continue main

# Resume a squash commit with a specific message (skip the editor):
daft merge --continue --no-edit main
daft merge --continue -m "feat: squash feature" main
```

`--continue` also handles two in-progress states:

- **Regular merge conflict** — runs `git merge --continue`; creates the merge
  commit once all conflicts are resolved.
- **Squash staged, commit pending** — re-opens the editor on the preserved
  `SQUASH_MSG` (same as running `git commit`). Pass `--no-edit`, `-m`, or
  `-F` on the `--continue` invocation to skip the editor. If cleanup was
  originally requested (`-r`), it runs after the commit succeeds.

### Cleanup after a successful merge {#cleanup-r}

```bash
# Merge and remove the source worktree + branch afterwards
daft merge feature/done --into main -r

# Squash + commit + full cleanup in one step (editor opens for message)
daft merge feature/done --into main --squash -r

# Same, but skip the editor (auto-generated message)
daft merge feature/done --into main --squash --no-edit -r

# Set -r as your default for future merges in this repo
daft merge feature/done --into main -r --set-default
```

`-r` / `--remove-branch` removes **both** the source worktree and the source
branch. For regular and rebase-style merges, daft uses `git branch -d` (safe)
semantics — it refuses to delete a branch that isn't fully merged into the
target. For squash merges, daft uses `branch -D` because the squash commit
captures the source's content; git's reachability check would always refuse
a squash-only branch. Before force-deleting, daft re-checks that the source
branch tip hasn't moved since the merge started. If it has (e.g. a concurrent
push happened during the editor session), cleanup is refused and a recovery
hint is shown; the squash commit already landed on the target.

`--no-commit` is incompatible with `-r` because cleanup requires a commit.

### Persist style and cleanup defaults with --set-default

```bash
# Run a squash + remove-branch merge and save those choices as repo defaults
daft merge feature/api -r --squash --set-default

# Now future merges in this repo default to squash + remove-branch
daft merge feature/next
```

`--set-default` writes `daft.merge.style` and `daft.merge.cleanup` to
`git config --local`. The config keys are only written after a successful
merge, so a failed or conflicted merge never changes your defaults.

### Ephemeral target worktree

```bash
# Merge into a branch that has no worktree — spin up a temporary one
daft merge feature/hotfix --into release/1.2 --adopt-target

# Auto-accept all prompts (useful in scripts/CI)
daft merge feature/hotfix --into release/1.2 -y
```

On success the ephemeral worktree is promoted to a permanent worktree; on
conflict it stays behind for you to resolve.

## Configuration

`daft.merge.*` config keys let you set defaults for frequently used flags so
you don't have to pass them every time. The most relevant keys:

| Key                                   | Values                                      | Effect                                                                              |
| ------------------------------------- | ------------------------------------------- | ----------------------------------------------------------------------------------- |
| `daft.merge.style`                    | `merge` (default), `squash`, `rebase`, `rebase-merge` | Default merge style. Overridden by `--merge`, `--squash`, `--rebase`, `--rebase-merge`. |
| `daft.merge.cleanup`                  | `keep` (default), `remove-branch`           | Default cleanup behavior. Overridden by `-r` / `--keep-branch`.                    |
| `daft.merge.edit`                     | `true`, `false`                             | Default message-edit behavior on a TTY.                                             |
| `daft.merge.commit`                   | `true`, `false`                             | Default commit-after-squash behavior.                                               |
| `daft.merge.signoff`                  | `true`, `false`                             | Default signoff behavior.                                                           |
| `daft.merge.gpgSign`                  | `true`, `false`, `<keyid>`                  | Default GPG-sign behavior.                                                          |
| `daft.merge.verifySignatures`         | `true`, `false`                             | Default signature verification.                                                     |
| `daft.merge.allowUnrelatedHistories`  | `true`, `false`                             | Default for merges across unrelated histories.                                      |
| `daft.merge.strategy`                 | strategy name                               | Default merge strategy.                                                             |
| `daft.merge.strategyOption`           | option string                               | Default strategy options (repeatable).                                              |
| `daft.merge.adoptTargetOnDemand`      | `prompt` (default), `yes`, `no`             | How to handle target worktree adoption when no worktree exists.                     |
| `daft.merge.requireCleanTarget`       | `true` (default), `false`                   | Refuse to merge when the target worktree has uncommitted changes.                   |

All keys can be set locally, globally, or system-wide through `git config`;
flag arguments always override config defaults. The easiest way to persist
your choices is to pass `--set-default` on a merge and let daft write them
for you. See the [configuration guide](../guide/configuration.md) for
precedence details.

### Migration from the old flag set

If you have scripts or habits using the v1.9 flag names, here is the mapping:

| Old (v1.9)             | New (v1.10+)            | Notes                                                           |
| ---------------------- | ----------------------- | --------------------------------------------------------------- |
| (default, no flag)     | `--merge`               | Old default was FF-when-possible; new default is always-merge-commit. |
| `--no-ff`              | `--merge`               | Explicit `--no-ff` is now the default behavior.                 |
| `--ff` / `--ff-only`   | `--rebase`              | Use `--rebase` for linear (fast-forward) history.               |
| `--squash`             | `--squash`              | Unchanged; now auto-commits by default (use `--no-commit` to opt out). |
| `-r`                   | _(removed)_             | Worktree-only removal is no longer a first-class operation.     |
| `-rb`                  | `-r`                    | New `-r` removes both worktree and branch.                      |
| `daft.merge.ff`        | `daft.merge.style`      | Set to `merge`, `squash`, `rebase`, or `rebase-merge`.          |
| `daft.merge.postMerge.removeSourceWorktree` + `daft.merge.postMerge.alsoRemoveSourceBranch` | `daft.merge.cleanup` | Set to `keep` or `remove-branch`. |

## Hooks

`daft merge` fires two lifecycle hooks, letting `daft.yml` gate merges on
custom preconditions and react to outcomes without wrapping the command:

- **`pre-merge`** — runs after pre-flight safety checks, before the merge
  executes. Non-zero exit aborts the merge (default fail mode: `abort`).
- **`post-merge`** — runs after the merge completes, regardless of success,
  conflict, or "already up to date". Non-zero exit is logged as a warning
  (default fail mode: `warn`); it never rolls the merge back.

Both hooks receive `DAFT_MERGE_*` environment variables describing the
sources, target, mode (`merge` / `ff` / `squash` / `octopus`), strategy, and
cross-worktree flag. `post-merge` additionally gets `RESULT`, `COMMIT_SHA`,
`CONFLICTED_FILES`, and `PROMOTED_FROM_EPHEMERAL`. Neither hook fires when
the merge is a pure no-op.

See the [hooks guide](../guide/hooks.md) for full env-var reference and
configuration.

## Output

By default, `daft merge` suppresses git's raw stdout on the success path.
Styled step lines render in its place:

| Outcome                       | Step line                                               |
| ----------------------------- | ------------------------------------------------------- |
| Fast-forward                  | `Fast-forwarded X to abc1234`                           |
| Regular merge commit          | `Merged X into Y (commit abc1234)`                      |
| Squash commit                 | `Squashed X into Y (commit abc1234)`                    |
| Squash staged, no commit yet  | `Squash staged on Y`                                    |
| Already up to date            | `Already up to date.` (emitted directly, no styled box) |

When cleanup runs (`-r`/`-rb`) and succeeds, a summary line follows:

```
Squash merged and cleaned up X.
```

### Verbose mode

Pass `--verbose` to dump git's full output to stderr alongside the styled step
lines. Useful for diagnosing unexpected merge behavior.

## Cleanup hooks

When `-r` is passed and the merge succeeds, daft removes the source worktree
and its branch by delegating to the same cleanup path as `daft remove`. As
part of that cleanup, `worktree-pre-remove` and `worktree-post-remove` hooks
fire for each source worktree that is removed.

The hooks receive the standard removal env vars (`DAFT_WORKTREE_PATH`,
`DAFT_BRANCH_NAME`, `DAFT_REMOVAL_REASON=manual`) plus `DAFT_COMMAND=merge`.
Scripts can branch on `DAFT_COMMAND` to distinguish merge cleanup from a
standalone `daft remove` invocation.

Example: revoke direnv trust only during standalone removes, not merge cleanup:

```bash
#!/bin/sh
# .daft/hooks/worktree-pre-remove
if [ "$DAFT_COMMAND" != "merge" ]; then
    direnv revoke "$DAFT_WORKTREE_PATH"
fi
```

**Limitation:** When cleanup is resumed via `daft merge --continue` after a
squash-staged abort, `worktree-pre-remove` and `worktree-post-remove` hooks are
NOT fired and the output reverts to plain text. This affects only the
`--continue` resume path; cleanup triggered directly by `-r` fires hooks as
normal. This limitation will be addressed in a future release.

## See Also

- [git worktree-merge](./git-worktree-merge.md) — exhaustive flag reference
- [daft list](./daft-list.md) — inspect worktrees (including in-progress merges)
- [daft carry](./daft-carry.md) — transfer uncommitted changes between worktrees
- [daft sync](./daft-sync.md) — rebase + push many worktrees at once
- [daft adopt](./daft-adopt.md) — convert a traditional repo into daft's layout
