# Remote-Only Delete Without Local Branch — Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use
> superpowers:subagent-driven-development (recommended) or
> superpowers:executing-plans to implement this plan task-by-task. Steps use
> checkbox (`- [ ]`) syntax for tracking.

**Goal:** Allow `daft remove BRANCH --remote` to delete a remote branch even
when the local branch doesn't exist.

**Architecture:** When `remote_only` is set, skip the "branch exists locally"
validation check (Check 1) in `validate_branches()`. Instead, resolve the remote
tracking info directly from the remote ref and validate that the remote branch
exists. All other validation checks (uncommitted changes, merge status,
local/remote sync) are already irrelevant for remote-only and are skipped by
`delete_single_branch()` via the early return at line 781.

**Tech Stack:** Rust, YAML integration tests

---

### Task 1: Regression test — remote-only delete fails when local branch missing

**Files:**

- Create: `tests/manual/scenarios/branch-delete/remote-only-no-local.yml`

- [ ] **Step 1: Write the failing YAML test scenario**

```yaml
name: Remote-only delete works when local branch does not exist
description:
  When --remote is used and the branch exists on the remote but not locally, the
  remote branch should still be deleted.

repos:
  - name: test-repo
    use_fixture: standard-remote

steps:
  - name: Clone the repository
    run: git-worktree-clone --layout contained $REMOTE_TEST_REPO
    expect:
      exit_code: 0

  - name: Verify remote branch exists before test
    run: git ls-remote --heads $REMOTE_TEST_REPO develop
    cwd: "$WORK_DIR/test-repo/main"
    expect:
      exit_code: 0
      output_contains:
        - "refs/heads/develop"

  - name: Verify local branch does NOT exist
    run: git branch --list develop
    cwd: "$WORK_DIR/test-repo/main"
    expect:
      exit_code: 0
      output_not_contains:
        - "develop"

  - name: Remove with --remote should succeed even without local branch
    run: daft-remove --remote develop
    cwd: "$WORK_DIR/test-repo/main"
    expect:
      exit_code: 0

  - name: Verify remote branch was deleted
    run: git ls-remote --heads $REMOTE_TEST_REPO develop
    cwd: "$WORK_DIR/test-repo/main"
    expect:
      exit_code: 0
      output_not_contains:
        - "refs/heads/develop"
```

- [ ] **Step 2: Run the test to verify it fails**

Run: `mise run test:manual -- --ci branch-delete:remote-only-no-local` Expected:
FAIL — the `daft-remove --remote develop` step exits with code 1 ("branch not
found").

- [ ] **Step 3: Commit the failing test**

```bash
git add tests/manual/scenarios/branch-delete/remote-only-no-local.yml
git commit -m "test: add regression test for --remote delete without local branch"
```

---

### Task 2: Fix validation to skip local-branch check when `remote_only`

**Files:**

- Modify: `src/core/worktree/branch_delete.rs` — `validate_branches()` function
  (~lines 332-531) and `execute()` function (~line 131)

The fix requires two changes:

1. Thread the `remote_only` flag into `validate_branches()`.
2. When `remote_only` is true, skip Check 1 (local branch existence) and instead
   resolve the remote ref directly, skipping all local-only checks (2-5).

- [ ] **Step 1: Add `remote_only` parameter to `validate_branches()`**

In `src/core/worktree/branch_delete.rs`, update the `validate_branches`
signature to accept the new parameter, and update the call site in `execute()`
to pass `params.remote_only`:

```rust
fn validate_branches(
    ctx: &BranchDeleteContext,
    branches: &[String],
    force: bool,
    remote_only: bool,
    worktree_map: &HashMap<String, PathBuf>,
    current_wt_path: Option<&PathBuf>,
    current_branch: Option<&str>,
    sink: &mut dyn ProgressSink,
) -> (Vec<ValidatedBranch>, Vec<ValidationError>) {
```

Update the call site in `execute()` (around line 168):

```rust
    let (validated, errors) = validate_branches(
        &ctx,
        &resolved,
        params.force,
        params.remote_only,
        &worktree_map,
        current_wt_path.as_ref(),
        current_branch.as_deref(),
        sink,
    );
```

- [ ] **Step 2: Add remote-only validation path in the per-branch loop**

At the top of the `for branch in branches` loop inside `validate_branches()`,
before the existing Check 1, add the remote-only fast path:

```rust
        // Remote-only mode: skip local branch checks entirely.
        // Just verify the remote branch exists and produce a ValidatedBranch
        // with only remote info populated.
        if remote_only {
            let (remote_name, remote_branch_name) =
                resolve_remote_for_missing_local(ctx, branch);

            if remote_name.is_none() || remote_branch_name.is_none() {
                errors.push(ValidationError {
                    branch: branch.clone(),
                    message: format!(
                        "no remote branch found for '{}' on '{}'",
                        branch, ctx.remote_name
                    ),
                });
                continue;
            }

            sink.on_step(&format!(
                "Branch '{branch}' — remote-only deletion, skipping local checks"
            ));

            validated.push(ValidatedBranch {
                name: branch.clone(),
                worktree_path: None,
                remote_name,
                remote_branch_name,
                is_current_worktree: false,
                worktree_only: false,
            });
            continue;
        }
```

- [ ] **Step 3: Add the `resolve_remote_for_missing_local` helper**

This helper tries `resolve_remote_tracking` first (works when the local branch
exists and has tracking config), then falls back to checking whether
`refs/remotes/{remote}/{branch}` exists (works when only the remote ref is
present). Add it near the existing `resolve_remote_tracking` function:

```rust
/// Resolve remote info for a branch that may not exist locally.
///
/// First tries the normal tracking config lookup. If the local branch doesn't
/// exist (so git config has no `branch.<name>.remote`), falls back to probing
/// `refs/remotes/<default-remote>/<branch>`.
fn resolve_remote_for_missing_local(
    ctx: &BranchDeleteContext,
    branch: &str,
) -> (Option<String>, Option<String>) {
    // Try normal tracking lookup first (works when local branch exists)
    let result = resolve_remote_tracking(ctx, branch);
    if result.0.is_some() {
        return result;
    }

    // Fallback: check if the default remote has this branch
    let remote_ref = format!("refs/remotes/{}/{branch}", ctx.remote_name);
    if let Ok(true) = ctx.git.show_ref_exists(&remote_ref) {
        return (Some(ctx.remote_name.clone()), Some(branch.to_string()));
    }

    (None, None)
}
```

- [ ] **Step 4: Run unit tests**

Run: `mise run test:unit` Expected: PASS (all existing unit tests still pass)

- [ ] **Step 5: Run the regression test**

Run: `mise run test:manual -- --ci branch-delete:remote-only-no-local` Expected:
PASS

- [ ] **Step 6: Run the full branch-delete test suite**

Run: `mise run test:manual -- --ci branch-delete` Expected: PASS (all existing
scenarios still pass, including `remote-flag`, `nonexistent`, `no-worktree`,
`local-flag`)

- [ ] **Step 7: Run clippy and fmt**

Run: `mise run fmt && mise run clippy` Expected: No warnings, no formatting
changes

- [ ] **Step 8: Commit the fix**

```bash
git add src/core/worktree/branch_delete.rs
git commit -m "fix: allow --remote delete when local branch does not exist"
```

---

### Task 3: Verify edge case — remote-only with nonexistent remote branch

This edge case should already be covered by the `remote_name.is_none()` check in
Task 2, but let's confirm with a test.

**Files:**

- Create: `tests/manual/scenarios/branch-delete/remote-only-nonexistent.yml`

- [ ] **Step 1: Write the edge-case test**

```yaml
name: Remote-only delete fails for branch with no remote
description:
  When --remote is used but the branch does not exist on the remote, the command
  should fail with a clear error.

repos:
  - name: test-repo
    use_fixture: standard-remote

steps:
  - name: Clone the repository
    run: git-worktree-clone --layout contained $REMOTE_TEST_REPO
    expect:
      exit_code: 0

  - name: Remove nonexistent remote branch should fail
    run: daft-remove --remote totally-fake-branch 2>&1
    cwd: "$WORK_DIR/test-repo/main"
    expect:
      exit_code: 1
      output_contains:
        - "no remote branch found"
```

- [ ] **Step 2: Run the test**

Run: `mise run test:manual -- --ci branch-delete:remote-only-nonexistent`
Expected: PASS

- [ ] **Step 3: Commit**

```bash
git add tests/manual/scenarios/branch-delete/remote-only-nonexistent.yml
git commit -m "test: add edge-case test for --remote with nonexistent remote branch"
```
