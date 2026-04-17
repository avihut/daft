# Repo Identity for Log Store — Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use
> superpowers:subagent-driven-development (recommended) or
> superpowers:executing-plans to implement this plan task-by-task. Steps use
> checkbox (`- [ ]`) syntax for tracking.

**Goal:** Replace path-derived repo hashes with a per-repo UUID v7 written to
`<git-common-dir>/daft-id`, so deleting and re-cloning a repo produces a clean
log-store view rather than re-attaching stale invocations.

**Architecture:** A single `core::repo_identity` module owns creation and
retrieval of the identity file. All eight existing call sites that compute a
path-based hash are replaced with `compute_repo_id()`. Lazy creation on first
read; race-safe via `create_new(true)`; corruption-aware (empty = retry, invalid
= error).

**Tech Stack:** Rust, `uuid` crate (v7 feature), `anyhow`, `tempfile` for tests.

**Spec:** `docs/superpowers/specs/2026-04-16-repo-identity-for-log-store.md`

---

### Task 1: Add the `uuid` dependency

**Files:**

- Modify: `Cargo.toml`

- [ ] **Step 1: Add the dep**

Edit `Cargo.toml`, insert the following line in the `[dependencies]` section
(keeping alphabetical ordering is not required — daft's deps are not sorted):

```toml
uuid = { version = "1", features = ["v7"] }
```

- [ ] **Step 2: Verify it compiles and resolves**

Run: `cargo check 2>&1 | tail -5` Expected: `Finished \`dev\` profile
...`with no errors. The`getrandom`crate should appear in the Cargo.lock (pulled in transitively by`uuid`with`v7`).

- [ ] **Step 3: Commit**

```bash
git add Cargo.toml Cargo.lock
git commit -m "chore(deps): add uuid crate with v7 feature"
```

---

### Task 2: Create `src/core/repo_identity.rs` with TDD

**Files:**

- Create: `src/core/repo_identity.rs`
- Modify: `src/core/mod.rs` (add module declaration)

- [ ] **Step 1: Write the failing tests**

Create `src/core/repo_identity.rs` with the tests only (no impl yet):

```rust
//! Repository identity management for the log store and coordinator socket.
//!
//! Every repo that daft touches is assigned a stable UUID v7 stored at
//! `<git-common-dir>/daft-id`. This ID keys the on-disk log store and
//! coordinator socket, so it survives repo moves and is destroyed when the
//! repo itself is deleted. Re-cloning at the same path produces a fresh
//! identity and a clean log-store view.

use anyhow::{Context, Result};
use std::fs::OpenOptions;
use std::io::{ErrorKind, Read, Write};
use std::path::Path;
use uuid::Uuid;

const IDENTITY_FILE: &str = "daft-id";

pub fn compute_repo_id() -> Result<String> {
    unimplemented!()
}

pub fn compute_repo_id_from_common_dir(_git_common_dir: &Path) -> Result<String> {
    unimplemented!()
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    #[test]
    fn creates_file_when_absent() {
        let tmp = TempDir::new().unwrap();
        let id = compute_repo_id_from_common_dir(tmp.path()).unwrap();
        assert!(tmp.path().join(IDENTITY_FILE).exists());
        assert_eq!(Uuid::parse_str(&id).unwrap().hyphenated().to_string(), id);
    }

    #[test]
    fn reuses_existing_file() {
        let tmp = TempDir::new().unwrap();
        let first = compute_repo_id_from_common_dir(tmp.path()).unwrap();
        let second = compute_repo_id_from_common_dir(tmp.path()).unwrap();
        assert_eq!(first, second);
    }

    #[test]
    fn generated_id_is_version_7() {
        let tmp = TempDir::new().unwrap();
        let id = compute_repo_id_from_common_dir(tmp.path()).unwrap();
        let uuid = Uuid::parse_str(&id).unwrap();
        assert_eq!(uuid.get_version_num(), 7);
    }

    #[test]
    fn distinct_common_dirs_yield_distinct_ids() {
        let a = TempDir::new().unwrap();
        let b = TempDir::new().unwrap();
        let id_a = compute_repo_id_from_common_dir(a.path()).unwrap();
        let id_b = compute_repo_id_from_common_dir(b.path()).unwrap();
        assert_ne!(id_a, id_b);
    }

    #[test]
    fn empty_file_is_overwritten() {
        let tmp = TempDir::new().unwrap();
        std::fs::write(tmp.path().join(IDENTITY_FILE), "").unwrap();
        let id = compute_repo_id_from_common_dir(tmp.path()).unwrap();
        assert!(!id.is_empty());
        assert_eq!(Uuid::parse_str(&id).unwrap().get_version_num(), 7);
    }

    #[test]
    fn corrupt_contents_produce_error() {
        let tmp = TempDir::new().unwrap();
        std::fs::write(tmp.path().join(IDENTITY_FILE), "not-a-uuid").unwrap();
        let result = compute_repo_id_from_common_dir(tmp.path());
        assert!(result.is_err());
        let msg = format!("{:#}", result.unwrap_err());
        assert!(
            msg.contains("Corrupt repo identity"),
            "unexpected error message: {msg}"
        );
    }

    #[test]
    fn concurrent_creation_converges_on_single_id() {
        use std::sync::Arc;
        use std::thread;
        let tmp = Arc::new(TempDir::new().unwrap());
        let handles: Vec<_> = (0..8)
            .map(|_| {
                let tmp_clone = Arc::clone(&tmp);
                thread::spawn(move || compute_repo_id_from_common_dir(tmp_clone.path()).unwrap())
            })
            .collect();
        let ids: Vec<String> = handles.into_iter().map(|h| h.join().unwrap()).collect();
        let unique: std::collections::HashSet<_> = ids.iter().collect();
        assert_eq!(unique.len(), 1, "concurrent calls disagreed: {ids:?}");
    }
}
```

Also add the module declaration. Edit `src/core/mod.rs`, and immediately after
the `pub mod repo;` line (around line 14), insert:

```rust
pub mod repo_identity;
```

- [ ] **Step 2: Run tests to verify they fail**

Run: `cargo test --lib core::repo_identity 2>&1 | tail -20` Expected: Compile
succeeds, tests run, all 7 panic with `not implemented` from `unimplemented!()`.

- [ ] **Step 3: Write the minimal implementation**

Replace the `unimplemented!()` bodies in `src/core/repo_identity.rs`:

```rust
pub fn compute_repo_id() -> Result<String> {
    let git_common_dir = crate::core::repo::get_git_common_dir()
        .context("Could not determine git common dir. Are you inside a git repository?")?;
    compute_repo_id_from_common_dir(&git_common_dir)
}

pub fn compute_repo_id_from_common_dir(git_common_dir: &Path) -> Result<String> {
    let id_path = git_common_dir.join(IDENTITY_FILE);
    loop {
        if let Some(id) = read_existing_id(&id_path)? {
            return Ok(id);
        }
        if let Some(id) = try_create_new(&id_path)? {
            return Ok(id);
        }
        // Raced with another process — loop back and read what they wrote.
    }
}

fn read_existing_id(path: &Path) -> Result<Option<String>> {
    let mut file = match std::fs::File::open(path) {
        Ok(f) => f,
        Err(e) if e.kind() == ErrorKind::NotFound => return Ok(None),
        Err(e) => {
            return Err(e).with_context(|| format!("Failed to open {}", path.display()));
        }
    };
    let mut contents = String::new();
    file.read_to_string(&mut contents)
        .with_context(|| format!("Failed to read {}", path.display()))?;
    let trimmed = contents.trim();
    if trimmed.is_empty() {
        return Ok(None);
    }
    match Uuid::parse_str(trimmed) {
        Ok(uuid) => Ok(Some(uuid.hyphenated().to_string())),
        Err(_) => anyhow::bail!(
            "Corrupt repo identity file at {}. Delete it to regenerate \
             (this will orphan existing job logs for this repo).",
            path.display()
        ),
    }
}

fn try_create_new(path: &Path) -> Result<Option<String>> {
    let uuid = Uuid::now_v7();
    let s = uuid.hyphenated().to_string();
    match OpenOptions::new().write(true).create_new(true).open(path) {
        Ok(mut file) => {
            file.write_all(s.as_bytes())
                .with_context(|| format!("Failed to write {}", path.display()))?;
            Ok(Some(s))
        }
        Err(e) if e.kind() == ErrorKind::AlreadyExists => Ok(None),
        Err(e) => Err(e).with_context(|| format!("Failed to create {}", path.display())),
    }
}
```

- [ ] **Step 4: Run tests to verify they pass**

Run: `cargo test --lib core::repo_identity 2>&1 | tail -15` Expected: 7 passed,
0 failed.

- [ ] **Step 5: Commit**

```bash
git add src/core/repo_identity.rs src/core/mod.rs
git commit -m "feat(core): add repo_identity module with UUID v7 per-repo ID"
```

---

### Task 3: Migrate `yaml_executor` to use `compute_repo_id`

**Files:**

- Modify: `src/hooks/yaml_executor/mod.rs`

- [ ] **Step 1: Remove the old `compute_repo_hash` function**

Delete the entire function in `src/hooks/yaml_executor/mod.rs` (it spans around
lines 436-443 — verify the exact range). Also remove the now-unused
`use std::collections::hash_map::DefaultHasher;` import if it was only used by
this function.

- [ ] **Step 2: Update the call site**

Find the line at ~146:

```rust
let repo_hash = compute_repo_hash(&hook_env_obj);
```

Replace with:

```rust
let repo_hash = crate::core::repo_identity::compute_repo_id()?;
```

The surrounding function already returns `Result`, so the `?` is safe. If it
does not, propagate the change to the caller chain.

- [ ] **Step 3: Verify the function compiles**

Run: `cargo check --lib 2>&1 | tail -10` Expected: clean compile, no
unused-import warnings for `DefaultHasher` in this file.

- [ ] **Step 4: Run the full test suite**

Run: `cargo test --lib 2>&1 | grep -E "^test result:"` Expected: all
previously-passing tests still pass.

- [ ] **Step 5: Commit**

```bash
git add src/hooks/yaml_executor/mod.rs
git commit -m "refactor(hooks): use compute_repo_id instead of path-based hash"
```

---

### Task 4: Migrate `src/commands/hooks/jobs.rs`

**Files:**

- Modify: `src/commands/hooks/jobs.rs`

- [ ] **Step 1: Remove the old function**

Delete `compute_repo_hash_from_path` (around lines 430-445) from
`src/commands/hooks/jobs.rs` entirely, along with the inner
`use std::collections::hash_map::DefaultHasher;` import inside the function.

- [ ] **Step 2: Replace all 7 internal call sites**

Find each of these lines and replace the body. The locations as of writing:

- Line ~575 in `list_jobs`
- Line ~741 in `show_logs`
- Line ~960 in `cancel_job`
- Line ~982 in another cancel path
- Line ~1078 in `clean_logs` (non-all branch — see step 4)
- Line ~1340 in `retry_command`

In each of these, replace:

```rust
let repo_hash = compute_repo_hash_from_path(path)?;
```

with:

```rust
let repo_hash = crate::core::repo_identity::compute_repo_id()?;
```

Note: the `path: &Path` argument to these fns was passed to the old helper but
the old helper actually ignored it (used `get_project_root()` instead). You can
keep the `path` arg for now to minimize churn; a future cleanup can remove it.

- [ ] **Step 3: Write a failing test for `list_all_repo_hashes` UUID filter**

In the `mod tests` at the bottom of `src/commands/hooks/jobs.rs`, add:

```rust
#[test]
fn list_all_repo_hashes_filters_non_uuid_dirs() {
    use tempfile::TempDir;

    let tmp = TempDir::new().unwrap();
    std::env::set_var("DAFT_STATE_DIR", tmp.path());
    let jobs_dir = tmp.path().join("jobs");
    std::fs::create_dir_all(&jobs_dir).unwrap();

    // One valid UUID-named dir, one legacy 16-hex-char name.
    let uuid_name = "01900000-0000-7000-8000-000000000000";
    std::fs::create_dir(jobs_dir.join(uuid_name)).unwrap();
    std::fs::create_dir(jobs_dir.join("019d12345678abcd")).unwrap();

    let hashes = list_all_repo_hashes().unwrap();
    assert_eq!(hashes, vec![uuid_name.to_string()]);

    std::env::remove_var("DAFT_STATE_DIR");
}
```

Note: if other tests in the file also touch `DAFT_STATE_DIR`, use
`serial_test::serial` to avoid interference (the crate is already a
dev-dependency).

- [ ] **Step 4: Run test to verify it fails**

Run:
`cargo test --lib commands::hooks::jobs::tests::list_all_repo_hashes_filters_non_uuid_dirs 2>&1 | tail -10`
Expected: test FAILS — current impl returns both directories.

- [ ] **Step 5: Update `list_all_repo_hashes` to filter**

Edit the function body (~line 448) to parse each dir name as a UUID:

```rust
fn list_all_repo_hashes() -> Result<Vec<String>> {
    let jobs_dir = crate::daft_state_dir()?.join("jobs");
    if !jobs_dir.exists() {
        return Ok(vec![]);
    }
    let mut hashes = Vec::new();
    for entry in std::fs::read_dir(&jobs_dir)? {
        let entry = entry?;
        if entry.file_type()?.is_dir() {
            if let Some(name) = entry.file_name().to_str() {
                if uuid::Uuid::parse_str(name).is_ok() {
                    hashes.push(name.to_string());
                }
            }
        }
    }
    Ok(hashes)
}
```

- [ ] **Step 6: Run the filter test to verify it passes**

Run:
`cargo test --lib commands::hooks::jobs::tests::list_all_repo_hashes_filters_non_uuid_dirs 2>&1 | tail -5`
Expected: PASS.

- [ ] **Step 7: Run the full test suite**

Run: `cargo test --lib 2>&1 | grep -E "^test result:"` Expected: all
previously-passing tests still pass, plus the new one.

- [ ] **Step 8: Commit**

```bash
git add src/commands/hooks/jobs.rs
git commit -m "refactor(jobs): use compute_repo_id; filter non-UUID dirs in list"
```

---

### Task 5: Migrate `src/core/worktree/prune.rs`

**Files:**

- Modify: `src/core/worktree/prune.rs`

- [ ] **Step 1: Inspect the current block**

Read lines 830-845 of `src/core/worktree/prune.rs` to confirm the shape of the
inline `DefaultHasher` computation.

- [ ] **Step 2: Replace the inline block**

Replace:

```rust
use std::collections::hash_map::DefaultHasher;
use std::hash::{Hash, Hasher};

// ... existing path resolution ...
let mut hasher = DefaultHasher::new();
// ... hash calls ...
let repo_hash = format!("{:016x}", hasher.finish());
```

with:

```rust
let repo_hash = crate::core::repo_identity::compute_repo_id()?;
```

Remove the now-unused imports if they are only used by this block. Check that
the surrounding function returns `Result` so `?` is valid; if it does not,
propagate.

- [ ] **Step 3: Run clippy to catch any unused imports**

Run: `cargo clippy --lib 2>&1 | tail -10` Expected: clean, no unused-import
warnings.

- [ ] **Step 4: Run tests**

Run: `cargo test --lib core::worktree::prune 2>&1 | grep -E "^test result:"`
Expected: all prune tests pass.

- [ ] **Step 5: Commit**

```bash
git add src/core/worktree/prune.rs
git commit -m "refactor(prune): use compute_repo_id instead of inline hash"
```

---

### Task 6: Migrate the 5 completion helpers in `src/commands/complete.rs`

**Files:**

- Modify: `src/commands/complete.rs`

- [ ] **Step 1: Replace the block in `complete_job_addresses`**

Find the block near line 794:

```rust
use std::collections::hash_map::DefaultHasher;
use std::hash::{Hash, Hasher};
let repo_hash = find_project_root().ok().map(|root| {
    let mut hasher = DefaultHasher::new();
    root.display().to_string().hash(&mut hasher);
    format!("{:016x}", hasher.finish())
});
let repo_hash = match repo_hash {
    Some(h) => h,
    None => return Vec::new(),
};
```

Replace with:

```rust
let repo_hash = match crate::core::repo_identity::compute_repo_id() {
    Ok(h) => h,
    Err(_) => return Vec::new(),
};
```

- [ ] **Step 2: Replace the same block in the other 4 helpers**

Apply the identical substitution at these locations:

- `complete_retry_targets` — near line 1003
- `complete_retry_worktrees` — near line 1115
- `complete_listing_worktrees` — near line 1174
- `complete_hook_types` — near line 1220

Each block has the same shape (find_project_root + DefaultHasher + fallback to
empty vec). Replace each.

- [ ] **Step 3: Remove now-unused imports**

Check the top of `src/commands/complete.rs` and the test module. Remove any
`use std::collections::hash_map::DefaultHasher;` or
`use std::hash::{Hash, Hasher};` imports that no longer have consumers.

- [ ] **Step 4: Run clippy**

Run: `cargo clippy --lib 2>&1 | tail -10` Expected: clean.

- [ ] **Step 5: Run complete.rs tests**

Run: `cargo test --lib commands::complete 2>&1 | grep -E "^test result:"`
Expected: all complete tests pass.

- [ ] **Step 6: Commit**

```bash
git add src/commands/complete.rs
git commit -m "refactor(complete): use compute_repo_id in 5 helpers"
```

---

### Task 7: Integration scenario — repo identity on reclone

**Files:**

- Create: `tests/manual/scenarios/hooks/repo-identity-on-reclone.yml`

- [ ] **Step 1: Write the scenario**

Create `tests/manual/scenarios/hooks/repo-identity-on-reclone.yml`:

```yaml
name: Re-cloning a repo produces a fresh log-store view
description:
  "Verifies that deleting a repo directory and re-cloning the same remote to the
  same path yields a clean `daft hooks jobs` listing — the old invocations do
  not re-attach to the new repo because repo identity is keyed by the
  `.git/daft-id` file, which is destroyed with the repo."

repos:
  - name: test-reclone-id
    default_branch: main
    branches:
      - name: main
        files:
          - path: README.md
            content: "# Reclone identity test"
        commits:
          - message: "Initial commit"
      - name: feature/rx
        from: main
    daft_yml: |
      hooks:
        worktree-post-create:
          jobs:
            - name: will-fail
              run: "exit 1"

steps:
  - name: Clone the repository
    run: git-worktree-clone --layout contained $REMOTE_TEST_RECLONE_ID
    expect:
      exit_code: 0

  - name: Trust the repository
    run: daft hooks trust --force 2>&1
    cwd: "$WORK_DIR/test-reclone-id/main"
    expect:
      exit_code: 0

  - name: Checkout feature branch (post-create fails)
    run: env -u DAFT_TESTING git-worktree-checkout feature/rx 2>&1
    cwd: "$WORK_DIR/test-reclone-id/main"
    expect:
      exit_code: 1

  - name: List jobs — should show the failed invocation
    run: daft hooks jobs --all 2>&1
    cwd: "$WORK_DIR/test-reclone-id/main"
    expect:
      exit_code: 0
      output_contains:
        - "will-fail"
        - "failed"

  - name: Delete the repo directory
    run: rm -rf "$WORK_DIR/test-reclone-id"
    expect:
      exit_code: 0

  - name: Re-clone the same remote to the same path
    run: git-worktree-clone --layout contained $REMOTE_TEST_RECLONE_ID
    expect:
      exit_code: 0

  - name: Trust the re-cloned repository
    run: daft hooks trust --force 2>&1
    cwd: "$WORK_DIR/test-reclone-id/main"
    expect:
      exit_code: 0

  - name: List jobs in re-cloned repo — should be empty (fresh identity)
    run: daft hooks jobs --all 2>&1
    cwd: "$WORK_DIR/test-reclone-id/main"
    expect:
      exit_code: 0
      output_contains:
        - "No background job history found"
```

- [ ] **Step 2: Build daft (the scenario runs the binary)**

Run: `mise run dev 2>&1 | tail -3` Expected: clean build with symlinks.

- [ ] **Step 3: Run the scenario**

Run:
`mise run test:manual -- --no-interactive hooks/repo-identity-on-reclone 2>&1 | tail -20`
Expected: scenario passes all steps.

- [ ] **Step 4: Commit**

```bash
git add tests/manual/scenarios/hooks/repo-identity-on-reclone.yml
git commit -m "test(hooks): integration scenario for repo identity on reclone"
```

---

### Task 8: Final verification

- [ ] **Step 1: Run the full unit test suite**

Run: `mise run test:unit 2>&1 | grep -E "^test result:"` Expected: all tests
pass. Should be at least 1113 + 7 (new repo_identity) + 1 (new
list_all_repo_hashes test) = 1121 tests.

- [ ] **Step 2: Run clippy**

Run: `mise run clippy 2>&1 | tail -5` Expected: clean, zero warnings.

- [ ] **Step 3: Run fmt check**

Run: `mise run fmt:check 2>&1 | tail -3` Expected:
`All matched files use Prettier code style!`.

- [ ] **Step 4: Run the hooks integration scenarios**

Run: `mise run test:manual -- --no-interactive hooks 2>&1 | tail -5` Expected:
all scenarios pass (existing 17 from A/B/C plus the new one from this plan = 18
scenarios).

- [ ] **Step 5: Verify man pages are up-to-date**

Run: `mise run man:verify 2>&1 | tail -5` Expected: man pages are up to date (no
changes expected — this PR does not touch user-facing CLI).

- [ ] **Step 6: Final commit (empty, for tagging completion)**

If anything was fixed up in prior steps, squash/amend per project policy. No
empty commit needed otherwise.

---

## Summary

8 tasks, 8 commits. Each task produces a working, testable state:

1. `chore(deps): add uuid crate` — dependency landed.
2. `feat(core): add repo_identity module` — module compiles and is unit-tested.
3. `refactor(hooks): use compute_repo_id instead of path-based hash` —
   yaml_executor migrated.
4. `refactor(jobs): use compute_repo_id; filter non-UUID dirs` — jobs.rs fully
   migrated.
5. `refactor(prune): use compute_repo_id instead of inline hash` — prune.rs
   migrated.
6. `refactor(complete): use compute_repo_id in 5 helpers` — completions
   migrated.
7. `test(hooks): integration scenario for repo identity on reclone` — end-to-end
   coverage.
8. Final verification — no additional commit unless fixes land.

After task 4 or 5, old path-hash log-store dirs in dev environments should be
wiped (`rm -rf "$DAFT_STATE_DIR/jobs/"`) to avoid confusing listings during
testing.
