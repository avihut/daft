# daft merge — squash + cleanup refinement plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use
> superpowers:subagent-driven-development (recommended) or
> superpowers:executing-plans to implement this plan task-by-task. Steps use
> checkbox (`- [ ]`) syntax for tracking.

**Branch:** `daft-330/feat/merge` (continuation of the original merge work —
this refinement ships before the branch lands).

**Goal:** Make `--squash` always-commit by default with editor prepopulation,
support `--squash -rb` as a real cleanup flow with a justified force-delete,
make all cleanup transactional, and teach `--abort` / `--continue` the new
squash-staged state.

**Spec:**
[2026-04-25 squash + cleanup design](../specs/2026-04-25-daft-merge-squash-cleanup-design.md).

**Architecture:** Continue the existing `core::worktree::merge` /
`commands::merge` split. Cleanup logic moves from "do, continue on failure" to
"validate, then mutate." A new in-progress state ("squash staged, commit
pending") is recognized in `detect_in_progress`. Source SHA capture is added to
the merge plan and surfaces in `MergeHookContext`. Progress prints go through
the existing `CliPresenter`.

**Tech Stack:** Rust, anyhow, clap derive, gix, dialoguer (for TTY prompts),
existing daft test harness.

---

## Slice 1: Refuse contradictory flag combos and non-TTY editor paths

Goal: Reject combinations that cannot succeed before any merge work runs. Three
rejections, all pre-flight:

- `--squash --no-commit -r` and `--squash --no-commit -rb` at clap parse time
  (clap `conflicts_with`).
- `daft.merge.commit = false` +
  `daft.merge.postMerge.alsoRemoveSourceBranch = true` at config load time
  (matched flag-combo error from settings).
- A path that would open an editor on a non-TTY without `--no-edit`/`-m`/`-F`
  (regular merge with `--edit`, or squash + commit with no message-supplying
  flag), refused before any merge work runs.

### Task 1.1: Reject `--squash --no-commit -r/-rb` at parse time

**Files:**

- Modify: `src/commands/merge.rs` (Args struct: extend `conflicts_with` on
  `no_commit` to include `remove` and `and_branch`)
- Test: `tests/manual/scenarios/merge/squash-no-commit-rb-refused.yml`

- [ ] **Step 1: Write failing scenario**

```yaml
name: --squash --no-commit -rb refused at parse time
description: "--no-commit conflicts with cleanup; cleanup needs a commit"
repos:
  - name: test-repo
    use_fixture: standard-remote
steps:
  - name: Clone
    run: git-worktree-clone --layout contained $REMOTE_TEST_REPO
    expect: { exit_code: 0 }
  - name: Attempt --squash --no-commit -rb
    run: git-worktree-merge feature/test-feature --squash --no-commit -rb 2>&1
    cwd: "$WORK_DIR/test-repo/main"
    expect:
      exit_code: 2
      output_contains:
        - "--no-commit"
        - "cannot be used"
```

- [ ] **Step 2: Run and verify it fails**

  Run: `mise run test:manual -- --ci squash-no-commit-rb-refused` Expected:
  failure (exit_code 0 today; flags don't conflict yet).

- [ ] **Step 3: Add `conflicts_with` to clap Args**

  Locate the `no_commit` field in `Args` and extend its `conflicts_with` list to
  include `remove` (and existing `and_branch` already requires `remove`).

- [ ] **Step 4: Re-run scenario, verify pass**

- [ ] **Step 5: Commit**

```bash
git add src/commands/merge.rs tests/manual/scenarios/merge/squash-no-commit-rb-refused.yml
git commit -m "feat(merge): refuse --squash --no-commit with -r/-rb at parse time"
```

### Task 1.2: Reject contradictory config combination at load time

**Files:**

- Modify: `src/core/settings.rs` (`load` for `DaftSettings`: add validation
  branch after both keys are read)
- Test: unit test in `settings.rs`

- [ ] **Step 1: Write failing unit test**

```rust
#[test]
fn refuses_no_commit_with_also_remove_branch() {
    let mut git = MockGit::new();
    git.set("daft.merge.commit", "false");
    git.set("daft.merge.postMerge.alsoRemoveSourceBranch", "true");
    let result = DaftSettings::load(&git);
    assert!(result.is_err());
    let msg = result.unwrap_err().to_string();
    assert!(msg.contains("daft.merge.commit"));
    assert!(msg.contains("alsoRemoveSourceBranch"));
}
```

- [ ] **Step 2: Run and verify failure**
      (`cargo test refuses_no_commit_with_also_remove_branch`)

- [ ] **Step 3: Add the validation in `load`** after both keys are parsed.

- [ ] **Step 4: Run, verify pass.**

- [ ] **Step 5: Commit**

```bash
git add src/core/settings.rs
git commit -m "feat(merge): refuse daft.merge.commit=false + alsoRemoveSourceBranch=true"
```

### Task 1.3: TTY guard for editor-opening paths

**Files:**

- Modify: `src/commands/merge.rs` (run dispatch — pre-flight TTY check)
- Modify: `src/core/worktree/merge.rs` (add `would_open_editor` helper on
  `EffectiveFlags`)
- Test: `tests/manual/scenarios/merge/squash-no-tty.yml`

- [ ] **Step 1: Write failing scenario**

```yaml
name: --squash without TTY refuses without --no-edit/-m
description: "Editor cannot open without a TTY; refuse before merge"
repos:
  - name: test-repo
    use_fixture: standard-remote
steps:
  - name: Clone
    run: git-worktree-clone --layout contained $REMOTE_TEST_REPO
    expect: { exit_code: 0 }
  - name: --squash via piped stdin (no TTY) and no --no-edit/-m
    run: echo "" | git-worktree-merge feature/test-feature --squash 2>&1
    cwd: "$WORK_DIR/test-repo/main"
    expect:
      exit_code: 1
      output_contains:
        - "No TTY available"
        - "--no-edit"
        - "-m"
```

- [ ] **Step 2: Run, verify failure** (today: would hang or commit with empty
      editor).

- [ ] **Step 3: Add `would_open_editor` helper and TTY check**

  ```rust
  // In core::worktree::merge:
  impl EffectiveFlags {
      pub fn squash_would_open_editor(&self) -> bool {
          self.squash
              && !self.no_commit
              && self.message.is_none()
              && self.message_file.is_none()
              && !matches!(self.edit, Some(false))
      }
  }
  ```

  Then in `commands::merge::run`, before any merge work:

  ```rust
  if effective.squash_would_open_editor() && !std::io::stdin().is_terminal() {
      anyhow::bail!(
          "No TTY available for the commit-message editor.\n\
           Pass --no-edit to use the auto-generated message, \
           -m <msg> for an explicit message, or -F <file> to read from a file."
      );
  }
  ```

  Use `std::io::IsTerminal` (stable, std).

- [ ] **Step 4: Run, verify pass.**

- [ ] **Step 5: Commit**

```bash
git add src/commands/merge.rs src/core/worktree/merge.rs tests/manual/scenarios/merge/squash-no-tty.yml
git commit -m "feat(merge): refuse --squash on non-TTY without --no-edit/-m/-F"
```

---

## Slice 2: Always-commit on `--squash`, with honest messaging

Goal: Replace the "stage and stop" execute path with squash-then-commit when
`--no-commit` is not set. Honor the existing flag passthroughs (`-m`, `-F`,
`--no-edit`, `--signoff`, `--gpg-sign`). Replace the unconditional "Merge
complete." with state-aware terminal lines.

### Task 2.1: Squash + commit execution path

**Files:**

- Modify: `src/core/worktree/merge.rs` (`execute_start_in_worktree`,
  `execute_ephemeral_merge`)
- Modify: `src/commands/merge.rs` (no behavior change here, but verify flag
  passthrough)
- Test: replace `tests/manual/scenarios/merge/squash.yml`

- [ ] **Step 1: Update `squash.yml` to assert the new behavior**

  Replace the existing scenario contents with:

```yaml
name: --squash with --no-edit creates a real commit
description:
  "daft merge --squash --no-edit produces a commit on the target; no stale
  MERGE_HEAD/MERGE_MSG; no staged changes left over."
repos:
  - name: test-repo
    use_fixture: standard-remote
steps:
  - name: Clone
    run: git-worktree-clone --layout contained $REMOTE_TEST_REPO
    expect: { exit_code: 0 }
  - name: Materialize feature
    run: git-worktree-checkout feature/test-feature
    cwd: "$WORK_DIR/test-repo"
    expect: { exit_code: 0 }
  - name: Squash + commit
    run: git-worktree-merge feature/test-feature --squash --no-edit 2>&1
    cwd: "$WORK_DIR/test-repo/main"
    expect:
      exit_code: 0
      output_contains:
        - "Squash merged feature/test-feature into main"
  - name: Verify a real commit landed (no MERGE_HEAD/MERGE_MSG, no staged)
    run: |
      real=$(git -C $WORK_DIR/test-repo/main rev-parse --absolute-git-dir)
      [ ! -e "$real/MERGE_HEAD" ] && echo "no_merge_head"
      [ ! -e "$real/MERGE_MSG" ] && echo "no_merge_msg"
      git -C $WORK_DIR/test-repo/main diff --cached --quiet && echo "no_staged"
      git -C $WORK_DIR/test-repo/main log --oneline -1 main
    cwd: "$WORK_DIR/test-repo/main"
    expect:
      exit_code: 0
      output_contains:
        - "no_merge_head"
        - "no_merge_msg"
        - "no_staged"
        - "Add feature branch"
```

- [ ] **Step 2: Run, verify it fails** (current path stages and stops).

- [ ] **Step 3: Refactor `execute_start_in_worktree` to commit after squash**

  After `git merge --squash <source>` succeeds, when `flags.no_commit == false`,
  run `git commit` with appropriate flags forwarded (`--no-edit`, `-m`, `-F`,
  `--signoff`, `-S`/`--gpg-sign`, `--no-gpg-sign`). On non-zero exit (editor
  abort, hook fail, GPG fail), bubble up as `CommitAborted` (new variant on
  `StartOutcome` or carried via `PostOutcome::Aborted`); do not run cleanup; let
  the caller print the abort message and skip cleanup.

  Apply the same change to `execute_ephemeral_merge` for consistency.

- [ ] **Step 4: Add a non-commit scenario to lock the opt-out**

  Create `tests/manual/scenarios/merge/squash-no-commit.yml`:

```yaml
name: --squash --no-commit stages without committing
description:
  "Explicit --no-commit (or daft.merge.commit=false) preserves git's historical
  stage-only behavior."
repos:
  - name: test-repo
    use_fixture: standard-remote
steps:
  - name: Clone
    run: git-worktree-clone --layout contained $REMOTE_TEST_REPO
    expect: { exit_code: 0 }
  - name: Materialize feature
    run: git-worktree-checkout feature/test-feature
    cwd: "$WORK_DIR/test-repo"
    expect: { exit_code: 0 }
  - name: Squash --no-commit
    run: git-worktree-merge feature/test-feature --squash --no-commit 2>&1
    cwd: "$WORK_DIR/test-repo/main"
    expect:
      exit_code: 0
      output_contains:
        - "Squash staged on main"
        - "Commit when ready"
  - name: Verify staged but not committed
    run: |
      git -C $WORK_DIR/test-repo/main diff --cached --quiet && echo "no_staged" || echo "has_staged"
    cwd: "$WORK_DIR/test-repo/main"
    expect:
      exit_code: 0
      output_contains: ["has_staged"]
```

- [ ] **Step 5: Run all merge scenarios, verify pass**

  ```bash
  mise run test:manual -- --ci squash
  mise run test:manual -- --ci squash-no-commit
  mise run test:manual -- --ci  # full sweep — catch regressions
  ```

- [ ] **Step 6: Commit**

```bash
git add src/core/worktree/merge.rs src/commands/merge.rs \
  tests/manual/scenarios/merge/squash.yml \
  tests/manual/scenarios/merge/squash-no-commit.yml
git commit -m "feat(merge): always commit on --squash unless --no-commit"
```

### Task 2.2: Honest state-aware terminal messaging

**Files:**

- Modify: `src/core/worktree/merge.rs` (`announcement` /
  `StartOutcome.emitted_terminal_message` plumbing)
- Modify: `src/commands/merge.rs` (replace any direct "Merge complete." prints)
- Test: existing scenarios that asserted "Merge complete." — update expectations
  where appropriate, or assert the new state-aware lines.

- [ ] **Step 1: Audit all scenarios asserting "Merge complete."**

  Run: `grep -l "Merge complete\." tests/manual/scenarios/merge/*.yml`

  For each, decide whether the new line is one of:
  - `Merge complete.` (true merge with merge commit) — keep
  - `Fast-forwarded <target> to <sha>.` — already exists in pure-FF path; verify
  - `Squash merged <source> into <target> as <sha>.` — new for squash
  - `Squash merged and cleaned up <source>.` — new for squash + cleanup

- [ ] **Step 2: Update assertions in affected scenarios**

  Touch only the scenarios where the assertion would otherwise become false.
  Don't sweep broadly.

- [ ] **Step 3: Implement the new lines**

  Add `StartOutcome` fields or an enum variant carrying enough info to print the
  right line at the command layer (sources joined, target branch, new commit
  SHA, cleanup-performed flag).

- [ ] **Step 4: Run full YAML suite, verify pass.**

- [ ] **Step 5: Commit**

```bash
git add src/core/worktree/merge.rs src/commands/merge.rs tests/manual/scenarios/merge/
git commit -m "feat(merge): state-aware terminal messaging for squash and cleanup"
```

---

## Slice 3: Source SHA capture + transactional cleanup

Goal: Capture each source's SHA before any merge work begins. Refactor
`execute_cleanup` from "do; continue on failure" to "pre-validate; then mutate."
Add the missing unit tests.

### Task 3.1: Capture source SHAs into the merge plan

**Files:**

- Modify: `src/core/worktree/merge.rs` (`StartParams` or new `MergePlan` struct:
  add `source_shas: Vec<String>` populated by `git rev-parse <source>` early;
  thread through to cleanup)
- Test: unit test for SHA capture

- [ ] **Step 1: Write failing unit test** for SHA capture in `MergePlan`.

- [ ] **Step 2: Run, verify fail.**

- [ ] **Step 3: Implement** SHA capture. Capture happens after target resolution
      and pre-flight checks, before `pre-merge` hook fires. Surface as
      `DAFT_MERGE_SOURCE_SHAS` (newline- or space-separated) in the hook env
      (optional, low cost).

- [ ] **Step 4: Run, verify pass.**

- [ ] **Step 5: Commit**

```bash
git add src/core/worktree/merge.rs
git commit -m "feat(merge): capture source SHAs at merge start for cleanup safety"
```

### Task 3.2: Transactional cleanup — validate before mutate

**Files:**

- Modify: `src/core/worktree/merge.rs` (`execute_cleanup`)
- Test: `tests/manual/scenarios/merge/cleanup-prevalidates.yml`
- Test: unit tests in `merge.rs`

- [ ] **Step 1: Write failing unit test for pre-validation**

```rust
#[test]
fn execute_cleanup_validates_before_mutating() {
    // Set up a repo where the source branch has a local-only commit
    // (regular -d would refuse). Run execute_cleanup with -rb.
    // Expect: error returned, worktree NOT removed.
    // ...
    let path = tempdir().unwrap();
    init_repo(path.path());
    // ... set up source branch with local-only commit ...
    let result = execute_cleanup(&sources, &CleanupOptions {
        remove_worktree: true,
        also_branch: true,
        squash_committed: false, // regular -d path
    }, &git, project_root);
    assert!(result.is_err());
    assert!(source_worktree_exists(path.path()));  // NOT removed
}
```

- [ ] **Step 2: Run, verify fail.**

- [ ] **Step 3: Refactor `execute_cleanup`**

  Two-phase:

  ```rust
  // Phase 1: validate every step that would mutate
  for src in sources {
      validate_worktree_removable(&src)?;  // if remove_worktree
      validate_branch_deletable(&src, options.squash_committed)?;  // if also_branch
  }
  // Phase 2: only now mutate
  for src in sources {
      // print progress, then remove worktree
      // print progress, then delete branch (use -D iff squash_committed)
  }
  ```

- [ ] **Step 4: Add YAML scenario**

  `cleanup-prevalidates.yml` — set up a regular merge where the source worktree
  has uncommitted changes; assert `-rb` refuses without touching either the
  worktree or the branch.

- [ ] **Step 5: Run, verify pass.**

- [ ] **Step 6: Commit**

```bash
git add src/core/worktree/merge.rs tests/manual/scenarios/merge/cleanup-prevalidates.yml
git commit -m "refactor(merge): transactional cleanup — validate before mutate"
```

---

## Slice 4: Stability check + justified `branch -D` in squash-cleanup

Goal: After a squash + commit, re-resolve source ref and refuse cleanup if it
moved. If stable, force-delete with `branch -D` (justified by the content
equivalence proof). Replace the existing `remove-unmerged-branch.yml` (which
ratified the broken behavior) with the new happy path and the SHA-moved abort.

### Task 4.1: Stability check before cleanup

**Files:**

- Modify: `src/core/worktree/merge.rs` (after squash commit, before cleanup:
  re-resolve `<source>`; compare to captured SHA; bail if diverged)
- Test: `tests/manual/scenarios/merge/squash-rb-source-moved.yml`

- [ ] **Step 1: Write failing scenario**

```yaml
name: --squash -rb refuses cleanup when source ref moved
description:
  "Source branch tip moved between merge start and cleanup; cleanup refuses to
  avoid losing work; commit stays on target."
repos:
  - name: test-repo
    use_fixture: standard-remote
steps:
  - name: Clone
    run: git-worktree-clone --layout contained $REMOTE_TEST_REPO
    expect: { exit_code: 0 }
  - name: Materialize feature
    run: git-worktree-checkout feature/test-feature
    cwd: "$WORK_DIR/test-repo"
    expect: { exit_code: 0 }
  # Use a hook to advance the source branch during the merge — the
  # pre-merge hook fires after SHA capture; advancing in pre-merge
  # simulates a concurrent push.
  - name: Configure pre-merge hook that advances feature
    run: |
      mkdir -p .daft/hooks
      cat > daft.yml <<'EOF'
      hooks:
        pre-merge:
          jobs:
            - name: advance-feature
              run: |
                git -C "$WORK_DIR/test-repo/feature/test-feature" \
                  commit --allow-empty -m "concurrent advance" \
                  --author="t <t@t>"
      EOF
      git add daft.yml
      GIT_AUTHOR_NAME=t GIT_AUTHOR_EMAIL=t@t GIT_COMMITTER_NAME=t \
        GIT_COMMITTER_EMAIL=t@t git commit -m "add daft.yml"
    cwd: "$WORK_DIR/test-repo/main"
    expect: { exit_code: 0 }
  - name: Trust hooks
    run: git daft hooks trust
    cwd: "$WORK_DIR/test-repo/main"
    expect: { exit_code: 0 }
  - name: --squash --no-edit -rb — should refuse cleanup
    run: git-worktree-merge feature/test-feature --squash --no-edit -rb 2>&1
    cwd: "$WORK_DIR/test-repo/main"
    expect:
      exit_code: 1
      output_contains:
        - "moved during merge"
        - "Re-run cleanup manually"
  - name: Verify commit still landed on main
    run: git -C $WORK_DIR/test-repo/main log --oneline -1 main
    cwd: "$WORK_DIR/test-repo/main"
    expect:
      exit_code: 0
      output_contains: ["Add feature branch"]
  - name: Verify source worktree NOT removed (cleanup didn't run)
    run: test -d $WORK_DIR/test-repo/feature/test-feature && echo "wt_present"
    cwd: "$WORK_DIR/test-repo/main"
    expect: { exit_code: 0, output_contains: ["wt_present"] }
  - name: Verify source branch NOT deleted
    run:
      git -C $WORK_DIR/test-repo/main show-ref --verify --quiet
      refs/heads/feature/test-feature && echo "branch_present"
    cwd: "$WORK_DIR/test-repo/main"
    expect: { exit_code: 0, output_contains: ["branch_present"] }
```

- [ ] **Step 2: Run, verify fail.**

- [ ] **Step 3: Implement the stability check** between commit success and
      cleanup pre-validation. Use captured SHA from Slice 3.

- [ ] **Step 4: Run, verify pass.**

- [ ] **Step 5: Commit**

```bash
git add src/core/worktree/merge.rs tests/manual/scenarios/merge/squash-rb-source-moved.yml
git commit -m "feat(merge): refuse --squash cleanup if source ref moved during merge"
```

### Task 4.2: Justified `branch -D` for squash-cleanup happy path

**Files:**

- Modify: `src/core/worktree/merge.rs` (`execute_cleanup`: route `-D` vs `-d`
  based on `CleanupOptions.squash_committed`)
- Modify: `src/git/mod.rs` if needed (add `branch_delete_force` helper)
- Modify: `tests/manual/scenarios/merge/remove-unmerged-branch.yml` → rename or
  repurpose as `squash-rb.yml` (happy path)
- Delete: the old `remove-unmerged-branch.yml` semantic

- [ ] **Step 1: Write the new happy-path scenario**

  Replace `remove-unmerged-branch.yml` with `squash-rb.yml`:

```yaml
name: --squash -rb cleanly removes worktree and branch
description:
  "After a daft-driven squash + commit, branch -D is justified by content
  equivalence; -rb completes end to end."
repos:
  - name: test-repo
    use_fixture: standard-remote
steps:
  - name: Clone
    run: git-worktree-clone --layout contained $REMOTE_TEST_REPO
    expect: { exit_code: 0 }
  - name: Materialize feature with a local-only commit
    run: |
      git-worktree-checkout feature/test-feature
      cd $WORK_DIR/test-repo/feature/test-feature
      echo "local extra" > local-only.txt
      git add local-only.txt
      GIT_AUTHOR_NAME=t GIT_AUTHOR_EMAIL=t@t GIT_COMMITTER_NAME=t \
        GIT_COMMITTER_EMAIL=t@t git commit -m "local-only commit"
    cwd: "$WORK_DIR/test-repo"
    expect: { exit_code: 0 }
  - name: --squash --no-edit -rb
    run: git-worktree-merge feature/test-feature --squash --no-edit -rb 2>&1
    cwd: "$WORK_DIR/test-repo/main"
    expect:
      exit_code: 0
      output_contains:
        - "Squash merged and cleaned up feature/test-feature"
        - "Removing worktree"
        - "Deleting branch"
  - name: Verify worktree gone
    run: test ! -d $WORK_DIR/test-repo/feature/test-feature && echo "wt_gone"
    cwd: "$WORK_DIR/test-repo/main"
    expect: { exit_code: 0, output_contains: ["wt_gone"] }
  - name: Verify branch gone
    run: |
      if git -C $WORK_DIR/test-repo/main show-ref --verify --quiet \
        refs/heads/feature/test-feature; then echo "still"; else echo "gone"; fi
    cwd: "$WORK_DIR/test-repo/main"
    expect: { exit_code: 0, output_contains: ["gone"] }
  - name: Verify squash commit on main
    run: git -C $WORK_DIR/test-repo/main log --oneline -1 main
    cwd: "$WORK_DIR/test-repo/main"
    expect: { exit_code: 0, output_contains: ["Add feature branch"] }
```

- [ ] **Step 2: Delete `remove-unmerged-branch.yml`**

  ```bash
  git rm tests/manual/scenarios/merge/remove-unmerged-branch.yml
  ```

- [ ] **Step 3: Run, verify fail** (cleanup will currently use `-d`).

- [ ] **Step 4: Add `squash_committed: bool` to `CleanupOptions`** and branch in
      `execute_cleanup` to use `-D` when set.

- [ ] **Step 5: Wire `squash_committed = true` from the squash-and-committed
      path** in `execute_start_in_worktree` / `execute_ephemeral_merge`.

- [ ] **Step 6: Run, verify pass.**

- [ ] **Step 7: Commit**

```bash
git add src/core/worktree/merge.rs src/git/mod.rs \
  tests/manual/scenarios/merge/squash-rb.yml
git rm tests/manual/scenarios/merge/remove-unmerged-branch.yml
git commit -m "feat(merge): justified branch -D for daft-driven squash + cleanup"
```

---

## Slice 5: `--abort` / `--continue` for squash-staged state

Goal: Recognize the new in-progress state (MERGE_MSG present without MERGE_HEAD)
in `detect_in_progress`; teach `--abort` and `--continue` the right operations
for it. Persist the original cleanup intent so `--continue` can resume cleanup
after the editor commit.

### Task 5.1: Detect squash-staged state

**Files:**

- Modify: `src/core/worktree/merge.rs` (`detect_in_progress`, `InProgressOp`
  enum: add `SquashStaged` variant)
- Test: unit tests for the new detection

- [ ] **Step 1: Write failing unit test** that sets up a worktree with MERGE_MSG
      and staged changes but no MERGE_HEAD; assert `detect_in_progress` returns
      `SquashStaged`.

- [ ] **Step 2: Run, verify fail.**

- [ ] **Step 3: Implement detection.** Read `MERGE_MSG` and `MERGE_HEAD`
      presence; require staged changes for a positive signal (avoids
      false-positives on stale `MERGE_MSG`).

- [ ] **Step 4: Run, verify pass.**

- [ ] **Step 5: Commit**

```bash
git add src/core/worktree/merge.rs
git commit -m "feat(merge): detect squash-staged in-progress state"
```

### Task 5.2: `--abort` clears squash-staged state

**Files:**

- Modify: `src/core/worktree/merge.rs` (finish-mode dispatch)
- Test: `tests/manual/scenarios/merge/abort-squash-staged.yml`

- [ ] **Step 1: Write failing scenario**

```yaml
name: --abort clears squash-staged state
description:
  "After --squash --no-commit (or interrupted squash commit), --abort resets the
  index, clears MERGE_MSG, and exits cleanly."
repos:
  - name: test-repo
    use_fixture: standard-remote
steps:
  - name: Clone + checkout
    run: |
      git-worktree-clone --layout contained $REMOTE_TEST_REPO
      cd $WORK_DIR/test-repo
      git-worktree-checkout feature/test-feature
    expect: { exit_code: 0 }
  - name: Stage a squash without committing
    run: git-worktree-merge feature/test-feature --squash --no-commit
    cwd: "$WORK_DIR/test-repo/main"
    expect: { exit_code: 0 }
  - name: --abort
    run: git-worktree-merge --abort 2>&1
    cwd: "$WORK_DIR/test-repo/main"
    expect:
      exit_code: 0
      output_contains: ["Aborted"]
  - name: Verify clean state
    run: |
      real=$(git -C $WORK_DIR/test-repo/main rev-parse --absolute-git-dir)
      [ ! -e "$real/MERGE_MSG" ] && echo "no_merge_msg"
      git -C $WORK_DIR/test-repo/main diff --cached --quiet && echo "no_staged"
    cwd: "$WORK_DIR/test-repo/main"
    expect:
      exit_code: 0
      output_contains: ["no_merge_msg", "no_staged"]
```

- [ ] **Step 2: Run, verify fail.**

- [ ] **Step 3: Implement abort path** for `SquashStaged`: run
      `git reset --merge` (which handles both no-MERGE_HEAD and the staged
      index) and remove `MERGE_MSG`.

- [ ] **Step 4: Run, verify pass.**

- [ ] **Step 5: Commit**

```bash
git add src/core/worktree/merge.rs tests/manual/scenarios/merge/abort-squash-staged.yml
git commit -m "feat(merge): --abort clears squash-staged state"
```

### Task 5.3: `--continue` re-opens editor for squash-staged state

**Files:**

- Modify: `src/core/worktree/merge.rs` (finish-mode dispatch)
- Decide: how to persist the original cleanup intent (recommended: small
  daft-specific marker file at `.git/daft-merge-intent.json` with
  `{ remove: bool, also_branch: bool, source_shas: [..] }`)
- Test: `tests/manual/scenarios/merge/continue-squash-staged.yml`

- [ ] **Step 1: Persist cleanup intent at squash time**

  When the squash + commit step is about to run (or aborts), write the intent
  marker file. Remove it on successful completion or `--abort`.

- [ ] **Step 2: Write failing scenario**

  Continue scenario covers: stage a squash, abort the editor, run
  `--continue --no-edit -rb`, verify cleanup runs.

- [ ] **Step 3: Implement continue path** for `SquashStaged`: re-open editor (or
      honor `-m`/`--no-edit`/`-F` from the continue invocation) via
      `git commit`. On success, read intent file and run cleanup if requested.

- [ ] **Step 4: Run, verify pass.**

- [ ] **Step 5: Commit**

```bash
git add src/core/worktree/merge.rs tests/manual/scenarios/merge/continue-squash-staged.yml
git commit -m "feat(merge): --continue resumes squash-staged commit and cleanup"
```

---

## Slice 6: `post-merge` `aborted` outcome + cleanup progress messages

Goal: Surface a fourth `RESULT` value, `aborted`, when the squash-commit step is
aborted (editor empty, hook fail, GPG fail). Print progress lines before each
slow cleanup operation.

### Task 6.1: post-merge `aborted` outcome

**Files:**

- Modify: `src/core/worktree/merge.rs` (`PostOutcome` enum: add `Aborted`
  variant; `MergeHookContext::for_post` emits `RESULT=aborted` with empty
  `COMMIT_SHA`)
- Modify: caller paths to pair `pre-merge` with a final `post-merge` even when
  commit aborts
- Test: `tests/manual/scenarios/merge/squash-edit-aborted.yml`

- [ ] **Step 1: Write failing scenario**

  Scenario uses `EDITOR=false` (which exits non-zero) to simulate an editor
  abort, asserts `post-merge` fired with `RESULT=aborted`, changes still staged,
  no commit on target.

- [ ] **Step 2: Run, verify fail.**

- [ ] **Step 3: Implement `Aborted` variant and wiring.**

- [ ] **Step 4: Run, verify pass.**

- [ ] **Step 5: Commit**

```bash
git add src/core/worktree/merge.rs tests/manual/scenarios/merge/squash-edit-aborted.yml
git commit -m "feat(merge): post-merge fires with RESULT=aborted on commit abort"
```

### Task 6.2: Progress messages on cleanup

**Files:**

- Modify: `src/core/worktree/merge.rs` (`execute_cleanup`: print before each
  slow op via the existing presenter)
- Test: assertions added to existing cleanup scenarios

- [ ] **Step 1: Add the prints** before `git worktree remove` and
      `git branch -d`/`-D` in `execute_cleanup`.

- [ ] **Step 2: Update `remove-source.yml`, `remove-source-and-branch.yml`,
      `squash-rb.yml` to assert presence of "Removing worktree" and "Deleting
      branch" lines.**

- [ ] **Step 3: Run, verify pass.**

- [ ] **Step 4: Commit**

```bash
git add src/core/worktree/merge.rs tests/manual/scenarios/merge/
git commit -m "feat(merge): progress messages before slow cleanup ops"
```

---

## Slice 7: Documentation refresh

Goal: Bring `docs/cli/daft-merge.md`, `docs/guide/hooks.md`,
`docs/guide/configuration.md`, and `SKILL.md` in line with the new behavior.
Regenerate man pages and CLI doc stubs.

### Task 7.1: Update CLI reference

**Files:**

- Modify: `docs/cli/daft-merge.md`

- [ ] **Step 1: Update Squash section** to describe always-commit default with
      editor; document `--no-commit` opt-out; `daft.merge.commit = false` config
      alternative.

- [ ] **Step 2: Update Cleanup section** to describe the transactional ordering
      and the justified `-D` for squash-cleanup. Note the stability-check abort
      mode.

- [ ] **Step 3: Update Examples** to include a `--squash -rb` example that
      highlights the editor flow.

- [ ] **Step 4: Update --abort/--continue sections** with the new in-progress
      state.

### Task 7.2: Update hooks guide

**Files:**

- Modify: `docs/guide/hooks.md`

- [ ] **Step 1: Document `RESULT=aborted` in the post-merge env-var table.**

- [ ] **Step 2: Add `DAFT_MERGE_SOURCE_SHAS` if exposed.**

### Task 7.3: Update configuration guide

**Files:**

- Modify: `docs/guide/configuration.md`

- [ ] **Step 1: Add the "default `--squash -rb` recipe"** in the Merge Settings
      section: which keys to set, with caveat about `daft.merge.edit = false`
      for non-interactive use.

- [ ] **Step 2: Document the contradictory-combo error** for `commit = false` +
      `alsoRemoveSourceBranch = true`.

### Task 7.4: Update SKILL.md

**Files:**

- Modify: `SKILL.md`

- [ ] **Step 1: Update Cross-Worktree Merges section** to mention the
      always-commit default and the editor flow.

### Task 7.5: Regenerate man pages and verify

- [ ] **Step 1:** `mise run man:gen`
- [ ] **Step 2:** `mise run docs:cli:verify` (will regenerate the auto stubs as
      needed)
- [ ] **Step 3:** `mise run docs:site:build` (catches dead links)

### Task 7.6: Commit docs

```bash
git add docs/ SKILL.md man/
git commit -m "docs(merge): update for always-commit --squash and transactional cleanup"
```

---

## Final verification

After all slices:

- [ ] `mise run fmt:check` clean
- [ ] `mise run clippy` zero warnings
- [ ] `cargo test --lib` green
- [ ] `mise run test:manual -- --ci` full sweep green
- [ ] Manual smoke test of the original bug:
  ```bash
  cd $(mktemp -d)
  git init test && cd test && echo "x" > x.txt && git add x.txt && git commit -m init
  git checkout -b feature && echo "y" > y.txt && git add y.txt && git commit -m feat
  git checkout main
  daft merge feature --squash -rb --no-edit
  ```
  Expected: clean exit, single commit on main, no `feature` branch or worktree,
  no staged changes left over.

## Out of scope (explicit non-goals)

- Squash-reachability detection for non-daft commits.
- Auto-rolling back the squash commit on cleanup pre-validation failure.
- Scriptable retry of the editor abort path beyond `--continue`.
- Changes to the regular (non-squash) merge commit flow.

These are documented in the spec under Non-goals.
