# daft merge — PR-style redesign

**Status:** Design proposal, awaiting implementation plan. **Issue:** daft-330
(Merge Branch). **Branch:** `daft-330/feat/merge` (continuation; previous slices
already shipped). **Supersedes:** `2026-04-25-daft-merge-rich-output-design.md`
(rich output stays; the flag and config surface described there is replaced by
what follows).

## Goal

Reshape `daft merge` to feel like GitHub's PR merge UX while keeping daft's
worktree-aware behavior. Selection of merge mechanics becomes a short, named
list of styles. What happens to the source after merge becomes a single binary
choice. Pre-merge hooks behave like PR checks (abort by default, overridable as
a manual escape hatch). Post-merge hooks fire whenever the merge actually
happened. Cleanup remains the responsibility of the existing
`worktree-pre-remove`, `worktree-post-remove`, and branch-delete hooks. Defaults
are configurable globally and overridable per invocation, with an inline
`--set-default` affordance to promote the current invocation's preferences.

## Non-goals

- PR conversations, reviews, approvals, labels, tags. Out of scope; tracked as
  follow-up tickets (see "Out-of-scope follow-up tickets" below).
- Decoupling merge cleanup's remote-deletion behavior from
  `branch.deleteRemote`. Out of scope; tracked as a follow-up.
- `--set-default` writing to user/global git config or daft.yml. This iteration
  writes to `--local` git config only.

## Background

The branch already shipped:

- A `daft merge` command supporting plain merge, `--squash`, `--no-ff`,
  multi-source, `--continue/--abort/--quit`, and `-rb` cleanup.
- Rich output / hook-box parity with `daft remove` (slices 1–7).
- Cleanup that delegates to `branch_delete::execute` with `force=true` and
  `delete_remote=false`, firing `worktree-pre-remove` and `worktree-post-remove`
  for each source.
- Pre-merge / post-merge hook execution with the existing `HookExecutor`
  pipeline.

What changes in this redesign:

- The merge-mechanics surface (`--ff/--no-ff/--ff-only/--squash`) is replaced by
  a 4-style enum (`merge`, `squash`, `rebase`, `rebase-merge`).
- The cleanup surface (`-r`, `-rb`, `--and-branch`) is replaced by a 2-outcome
  enum (`keep`, `remove-branch`).
- Rebase mechanics are added (today's daft merge has no rebase path).
- An inline `--set-default` flag persists the invocation's style/cleanup as the
  new defaults.
- Default behavior changes: no-flag `daft merge` produces an always-merge-commit
  result (today's no-flag invocation produces git's default
  `--ff if possible, else merge commit`).

## Surface

### CLI flags

**Style** (mutually exclusive booleans; default = `merge`):

```
--merge          # explicit default; rarely needed
--squash         # collapse source into one commit on target
--rebase         # rebase source onto target, fast-forward (linear, preserves commits)
--rebase-merge   # rebase source onto target, then create merge commit
```

Mutual exclusion enforced via clap `conflicts_with_all`. Rationale for keeping
`--merge` despite the awkward spelling: it's the only way to override a
config-set default style on a single invocation. Adding three `--no-<style>`
cancellation flags is a larger surface for the same job.

**Cleanup** (mutually exclusive booleans; default = `keep`):

```
-r, --remove-branch     # remove worktree + delete branch (local; remote follows branch.deleteRemote)
    --keep-branch       # explicit keep; for canceling a config default
```

`-r` short form preserved as muscle memory. `--remove-branch` semantically
implies the worktree is removed too (deleting a branch with a worktree present
is rejected by git; the worktree comes off as part of the operation).

**Defaults persistence:**

```
--set-default    # write the invocation's --style/--cleanup choices to .git/config
```

Writes via `git config --local daft.merge.style <value>` and
`daft.merge.cleanup <value>`. Both keys are always written (idempotent — even if
the value already matches). Best-effort: failure to write surfaces a warning,
does not fail the merge.

**Removed under hard cutover** (no compatibility shims, no deprecation period):

```
--ff
--no-ff
--ff-only
--no-squash
--remove        # repurposed name; today's behavior gone (today's --remove was worktree-only)
-b, --and-branch
```

Skipping the `feat!` / `BREAKING CHANGE:` marker per
`feedback_breaking_change_marker.md` — this surface is unreleased on master
(branch work in progress, low-adoption pre-1.0). Remove cleanly.

**Preserved (orthogonal to style and cleanup):**

```
-m, -F, --edit, --no-edit, --cleanup    # commit message — invalid with --rebase
--commit, --no-commit                    # commit control — invalid with --rebase
--signoff, --no-signoff                  # universal
--strategy, -X, --strategy-option        # universal (git rebase also takes -X)
--gpg-sign, --no-gpg-sign                # universal
--verify-signatures, --no-verify-signatures  # universal
--allow-unrelated-histories              # invalid with --rebase, --rebase-merge
--stat, --no-stat                        # universal
--into                                   # target selection
--adopt-target, --no-adopt-target, -y    # ephemeral target adoption
--abort, --continue, --quit              # finish-mode
```

**Per-flag conflict matrix:**

| Flag                                           | Conflicts with               | Reason                                                |
| ---------------------------------------------- | ---------------------------- | ----------------------------------------------------- |
| `-m`, `-F`, `--edit`, `--no-edit`, `--cleanup` | `--rebase`                   | Rebase produces no merge commit message               |
| `--commit`, `--no-commit`                      | `--rebase`                   | Rebase has no auto-commit toggle                      |
| `--allow-unrelated-histories`                  | `--rebase`, `--rebase-merge` | Rebase requires a common ancestor; flag is merge-only |

### Config schema

Two new keys, four removed under hard cutover:

```yaml
# git config (canonical) and daft.yml (mirror)

merge.style       = merge | squash | rebase | rebase-merge   # NEW; default: merge
merge.cleanup     = keep | remove-branch                     # NEW; default: keep
merge.signoff     = bool                                     # kept (existing)
merge.adoptTargetOnDemand = prompt | auto-yes | auto-no      # kept (existing)
branch.deleteRemote = bool                                   # kept (drives --remove-branch's remote behavior)
```

Removed:

```
merge.squash
merge.ff
merge.postMerge.removeSourceWorktree
merge.postMerge.alsoRemoveSourceBranch
```

Settings struct (`src/core/settings.rs`) loses four fields, gains two. Default
values: `MERGE_STYLE = MergeStyle::Merge`, `MERGE_CLEANUP = CleanupKind::Keep`.

Resolution order at invocation time (highest wins):

1. CLI flag (e.g., `--squash`).
2. Git config local (`git config --local daft.merge.style`).
3. Git config global (`git config --global daft.merge.style`).
4. daft.yml (project-level).
5. Hardcoded default.

### Behavior of `--set-default`

- Writes the resolved `--style` value (any of the four; defaults to `merge` if
  no style flag was passed) and the resolved `--cleanup` value (`keep` or
  `remove-branch`) to `git config --local`.
- Always writes both keys, even if their values match the current config. The
  rendered output line below is the user's confirmation; we don't conditionally
  suppress it.
- Writes happen **after** the merge step succeeds, **before** the cleanup phase.
  If the merge fails, no defaults are written. If cleanup fails, the defaults
  are already written.
- Failure modes: write rejection (readonly filesystem, missing `.git/config`,
  permission denied) surfaces as a single warning line; the merge result and
  cleanup phase proceed unaffected.
- Output line, in a discrete cyan/blue style (distinct from green success and
  yellow warn), placed between the merge step output and the cleanup phase:

  ```
  Updated repository defaults: merge.style=squash, merge.cleanup=remove-branch
  ```

## Hook semantics

Three hook lifecycle events touch the merge command. Their default behavior,
override permissions, and firing conditions:

| Hook                   | Default fail-mode | Overridable to `warn`  | Fires when                                                          |
| ---------------------- | ----------------- | ---------------------- | ------------------------------------------------------------------- |
| `pre-merge`            | `abort`           | **Yes** (escape hatch) | Before the merge mechanics begin                                    |
| `post-merge`           | `warn`            | Yes                    | **After the merge actually happened**, regardless of pre-merge mode |
| `worktree-pre-remove`  | `warn`            | Yes                    | During cleanup phase, per source being removed                      |
| `worktree-post-remove` | `warn`            | Yes                    | After each source's removal                                         |

**Pre-merge contract.** Default abort matches PR-check semantics: a failing
pre-merge hook stops the merge before any state changes. Users may downgrade to
`warn` per-hook (existing per-hook `fail-mode: warn`) as a manual escape hatch —
the use case is "I know what I'm doing, plow through." This is explicitly
permitted; the hook system does not validate-fail when a `pre-merge` hook
declares `fail-mode: warn`.

**Post-merge contract.** The trigger is "the merge happened," not "pre-merge
approved." If a user overrides pre-merge to `warn` and the hook fails (warning
only), the merge proceeds; if the merge then succeeds, post-merge fires. This
preserves the orthogonal use case of post-merge announcing/notifying success
(e.g., a Slack post, a release note generation) regardless of pre-merge state.

**Post-merge does not fire** if the merge mechanics produced no commit / no
fast-forward / no squash-commit. This includes:

- Pre-merge aborted (no merge attempted).
- Merge had conflicts and the user has not yet completed `--continue` (no commit
  yet).
- Merge was aborted via `daft merge --abort` (no commit).
- Squash style staged but did not commit (`squash_staged_only` outcome).

When the user later runs `daft merge --continue` and that produces a commit,
post-merge fires at that point.

**No change required to the hook executor itself.** `default_fail_mode()` for
`HookType::PreMerge` is already `FailMode::Abort` (`src/hooks/mod.rs:204-208`).
Per-hook override is already supported. Post-merge already fires only on
successful merge in this branch's current state.

## Architecture

### New types

In `src/core/worktree/merge.rs`:

```rust
#[derive(Debug, Clone, Copy, PartialEq, Eq, ValueEnum)]
pub enum MergeStyle {
    Merge,        // git merge --no-ff
    Squash,       // git merge --squash; git commit
    Rebase,       // git rebase target source; git merge --ff-only
    RebaseMerge,  // git rebase target source; git merge --no-ff
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, ValueEnum)]
pub enum CleanupKind {
    Keep,
    RemoveBranch,
}
```

`MergeStyle` replaces today's `FfMode` and `squash: bool` in `EffectiveFlags`.
`CleanupKind` replaces `Args.remove + Args.and_branch` derivation.

### Replaced fields

In `src/core/settings.rs`:

```diff
-pub merge_ff: FfMode,
-pub merge_squash: bool,
-pub merge_post_merge_remove_source_worktree: bool,
-pub merge_post_merge_also_remove_source_branch: bool,
+pub merge_style: MergeStyle,
+pub merge_cleanup: CleanupKind,
```

In `src/core/worktree/merge.rs::EffectiveFlags`:

```diff
-pub squash: Option<bool>,
-pub ff: FfMode,
+pub style: MergeStyle,
+pub cleanup: CleanupKind,
```

In `src/commands/merge.rs::Args`:

```diff
-pub ff: bool,
-pub no_ff: bool,
-pub ff_only: bool,
-pub squash: bool,
-pub no_squash: bool,
-pub remove: bool,
-pub and_branch: bool,
+pub style_merge: bool,         // --merge
+pub squash: bool,              // --squash
+pub rebase: bool,              // --rebase
+pub rebase_merge: bool,        // --rebase-merge
+pub remove_branch: bool,       // -r/--remove-branch
+pub keep_branch: bool,         // --keep-branch
+pub set_default: bool,         // --set-default
```

The four style booleans are mutually exclusive. The two cleanup booleans are
mutually exclusive. `--set-default` is independent.

### New code paths

**Rebase mechanics.** `src/core/worktree/merge.rs` gains a rebase phase used by
both `Rebase` and `RebaseMerge` styles. Sequence:

1. Resolve the rebase worktree:
   - If the source has its own worktree, use it (cd in via `-C`).
   - Else if the source is a ref-only branch (no worktree), reuse the
     ephemeral-target adoption pathway (today's `--adopt-target`) inverted —
     adopt the source ephemerally for the duration of the rebase, then drop the
     ephemeral worktree on success.
   - On adoption decline (`--no-adopt-target` semantics applied to source) or
     when source is a non-branch ref (e.g., a SHA): error before any state
     change with a clear message that rebase styles require a branch source.
2. Run `git -C <rebase-worktree> rebase <target> <source>`.
3. On conflict: leave the rebase in progress, exit. Finish-mode `--continue`
   resumes via `git rebase --continue`. `--abort` runs `git rebase --abort`.
   Rebase state file is `.git/rebase-merge/` or `.git/rebase-apply/` (git's own
   naming; both are recognized).
4. On success: switch to target's worktree.
5. For `Rebase`: `git merge --ff-only <source>`. The source ref now points at
   the rebased tip; FF must succeed (any failure is a bug or concurrent ref
   mutation).
6. For `RebaseMerge`: `git merge --no-ff <source>`. Creates a merge commit
   pointing at the rebased tip and the original target tip.
7. If an ephemeral source worktree was adopted in step 1, drop it after the
   FF/merge succeeds. If the FF/merge fails, leave it for finish-mode
   investigation.

The capture-output infrastructure from this branch's slice 3 is reused for both
phases.

**Finish-mode dispatch.** `--continue/--abort/--quit` examine the on-disk state
to determine whether they're resuming a rebase or a merge:

```
.git/MERGE_HEAD       → merge in progress; use git merge --continue/--abort/--quit
.git/rebase-merge/    → rebase in progress; use git rebase --continue/--abort/--quit
.git/rebase-apply/    → rebase (apply variant) in progress; use git rebase --continue/--abort/--quit
```

The dispatch is encapsulated in a new helper `detect_in_progress_state()` in
`src/core/worktree/merge.rs`. Today's finish-mode code paths (which assume merge
state) become one branch of the dispatch.

**`--set-default` writer.** A new function `write_default_settings()` in
`src/core/worktree/merge.rs` (or a small new module
`src/core/worktree/merge_set_default.rs`):

```rust
pub fn write_default_settings(
    git: &GitCommand,
    project_root: &Path,
    style: MergeStyle,
    cleanup: CleanupKind,
) -> Result<()> {
    git.run(&["config", "--local", "daft.merge.style", style.as_str()])?;
    git.run(&["config", "--local", "daft.merge.cleanup", cleanup.as_str()])?;
    Ok(())
}
```

Called after the merge step succeeds, before cleanup. Errors are caught at the
call site and rendered as a single-line warning; merge proceeds.

**Output rendering.** A new method on `Output`:

```rust
pub fn defaults_updated(&self, style: MergeStyle, cleanup: CleanupKind) {
    // Renders: "Updated repository defaults: merge.style=<v>, merge.cleanup=<v>"
    // in a cyan/blue style, distinct from success/warn/error.
}
```

### Reused code paths

No change required to:

- `branch_delete::execute` — already supports `delete_remote` and was wired in
  this branch's slice 2 to receive arguments from merge cleanup. The merge
  cleanup site now passes `delete_remote: settings.branch_delete_remote` and
  `keep_local_branch: false`.
- `MergeHookRunner` — pause/resume infrastructure for the editor, already in
  place.
- `plan_cleanup` planner — used by all cleanup paths regardless of style.
- Hook executor — `default_fail_mode()` for `PreMerge` is already `Abort`.

## Data flow (start mode)

```
parse Args
  → resolve EffectiveFlags (Args + Settings → MergeStyle, CleanupKind, set_default flag)
  → preflight (target resolution, adopt-target prompt, source validation, plan_cleanup)
  → fire pre-merge hook(s)
      → if FailMode::Abort and any failed: exit; no merge; no post-merge
      → if FailMode::Warn or all passed: continue
  → execute merge per MergeStyle:
      Merge        → git merge --no-ff <source>
      Squash       → git merge --squash <source>; git commit (editor or -m)
      Rebase       → git rebase <target> <source>; git merge --ff-only <source>
      RebaseMerge  → git rebase <target> <source>; git merge --no-ff <source>
      → on conflict: write state file, exit
      → on success: capture commit SHA(s), set StartOutcome flags
  → if merge succeeded:
      → fire post-merge hook(s)
      → if Args.set_default: write git config; render "Updated repository defaults: ..."
      → if CleanupKind::RemoveBranch:
          for each cleanup item from plan_cleanup:
            → fire worktree-pre-remove hook
            → branch_delete::execute with:
                delete_remote = settings.branch_delete_remote
                keep_local_branch = item.branch_name.is_none()  // worktree-only when no branch
                force = true
            → fire worktree-post-remove hook
  → render summary
```

## Error handling

| Failure                              | Behavior                                                                |
| ------------------------------------ | ----------------------------------------------------------------------- |
| Pre-merge hook fails (default Abort) | Exit before merge; no post-merge; no cleanup; no set-default            |
| Pre-merge hook fails (override Warn) | Warn line; continue to merge phase                                      |
| Merge mechanics fail (conflict)      | State file persisted; exit; no post-merge; finish-mode resumes later    |
| Rebase mechanics fail (conflict)     | Rebase state persisted; exit; finish-mode `--continue` uses rebase path |
| Merge mechanics fail (non-conflict)  | Exit; no post-merge; no cleanup; no set-default                         |
| Merge succeeds, post-merge fails     | Default Warn (non-blocking); continue to set-default + cleanup          |
| `--set-default` write fails          | Warn line; merge result stands; cleanup proceeds                        |
| Cleanup fails (any sub-step)         | Warn line; merge result stands (no rollback)                            |

**No-rollback invariant.** Once a merge commit / fast-forward / squash-commit
has landed on the target, we never undo it. This was already established in the
rich-output design; restated here.

## Testing

### Unit tests

In `src/core/worktree/merge.rs`:

- `MergeStyle::resolve()` from Args + Settings (12 cases: 4 styles × 3 sources
  of value).
- `CleanupKind::resolve()` from Args + Settings (6 cases: 2 outcomes × 3
  sources).
- `detect_in_progress_state()` (4 cases: no state, merge state, rebase-merge
  state, rebase-apply state).

In `src/core/settings.rs`:

- Default values for `merge_style` and `merge_cleanup`.
- Loading from git config keys `daft.merge.style`, `daft.merge.cleanup`.
- Loading from daft.yml.
- Removed-key tests deleted (no compat tests).

In a new `src/core/worktree/merge_set_default.rs` module:

- `write_default_settings()` issues correct git config commands.
- Failure path returns `Err`.

### Manual YAML scenarios

Create under `tests/manual/scenarios/merge/`:

- `style-merge.yml` — golden-path no-flag invocation produces a merge commit
  (verifies default change from FF-when-possible to always-merge-commit).
- `style-squash.yml` — `--squash` produces single squash commit.
- `style-rebase.yml` — `--rebase` produces linear history with source's commit
  SHAs replayed onto target.
- `style-rebase-merge.yml` — `--rebase-merge` produces rebased commits + merge
  commit.
- `style-rebase-conflict-then-continue.yml` — rebase conflict; `--continue`
  resumes via rebase mechanics.
- `style-rebase-merge-conflict-then-continue.yml` — same for rebase-merge.
- `cleanup-keep.yml` — default; source worktree and branch survive.
- `cleanup-remove-branch-local.yml` — `branch.deleteRemote=false`; `-r` deletes
  local branch only.
- `cleanup-remove-branch-with-remote.yml` — `branch.deleteRemote=true`; `-r`
  deletes local + remote.
- `set-default-writes-config.yml` — `--set-default` updates `.git/config`;
  verify keys via `git config --get`.
- `set-default-failure-warns.yml` — readonly `.git/config`; merge succeeds,
  warning rendered.
- `pre-merge-warn-override-allows-merge.yml` — pre-merge hook with
  `fail-mode: warn` exits non-zero; merge proceeds.
- `post-merge-fires-after-warn-override.yml` — confirms post-merge runs after
  the override path succeeds.
- `flag-conflict-message-with-rebase.yml` — `--rebase -m "x"` errors at parse
  time.
- `flag-conflict-allow-unrelated-with-rebase-merge.yml` — same for the
  merge-only flag.

Update or remove existing scenarios that reference the removed flags (`-rb`,
`--no-ff`, etc.).

### Regression coverage

- All scenarios from the rich-output design
  (`merge-fires-worktree-remove-hooks`, etc.) updated for the new flag names.
- Editor pause/resume scenario updated for the new flag set.
- `--continue/--abort/--quit` scenarios extended with rebase variants.

## Migration impact

This is a breaking change for users on this branch (master is unaffected until
merge). Behavioral changes:

1. **Default merge result changes.** Previously: FF when possible, else merge
   commit. Now: always-merge-commit (`--merge` style is the new default).
2. **`-rb` is gone.** Users who typed `-rb` now type `-r` (semantics changed:
   `-r` was worktree-only, now means full cleanup).
3. **`--ff/--no-ff/--ff-only` are gone.** Users who needed FF-or-fail use
   `--rebase`. Users who needed always-merge-commit don't need a flag any more.
4. **`--no-squash` is gone.** Users canceling a config-set `merge.squash`
   default now use `--merge`, `--rebase`, or `--rebase-merge`.
5. **Config keys renamed.** `merge.squash`, `merge.ff`, `merge.postMerge.*`
   removed. `merge.style`, `merge.cleanup` added. Users with the old keys set
   silently fall back to defaults — no migration warning, consistent with hard
   cutover. Stale keys can stay in `.git/config` harmlessly; users may clear
   them with `git config --local --unset daft.merge.squash` etc.

The branch has not been released; no `feat!` or `BREAKING CHANGE:` marker per
`feedback_breaking_change_marker.md`. Release notes for the eventual master
merge should call out the surface change.

## Out-of-scope follow-up tickets

To open after this design is approved (paragraph descriptions for each so they
land complete):

1. **PR conversations on merge proposals** — Per-line comment threads attached
   to a merge proposal. Requires a persistent storage layer for comments,
   resolution state, and threading, plus UI surface in the merge command's
   interactive flow. Substantial new feature.

2. **PR review / approval gates** — Pre-merge integration with an "approval
   count" or "approver list" requirement. Could be implemented as a special
   pre-merge hook that consults a per-branch approvals file or external service.
   Decision needed: in-tree state vs. external service integration.

3. **PR labels / tags on merge proposals** — Metadata system for marking merge
   proposals (e.g., `bug`, `feature`, `breaking`). Pairs with conversations and
   reviews; likely shares storage layer.

4. **`merge.removeRemoteBranch` standalone config** — Decouple merge cleanup's
   remote-deletion behavior from `branch.deleteRemote`. Useful for users who
   want `daft branch-delete` to delete remote refs but
   `daft merge --remove-branch` to leave them alone (or vice versa). Adds a new
   merge-specific config key.

5. **`--set-default --user` (global scope)** — Extend `--set-default` to accept
   a target scope: `--local` (today), `--user` (writes `git config --global`).
   Useful for cross-repo personal preferences.

6. **`--set-default` saves additional flags** — Extend the set-default writer to
   optionally persist `--signoff`, `--strategy`, `-X`, etc. Likely an opt-in
   modifier (`--set-default=all`) since the GitHub mental model is limited to
   style + cleanup.

7. **Strict FF precondition flag** — Replacement for the dropped `--ff-only` for
   users who want a hard fail-fast assertion before any merge work begins.
   Distinct from `--rebase` (which does the work to make FF possible). Likely a
   new orthogonal flag like `--require-fast-forward` that errors if the merge
   wouldn't be a pure FF.

## Open questions for the implementer

None expected — the spec is intended to be self-sufficient for plan creation. If
a question arises during implementation, the writing-plans phase or an
implementer subagent can flag it for the controller.
