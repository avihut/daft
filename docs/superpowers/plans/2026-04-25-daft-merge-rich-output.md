# daft merge — rich output and hook parity Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use
> superpowers:subagent-driven-development (recommended) or
> superpowers:executing-plans to implement this plan task-by-task. Steps use
> checkbox (`- [ ]`) syntax for tracking.

**Goal:** Bring `daft merge` output to the same level of polish as
`daft remove`: hook-bracketed cleanup, sink-routed step messages, suppressed git
stdout on success, state-aware step lines, and editor-pause/resume around the
squash commit. Fix the latent bug where `worktree-pre-remove` /
`worktree-post-remove` hooks do not fire on merge cleanup.

**Architecture:** Pure refactor first — split `execute_cleanup` into a planner
(`plan_cleanup`, returns `Vec<CleanupItem>`) and an executor (lives in the
command layer). Then add a `keep_local_branch: bool` knob to
`BranchDeleteParams` and have the merge command's cleanup loop dispatch each
`CleanupItem` through `branch_delete::execute` — gaining hook firing,
sink-routed step messages, and the styled summary line for free. Then
capture-and-suppress git's stdout during the merge phase, replacing it with
single styled step lines, and bracket the squash-commit editor invocation with
the existing `pause_spinner`/`resume_spinner` infrastructure plumbed through a
small extension to the `HookRunner` trait.

**Tech Stack:** Rust + clap derive; existing `GitCommand` wrapper; existing
`Output` trait + `CliOutput` impl; existing `CommandBridge`/`OutputSink`;
existing `HookExecutor` + `CliPresenter`; existing YAML test harness with
`output_contains` / `output_not_contains`; `serial_test` for cwd-sensitive
tests.

**Reference spec:**
[`docs/superpowers/specs/2026-04-25-daft-merge-rich-output-design.md`](../specs/2026-04-25-daft-merge-rich-output-design.md)

---

## Slice 1: Extract `CleanupPlan` from `execute_cleanup` (pure refactor)

Goal: split today's monolithic `execute_cleanup` into a pure planner
(`plan_cleanup`, returns `Vec<CleanupItem>` after pre-validation) and a thin
caller-side executor (initial form: same `println!` + `git.worktree_remove` +
`git.branch_delete` calls, just relocated to the command layer). No behavior
change — every existing test passes unchanged. This sets the stage for Slice 2
to swap the executor for a hook-firing rich-output version.

**Files:**

- Modify: `src/core/worktree/merge.rs` (extract types + `plan_cleanup`;
  deprecate `execute_cleanup` → call sites updated)
- Modify: `src/commands/merge.rs` (call `plan_cleanup` then run mutation loop
  locally; ~2 call sites — one in the post-merge cleanup path around line 555,
  one in the squash-cleanup path around line 750)
- Test: `src/core/worktree/merge.rs` (existing test module — add `plan_cleanup`
  unit tests; keep slice 3 cwd-guard pattern)

### Step 1: Add `CleanupItem` type and stub `plan_cleanup`

- [ ] **Step 1.1: Write the failing test for the basic plan shape**

In `src/core/worktree/merge.rs`'s test module, add:

```rust
#[test]
#[serial]
fn plan_cleanup_returns_item_for_branch_with_worktree() {
    let _cwd = CwdGuard::new();
    let tmp = tempfile::tempdir().unwrap();
    init_repo(tmp.path());
    setup_worktree(tmp.path(), "feature", &tmp.path().join("feature"));
    let project_root = tmp.path().to_path_buf();
    let git = GitCommand::new(true);

    let opts = CleanupOptions {
        remove_worktree: true,
        also_branch: true,
        squash_committed: false,
    };
    let plan = plan_cleanup(
        &["feature".to_string()],
        &opts,
        &git,
        &project_root,
        "main",
    )
    .expect("plan_cleanup should succeed when branch is mergeable");
    assert_eq!(plan.len(), 1);
    let item = &plan[0];
    assert_eq!(item.source, "feature");
    assert!(item.worktree_path.is_some());
    assert_eq!(item.branch_name.as_deref(), Some("feature"));
    assert!(!item.force_delete);
}
```

(Use `init_repo(tmp.path())` (`src/core/worktree/merge.rs:2733`) and
`setup_worktree(tmp.path(), "feature", &tmp.path().join("feature"))`
(`src/core/worktree/merge.rs:4137`) — the same helpers used by
`execute_cleanup_succeeds_when_all_validates_pass` and the slice-3 cleanup
tests.)

- [ ] **Step 1.2: Run test to verify it fails**

Run:
`cargo test --lib plan_cleanup_returns_item_for_branch_with_worktree -- --nocapture`
Expected: FAIL with "cannot find function `plan_cleanup`" (compile error).

- [ ] **Step 1.3: Add `CleanupItem` struct and `plan_cleanup` stub**

In `src/core/worktree/merge.rs`, immediately above the existing
`pub fn execute_cleanup`:

```rust
/// One unit of cleanup work, produced by [`plan_cleanup`] after
/// pre-validation. The caller iterates these and dispatches each to the
/// rich-output cleanup helper (Slice 2). At least one of `worktree_path`
/// or `branch_name` is `Some` for every item produced — items where both
/// are `None` are silently dropped during planning.
#[derive(Debug, Clone)]
pub struct CleanupItem {
    /// Original source spec — used in error messages and the styled
    /// summary line.
    pub source: String,
    /// Worktree to remove, or `None` if `-r` was not effective for this
    /// source (or the source has no worktree).
    pub worktree_path: Option<PathBuf>,
    /// Branch to delete, or `None` if `-b` was not effective for this
    /// source (or the source is not a branch).
    pub branch_name: Option<String>,
    /// `true` iff daft has direct first-party evidence that the squash
    /// commit captured all content on this source branch (Slice 4 of the
    /// 2026-04-25 squash-cleanup design). When set, branch deletion uses
    /// `-D` and skips the reachability check.
    pub force_delete: bool,
}

/// Pre-validate cleanup for `sources` and return the items the caller
/// should mutate. Pure: never touches the filesystem or git refs.
///
/// Returns `Err` with a multi-line context if any source fails
/// pre-validation; the caller must not mutate anything in that case.
pub fn plan_cleanup(
    sources: &[String],
    options: &CleanupOptions,
    git: &GitCommand,
    project_root: &Path,
    target_branch: &str,
) -> Result<Vec<CleanupItem>> {
    // Body added in Step 1.4.
    let _ = (sources, options, git, project_root, target_branch);
    Ok(Vec::new())
}
```

- [ ] **Step 1.4: Move Phase 1 from `execute_cleanup` into `plan_cleanup`**

Copy the existing Phase 1 block from `execute_cleanup` (the loop building
`work_items` + `validation_errors`, the `validation_errors.is_empty()` bail, and
the `SourceClass`-based logic) into `plan_cleanup`. Replace each
`work_items.push(CleanupWork { ... })` with
`items.push(CleanupItem { source: src.clone(), worktree_path: ..., branch_name: ..., force_delete: options.squash_committed })`.
Skip pushing items where both `worktree_path` and `branch_name` would be `None`.

The body should look like:

```rust
pub fn plan_cleanup(
    sources: &[String],
    options: &CleanupOptions,
    git: &GitCommand,
    project_root: &Path,
    target_branch: &str,
) -> Result<Vec<CleanupItem>> {
    let mut items: Vec<CleanupItem> = Vec::with_capacity(sources.len());
    let mut validation_errors: Vec<String> = Vec::new();

    for src in sources {
        let class = classify_source(src, git, project_root);
        match &class {
            SourceClass::BranchWithWorktree { worktree_path, branch } => {
                if options.remove_worktree {
                    match git.has_uncommitted_changes_in(worktree_path) {
                        Ok(true) => validation_errors.push(format!(
                            "source worktree '{}' has uncommitted changes; \
                             commit or stash them before cleanup",
                            worktree_path.display()
                        )),
                        Ok(false) => {}
                        Err(e) => validation_errors.push(format!(
                            "failed to check cleanliness of '{}': {}",
                            worktree_path.display(), e
                        )),
                    }
                }
                if options.also_branch
                    && !options.squash_committed
                    && !is_branch_merged_into(git, branch, target_branch)
                {
                    validation_errors.push(format!(
                        "source branch '{}' is not fully merged into '{}'; \
                         cleanup pre-validation refused branch deletion",
                        branch, target_branch
                    ));
                }
                items.push(CleanupItem {
                    source: src.clone(),
                    worktree_path: options.remove_worktree.then(|| worktree_path.clone()),
                    branch_name: options.also_branch.then(|| branch.clone()),
                    force_delete: options.squash_committed,
                });
            }
            SourceClass::BranchNoWorktree { branch } => {
                if options.also_branch
                    && !options.squash_committed
                    && !is_branch_merged_into(git, branch, target_branch)
                {
                    validation_errors.push(format!(
                        "source branch '{}' is not fully merged into '{}'; \
                         cleanup pre-validation refused branch deletion",
                        branch, target_branch
                    ));
                }
                if options.also_branch {
                    items.push(CleanupItem {
                        source: src.clone(),
                        worktree_path: None,
                        branch_name: Some(branch.clone()),
                        force_delete: options.squash_committed,
                    });
                }
            }
            SourceClass::CommitOrOther => {
                // Nothing to clean up.
            }
        }
    }

    if !validation_errors.is_empty() {
        anyhow::bail!(
            "cleanup pre-validation failed:\n  {}",
            validation_errors.join("\n  ")
        );
    }
    Ok(items)
}
```

- [ ] **Step 1.5: Run the new test — should pass**

Run:
`cargo test --lib plan_cleanup_returns_item_for_branch_with_worktree -- --nocapture`
Expected: PASS.

### Step 2: Add coverage tests for `plan_cleanup`

- [ ] **Step 2.1: Add four more `plan_cleanup` tests covering all cases**

```rust
#[test]
#[serial]
fn plan_cleanup_drops_commit_or_other_silently() {
    let _cwd = CwdGuard::new();
    let tmp = tempfile::tempdir().unwrap();
    init_repo(tmp.path());
    setup_worktree(tmp.path(), "feature", &tmp.path().join("feature"));
    let git = GitCommand::new(true);
    let opts = CleanupOptions {
        remove_worktree: true,
        also_branch: true,
        squash_committed: false,
    };
    let plan = plan_cleanup(
        &["HEAD~0".to_string()],
        &opts,
        &git,
        tmp.path(),
        "main",
    )
    .expect("plan_cleanup should succeed for non-branch source");
    assert!(plan.is_empty(), "non-branch sources produce no items");
}

#[test]
#[serial]
fn plan_cleanup_force_delete_set_when_squash_committed() {
    let _cwd = CwdGuard::new();
    let tmp = tempfile::tempdir().unwrap();
    init_repo(tmp.path());
    setup_worktree(tmp.path(), "feature", &tmp.path().join("feature"));
    let git = GitCommand::new(true);
    let opts = CleanupOptions {
        remove_worktree: true,
        also_branch: true,
        squash_committed: true,
    };
    let plan = plan_cleanup(
        &["feature".to_string()],
        &opts,
        &git,
        tmp.path(),
        "main",
    )
    .expect("squash-committed plan should bypass reachability check");
    assert_eq!(plan.len(), 1);
    assert!(plan[0].force_delete, "squash_committed → force_delete");
}

#[test]
#[serial]
fn plan_cleanup_remove_only_no_branch() {
    let _cwd = CwdGuard::new();
    let tmp = tempfile::tempdir().unwrap();
    init_repo(tmp.path());
    setup_worktree(tmp.path(), "feature", &tmp.path().join("feature"));
    let git = GitCommand::new(true);
    let opts = CleanupOptions {
        remove_worktree: true,
        also_branch: false,
        squash_committed: false,
    };
    let plan = plan_cleanup(
        &["feature".to_string()],
        &opts,
        &git,
        tmp.path(),
        "main",
    )
    .expect("remove-only plan should succeed regardless of merge state");
    assert_eq!(plan.len(), 1);
    assert!(plan[0].worktree_path.is_some());
    assert!(plan[0].branch_name.is_none());
}

#[test]
#[serial]
fn plan_cleanup_validation_errors_short_circuit() {
    let _cwd = CwdGuard::new();
    let tmp = tempfile::tempdir().unwrap();
    init_repo(tmp.path());
    setup_worktree(tmp.path(), "feature", &tmp.path().join("feature"));
    // Mutate the feature worktree to be dirty.
    let feature_worktree = tmp.path().join("feature");
    std::fs::write(feature_worktree.join("uncommitted.txt"), "dirty\n").unwrap();
    let git = GitCommand::new(true);
    let opts = CleanupOptions {
        remove_worktree: true,
        also_branch: true,
        squash_committed: false,
    };
    let err = plan_cleanup(
        &["feature".to_string()],
        &opts,
        &git,
        tmp.path(),
        "main",
    )
    .expect_err("dirty source must fail pre-validation");
    let msg = err.to_string();
    assert!(msg.contains("cleanup pre-validation failed"));
    assert!(msg.contains("uncommitted changes"));
}
```

- [ ] **Step 2.2: Run all `plan_cleanup` tests**

Run: `cargo test --lib plan_cleanup -- --nocapture` Expected: 5 tests pass.

### Step 3: Migrate `execute_cleanup` to a thin wrapper, update callers

- [ ] **Step 3.1: Reduce `execute_cleanup` to call `plan_cleanup` then run Phase
      2**

In `src/core/worktree/merge.rs`, replace the body of `pub fn execute_cleanup`
with:

```rust
pub fn execute_cleanup(
    sources: &[String],
    options: &CleanupOptions,
    git: &GitCommand,
    project_root: &Path,
    target_branch: &str,
) -> Result<()> {
    let plan = plan_cleanup(sources, options, git, project_root, target_branch)?;
    let mut completed: Vec<String> = Vec::new();
    for item in &plan {
        if let Some(ref wt_path) = item.worktree_path {
            println!("Removing worktree at {}...", wt_path.display());
            git.worktree_remove(wt_path, false).with_context(|| {
                let done = if completed.is_empty() {
                    "nothing removed yet".to_string()
                } else {
                    format!("already removed: {}", completed.join(", "))
                };
                format!(
                    "cleanup partially failed: failed to remove worktree '{}' \
                     (source '{}'); {}",
                    wt_path.display(), item.source, done
                )
            })?;
            completed.push(format!("worktree '{}'", wt_path.display()));
        }
    }
    for item in &plan {
        if let Some(ref branch) = item.branch_name {
            println!("Deleting branch {}...", branch);
            git.branch_delete(branch, item.force_delete)
                .with_context(|| {
                    let done = if completed.is_empty() {
                        "nothing removed yet".to_string()
                    } else {
                        format!("already removed: {}", completed.join(", "))
                    };
                    format!(
                        "cleanup partially failed: failed to delete branch '{}' \
                         (source '{}'); {}",
                        branch, item.source, done
                    )
                })?;
            completed.push(format!("branch '{}'", branch));
        }
    }
    Ok(())
}
```

The function is now a thin wrapper. Slice 2 will replace its body with a
per-item dispatch through the rich pipeline.

- [ ] **Step 3.2: Run the entire merge unit-test suite to confirm parity**

Run: `cargo test --lib merge:: -- --nocapture` Expected: All existing merge
tests pass (`execute_cleanup_*`, `plan_cleanup_*`, etc.).

- [ ] **Step 3.3: Run integration test scenarios for merge cleanup**

Run: `mise run test:manual -- --ci squash-rb` Expected: PASS — verifies
end-to-end cleanup still works.

Run: `mise run test:manual -- --ci remove-source` Expected: PASS.

Run: `mise run test:manual -- --ci remove-source-and-branch` Expected: PASS.

- [ ] **Step 3.4: Commit**

```bash
git add src/core/worktree/merge.rs
git commit -m "$(cat <<'EOF'
refactor(merge): split execute_cleanup into planner + executor

Extract Phase 1 (pre-validation) of execute_cleanup into a pure
plan_cleanup function returning Vec<CleanupItem>. execute_cleanup
becomes a thin wrapper that calls the planner and then runs the
existing Phase 2 mutations. No behavior change.

This sets up Slice 2 to replace the executor with a hook-firing,
sink-routed implementation.
EOF
)"
```

---

## Slice 2: Delegate cleanup to `branch_delete::execute` (with new `keep_local_branch` flag)

Goal: Fix the latent bug where `worktree-pre-remove` / `worktree-post-remove` do
not fire on merge cleanup, and bring cleanup output to the same level of polish
as `daft remove`. Add a `keep_local_branch: bool` field to `BranchDeleteParams`
so `daft merge -r` (without `-b`) can remove a worktree without deleting the
local branch. The merge command's cleanup loop builds `BranchDeleteParams` per
`CleanupItem` and invokes `branch_delete::execute` directly, reusing the
existing rich-output pipeline.

**Files:**

- Modify: `src/core/worktree/branch_delete.rs` — add `keep_local_branch: bool`
  to `BranchDeleteParams`; thread it through `validate_branches` and
  `delete_single_branch` so branch-deletion validation + steps are skipped when
  set
- Modify: `src/commands/branch_delete.rs` — set `keep_local_branch: false` at
  the existing call site (no behavior change)
- Modify: `src/commands/merge.rs` — replace inline cleanup with `plan_cleanup` +
  per-item `branch_delete::execute` calls; build a `CommandBridge` for each call
- Modify: `src/core/worktree/merge.rs` — `execute_cleanup` is no longer called
  by command layer; reduce to test-only `pub(crate)` (callers and existing unit
  tests verify in Step 4)
- Test: `src/core/worktree/branch_delete.rs` (unit tests for
  `keep_local_branch`)
- Create: `tests/manual/scenarios/merge/merge-fires-worktree-remove-hooks.yml`
- Create: `tests/manual/scenarios/merge/merge-pre-remove-hook-warns.yml`

**Reference call site in `commands/branch_delete.rs:88-116`** — the existing
pattern for invoking `branch_delete::execute` with a `CommandBridge`. Merge
cleanup follows the same shape.

### Step 1: Add `keep_local_branch` to `BranchDeleteParams`

- [ ] **Step 1.1: Write the failing test for `keep_local_branch`**

In `src/core/worktree/branch_delete.rs`'s test module, add (use
`serial_test::serial` and the `CwdGuard` pattern from `core/worktree/merge.rs`'s
tests if branch_delete's tests don't already have one):

```rust
#[test]
#[serial]
fn keep_local_branch_removes_worktree_only() {
    let _cwd = CwdGuard::new();
    let tmp = tempfile::tempdir().unwrap();
    init_repo(tmp.path());
    setup_worktree(tmp.path(), "feature", &tmp.path().join("feature"));
    std::env::set_current_dir(tmp.path()).unwrap();

    let params = BranchDeleteParams {
        branches: vec!["feature".to_string()],
        force: false,
        use_gitoxide: true,
        is_quiet: true,
        remote_name: "origin".to_string(),
        delete_remote: false,
        remote_only: false,
        keep_local_branch: true,
        prune_cd_target: PruneCdTarget::Default,
    };
    let mut output = TestOutput::new();
    let executor = HookExecutor::new(HooksConfig::default()).unwrap();
    let mut bridge = CommandBridge::new(&mut output, executor);
    let result = execute(&params, &mut bridge).expect("keep_local_branch should succeed");

    assert_eq!(result.deletions.len(), 1);
    assert!(result.deletions[0].worktree_removed, "worktree must be removed");
    assert!(!result.deletions[0].branch_deleted, "branch must NOT be deleted");
    // Verify branch still exists on disk.
    let git = GitCommand::new(true);
    assert!(
        git.show_ref_exists("refs/heads/feature").unwrap_or(false),
        "feature branch must still exist after keep_local_branch=true"
    );
}
```

(`TestOutput` is exported from `src/output/mod.rs:30`
(`pub use test::{OutputEntry, TestOutput};`). For repo setup, mirror the helpers
`branch_delete.rs`'s existing tests already use — run
`grep -n "fn init_test\|fn setup_test\|fn make_repo" src/core/worktree/branch_delete.rs`
to find them. If `CwdGuard` isn't yet present in this file, add the struct from
`core/worktree/merge.rs`'s test module (search for `struct CwdGuard`).)

- [ ] **Step 1.2: Run the test to verify it fails**

Run: `cargo test --lib keep_local_branch_removes_worktree_only -- --nocapture`
Expected: FAIL with "no field `keep_local_branch`" (compile error).

- [ ] **Step 1.3: Add `keep_local_branch` to `BranchDeleteParams`**

In `src/core/worktree/branch_delete.rs`, around line 30 (after `remote_only`):

```rust
pub struct BranchDeleteParams {
    pub branches: Vec<String>,
    pub force: bool,
    pub use_gitoxide: bool,
    pub is_quiet: bool,
    pub remote_name: String,
    pub delete_remote: bool,
    pub remote_only: bool,
    /// Skip local branch deletion and remote branch deletion. Only the
    /// worktree is removed, with `worktree-pre-remove` /
    /// `worktree-post-remove` hooks firing as usual. Used by `daft merge -r`
    /// (without `-b`) to remove a source worktree while keeping the local
    /// branch ref intact.
    pub keep_local_branch: bool,
    pub prune_cd_target: PruneCdTarget,
}
```

In `src/commands/branch_delete.rs`, update the existing call site (around
line 88) to populate the new field:

```rust
let params = branch_delete::BranchDeleteParams {
    branches: args.branches.clone(),
    force: args.force,
    use_gitoxide: settings.use_gitoxide,
    is_quiet: args.quiet,
    remote_name: settings.remote.clone(),
    delete_remote: if args.local {
        false
    } else if args.remote {
        true
    } else {
        settings.branch_delete_remote
    },
    remote_only: args.remote,
    keep_local_branch: false,        // existing daft-remove path always deletes the branch
    prune_cd_target: settings.prune_cd_target,
};
```

- [ ] **Step 1.4: Thread `keep_local_branch` into `delete_single_branch`**

In `src/core/worktree/branch_delete.rs`, modify `delete_single_branch` (around
line 788) to accept the new flag and skip Steps 2 and 4 when set:

```rust
fn delete_single_branch(
    ctx: &BranchDeleteContext,
    branch: &ValidatedBranch,
    force: bool,
    delete_remote: bool,
    remote_only: bool,
    keep_local_branch: bool,
    sink: &mut (impl ProgressSink + HookRunner),
) -> DeletionResult {
    // ... existing setup unchanged ...

    // Step 1: Run pre-remove hook (only if worktree exists)
    if let Some(ref wt_path) = branch.worktree_path {
        run_removal_hook(HookType::PreRemove, ctx, wt_path, &branch.name, sink);
    }

    // Step 2: Delete remote branch (skipped under keep_local_branch — merge
    // cleanup never touches the remote).
    if !keep_local_branch && !branch.worktree_only && (delete_remote || remote_only) {
        // ... existing remote-delete block unchanged ...
    }

    if remote_only {
        // ... existing remote_only short-circuit unchanged ...
    }

    // Step 3: Remove worktree (unchanged).
    if let Some(ref wt_path) = branch.worktree_path {
        // ... existing block unchanged ...
    }

    // Step 4: Delete local branch — skipped under keep_local_branch.
    if !keep_local_branch && !branch.worktree_only {
        sink.on_step(&format!("Deleting local branch {}...", branch.name));
        match ctx.git.branch_delete(&branch.name, true) {
            // ... existing block unchanged ...
        }
    }

    // Step 5: Run post-remove hook (unchanged).
    if has_worktree {
        if let Some(ref wt_path) = branch.worktree_path {
            run_removal_hook(HookType::PostRemove, ctx, wt_path, &branch.name, sink);
        }
    }
    result
}
```

Update the two call sites of `delete_single_branch` (in `execute_deletions`
around lines 711-718 and 752-759) to pass `params.keep_local_branch`.

- [ ] **Step 1.5: Skip branch-deletion validation when `keep_local_branch` is
      set**

In `src/core/worktree/branch_delete.rs`, modify `validate_branches` (around
line 332) to accept `keep_local_branch: bool` and skip the merged-into-default
and remote-sync checks when set. The branch-existence check still runs (we need
a real branch to find its worktree). The dirty-worktree check still runs (the
user explicitly asked to remove the worktree).

```rust
fn validate_branches(
    ctx: &BranchDeleteContext,
    branches: &[String],
    force: bool,
    remote_only: bool,
    keep_local_branch: bool,
    worktree_map: &HashMap<String, PathBuf>,
    current_wt_path: Option<&PathBuf>,
    current_branch: Option<&str>,
    sink: &mut dyn ProgressSink,
) -> (Vec<ValidatedBranch>, Vec<ValidationError>) {
    // ... existing setup unchanged ...

    for branch in branches {
        sink.on_step(&format!("Validating branch '{branch}'..."));

        // Existing remote_only short-circuit stays.
        if remote_only { /* unchanged */ }

        // Branch-existence check stays.
        // Default-branch protection stays.
        // Uncommitted-changes check stays.

        // Skip merged-into-default check under force OR keep_local_branch.
        if !force && !keep_local_branch {
            // ... existing merged-into-default check ...
        }

        // Skip remote-sync check under force OR keep_local_branch.
        if !force && !keep_local_branch {
            // ... existing remote-sync check ...
        }

        // ... rest of the loop unchanged ...
    }
    // ... return value unchanged ...
}
```

Update the call site in `execute()` (around line 168) to pass
`params.keep_local_branch`.

- [ ] **Step 1.6: Run the failing test — must now pass**

Run: `cargo test --lib keep_local_branch_removes_worktree_only -- --nocapture`
Expected: PASS.

- [ ] **Step 1.7: Add coverage tests**

```rust
#[test]
#[serial]
fn keep_local_branch_skips_merged_into_default_check() {
    // Branch with a unique commit not merged into main — would normally
    // fail validation. With keep_local_branch=true, the check is skipped.
    let _cwd = CwdGuard::new();
    let tmp = tempfile::tempdir().unwrap();
    init_repo(tmp.path());
    let feat_wt = tmp.path().join("feature");
    setup_worktree(tmp.path(), "feature", &feat_wt);
    // Add a commit on feature so it's ahead of main (unmerged).
    crate::testing::shell::ShellCommand::new("git")
        .args(["commit", "--allow-empty", "-q", "-m", "feature work"])
        .current_dir(&feat_wt)
        .env("GIT_AUTHOR_NAME", "Test")
        .env("GIT_AUTHOR_EMAIL", "test@test.com")
        .env("GIT_COMMITTER_NAME", "Test")
        .env("GIT_COMMITTER_EMAIL", "test@test.com")
        .status().unwrap();
    std::env::set_current_dir(tmp.path()).unwrap();

    let params = BranchDeleteParams {
        branches: vec!["feature".to_string()],
        force: false,
        use_gitoxide: true,
        is_quiet: true,
        remote_name: "origin".to_string(),
        delete_remote: false,
        remote_only: false,
        keep_local_branch: true,
        prune_cd_target: PruneCdTarget::Default,
    };
    let mut output = TestOutput::new();
    let executor = HookExecutor::new(HooksConfig::default()).unwrap();
    let mut bridge = CommandBridge::new(&mut output, executor);
    let result = execute(&params, &mut bridge).unwrap();

    assert!(result.validation_errors.is_empty(), "merged-into-default check skipped under keep_local_branch");
    assert_eq!(result.deletions.len(), 1);
    assert!(result.deletions[0].worktree_removed);
    assert!(!result.deletions[0].branch_deleted);
}
```

(The remote-not-touched assertion is covered by the
`keep_local_branch_removes_worktree_only` test in Step 1.1 — adding
`delete_remote: true` to that test's params and asserting `!remote_deleted`
would duplicate the structural coverage. If a remote-bearing scenario is
desired, build the remote separately at implementation time using whatever
pattern `core/worktree/branch_delete.rs`'s existing tests already use for
remotes.)

- [ ] **Step 1.8: Run all branch_delete tests**

Run: `cargo test --lib branch_delete -- --nocapture` Expected: All existing
tests still pass; 2 new tests pass (`keep_local_branch_removes_worktree_only`,
`keep_local_branch_skips_merged_into_default_check`).

- [ ] **Step 1.9: Commit**

```bash
git add src/core/worktree/branch_delete.rs src/commands/branch_delete.rs
git commit -m "$(cat <<'EOF'
feat(branch-delete): add keep_local_branch flag to BranchDeleteParams

When set, validation skips merged-into-default and remote-sync checks,
and the deletion phase skips remote and local branch deletion. Only
the worktree is removed (with worktree-pre/post-remove hooks).

Used by `daft merge -r` (without `-b`) to remove a source worktree
while keeping the local branch intact.
EOF
)"
```

### Step 2: Wire `commands::merge::run` to call `branch_delete::execute` per cleanup item

- [ ] **Step 2.1: Replace inline cleanup with `plan_cleanup` + per-item
      `branch_delete::execute`**

In `src/commands/merge.rs`, locate both `execute_cleanup` invocations (currently
around lines 555 and 750 — verify with
`grep -n "execute_cleanup" src/commands/merge.rs`). Replace each block with:

```rust
let cleanup_result: Result<()> = (|| {
    let plan = crate::core::worktree::merge::plan_cleanup(
        &params.sources,
        &cleanup_opts,
        &git,
        &project_root,
        &outcome.target_branch,
    )?;

    for item in &plan {
        // Skip pure CommitOrOther items (planner already drops these,
        // but defend against future changes).
        if item.worktree_path.is_none() && item.branch_name.is_none() {
            continue;
        }

        // Build BranchDeleteParams matching the flag matrix in the spec:
        // -r without -b      → keep_local_branch = true
        // -rb (regular)      → keep_local_branch = false, force = false
        // -rb (squash)       → keep_local_branch = false, force = true
        let bd_params = crate::core::worktree::branch_delete::BranchDeleteParams {
            branches: vec![item
                .branch_name
                .clone()
                .unwrap_or_else(|| item.source.clone())],
            force: item.force_delete,
            use_gitoxide: settings.use_gitoxide,
            is_quiet: false,
            remote_name: settings.remote.clone(),
            delete_remote: false,
            remote_only: false,
            keep_local_branch: item.branch_name.is_none(),
            prune_cd_target: settings.prune_cd_target,
        };

        let executor = HookExecutor::new(HooksConfig::default())?;
        let mut bridge = CommandBridge::new(&mut *output, executor);
        let bd_result =
            crate::core::worktree::branch_delete::execute(&bd_params, &mut bridge)?;

        if !bd_result.validation_errors.is_empty() {
            for err in &bd_result.validation_errors {
                output.error(&format!(
                    "cleanup of '{}' failed: {}",
                    err.branch, err.message
                ));
            }
            anyhow::bail!(
                "cleanup pre-validation failed for {} source(s)",
                bd_result.validation_errors.len()
            );
        }
    }
    Ok(())
})();
```

(`output` is already a `&mut dyn Output` available in scope;
`settings.use_gitoxide`, `settings.remote`, `settings.prune_cd_target`, `git`,
`project_root`, and `outcome.target_branch` are all already in scope at the
existing call sites.)

- [ ] **Step 2.2: Re-route the `Squash merged and cleaned up X.` final line**

Around line 755 (after a successful squash + cleanup), replace `println!` with
`output.result(...)`:

```rust
// Before:
println!("Squash merged and cleaned up {sources_display}.");

// After:
output.result(&format!("Squash merged and cleaned up {sources_display}."));
```

- [ ] **Step 2.3: Run unit tests**

Run: `mise run test:unit` Expected: All tests pass.

- [ ] **Step 2.4: Run existing merge integration scenarios**

Run: `mise run test:manual -- --ci squash-rb` Run:
`mise run test:manual -- --ci remove-source` Run:
`mise run test:manual -- --ci remove-source-and-branch` Run:
`mise run test:manual -- --ci continue-squash-staged` Expected: All pass.

### Step 3: Add new merge scenarios for the hook-firing surface

- [ ] **Step 3.1: Author `merge-fires-worktree-remove-hooks.yml`**

Create `tests/manual/scenarios/merge/merge-fires-worktree-remove-hooks.yml`:

```yaml
name: merge fires worktree-pre/post-remove hooks on cleanup
description: |
  When `daft merge X -r` cleans up a source worktree, the user-installed
  worktree-pre-remove and worktree-post-remove hooks must fire. Regression
  for the latent bug where merge cleanup bypassed these hooks.

setup:
  - command: daft init
  - command: |
      mkdir -p .daft/hooks
      cat > .daft/hooks/worktree-pre-remove <<'EOF'
      #!/bin/sh
      echo "PRE_REMOVE_FIRED:$DAFT_WORKTREE_PATH"
      EOF
      cat > .daft/hooks/worktree-post-remove <<'EOF'
      #!/bin/sh
      echo "POST_REMOVE_FIRED:$DAFT_WORKTREE_PATH"
      EOF
      chmod +x .daft/hooks/worktree-pre-remove .daft/hooks/worktree-post-remove
      touch .daft/hooks/.trusted
  - command: git checkout -b feature
  - command: |
      git config user.email test@test.com
      git config user.name Test
      echo content > new-file
      git add new-file
      git commit -m feature-work
  - command: git checkout main
  - command: daft go feature
  - command: daft go main

scenarios:
  fires-hooks-on-merge-r:
    steps:
      - command: daft merge feature -r --no-edit
        output_contains:
          - "PRE_REMOVE_FIRED"
          - "POST_REMOVE_FIRED"
          - "Removed worktree for feature"
        output_not_contains:
          - "Updating "
          - "Squash commit -- not updating HEAD"
```

(Adapt setup to whatever helper macro/format the existing scenarios use — e.g.
some scenarios use `daft sandbox` blocks. Match the pattern of
`tests/manual/scenarios/merge/squash-rb.yml`.)

- [ ] **Step 3.2: Author `merge-pre-remove-hook-warns.yml`**

```yaml
name: merge pre-remove hook abort halts cleanup
description: |
  When the worktree-pre-remove hook exits non-zero, cleanup for that
  source is skipped. Merge commit on target stays. Exit non-zero.

setup:
  - command: daft init
  - command: |
      mkdir -p .daft/hooks
      cat > .daft/hooks/worktree-pre-remove <<'EOF'
      #!/bin/sh
      echo "REFUSING_TO_REMOVE"
      exit 1
      EOF
      chmod +x .daft/hooks/worktree-pre-remove
      touch .daft/hooks/.trusted
  - command: git checkout -b feature
  - command: |
      git config user.email test@test.com
      git config user.name Test
      echo content > new-file
      git add new-file
      git commit -m feature-work
  - command: git checkout main
  - command: daft go feature
  - command: daft go main

scenarios:
  hook-abort-halts-cleanup:
    steps:
      - command: daft merge feature -r --no-edit
        expect_failure: true
        output_contains:
          - "REFUSING_TO_REMOVE"
          - "worktree-pre-remove hook failed"
      - command: git rev-parse HEAD
        # Merge commit must still exist on main.
        output_contains:
          - "[0-9a-f]{7,}"
      - command: git worktree list
        # Source worktree must still exist (cleanup was halted).
        output_contains:
          - "feature"
```

- [ ] **Step 3.3: Run the new scenarios**

Run: `mise run test:manual -- --ci merge-fires-worktree-remove-hooks` Run:
`mise run test:manual -- --ci merge-pre-remove-hook-warns` Expected: Both PASS.

### Step 4: Decommission the now-unused `execute_cleanup` body

- [ ] **Step 4.1: Determine whether `execute_cleanup` is still referenced**

Run: `grep -rn "execute_cleanup\b" src/ tests/`

If the only remaining references are within `execute_cleanup`'s own definition
and its existing tests (`execute_cleanup_succeeds_when_all_validates_pass`,
`execute_cleanup_refuses_dirty_source_worktree`,
`execute_cleanup_refuses_unmerged_branch_when_not_squash`), keep
`execute_cleanup` as a test-only helper (cfg-gated) and reduce its visibility to
`pub(crate)`. If callers exist outside tests, leave the wrapper public.

- [ ] **Step 4.2: Run full unit-test suite**

Run: `mise run test:unit` Expected: PASS.

### Step 5: Commit the merge integration changes

- [ ] **Step 5.1: Commit**

```bash
git add src/commands/merge.rs src/core/worktree/merge.rs tests/manual/scenarios/merge/merge-fires-worktree-remove-hooks.yml tests/manual/scenarios/merge/merge-pre-remove-hook-warns.yml
git commit -m "$(cat <<'EOF'
fix(merge): delegate cleanup to branch_delete::execute

Replace the inline `git.worktree_remove` + `git.branch_delete` loop
in execute_cleanup with per-item branch_delete::execute calls. Each
item gets the appropriate flag combination per the cleanup matrix
(keep_local_branch=true for `-r` without `-b`, force=true for the
squash-committed `-rb` path, etc.).

Fixes the latent bug where `daft merge -r` / `-rb` removed source
worktrees without firing the user's configured worktree-pre-remove /
worktree-post-remove hooks. As a side benefit, cleanup output now
matches `daft remove`'s polish (hook box, spinner, sink-routed step
messages, styled `Deleted X (worktree, local branch)` summary line).
EOF
)"
```

---

## Slice 3: Capture-and-suppress git stdout during merge phase

Goal: Wrap git's merge / squash / commit invocations with stdout/stderr capture
so the user no longer sees `Updating ...`, `Fast-forward`,
`Squash commit -- not updating HEAD`, `[feature 8931f31] ...` on the success
path. On failure, dump the captured buffer to stderr (after the spinner stops,
to avoid carriage-return mangling). With `--verbose`, dump on success too.

**Files:**

- Modify: `src/git/command.rs` (add capture-mode variants to merge/commit
  invocations, OR introduce a `capture_output: bool` config knob — verify what's
  already there)
- Modify: `src/core/worktree/merge.rs` (capture in core, return captured buffer
  in `MergeOutcome`)
- Modify: `src/commands/merge.rs` (start spinner, drive merge, finish spinner,
  decide on display)
- Create: `tests/manual/scenarios/merge/merge-rich-output-on-success.yml`
- Create: `tests/manual/scenarios/merge/merge-verbose-shows-git-output.yml`

### Step 1: Add captured-output field to `StartOutcome`

The existing return type from `core::worktree::merge::execute_start` is
`Result<StartOutcome>` (`src/core/worktree/merge.rs:969`). `StartOutcome` is a
flat struct with fields like `already_up_to_date`, `failed`, `target_path`,
`conflicted_files`, `squash_staged_only`, `squash_commit_sha`, `commit_aborted`,
`target_branch`, `source_shas`. We extend it (not wrap it) with the captured
buffer plus two new fields needed by Slice 4 to distinguish FF from
merge-commit.

- [ ] **Step 1.1: Add `captured_git_output`, `was_fast_forward`,
      `merge_commit_sha` to `StartOutcome`**

In `src/core/worktree/merge.rs` (around line 969-1028), add three fields to
`StartOutcome`:

```rust
pub struct StartOutcome {
    // ... existing fields up through source_shas ...

    /// Combined stdout+stderr captured from `git merge` / `git merge --squash`
    /// / `git commit` invocations during the merge phase. Suppressed by the
    /// command layer on success; dumped to stderr on failure (after the
    /// spinner stops) and on `--verbose` regardless of outcome.
    pub captured_git_output: Vec<u8>,

    /// True iff the regular (non-squash) merge fast-forwarded the target.
    /// `false` for non-FF merge commits, squash, conflict, AUTD, and any
    /// failure path. Used by the command layer to render
    /// `Fast-forwarded <target> to <sha>` instead of
    /// `Merged <source> into <target> (commit <sha>)`.
    pub was_fast_forward: bool,

    /// The SHA of the resulting commit on `target_branch` for non-squash
    /// merges. `Some` only when the regular merge succeeded — both FF and
    /// merge-commit paths populate this. `None` for squash (use
    /// `squash_commit_sha`), AUTD, conflict, and failure paths.
    pub merge_commit_sha: Option<String>,
}
```

Update the `Default` derive automatically handles the new fields (`Vec::new()`,
`false`, `None`).

- [ ] **Step 1.2: Inspect existing git merge invocations in core**

Run:
`grep -n "Command::new(\"git\")\|GitCommand\|\.status()\|\.output()" src/core/worktree/merge.rs | head -40`

Note every site that invokes git for the merge phase: regular merge, squash
merge, post-squash commit. These all need to switch from `.status()` (inherits
stdio) to `.output()` (captures), with bytes appended to
`outcome.captured_git_output`.

- [ ] **Step 1.3: Convert merge invocations to capture mode**

For each merge-related git invocation in `src/core/worktree/merge.rs`, replace
`.status()` + `Stdio::inherit()` with `.output()` and accumulate bytes:

```rust
let cmd_output = std::process::Command::new("git")
    .arg("merge")
    .args(/* existing args */)
    .current_dir(/* existing cwd */)
    .output()
    .context("failed to invoke git merge")?;

outcome.captured_git_output.extend_from_slice(&cmd_output.stdout);
outcome.captured_git_output.extend_from_slice(&cmd_output.stderr);

if !cmd_output.status.success() {
    outcome.failed = true;
    // existing failure-path handling continues; captured_git_output is
    // returned in the StartOutcome so the command layer can dump it.
}
```

**Exception — the editor-opening squash commit (`git commit` without `-m` /
`--no-edit`):** this invocation MUST inherit stdio (so `$EDITOR` can attach to
the terminal). Do not capture this one. Slice 5 wires the spinner pause/resume
around it; this slice leaves it inherit-only. The pre-edit/post-edit
`git commit -m "..."` (with explicit message) DOES capture.

- [ ] **Step 1.4: Detect FF vs merge-commit in the regular-merge path**

In the regular (non-squash) merge success path, after the merge succeeds,
populate `was_fast_forward` and `merge_commit_sha`:

```rust
// After git merge succeeds (regular, non-squash path):
let head_sha = git.rev_parse("HEAD")?;
outcome.merge_commit_sha = Some(head_sha.clone());

// Detect FF: HEAD before == merge-base; HEAD after == source tip.
// Equivalent test: parent count of HEAD is 1 (FF) vs 2 (merge commit).
let parent_count = git.commit_parent_count("HEAD").unwrap_or(2);
outcome.was_fast_forward = parent_count == 1;
```

(The exact helper names — `rev_parse`, `commit_parent_count` — should match what
already exists in `GitCommand`. Run
`grep -n "fn rev_parse\|fn commit_parent_count\|fn parent_count" src/git/command.rs src/git/oxide.rs`
to find or add them.)

### Step 2: Drive spinner + display from the command layer

- [ ] **Step 2.1: Wrap the merge invocation in
      `start_spinner`/`finish_spinner`**

In `src/commands/merge.rs::run`, immediately before calling `execute_start`
(around line 580 — verify with `grep -n "execute_start" src/commands/merge.rs`):

```rust
let spinner_label = if flags.squash == Some(true) {
    format!("Squashing {} into {}...", args.sources.join(", "), target_branch_label)
} else {
    format!("Merging {} into {}...", args.sources.join(", "), target_branch_label)
};
output.start_spinner(&spinner_label);
let outcome_result = crate::core::worktree::merge::execute_start(&params, &mut runner);
output.finish_spinner();
```

`target_branch_label` is the resolved target name. If not yet computed at this
point in `run()`, derive from `args.into.clone().unwrap_or_default()` or move
the spinner start to after target resolution.

- [ ] **Step 2.2: Dump captured output on failure (after spinner stops)**

```rust
let outcome = match outcome_result {
    Ok(o) => o,
    Err(e) => {
        // Spinner already finished above — safe to write stderr without
        // carriage-return mangling.
        // Best-effort: the error path may not have populated captured_git_output
        // (anyhow chain doesn't carry it). The captured bytes are on the
        // StartOutcome only when execute_start returned Ok with failed=true,
        // OR the error type carries them. For the common case where the merge
        // truly errored before producing a StartOutcome, the error itself has
        // git's diagnostic.
        return Err(e);
    }
};

// Soft-failure path: outcome.failed is true but execute_start returned Ok
// (e.g. conflict, abort). Dump the captured buffer so the user sees git's
// view of what happened.
if outcome.failed && !outcome.captured_git_output.is_empty() {
    eprint!("{}", String::from_utf8_lossy(&outcome.captured_git_output));
}

// Verbose path: dump on success too.
if args.verbose && !outcome.captured_git_output.is_empty() && !outcome.failed {
    eprint!("{}", String::from_utf8_lossy(&outcome.captured_git_output));
}
```

(`outcome.failed` is the existing flag at `StartOutcome:976`. `commit_aborted`
is a more specific subset; both populate `captured_git_output`.)

- [ ] **Step 2.3: Move SQUASH_MSG / commit-message editor invocation outside the
      spinner**

If the squash commit's interactive editor invocation lives inside the same
execute_start call as the merge, the spinner needs to pause around it. Slice 5
handles this — for now, ensure the existing editor invocation is not captured
(see Step 1.3 exception). If `execute_start` already invokes `git commit` with
inherited stdio, no change needed in this slice.

### Step 3: Add scenarios

- [ ] **Step 3.1: Author `merge-rich-output-on-success.yml`**

```yaml
name: merge phase suppresses raw git stdout on success
description: |
  Without --verbose, `daft merge` produces no raw git stdout
  (`Updating ...`, `Fast-forward`, `[branch sha] ...`). Only daft's
  styled step + summary lines render.

setup:
  - command: daft init
  - command: git checkout -b feature
  - command: |
      git config user.email test@test.com
      git config user.name Test
      echo content > new-file
      git add new-file
      git commit -m feature-work
  - command: git checkout main

scenarios:
  no-raw-git-output:
    steps:
      - command: daft merge feature --no-edit
        output_not_contains:
          - "Updating "
          - "Fast-forward"
          - "Squash commit -- not updating HEAD"
        output_contains:
          - "Merge complete"
```

- [ ] **Step 3.2: Author `merge-verbose-shows-git-output.yml`**

```yaml
name: merge --verbose shows git output even on success
description: |
  With --verbose, the captured git buffer is dumped to stderr after
  the spinner stops, even on success.

setup:
  # same as merge-rich-output-on-success.yml

scenarios:
  verbose-shows-git:
    steps:
      - command: daft merge feature --no-edit --verbose
        output_contains:
          - "Updating "
          - "Fast-forward"
          - "Merge complete"
```

- [ ] **Step 3.3: Run scenarios**

Run: `mise run test:manual -- --ci merge-rich-output-on-success` Run:
`mise run test:manual -- --ci merge-verbose-shows-git-output` Expected: Both
PASS.

- [ ] **Step 3.4: Commit**

```bash
git add src/git/command.rs src/core/worktree/merge.rs src/commands/merge.rs tests/manual/scenarios/merge/merge-rich-output-on-success.yml tests/manual/scenarios/merge/merge-verbose-shows-git-output.yml
git commit -m "$(cat <<'EOF'
feat(merge): suppress git stdout on success, dump on failure

Capture stdout+stderr of git merge / squash / commit invocations into
a CapturedGitOutput buffer. On success, discard. On failure, dump to
stderr after the spinner stops. With --verbose, dump on success too.

Removes the noisy `Updating ...` / `Fast-forward` / `Squash commit --
not updating HEAD` / `[branch sha] ...` lines from the happy path,
matching daft remove's zero-git-noise feel.
EOF
)"
```

---

## Slice 4: State-aware step messages for the merge phase

Goal: Replace the now-suppressed git output with single styled lines emitted via
`output.step(...)` (or the equivalent in the codebase). One per source for
octopus merges.

**Files:**

- Modify: `src/commands/merge.rs` (add step emission after merge phase completes
  successfully)
- Update: existing scenarios that assert on legacy success lines, if any

### Step 1: Emit step messages from `StartOutcome` flags

`StartOutcome` is a flat struct (no Kind enum). Step-message dispatch keys off
the existing flags + the two new fields added in Slice 3 (`was_fast_forward`,
`merge_commit_sha`).

- [ ] **Step 1.1: Add a `short_sha` helper in `src/commands/merge.rs`**

```rust
fn short_sha(sha: &str) -> &str {
    &sha[..sha.len().min(7)]
}
```

- [ ] **Step 1.2: Emit a step message after a successful merge phase**

In `src/commands/merge.rs::run`, immediately after the spinner finishes (Slice 3
Step 2.1) and the `outcome` is bound (Slice 3 Step 2.2), before the cleanup
block:

```rust
// Skip step emission on failure / conflict / abort — those have their own
// state-aware messaging downstream.
if !outcome.failed {
    let sources_label = sources_for_message.join(", ");
    if outcome.already_up_to_date {
        output.step(&format!(
            "{} is already up to date with {}",
            outcome.target_branch, sources_label
        ));
    } else if outcome.squash_staged_only {
        output.step(&format!("Squash staged on {}", outcome.target_branch));
    } else if let Some(sha) = &outcome.squash_commit_sha {
        output.step(&format!(
            "Squashed {} into {} (commit {})",
            sources_label,
            outcome.target_branch,
            short_sha(sha)
        ));
    } else if outcome.was_fast_forward {
        if let Some(sha) = &outcome.merge_commit_sha {
            output.step(&format!(
                "Fast-forwarded {} to {}",
                outcome.target_branch,
                short_sha(sha)
            ));
        }
    } else if let Some(sha) = &outcome.merge_commit_sha {
        output.step(&format!(
            "Merged {} into {} (commit {})",
            sources_label,
            outcome.target_branch,
            short_sha(sha)
        ));
    }
}
```

(`sources_for_message` already exists in `run()` for the existing
`Squash merged and cleaned up` line. If the binding name differs at the new
emission site, use the existing variable for source-display formatting — match
what the trailing success line does.)

- [ ] **Step 1.2: Run unit tests + existing integration scenarios**

Run: `mise run test:unit` Run: `mise run test:manual -- --ci basic` Run:
`mise run test:manual -- --ci ff` Run: `mise run test:manual -- --ci ff-only`
Run: `mise run test:manual -- --ci squash` Expected: All PASS. Any scenario
asserting on the previous final-line text needs an update — search for
`Merge complete` / `Squash merged` and adjust if necessary.

- [ ] **Step 1.3: Update `merge-rich-output-on-success.yml` to assert on the new
      step lines**

Add to the scenario from Slice 3:

```yaml
output_contains:
  - "Merged feature into main"
  - "Merge complete"
```

(Step line + final summary line both render.)

- [ ] **Step 1.4: Commit**

```bash
git add src/commands/merge.rs tests/manual/scenarios/merge/merge-rich-output-on-success.yml
git commit -m "$(cat <<'EOF'
feat(merge): emit state-aware step messages after merge phase

Replace git's suppressed multi-line stdout with single styled step
lines: `Fast-forwarded X to abc`, `Merged X into Y (commit abc)`,
`Squashed X into Y (commit abc)`, `Squash staged on Y`,
`X is already up to date with Y`.
EOF
)"
```

---

## Slice 5: Editor pause/resume around squash commit

Goal: When the squash commit step opens `$EDITOR`, the merge-phase spinner
pauses cleanly so the editor owns the terminal. Bracket with `pause_spinner` /
`resume_spinner` (existing infra at `src/output/cli.rs:252` / `:262`).

**Files:**

- Modify: `src/commands/merge.rs` or `src/core/worktree/merge.rs` (wherever
  `git commit` is invoked for squash)
- Test: a new unit test mirroring
  `progress.rs::run_hook_brackets_executor_with_spinner_pause_resume`

### Step 1: Bracket the editor invocation

- [ ] **Step 1.1: Locate the squash commit invocation**

Run:
`grep -n "git commit\|fn finish_squash\|squash_commit\|run_squash\|finish_squash_staged" src/core/worktree/merge.rs src/commands/merge.rs | head`

The squash-commit invocation lives in `core::worktree::merge` (the squash +
commit path inside `execute_start_in_worktree` and `finish_squash_staged`).
Since `core` doesn't have an `Output`, we extend the existing `HookRunner` trait
(already passed into `execute_start` by the command layer via `MergeHookRunner`)
with two new methods that the command layer's runner implements via the existing
`pause_spinner` / `resume_spinner` on `Output`.

- [ ] **Step 1.2: Add `pause_spinner` / `resume_spinner` to `HookRunner`**

In `src/core/worktree/merge.rs`, around line 458 where `HookRunner` is defined:

```rust
pub trait HookRunner {
    fn fire_pre_merge(&mut self, ctx: &MergeHookContext) -> Result<()>;
    fn fire_post_merge(&mut self, ctx: &MergeHookContext) -> Result<()>;
    /// Pause any active progress indicator (spinner) so that an
    /// interactive subprocess (typically `$EDITOR` for the squash
    /// commit) can attach to the terminal cleanly. Resumed by
    /// [`HookRunner::resume_spinner`].
    fn pause_spinner(&mut self) {}
    /// Resume the spinner paused by [`HookRunner::pause_spinner`].
    /// No-op if no spinner was active.
    fn resume_spinner(&mut self) {}
}
```

Default empty implementations keep the existing `NoopHookRunner` test stub
(around line 467) compiling without changes.

In `src/commands/merge.rs::MergeHookRunner` (around line 900), wire the new
methods:

```rust
impl<'a> HookRunner for MergeHookRunner<'a> {
    fn fire_pre_merge(&mut self, ctx: &MergeHookContext) -> Result<()> {
        self.fire(HookType::PreMerge, ctx)
    }
    fn fire_post_merge(&mut self, ctx: &MergeHookContext) -> Result<()> {
        // ... existing impl ...
    }
    fn pause_spinner(&mut self) {
        self.output.pause_spinner();
    }
    fn resume_spinner(&mut self) {
        self.output.resume_spinner();
    }
}
```

- [ ] **Step 1.3: Bracket the editor-opening `git commit` in core**

In `src/core/worktree/merge.rs`, locate the squash-commit step that opens
`$EDITOR` (the `git commit` invocation without `-m` / `--no-edit` — find via
`grep -n "git\".*commit\b" src/core/worktree/merge.rs`). Bracket the invocation:

```rust
runner.pause_spinner();
let commit_status = std::process::Command::new("git")
    .arg("commit")
    /* existing args without -m / --no-edit */
    .status()
    .context("failed to invoke git commit");
runner.resume_spinner();
let commit_status = commit_status?;
```

The `git commit` here continues to inherit stdio (Slice 3 Step 1.3 marked it as
the capture exception), so `$EDITOR` attaches to the terminal as expected. The
same bracketing applies to the equivalent invocation in `finish_squash_staged`
(the `--continue` resume path).

- [ ] **Step 1.4: Add a regression test mirroring `progress.rs`'s
      spinner-bracket test**

In `src/core/worktree/merge.rs`'s test module, add:

```rust
#[derive(Default)]
struct PauseTrackingRunner {
    pre_merge_fired: bool,
    post_merge_fired: bool,
    pause_count: usize,
    resume_count: usize,
}

impl HookRunner for PauseTrackingRunner {
    fn fire_pre_merge(&mut self, _: &MergeHookContext) -> Result<()> {
        self.pre_merge_fired = true;
        Ok(())
    }
    fn fire_post_merge(&mut self, _: &MergeHookContext) -> Result<()> {
        self.post_merge_fired = true;
        Ok(())
    }
    fn pause_spinner(&mut self) { self.pause_count += 1; }
    fn resume_spinner(&mut self) { self.resume_count += 1; }
}

#[test]
#[serial]
fn squash_commit_with_editor_brackets_runner_pause_resume() {
    // Set up a repo where `daft merge feature --squash` would open the editor
    // (i.e. no -m / --no-edit / -F supplied). Confirm that pause + resume
    // were each called exactly once around the editor invocation.
    //
    // The test stubs $EDITOR to /usr/bin/true so the "editor" exits
    // immediately with a non-empty SQUASH_MSG — letting the commit succeed
    // without human interaction.
    let _cwd = CwdGuard::new();
    let tmp = tempfile::tempdir().unwrap();
    init_repo(tmp.path());
    let feat_wt = tmp.path().join("feature");
    setup_worktree(tmp.path(), "feature", &feat_wt);
    // Add a commit on feature.
    crate::testing::shell::ShellCommand::new("git")
        .args(["commit", "--allow-empty", "-q", "-m", "feature work"])
        .current_dir(&feat_wt)
        .env("GIT_AUTHOR_NAME", "Test")
        .env("GIT_AUTHOR_EMAIL", "test@test.com")
        .env("GIT_COMMITTER_NAME", "Test")
        .env("GIT_COMMITTER_EMAIL", "test@test.com")
        .status().unwrap();
    std::env::set_current_dir(tmp.path()).unwrap();
    std::env::set_var("EDITOR", "/usr/bin/true");

    let mut runner = PauseTrackingRunner::default();
    let params = StartParams {
        sources: vec!["feature".to_string()],
        target: None,
        flags: EffectiveFlags { squash: Some(true), ..Default::default() },
        adopt: AdoptDecision::None,
        require_clean_target: false,
        cleanup_intent: None,
    };
    let _ = execute_start(&params, &mut runner);

    assert_eq!(runner.pause_count, 1, "pause must fire exactly once around editor");
    assert_eq!(runner.resume_count, 1, "resume must fire exactly once around editor");
}
```

(`AdoptDecision::None` and `EffectiveFlags::default()` may need adjustment to
whatever variant names already exist; check `src/core/worktree/merge.rs` types
when implementing.)

Run:
`cargo test --lib squash_commit_with_editor_brackets_runner_pause_resume -- --nocapture`
Expected: PASS.

- [ ] **Step 1.5: Manual verification on macOS and Linux with two editors**

Build a local sandbox:

```bash
mise run dev
EDITOR=vi   daft merge feature --squash -rb   # vi opens, exits cleanly
EDITOR='code --wait' daft merge feature --squash -rb   # VS Code opens, exits cleanly
```

Expected: spinner pauses cleanly, editor takes the terminal, spinner resumes
after editor exits, cleanup proceeds. Note any terminal-state corruption issues
— these would indicate a missing `tcsetattr` or similar; if observed, address
before merging.

- [ ] **Step 1.6: Run unit + integration tests**

Run: `mise run test:unit` Run: `mise run test:manual -- --ci squash-rb`
Expected: PASS.

- [ ] **Step 1.7: Commit**

```bash
git add src/core/worktree/merge.rs src/commands/merge.rs
git commit -m "$(cat <<'EOF'
feat(merge): pause spinner around squash-commit editor invocation

Extend HookRunner with optional pause_spinner / resume_spinner
methods (default no-op). MergeHookRunner forwards them to the
underlying Output. core::worktree::merge brackets the editor-opening
`git commit` step with these calls so the editor attaches to the
terminal cleanly and the spinner resumes when it exits.
EOF
)"
```

---

## Slice 6: Pre-merge / post-merge regression scenario + multi-source rendering

Goal: Add YAML scenarios pinning down the existing-correct behavior (pre-merge /
post-merge render rich) and the new multi-source behavior (each source's
worktree-pre/post-remove fires independently in `-rb` octopus merges).

**Files:**

- Create:
  `tests/manual/scenarios/merge/merge-pre-post-merge-hooks-render-rich.yml`
- Create: `tests/manual/scenarios/merge/merge-multi-source-rb-hooks-each.yml`

### Step 1: Add scenarios

- [ ] **Step 1.1: Author `merge-pre-post-merge-hooks-render-rich.yml`**

Modeled on `merge-fires-worktree-remove-hooks.yml` from Slice 2 but installs
`pre-merge` / `post-merge` hooks. Asserts on box-rendering signatures (e.g.,
`daft hooks v` and `hook: pre-merge` / `hook: post-merge`).

- [ ] **Step 1.2: Author `merge-multi-source-rb-hooks-each.yml`**

Two sources merged in one `daft merge A B -rb` invocation; each gets its own
`worktree-pre-remove` and `worktree-post-remove` firing.

- [ ] **Step 1.3: Run scenarios**

Run: `mise run test:manual -- --ci merge-pre-post-merge-hooks-render-rich` Run:
`mise run test:manual -- --ci merge-multi-source-rb-hooks-each` Expected: Both
PASS.

- [ ] **Step 1.4: Commit**

```bash
git add tests/manual/scenarios/merge/merge-pre-post-merge-hooks-render-rich.yml tests/manual/scenarios/merge/merge-multi-source-rb-hooks-each.yml
git commit -m "$(cat <<'EOF'
test(merge): pin down hook box rendering and multi-source firing

Regression scenarios for the existing-correct pre-merge / post-merge
hook box rendering (already wired via MergeHookRunner) and the new
multi-source worktree-pre/post-remove firing per source.
EOF
)"
```

---

## Slice 7: Documentation, CLAUDE.md / SKILL.md sync, manual sandbox validation

Goal: Bring user-facing docs in line with the new behavior —
`worktree-pre-remove` / `worktree-post-remove` now fire on merge cleanup;
document the merge phase's verbose / quiet semantics. Run a manual sandbox
session matching the user's original screenshot to verify the polish gap is
closed.

**Files:**

- Modify: `docs/guide/hooks.md` (add to the worktree-pre-remove /
  worktree-post-remove rows: "Also fires on `daft merge -r` / `-rb` cleanup")
- Modify: `docs/cli/daft-merge.md` (add a "Verbose output" subsection and a note
  that cleanup goes through the standard worktree-remove hook pipeline)
- Modify: `SKILL.md` if any agent-facing guidance needs updating
- No code changes in this slice.

### Step 1: Update docs

- [ ] **Step 1.1: Update `docs/guide/hooks.md`**

In the table row for `worktree-pre-remove` and `worktree-post-remove`, append:
"Also fires on `daft merge -r` / `-rb` source cleanup, with the same env vars."

- [ ] **Step 1.2: Update `docs/cli/daft-merge.md`**

Add a new subsection:

```markdown
## Output

By default `daft merge` suppresses git's raw stdout on the success path and
renders single styled step lines instead:

- `Fast-forwarded <target> to <sha>`
- `Merged <source> into <target> (commit <sha>)`
- `Squashed <source> into <target> (commit <sha>)`

For multi-source merges, one step line per source.

### Verbose mode

`--verbose` (or `DAFT_VERBOSE=1`) dumps git's full output to stderr after the
spinner stops, useful for debugging unusual merge interactions.

### Quiet mode

`--quiet` suppresses step messages and the final summary line. Errors and
warnings always render.

## Cleanup hooks

`daft merge -r` (or `-rb`) fires `worktree-pre-remove` and
`worktree-post-remove` for each cleaned-up source — same hooks `daft remove`
fires. Use them for environment teardown (e.g. `direnv-revoke`).
```

- [ ] **Step 1.3: Verify docs site builds**

Run: `mise run docs:site:build` Expected: PASS.

### Step 2: Manual sandbox validation

- [ ] **Step 2.1: Reproduce the original screenshot scenario**

```bash
# Start a sandbox.
mise run sandbox

# In the sandbox, configure direnv-revoke (or any worktree-pre-remove hook).
mkdir -p .daft/hooks
cat > .daft/hooks/worktree-pre-remove <<'EOF'
#!/bin/sh
echo "direnv-revoke: $DAFT_WORKTREE_PATH"
EOF
chmod +x .daft/hooks/worktree-pre-remove
touch .daft/hooks/.trusted

# Set up source worktree.
git checkout -b test
echo content > new-file
git add new-file
git commit -m test-work
git checkout main
daft go test
daft go feature   # or wherever target is

# Run the original failing case.
daft merge test --squash -rb
```

Expected output (matches the polish level of `daft remove`):

- Hook box for `worktree-pre-remove` with the `direnv-revoke` step.
- No raw git output (no `Updating`, no `Fast-forward`, no `Squash commit`).
- Styled step `Squashed test into feature (commit <sha>)`.
- Hook box for `worktree-post-remove`.
- Styled summary `Squash merged and cleaned up test.`.

- [ ] **Step 2.2: Compare with `daft remove` to confirm parity**

```bash
# Compare:
daft remove test      # (separate worktree, separate test)
# Same level of styling, same hook boxes, same summary.
```

- [ ] **Step 2.3: Final test gates**

```bash
mise run fmt
mise run clippy
mise run test:unit
mise run test:manual -- --ci
```

Expected: all PASS.

- [ ] **Step 2.4: Commit**

```bash
git add docs/guide/hooks.md docs/cli/daft-merge.md SKILL.md
git commit -m "$(cat <<'EOF'
docs(merge): document rich output semantics and cleanup hooks

Document that worktree-pre-remove / worktree-post-remove fire on
`daft merge -r` / `-rb` cleanup. Add an Output / Verbose / Quiet
section to the merge CLI reference.
EOF
)"
```

---

## Final review

After all 7 slices land, dispatch a final code-reviewer subagent for the entire
branch:

- Spec compliance: every behavior in the spec is implemented and tested.
- No regressions in existing merge scenarios.
- Visual parity with `daft remove` confirmed in sandbox.
- `docs/guide/hooks.md` and `docs/cli/daft-merge.md` reflect the new behavior.
- `mise run ci` passes locally.

Use `superpowers:finishing-a-development-branch` to complete the branch.
