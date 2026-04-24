# daft merge

## Problem

daft wraps git worktree workflows but has no first-class merge operation. Users
fall back to raw `git merge` inside individual worktrees, which:

- Forces `cd` into the target worktree before merging, even when the user is
  standing in another branch's worktree and just wants to move a feature into
  `main`.
- Disconnects the merge from daft's configuration, hooks, and cross-worktree
  state awareness (dirty state, in-progress operations, layout-aware worktree
  resolution).
- Makes finish-line operations (remove the merged feature's worktree, delete the
  merged branch) a manual multi-step dance that doesn't fit daft's verb-oriented
  command surface.

This spec introduces `daft merge` — a cross-worktree merge command with full git
flag parity, layered configuration, optional post-merge cleanup, and explicit
handling of the edge cases that the worktree-per-branch model surfaces (target
branch has no worktree, conflict lands in a non-current worktree, in-progress
merge needs to be aborted from elsewhere).

## Goals

- Ship `daft merge <source>... [--into <target>]` as a cross-worktree merge verb
  that mirrors `git merge` when `--into` is omitted.
- Full git flag parity for the start form: all common `git merge` options are
  accepted and passed through (strategies, signing, squash, fast-forward
  control, commit control, signoff, etc.).
- Multi-source automatically invokes git's octopus strategy, communicated
  explicitly in daft's output.
- Finish commands (`--abort`, `--continue`, `--quit`) take an optional
  positional `<worktree|branch>` argument, default to CWD, and never require the
  user to `cd` to a different worktree to act on an in-progress merge.
- Layered defaults via git config (global < local < CLI) let users configure
  their preferred merge style (e.g. squash by default in their global
  `~/.gitconfig`) while allowing projects to override per-repo (e.g. team-wide
  `ff-only` in the repo's local config).
- Opt-in post-merge cleanup with `-r` (remove source worktree) and `-rb` (also
  remove source branch).
- Reuse existing daft infrastructure: worktree hooks, temp-worktree module,
  layout resolver, error formatting, completion generation, man-page pipeline.
- Expose `merge-pre` and `merge-post` hook types that carry daft-layer context
  (cross-worktree invocation, octopus count, ephemeral-worktree state) —
  information that git's native merge hooks cannot see.
- Provide a `-y` / `--yes` flag to suppress interactive prompts for scripted /
  non-TTY use.

## Non-goals

- **No auto-fetch before merge.** User runs `daft sync` or `git fetch`
  separately. `daft merge` does not reach the network.
- **No auto-push after merge.** Same reasoning.
- **No `--finish` composite flag.** If users want "merge and clean up", they
  type `daft merge feat -rb`. Keeps the surface explicit.
- **No force cleanup.** `-rb` uses `git branch -d` semantics (refuses unmerged
  branches). No `-D`-style force variant in this spec; users who insist on
  force-deleting after a squash merge fall out to raw git.
- **No squash-reachability logic.** After a squash merge, `-rb` passes through
  git's "not fully merged" error verbatim. daft does not attempt to verify "this
  branch is squash-merged into target" via `git cherry` or similar.
- **No `merge-conflict` dedicated hook.** Users branch on `DAFT_MERGE_RESULT`
  inside `merge-post` to detect conflicts. Adding a third event-specific hook
  would break daft's existing paired pre/post-only pattern.
- **No auto-cd on conflict.** When a merge conflicts, daft reports the target
  worktree path and exits non-zero. It does not use the `DAFT_CD_FILE` mechanism
  to move the user into the conflict worktree.
- **No branch protection list.** A config like
  `merge.protected_branches: [main]` is not in scope. Server-side branch
  protection belongs on the remote, not duplicated client-side.
- **No session hint env var.** A `DAFT_MERGE_TARGET` that lets
  `--abort`/`--continue` guess based on the most recent merge launched from the
  same shell is deferred.

## CLI Surface

### Start form

```
daft merge [FLAGS] [--into <target>] <source>...
```

- `--into <target>` — target worktree/branch. Defaults to the current worktree's
  branch. Accepts either a worktree directory name or a branch name (following
  `daft carry`'s convention — worktree name wins on conflict).
- `<source>...` — one or more source branches/commits. One source ⇒ normal
  merge. Two or more ⇒ octopus merge (explicitly announced in output).

### Finish forms

```
daft merge --abort    [<worktree|branch>]
daft merge --continue [<worktree|branch>]
daft merge --quit     [<worktree|branch>]
```

Positional argument defaults to CWD's branch. If the resolved worktree has no
in-progress merge, daft errors out and lists any worktrees that do, with a retry
hint.

### Status form

Adds `--merging` flag to the existing `daft list` command — no new top-level
verb. Shows all worktrees with active `MERGE_HEAD`, the branches being merged
in, and how long ago the merge started.

### Cleanup flags (start form only)

- `-r`, `--remove` — remove source worktree after successful merge. Fires
  existing `worktree-pre-remove` / `worktree-post-remove` hooks.
- `-b`, `--and-branch` — also remove the source branch. Requires `-r`; `-b`
  alone errors. Uses `git branch -d` semantics (refuses unmerged branches, no
  force).

### Target-worktree handling flags

- `--adopt-target` — when `<target>` has no worktree and the merge is not a pure
  fast-forward, create an ephemeral worktree without prompting.
- `--no-adopt-target` — refuse in the same situation without prompting.
- Neither flag and a TTY → prompt. Neither flag and non-TTY → refuse.

### Non-interactive flag

- `-y`, `--yes` — auto-accept all interactive prompts. Implies `--adopt-target`
  for the ephemeral-worktree prompt. Future-proofs any new prompts daft might
  add. Exits with an explicit message when a prompt that would have been shown
  is auto-accepted so the log stays self-describing.

### Passthrough flags (git parity)

daft accepts and passes through these `git merge` flags on the start form. Where
a flag conflicts with a daft-specific concept (e.g. `--abort`), the daft mode
takes precedence.

- Commit message and editor: `-m <msg>`, `-F <file>`, `--edit`, `--no-edit`,
  `--cleanup <mode>`
- Fast-forward control: `--ff`, `--no-ff`, `--ff-only`
- Squash: `--squash`, `--no-squash`
- Commit control: `--commit`, `--no-commit`
- Signoff: `--signoff`, `--no-signoff`
- Strategy: `-s <strategy>`, `-X <opt>`
- GPG: `-S`, `--gpg-sign[=<keyid>]`, `--no-gpg-sign`
- Verification: `--verify-signatures`, `--no-verify-signatures`
- History: `--allow-unrelated-histories`
- Stat control: `--stat`, `--no-stat`, `-n`
- Verbose: `-q`, `-v` (daft-level verbose; not passed through)

### Shortcut

- `gwtm` — short alias for `git-worktree-merge`, registered in
  `src/shortcuts.rs` and documented alongside other shortcuts.

## Semantics

### Resolution order

1. **Parse and validate flags.** `-b` without `-r` → error. `--adopt-target` and
   `--no-adopt-target` mutually exclusive.
2. **Resolve target.** From `--into <t>` or CWD's branch. Accepts worktree name
   or branch name, worktree wins.
3. **Resolve sources.** Each must exist as a branch or commit. Sources also
   accept worktree names (worktree's current HEAD commit).
4. **Refuse impossible configurations:**
   - Any source equals target.
   - Target is invalid ref or bare repo root.
5. **Inspect target worktree state:**
   - Has `MERGE_HEAD`, `REBASE_HEAD`, `CHERRY_PICK_HEAD`, `BISECT_LOG`, etc. →
     refuse with git's state surfaced in the error ("main is mid-rebase; finish
     or abort it first").
   - Working tree dirty (per `git status --porcelain`) and
     `merge.require_clean_target` is true → refuse.
6. **Handle "target has no worktree":**
   - Compute whether the merge is a pure fast-forward. Pure FF means a single
     source, no `--squash`, no `--no-ff`, target's ref is an ancestor of
     source's ref.
   - **Pure FF** → advance the ref via `git update-ref`. No worktree needed.
     Skip ahead to post-merge cleanup.
   - **Not pure FF** → consult `--adopt-target` / `--no-adopt-target` /
     `merge.adopt_target_on_demand` / TTY state. If the answer is "no", refuse
     with a hint to run `daft checkout <target>` first. If the answer is "yes",
     create an ephemeral worktree via daft's `temp_worktree` module.
7. **Announce opinionated defaults (verbose only).** If any effective flag value
   came from a config layer rather than git's default, daft prints a one-line
   notice per override:

   ```
   merge: squash=true (from user config)
   merge: ff=never (from project config)
   ```

8. **Announce merge mode.** If sources.len() >= 2, daft prints an explicit
   octopus notice:

   ```
   Merging 3 sources into main via octopus strategy
   ```

   For squash merges, daft prints an explicit squash notice.

### Executing the merge

The merge itself always runs as `git -C <worktree> merge [flags...] <sources>`
inside the target's working tree (ephemeral or permanent). The only exception is
pure fast-forward when the target has no worktree, which runs as
`git update-ref` against the bare repo.

### Conflict handling

On conflict, daft:

1. Leaves the target worktree in its conflicted state (git has already done
   this).
2. Prints the target worktree path and the list of conflicted files.
3. Prints instructions: resolve in place, then run `daft merge --continue` or
   `daft merge --abort`.
4. Exits non-zero.

daft does not auto-cd into the target, even if the user is in a different
worktree. If the target worktree was ephemeral, daft first promotes it to the
layout-resolved sibling path (see next section) and reports _that_ path in the
conflict message.

### Ephemeral worktree lifecycle

When a non-FF merge is run against a target with no worktree, and the user
accepted the adopt prompt (or passed `--adopt-target`):

1. Create a temporary worktree in daft's temp area (reusing
   `src/core/worktree/temp_worktree.rs`). Do **not** fire `worktree-pre-create`
   / `worktree-post-create` yet — this is initially ephemeral.
2. Run the merge inside the temp worktree.
3. **On success** → remove the temp worktree, leaving only the updated ref. No
   hooks fire, because the worktree was never promoted to a real one.
4. **On conflict** → move the temp worktree to the layout-resolved sibling path
   (using `src/core/layout`'s resolver), update daft's internal worktree
   metadata / registry so the worktree is discoverable by `daft list` and the
   finish commands, then fire `worktree-post-create` (retroactively, since the
   worktree now exists as a real one). The user sees a conflicted worktree at
   the expected path. Subsequent `daft merge --continue <target>` /
   `--abort <target>` works against that promoted worktree just like any other.

### Finish commands

`daft merge --abort [<arg>]` and peers:

1. Resolve the worktree. Explicit arg → named worktree or branch. No arg → CWD's
   worktree.
2. Check for an in-progress merge (presence of `MERGE_HEAD` in
   `<worktree>/.git`).
3. **If present** → run `git -C <worktree> merge --abort|--continue|--quit`.
   Passthrough flags (`--no-edit`, etc.) are forwarded.
4. **If absent** → error. List all worktrees in the project that have
   `MERGE_HEAD` set, show what's being merged, and suggest retrying with a
   positional argument.

### Cleanup (post-success only)

When `-r` is set and the merge (including any post-merge prompts) finished
successfully:

1. For each source, classify it:
   - **Branch with a worktree** (whether specified by branch name or by worktree
     name) → eligible for both worktree removal and branch deletion.
   - **Branch without a worktree** (specified by branch name, no sibling
     worktree exists) → eligible for branch deletion only; nothing to remove on
     disk.
   - **Commit SHA / detached ref** → skip; there is no branch or worktree to
     clean up.
2. For each source eligible for worktree removal, remove its worktree via daft's
   existing worktree-remove code path. `worktree-pre-remove` and
   `worktree-post-remove` fire as normal.
3. If `-b` is also set, then for each source eligible for branch deletion, run
   `git branch -d <branch>` (after any worktree removal for that source). If git
   refuses (branch not fully merged — e.g. a squash merge), daft prints git's
   error verbatim and exits non-zero, but does not undo any earlier worktree
   removals in the same invocation.

### Status display

`daft list --merging` filters the existing `daft list` output to only worktrees
with `MERGE_HEAD` set, and adds two columns:

- `merging` — branch(es) being merged in (space-separated for octopus)
- `since` — relative time since the merge was initiated

Uses the existing list formatter (respects `--format`, `--no-headers`, etc. from
the multi-format emit feature).

## Configuration

Merge settings follow daft's existing convention: `git config daft.merge.*`
keys, read by `DaftSettings::load()`. Layering, merged in order:

1. Git's built-in defaults for unset flags.
2. daft's built-in defaults for `daft.merge.*` keys.
3. Global git config (`git config --global daft.merge.x`).
4. Repository-local git config (`git config daft.merge.x`).
5. CLI flags.

Later layers override earlier ones. CLI always wins. daft prints the source of
any non-default value in verbose mode.

### Config keys

| Key                                           | Type                        | Default                          | Purpose                                                                 |
| --------------------------------------------- | --------------------------- | -------------------------------- | ----------------------------------------------------------------------- |
| `daft.merge.ff`                               | `auto` \| `only` \| `never` | `auto`                           | Fast-forward policy                                                     |
| `daft.merge.squash`                           | bool                        | `false`                          | Default to squash merge                                                 |
| `daft.merge.commit`                           | bool                        | `true`                           | Create commit automatically after merge                                 |
| `daft.merge.edit`                             | bool                        | `true` (TTY) / `false` (non-TTY) | Open editor on merge commit message                                     |
| `daft.merge.signoff`                          | bool                        | `false`                          | Add `Signed-off-by` trailer                                             |
| `daft.merge.gpgSign`                          | bool \| string              | `false`                          | GPG sign the merge commit (`true`, `false`, or a key id)                |
| `daft.merge.verifySignatures`                 | bool                        | `false`                          | Verify source commit signatures                                         |
| `daft.merge.allowUnrelatedHistories`          | bool                        | `false`                          | Allow merging unrelated histories                                       |
| `daft.merge.strategy`                         | string                      | unset                            | Merge strategy (e.g. `ort`, `octopus`, `ours`)                          |
| `daft.merge.strategyOption`                   | string                      | unset                            | Passed as `-X <value>`; comma-separated for multiple                    |
| `daft.merge.adoptTargetOnDemand`              | `prompt` \| `yes` \| `no`   | `prompt` (TTY) / `no` (non-TTY)  | Behavior when target has no worktree and merge is not FF                |
| `daft.merge.requireCleanTarget`               | bool                        | `true`                           | Refuse if target worktree is dirty                                      |
| `daft.merge.postMerge.removeSourceWorktree`   | bool                        | `false`                          | Default `-r` behavior                                                   |
| `daft.merge.postMerge.alsoRemoveSourceBranch` | bool                        | `false`                          | Default `-b` behavior; effective only when `removeSourceWorktree: true` |

The key set is orthogonal: `ff` and `squash` are independent axes that mirror
git's actual flag structure. There is no composite `mode` enum. Key names use
lowerCamelCase to match existing daft convention (`daft.checkout.push`,
`daft.go.autoStart`, etc.).

## Error handling

| Condition                                                      | Behavior                                                           |
| -------------------------------------------------------------- | ------------------------------------------------------------------ |
| Source equals target                                           | Refuse immediately with clear error                                |
| Target has in-progress op (rebase, merge, cherry-pick, bisect) | Refuse; surface git's state                                        |
| Target working tree dirty                                      | Refuse (mirror git); config `require_clean_target: false` disables |
| Source already reachable from target ("already up to date")    | Exit 0 with git's message                                          |
| Source is invalid ref                                          | Refuse; git's error                                                |
| Target has no worktree, merge is pure FF                       | Advance ref via plumbing, no prompt                                |
| Target has no worktree, merge is non-FF, TTY, no flag          | Prompt to create ephemeral                                         |
| Target has no worktree, merge is non-FF, no TTY, no flag       | Refuse; suggest `daft checkout`                                    |
| `--abort`/`--continue` on worktree with no `MERGE_HEAD`        | Refuse; list candidates                                            |
| `-b` without `-r`                                              | Refuse at parse time                                               |
| `-b` after squash merge (branch not marked merged)             | Surface git's `branch -d` error; don't retry with force            |
| Ephemeral worktree merge conflicts                             | Promote to layout path; fire `worktree-post-create`                |
| Ephemeral worktree merge succeeds                              | Remove temp worktree silently                                      |

## Hooks

Two new daft hook types, plus reuse of existing ones.

### New: `merge-pre`

Fires after all pre-flight checks pass and the target is resolved, but before
any merge operation runs (whether plumbing FF, worktree-delegated merge, or
ephemeral creation). Hook failure aborts the merge with the hook's exit code; no
changes have been made yet so no rollback is needed.

Environment variables:

| Var                         | Value                                                             |
| --------------------------- | ----------------------------------------------------------------- |
| `DAFT_MERGE_SOURCES`        | Space-separated source refs (as passed on the CLI)                |
| `DAFT_MERGE_TARGET_BRANCH`  | Target branch name                                                |
| `DAFT_MERGE_TARGET_PATH`    | Target worktree path; empty if target has no worktree             |
| `DAFT_MERGE_MODE`           | `merge`, `ff`, `squash`, or `octopus`                             |
| `DAFT_MERGE_STRATEGY`       | Effective `-s` strategy, or empty if not set                      |
| `DAFT_MERGE_EPHEMERAL`      | `true` if this merge will use an ephemeral worktree, else `false` |
| `DAFT_MERGE_CROSS_WORKTREE` | `true` if invocation was from a worktree other than the target    |

### New: `merge-post`

Fires after the merge operation completes, regardless of success or conflict.
Hook failure is logged as a warning but does not roll back the merge — the
commit (or conflicted state) has already landed.

Additional environment variables (on top of the `merge-pre` set):

| Var                                  | Value                                                            |
| ------------------------------------ | ---------------------------------------------------------------- |
| `DAFT_MERGE_RESULT`                  | `success`, `conflict`, or `already-up-to-date`                   |
| `DAFT_MERGE_COMMIT_SHA`              | SHA of the resulting merge commit, empty on conflict or FF-no-op |
| `DAFT_MERGE_CONFLICTED_FILES`        | Newline-separated list of conflicted files, empty on success     |
| `DAFT_MERGE_PROMOTED_FROM_EPHEMERAL` | `true` if an ephemeral worktree was promoted on conflict         |

Rationale for adding these hooks: git's native `pre-merge-commit` and
`post-merge` run inside the target worktree's `.git/hooks` directory and see
only git-level state. They cannot observe daft-layer context — whether this was
a cross-worktree invocation, whether an ephemeral worktree was involved, whether
multiple sources were combined via octopus. Users who want to react to those
conditions (notifications, ticket-system updates, telemetry) need hooks that
carry that context.

### Existing hooks that continue to fire

- Git's own `pre-merge-commit`, `commit-msg`, `post-merge` fire inside the
  target worktree's `.git/hooks` directory, unchanged.
- `worktree-pre-remove` / `worktree-post-remove` fire when `-r` removes a source
  worktree.
- `worktree-post-create` fires when an ephemeral worktree is promoted to a
  permanent one on conflict. The `worktree-pre-create` hook does **not** fire
  for ephemerals — they were not intended to become permanent when created.

## Implementation plan (high-level)

New files:

- `src/commands/merge.rs` — clap `Args` struct with full flag surface, mode
  dispatch (start vs abort vs continue vs quit), prompt handling, verbose
  deviation-from-default output.
- `src/core/worktree/merge.rs` — core merge logic: plumbing FF path,
  worktree-delegated merge, ephemeral worktree lifecycle, status inspection
  across worktrees.

Modified files:

- `src/commands/mod.rs` — register module.
- `src/main.rs` — multicall routing for `git-worktree-merge` and `daft merge`.
- `src/shortcuts.rs` — `gwtm` shortcut.
- `src/commands/list.rs` — add `--merging` flag, status columns.
- `xtask/src/main.rs` — add `merge` to `COMMANDS` array and
  `get_command_for_name()`.
- `src/commands/docs.rs` — add `merge` to help output in its category.
- `src/commands/completions/{bash,zsh,fish,fig}.rs` — add `merge` + full flag
  completions + `list --merging` flag.
- `src/core/settings.rs` — extend `DaftSettings` struct with merge fields, add
  default constants, extend `load()` to read the new `daft.merge.*` keys; extend
  `src/core/settings/keys.rs` (or equivalent) with the new key names.
- `man/` — regenerated via `mise run man:gen`.

New shell symlink:

- `git-worktree-merge → daft` created by `daft setup` and equivalent installer
  paths.

New documentation:

- `docs/cli/daft-merge.md` — reference page following `docs/cli/daft-doctor.md`
  template.
- `docs/guide/` — mention merge in relevant guide pages (hooks, configuration,
  workflow).
- `SKILL.md` — update to teach AI agents about `daft merge`.

## Testing

### YAML scenarios (`tests/manual/scenarios/merge/`)

- `basic.yml` — simple merge into CWD branch.
- `cross-worktree.yml` — `--into` from a different worktree.
- `octopus.yml` — multi-source announces octopus; success case.
- `octopus-conflict.yml` — multi-source with conflict; git refuses.
- `ff.yml` — fast-forward succeeds.
- `ff-only.yml` — `--ff-only` fails on non-FF.
- `no-ff.yml` — `--no-ff` forces merge commit on FF-eligible merge.
- `squash.yml` — squash merge produces single commit on target.
- `squash-cleanup-fails.yml` — `-rb` after squash surfaces git's error.
- `signoff.yml` — `--signoff` threads through.
- `gpg.yml` — GPG signing threads through (CI may skip if no key).
- `strategy-ours.yml` — `-s ours` threads through.
- `strategy-option.yml` — `-X theirs` threads through.
- `conflict.yml` — report-and-stay behavior; exit code; target path surfaced.
- `abort.yml` — `--abort` from CWD (target worktree).
- `abort-cross-worktree.yml` — `--abort main` from a different worktree.
- `continue.yml` — continue after conflict resolution.
- `quit.yml` — quit leaves state in place.
- `abort-no-merge-in-progress.yml` — lists candidates.
- `dirty-target.yml` — refuses cleanly.
- `target-in-operation.yml` — refuses, surfaces state.
- `same-source-target.yml` — refuses.
- `already-up-to-date.yml` — exits 0 with git's message.
- `no-target-worktree-ff.yml` — plumbing FF advances ref.
- `no-target-worktree-prompt-accept.yml` — prompt, accept, ephemeral created and
  removed on success.
- `no-target-worktree-prompt-decline.yml` — prompt, decline, refused.
- `no-target-worktree-no-tty.yml` — refuses without prompt in non-TTY.
- `no-target-worktree-flag.yml` — `--adopt-target` bypasses prompt.
- `ephemeral-conflict-promote.yml` — conflict in ephemeral promotes to layout
  path.
- `remove-source.yml` — `-r` removes worktree after success.
- `remove-source-and-branch.yml` — `-rb` removes both.
- `remove-unmerged-branch.yml` — `-rb` refused by git's `branch -d` semantics.
- `config-layered-defaults.yml` — user config `squash=true` + project config
  `ff=never` + CLI flag wins.
- `config-verbose-reports-source.yml` — verbose output identifies config layers.
- `status-list-merging.yml` — `daft list --merging` shows in-progress merges.

### Unit tests

- Plumbing FF ref advancement (`src/core/worktree/merge.rs`).
- In-progress detection (scan for `MERGE_HEAD` across worktrees).
- Flag parsing edge cases: `-b` without `-r`, `--adopt-target` +
  `--no-adopt-target`, mutually exclusive modes (`--abort` with sources).
- Config merging precedence.
- Ephemeral-to-permanent promotion logic.

### Bash integration tests

Follow existing patterns in `tests/integration/`. Smoke test the happy path of
each form of the command against a real temp repo.

## Deferred / future work

Captured for later consideration; explicitly out of scope for this spec.

- `--finish` composite that expands to `-rb` (and hypothetically `--push` once
  that's in scope).
- Auto-fetch before merge (`merge.fetch_before`).
- Auto-push after merge (`merge.post_merge.push`).
- Session-hint env var (`DAFT_MERGE_TARGET`) for ergonomic abort/continue after
  launching a cross-worktree merge.
- Branch protection list (`merge.protected_branches`).
- Force-delete variants (`-D`, `--force`) for cleanup.
- Squash-reachability detection (`git cherry`-based) to enable cleanup after
  squash merges.
- `merge-conflict` dedicated hook (users branch on `DAFT_MERGE_RESULT` inside
  `merge-post` instead).
- A dedicated `daft merge status` / `daft merge list` command, if the
  `daft list --merging` extension proves cramped.
