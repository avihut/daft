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

| Option                  | Description                                                                                         |
| ----------------------- | --------------------------------------------------------------------------------------------------- |
| `--into <TARGET>`       | Target worktree/branch; omit to merge into the current worktree.                                    |
| `--abort`               | Abort an in-progress merge in the named worktree (default: CWD).                                    |
| `--continue`            | Continue a conflicted merge after you resolve conflicts.                                            |
| `--quit`                | Quit a merge without resetting the index.                                                           |
| `--adopt-target`        | When the target has no worktree, create an ephemeral worktree and run the merge there — no prompt. |
| `--no-adopt-target`     | Refuse instead of prompting when the target has no worktree.                                        |
| `-y, --yes`             | Auto-accept interactive prompts; implies `--adopt-target` unless overridden.                        |
| `-r, --remove`          | Remove the source worktree after a successful merge.                                                |
| `-b, --and-branch`      | Also delete the source branch (requires `-r`). Uses `git branch -d` safety semantics.               |
| `--squash`              | Squash the source's changes into a staged diff; no merge commit is created.                         |
| `-s, --strategy <STRAT>`| Merge strategy (`recursive`, `ours`, `octopus`, etc.).                                              |
| `-X, --strategy-option` | Strategy-specific option (repeatable).                                                              |

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
# Stage feature/api's changes on the current branch without a merge commit
daft merge --squash feature/api
```

The merge leaves changes staged; commit them yourself when ready.

### Abort a conflicted merge

```bash
# In the worktree where the merge is in progress:
daft merge --abort

# Or from anywhere, naming the worktree/branch:
daft merge --abort main
```

### Continue after resolving conflicts

```bash
# Edit conflicted files, `git add` them, then:
daft merge --continue

# Or from anywhere:
daft merge --continue main
```

### Cleanup after a successful merge

```bash
# Merge and delete the source worktree afterwards
daft merge feature/done --into main -r

# Also delete the source branch (safe -d semantics)
daft merge feature/done --into main -rb
```

`-b` requires `-r` and refuses to delete branches that aren't fully merged.

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

## See Also

- [git worktree-merge](./git-worktree-merge.md) — exhaustive flag reference
- [daft list](./daft-list.md) — inspect worktrees (including in-progress merges)
- [daft carry](./daft-carry.md) — transfer uncommitted changes between worktrees
- [daft sync](./daft-sync.md) — rebase + push many worktrees at once
- [daft adopt](./daft-adopt.md) — convert a traditional repo into daft's layout
