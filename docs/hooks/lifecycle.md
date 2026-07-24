---
title: Lifecycle hooks reference
description:
  Reference for daft's lifecycle hooks — types, triggers, environment, exit
  codes.
---

# Lifecycle hooks

This page is a complete reference for the **lifecycle hook types** that ship
today: clone setup, worktree create/remove, and merge gates. For commit-stage
hooks (the lefthook drop-in), see the [roadmap](/hooks/roadmap).

For the conceptual framing, see the [Hooks Overview](/hooks/).

For the YAML schema, see [YAML reference](/hooks/yaml-reference).

## Hook types

| Hook                   | Trigger                                                        | Runs From                            |
| ---------------------- | -------------------------------------------------------------- | ------------------------------------ |
| `post-clone`           | After `daft clone` completes                                   | New default branch worktree          |
| `worktree-pre-create`  | Before new worktree is added                                   | Source worktree (where command runs) |
| `worktree-post-create` | After new worktree is created                                  | New worktree                         |
| `worktree-pre-remove`  | Before worktree is removed                                     | Worktree being removed               |
| `worktree-post-remove` | After worktree is removed                                      | Current worktree (where prune runs)  |
| `pre-merge`            | After pre-flight checks pass, before the merge runs            | Target worktree                      |
| `post-merge`           | After the merge operation completes (success/conflict/aborted) | Target worktree                      |

### Execution order during clone

When running `daft clone`, hooks fire in this order:

1. **`post-clone`** -- one-time repo bootstrap (install toolchains, global
   setup)
2. **`worktree-post-create`** -- per-worktree setup (install dependencies,
   configure environment)

This lets `post-clone` install foundational tools (pnpm, bun, uv, etc.) that
`worktree-post-create` may depend on.

## Environment provided to hooks

Hooks receive context via environment variables. These are available to both
YAML jobs and shell script hooks.

### Universal (all hooks)

| Variable               | Description                                                                                                            |
| ---------------------- | ---------------------------------------------------------------------------------------------------------------------- |
| `DAFT_HOOK`            | Hook type (e.g., `worktree-post-create`)                                                                               |
| `DAFT_COMMAND`         | Command that triggered the hook (e.g., `checkout`). Note: `checkout` is used for both checkout and checkout `-b` modes |
| `DAFT_PROJECT_ROOT`    | Repository root (parent of `.git` directory)                                                                           |
| `DAFT_GIT_DIR`         | Path to the `.git` directory                                                                                           |
| `DAFT_REMOTE`          | Remote name (usually `origin`)                                                                                         |
| `DAFT_SOURCE_WORKTREE` | Worktree where the command was invoked                                                                                 |

### Worktree (creation and removal hooks)

| Variable             | Description                                                                     |
| -------------------- | ------------------------------------------------------------------------------- |
| `DAFT_WORKTREE_PATH` | Path to the target worktree                                                     |
| `DAFT_BRANCH_NAME`   | Branch name for the target worktree — the empty string for an anonymous sandbox |
| `DAFT_COMMIT`        | Pinned commit OID (anonymous sandbox worktrees only)                            |

### Creation (create hooks only)

| Variable             | Description                                               |
| -------------------- | --------------------------------------------------------- |
| `DAFT_IS_NEW_BRANCH` | `true` if the branch was newly created, `false` otherwise |
| `DAFT_BASE_BRANCH`   | Base branch (for `checkout -b` commands)                  |

### Clone (post-clone only)

| Variable              | Description                 |
| --------------------- | --------------------------- |
| `DAFT_REPOSITORY_URL` | The cloned repository URL   |
| `DAFT_DEFAULT_BRANCH` | The remote's default branch |

### Removal (remove hooks only)

| Variable              | Description                                                                  |
| --------------------- | ---------------------------------------------------------------------------- |
| `DAFT_REMOVAL_REASON` | Why the worktree is being removed: `remote-deleted`, `manual`, or `ejecting` |

### Merge (both merge hooks)

| Variable                    | Value                                                                |
| --------------------------- | -------------------------------------------------------------------- |
| `DAFT_MERGE_SOURCES`        | Space-separated list of source refs (branches/commits being merged)  |
| `DAFT_MERGE_TARGET_BRANCH`  | Name of the branch being merged into                                 |
| `DAFT_MERGE_TARGET_PATH`    | Filesystem path of the target worktree (empty on ref-only FF)        |
| `DAFT_MERGE_MODE`           | `merge` / `ff` / `squash` / `rebase` / `rebase-merge` / `octopus`    |
| `DAFT_MERGE_STRATEGY`       | Value of `-s`/`--strategy` (empty when not set)                      |
| `DAFT_MERGE_EPHEMERAL`      | `true` if the merge runs in an ephemeral worktree; otherwise `false` |
| `DAFT_MERGE_CROSS_WORKTREE` | `true` if the target worktree is not the current worktree            |

### Merge result (post-merge only)

| Variable                             | Value                                                                                                                |
| ------------------------------------ | -------------------------------------------------------------------------------------------------------------------- |
| `DAFT_MERGE_RESULT`                  | `success` / `conflict` / `already-up-to-date` / `aborted`                                                            |
| `DAFT_MERGE_COMMIT_SHA`              | SHA of the new tip on success (empty otherwise, including when `aborted`)                                            |
| `DAFT_MERGE_CONFLICTED_FILES`        | Newline-separated list of conflicted files (empty when not conflicted)                                               |
| `DAFT_MERGE_PROMOTED_FROM_EPHEMERAL` | `true` when a ref-only ephemeral merge was promoted to a sibling path                                                |
| `DAFT_MERGE_SOURCE_SHAS`             | Space-separated SHA list of source branch tips captured before the merge ran (one per source; empty for ref-only FF) |

### Move (move hooks only)

These variables are set when hooks run as part of a worktree move (rename,
layout transform, or adopt). They are available in all four move phases.

| Variable                 | Description                                     |
| ------------------------ | ----------------------------------------------- |
| `DAFT_IS_MOVE`           | `true` when running as part of a move operation |
| `DAFT_OLD_WORKTREE_PATH` | Worktree path before the move                   |
| `DAFT_OLD_BRANCH_NAME`   | Branch name before the move (rename only)       |

## Exit-code semantics

Each hook type has a default fail mode that determines what happens when a hook
exits with a non-zero status:

| Hook                   | Default Fail Mode | Behavior                                                                                         |
| ---------------------- | ----------------- | ------------------------------------------------------------------------------------------------ |
| `worktree-pre-create`  | `abort`           | Operation is cancelled                                                                           |
| `worktree-post-create` | `abort`           | Creation command exits non-zero and skips its `-x`/`--exec` commands; the worktree stays on disk |
| `pre-merge`            | `abort`           | Merge is aborted                                                                                 |
| All others             | `warn`            | Warning is shown, operation continues                                                            |

A failed `worktree-post-create` aborts because the `-x`/`--exec` commands (and
anything else scripted after the creation command) almost always depend on what
the hook just set up — installed dependencies, a written `.env`, a build.
Continuing would run them against a half-set-up worktree and exit `0`. The
worktree itself is deliberately kept so you can inspect or repair it; the error
names the path and how to re-run the hook.

A `worktree-pre-create` hook is a gate: it runs before `git worktree add`, so a
non-zero exit cancels creation and nothing is left on disk. Downgrading it to
`warn` runs the gate but ignores its veto — the worktree is created anyway.

Override per-hook:

```bash
# Let creation commands continue past a failing post-create hook
# (runs -x, warns, exits 0)
git config daft.hooks.worktreePostCreate.failMode warn

# Create the worktree even when the pre-create gate fails
# (creates it anyway, warns, exits 0)
git config daft.hooks.worktreePreCreate.failMode warn
```

A repo can also **commit** a hook's default fail mode in `daft.yml` via
`fail_mode:` (`abort` or `warn`), shipping it to every clone; the git config
above still takes precedence.

Hook failures during moves produce **warnings**, not errors. The move operation
(rename, transform, adopt) always completes. This prevents a broken hook from
leaving the worktree in a half-moved state.

## Merge hooks

`daft merge` fires `pre-merge` and `post-merge` around the merge operation,
giving scripts a chance to gate merges on custom preconditions or react to the
outcome — the [PR-check-parity boundary](/hooks/) of the boundaries thesis.

### When they fire

- **`pre-merge`** runs after all pre-flight safety rails (distinct-source check,
  clean-target check, in-progress-merge detection, already-up-to-date
  short-circuit) pass, but before any merge operation touches state. It fires
  uniformly for all merge styles and paths: worktree-backed merges, ref-only
  merges, rebase-style merges, and ephemeral worktree merges.
- **`post-merge`** runs after the merge operation completes, whether it
  succeeded, hit a conflict, or resolved without changes.

Both hooks read their config from the **target worktree** (the branch being
merged into). Neither fires when the merge is a no-op because the target is
already up to date.

### Failure semantics

- A `pre-merge` hook that exits non-zero **aborts the merge** with that exit
  code. No merge operation runs; no state is touched. The default fail mode is
  `abort`.
- A `post-merge` hook that exits non-zero is **logged as a warning** but does
  not roll back the merge. The default fail mode is `warn`.

The `pre-merge` fail mode can be downgraded per-repo:

```bash
# Downgrade to a warning — the merge proceeds even when pre-merge fails
git config daft.hooks.preMerge.failMode warn

# Restore the default abort behavior
git config --unset daft.hooks.preMerge.failMode
```

The same choice can be **committed** in `daft.yml`
(`hooks.pre-merge.fail_mode: warn`) to ship it to every clone; a local
`git config` override still takes precedence.

With `failMode=warn`, a failing pre-merge hook prints
`pre-merge hook failed with exit code N (continuing anyway)` and the merge
continues normally. This is useful for informational PR-check hooks that should
never block a merge while still surfacing failures.

`DAFT_MERGE_RESULT=aborted` fires when a squash-commit step is abandoned: the
editor was opened, the user wrote no commit message (empty buffer), and the
squash merge was discarded. `post-merge` still runs so cleanup logic can respond
to the abort.

## Hooks vs jobs

`daft.yml` lets a single hook fire **multiple jobs** in parallel or sequenced.
The hook is the trigger; the job is the unit of work. See
[Job orchestration](/hooks/job-orchestration) for parallelism, dependencies, and
conditions.
