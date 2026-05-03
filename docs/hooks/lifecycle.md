---
title: Lifecycle hooks reference
description:
  Reference for daft's worktree-lifecycle hooks — types, triggers, environment,
  exit codes.
---

# Lifecycle hooks

This page is a complete reference for the **worktree-lifecycle hook types** —
the stages that fire when worktrees are created or removed, and when a clone
finishes. For commit-stage and merge-stage hooks, see the
[roadmap](/hooks/roadmap).

For the conceptual framing, see the [Hooks Overview](/hooks/).

For the YAML schema, see [YAML reference](/hooks/yaml-reference).

## Hook types

| Hook                   | Trigger                       | Runs From                            |
| ---------------------- | ----------------------------- | ------------------------------------ |
| `post-clone`           | After `daft clone` completes  | New default branch worktree          |
| `worktree-pre-create`  | Before new worktree is added  | Source worktree (where command runs) |
| `worktree-post-create` | After new worktree is created | New worktree                         |
| `worktree-pre-remove`  | Before worktree is removed    | Worktree being removed               |
| `worktree-post-remove` | After worktree is removed     | Current worktree (where prune runs)  |

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

| Variable             | Description                         |
| -------------------- | ----------------------------------- |
| `DAFT_WORKTREE_PATH` | Path to the target worktree         |
| `DAFT_BRANCH_NAME`   | Branch name for the target worktree |

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

| Hook                  | Default Fail Mode | Behavior                              |
| --------------------- | ----------------- | ------------------------------------- |
| `worktree-pre-create` | `abort`           | Operation is cancelled                |
| All others            | `warn`            | Warning is shown, operation continues |

Override per-hook:

```bash
# Make post-create hooks abort on failure
git config daft.hooks.worktreePostCreate.failMode abort

# Make pre-create hooks just warn
git config daft.hooks.worktreePreCreate.failMode warn
```

Hook failures during moves produce **warnings**, not errors. The move operation
(rename, transform, adopt) always completes. This prevents a broken hook from
leaving the worktree in a half-moved state.

## Hooks vs jobs

`daft.yml` lets a single hook fire **multiple jobs** in parallel or sequenced.
The hook is the trigger; the job is the unit of work. See
[Job orchestration](/hooks/job-orchestration) for parallelism, dependencies, and
conditions.
