---
title: daft merge
description: Merge branches across worktrees (cross-worktree, octopus, squash, cleanup)
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

| Option                  | Description                                                                                                                                 |
| ----------------------- | ------------------------------------------------------------------------------------------------------------------------------------------- |
| `--into <TARGET>`       | Target worktree/branch; omit to merge into the current worktree.                                                                            |
| `--abort`               | Abort an in-progress merge (or squash-staged state) in the named worktree (default: CWD).                                                   |
| `--continue`            | Continue after resolving conflicts, or resume a squash-staged commit.                                                                       |
| `--quit`                | Quit a merge without resetting the index.                                                                                                   |
| `--adopt-target`        | When the target has no worktree, create an ephemeral worktree and run the merge there — no prompt.                                          |
| `--no-adopt-target`     | Refuse instead of prompting when the target has no worktree.                                                                                |
| `-y, --yes`             | Auto-accept interactive prompts; implies `--adopt-target` unless overridden.                                                                |
| `-r, --remove`          | Remove the source worktree after a successful merge.                                                                                        |
| `-b, --and-branch`      | Also delete the source branch (requires `-r`). Regular merges use `git branch -d` safety semantics; squash + commit uses `branch -D` (see [Cleanup](#cleanup-r-and--rb)). |
| `--squash`              | Squash all source commits into a single commit on the target. The commit is created automatically by default (editor opens for the message); use `--no-commit` to stage without committing. |
| `--no-commit`           | After `--squash`, stage the changes without creating a commit. Incompatible with `-r`/`-rb`.                                                |
| `-s, --strategy <STRAT>`| Merge strategy (`recursive`, `ours`, `octopus`, etc.).                                                                                      |
| `-X, --strategy-option` | Strategy-specific option (repeatable).                                                                                                      |

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
"stage only" behavior (incompatible with `-r`/`-rb`; see [Cleanup](#cleanup-r-and--rb)).

When no TTY is available (e.g. piped in CI), daft refuses to open an editor
and exits with a clear hint to pass `--no-edit` or `-m`.

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
  originally requested (`-r`/`-rb`), it runs after the commit succeeds.

### Cleanup after a successful merge {#cleanup-r-and--rb}

```bash
# Merge and delete the source worktree afterwards
daft merge feature/done --into main -r

# Also delete the source branch (safe -d semantics for regular merges)
daft merge feature/done --into main -rb

# Squash + commit + full cleanup in one step (editor opens for message)
daft merge feature/done --into main --squash -rb

# Same, but skip the editor (auto-generated message)
daft merge feature/done --into main --squash --no-edit -rb
```

`-b` requires `-r`. For regular merges, `-b` uses `git branch -d` (safe)
semantics — it refuses to delete a branch that isn't fully merged into the
target. For squash merges, daft uses `branch -D` because the squash commit
captures the source's content; git's reachability check would always refuse
a squash-only branch. Before force-deleting, daft re-checks that the source
branch tip hasn't moved since the merge started. If it has (e.g. a concurrent
push happened during the editor session), cleanup is refused and a recovery
hint is shown; the squash commit already landed on the target.

`--no-commit` is incompatible with `-r`/`-rb` because cleanup requires a commit.

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

| Key                                            | Effect                                                                                                   |
| ---------------------------------------------- | -------------------------------------------------------------------------------------------------------- |
| `daft.merge.ff`                                | Default fast-forward mode (`true`, `false`, or `only`).                                                  |
| `daft.merge.squash`                            | Default squash behavior.                                                                                 |
| `daft.merge.commit`                            | Default commit-after-merge behavior.                                                                     |
| `daft.merge.edit`                              | Default message-edit behavior on a TTY.                                                                  |
| `daft.merge.signoff`                           | Default signoff behavior.                                                                                |
| `daft.merge.gpgSign`                           | Default GPG-sign behavior (`true`, `false`, or `<keyid>`).                                               |
| `daft.merge.verifySignatures`                  | Default signature verification.                                                                          |
| `daft.merge.allowUnrelatedHistories`           | Default for merges across unrelated histories.                                                           |
| `daft.merge.strategy`                          | Default merge strategy.                                                                                  |
| `daft.merge.strategyOption`                    | Default strategy options (repeatable).                                                                   |
| `daft.merge.adoptTargetOnDemand`               | How to handle target worktree adoption: `prompt` (default), `yes`, or `no`.                              |
| `daft.merge.requireCleanTarget`                | Refuse to merge when the target worktree has uncommitted changes (default: `true`).                      |
| `daft.merge.postMerge.removeSourceWorktree`    | Default for `-r`: remove the source worktree on success.                                                 |
| `daft.merge.postMerge.alsoRemoveSourceBranch`  | Default for `-b`: also delete the source branch (requires removal).                                      |

All keys can be set locally, globally, or system-wide through `git config`;
flag arguments always override config defaults. See the
[configuration guide](../guide/configuration.md) for precedence details.

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

When `-r` or `-rb` is passed and the merge succeeds, daft removes the source
worktree (and optionally its branch) by delegating to the same cleanup path as
`daft remove`. As part of that cleanup, `worktree-pre-remove` and
`worktree-post-remove` hooks fire for each source worktree that is removed.

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
`--continue` resume path; cleanup triggered directly by `-r`/`-rb` fires hooks
as normal. This limitation will be addressed in a future release.

## See Also

- [git worktree-merge](./git-worktree-merge.md) — exhaustive flag reference
- [daft list](./daft-list.md) — inspect worktrees (including in-progress merges)
- [daft carry](./daft-carry.md) — transfer uncommitted changes between worktrees
- [daft sync](./daft-sync.md) — rebase + push many worktrees at once
- [daft adopt](./daft-adopt.md) — convert a traditional repo into daft's layout
