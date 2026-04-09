---

# `daft hooks jobs` Redesign -- Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use
> superpowers:subagent-driven-development (recommended) or
> superpowers:executing-plans to implement this plan task-by-task. Steps use
> checkbox (`- [ ]`) syntax for tracking.

**Goal:** Replace the flat, status-grouped `daft hooks jobs` output with a
hierarchical display grouped by worktree and invocation. Add composite job
addressing (`worktree:invocation:job`), shell completions, and JSON output.
Populate the currently-empty `hook_type` and `worktree` fields in `JobMeta`.

**Architecture:** The data model changes flow bottom-up: `LogStore` gets
`InvocationMeta` and new query methods, `CoordinatorState` carries metadata
from the dispatch site, `process.rs` writes it during execution, and
`jobs.rs` consumes it for display. The `JobAddress` parser is a pure function
that resolves composite addresses against the log store. Shell completions
use the existing `__complete` infrastructure.

**Tech Stack:** Rust, `serde`, `chrono`, `tabled`, `clap`, existing
`LogStore`/`CoordinatorState` infrastructure.

---

### Task 1: Add `InvocationMeta` struct and LogStore methods

**Files:**

- Modify: `src/coordinator/log_store.rs`

This task adds the `InvocationMeta` data type and the LogStore methods to write,
read, and list invocations. All new methods are testable against a `TempDir` log
store with no external dependencies.

- [ ] **Step 1: Write failing tests for InvocationMeta serialization and
      LogStore methods**

Add below the existing tests in `src/coordinator/log_store.rs` (inside the
`#[cfg(test)] mod tests` block, after `test_clean_old_logs`):

```rust
#[test]
fn test_write_and_read_invocation_meta() {
    let tmp = TempDir::new().unwrap();
    let store = LogStore::new(tmp.path().to_path_buf());
    // Create the invocation directory (normally done by create_job_dir)
    std::fs::create_dir_all(tmp.path().join("inv1")).unwrap();

    let meta = InvocationMeta {
        invocation_id: "inv1".to_string(),
        trigger_command: "worktree-post-create".to_string(),
        hook_type: "worktree-post-create".to_string(),
        worktree: "feature/tax-calc".to_string(),
        created_at: chrono::Utc::now(),
    };
    store.write_invocation_meta("inv1", &meta).unwrap();
    let loaded = store.read_invocation_meta("inv1").unwrap();
    assert_eq!(loaded.invocation_id, "inv1");
    assert_eq!(loaded.trigger_command, "worktree-post-create");
    assert_eq!(loaded.worktree, "feature/tax-calc");
}

#[test]
fn test_list_invocations() {
    let tmp = TempDir::new().unwrap();
    let store = LogStore::new(tmp.path().to_path_buf());

    // Create two invocations with different worktrees
    for (inv_id, wt) in &[("inv1", "feature/a"), ("inv2", "feature/b")] {
        std::fs::create_dir_all(tmp.path().join(inv_id)).unwrap();
        let meta = InvocationMeta {
            invocation_id: inv_id.to_string(),
            trigger_command: "worktree-post-create".to_string(),
            hook_type: "worktree-post-create".to_string(),
            worktree: wt.to_string(),
            created_at: chrono::Utc::now(),
        };
        store.write_invocation_meta(inv_id, &meta).unwrap();
    }

    let all = store.list_invocations().unwrap();
    assert_eq!(all.len(), 2);
}

#[test]
fn test_list_invocations_for_worktree() {
    let tmp = TempDir::new().unwrap();
    let store = LogStore::new(tmp.path().to_path_buf());

    for (inv_id, wt) in &[("inv1", "feature/a"), ("inv2", "feature/b"), ("inv3", "feature/a")] {
        std::fs::create_dir_all(tmp.path().join(inv_id)).unwrap();
        let meta = InvocationMeta {
            invocation_id: inv_id.to_string(),
            trigger_command: "worktree-post-create".to_string(),
            hook_type: "worktree-post-create".to_string(),
            worktree: wt.to_string(),
            created_at: chrono::Utc::now(),
        };
        store.write_invocation_meta(inv_id, &meta).unwrap();
    }

    let filtered = store.list_invocations_for_worktree("feature/a").unwrap();
    assert_eq!(filtered.len(), 2);
    assert!(filtered.iter().all(|m| m.worktree == "feature/a"));
}

#[test]
fn test_list_jobs_in_invocation() {
    let tmp = TempDir::new().unwrap();
    let store = LogStore::new(tmp.path().to_path_buf());

    // Create two jobs under inv1
    store.create_job_dir("inv1", "db-migrate").unwrap();
    store.create_job_dir("inv1", "warm-build").unwrap();
    // Create one job under inv2
    store.create_job_dir("inv2", "db-seed").unwrap();

    let jobs = store.list_jobs_in_invocation("inv1").unwrap();
    assert_eq!(jobs.len(), 2);
    let names: Vec<&str> = jobs.iter().map(|p| p.file_name().unwrap().to_str().unwrap()).collect();
    assert!(names.contains(&"db-migrate"));
    assert!(names.contains(&"warm-build"));
}

#[test]
fn test_list_invocations_empty_store() {
    let tmp = TempDir::new().unwrap();
    let store = LogStore::new(tmp.path().to_path_buf());
    let all = store.list_invocations().unwrap();
    assert!(all.is_empty());
}
```

Run: `mise run test:unit -- --lib coordinator::log_store` Expected: Compilation
fails (types/methods do not exist yet).

- [ ] **Step 2: Add `InvocationMeta` struct**

At the top of `src/coordinator/log_store.rs`, after the `JobMeta` struct (after
line 28), add:

```rust
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct InvocationMeta {
    pub invocation_id: String,
    pub trigger_command: String,
    pub hook_type: String,
    pub worktree: String,
    pub created_at: chrono::DateTime<chrono::Utc>,
}
```

- [ ] **Step 3: Add LogStore methods for invocation metadata**

Add these methods to the `impl LogStore` block (after the existing `clean`
method, before the closing `}`):

```rust
/// Write invocation metadata to `{invocation_id}/invocation.json`.
pub fn write_invocation_meta(&self, invocation_id: &str, meta: &InvocationMeta) -> Result<()> {
    let dir = self.base_dir.join(invocation_id);
    fs::create_dir_all(&dir)
        .with_context(|| format!("Failed to create invocation dir: {}", dir.display()))?;
    let path = dir.join("invocation.json");
    let content = serde_json::to_string_pretty(meta)?;
    fs::write(&path, content)?;
    Ok(())
}

/// Read invocation metadata from `{invocation_id}/invocation.json`.
pub fn read_invocation_meta(&self, invocation_id: &str) -> Result<InvocationMeta> {
    let path = self.base_dir.join(invocation_id).join("invocation.json");
    let content = fs::read_to_string(&path)
        .with_context(|| format!("Failed to read invocation meta: {}", path.display()))?;
    let meta: InvocationMeta = serde_json::from_str(&content)?;
    Ok(meta)
}

/// List all invocations in this log store, sorted by `created_at` ascending.
pub fn list_invocations(&self) -> Result<Vec<InvocationMeta>> {
    let mut invocations = Vec::new();
    if !self.base_dir.exists() {
        return Ok(invocations);
    }
    for entry in fs::read_dir(&self.base_dir)? {
        let entry = entry?;
        if entry.file_type()?.is_dir() {
            let inv_id = entry.file_name().to_string_lossy().to_string();
            if let Ok(meta) = self.read_invocation_meta(&inv_id) {
                invocations.push(meta);
            }
        }
    }
    invocations.sort_by(|a, b| a.created_at.cmp(&b.created_at));
    Ok(invocations)
}

/// List invocations filtered to a specific worktree, sorted by `created_at` ascending.
pub fn list_invocations_for_worktree(&self, worktree: &str) -> Result<Vec<InvocationMeta>> {
    let all = self.list_invocations()?;
    Ok(all.into_iter().filter(|m| m.worktree == worktree).collect())
}

/// List job directories within a specific invocation.
pub fn list_jobs_in_invocation(&self, invocation_id: &str) -> Result<Vec<PathBuf>> {
    let inv_dir = self.base_dir.join(invocation_id);
    let mut dirs = Vec::new();
    if !inv_dir.exists() {
        return Ok(dirs);
    }
    for entry in fs::read_dir(&inv_dir)? {
        let entry = entry?;
        if entry.file_type()?.is_dir() {
            dirs.push(entry.path());
        }
    }
    Ok(dirs)
}
```

- [ ] **Step 4: Run tests, clippy, fmt**

Run:
`mise run fmt && mise run clippy && mise run test:unit -- --lib coordinator::log_store`
Expected: All 5 new tests pass plus the 4 existing tests. No clippy warnings.

- [ ] **Step 5: Commit**

```bash
git add src/coordinator/log_store.rs
git commit -m "feat(jobs): add InvocationMeta struct and LogStore query methods"
```

---

### Task 2: Add `background` and `finished_at` fields to `JobMeta`

**Files:**

- Modify: `src/coordinator/log_store.rs`

Two new fields on `JobMeta`. Since this is unshipped code, no backward
compatibility is needed. All existing test helpers that construct `JobMeta` must
be updated.

- [ ] **Step 1: Write a failing test for the new fields**

Add to the test module in `src/coordinator/log_store.rs`:

```rust
#[test]
fn test_job_meta_background_and_finished_at() {
    let tmp = TempDir::new().unwrap();
    let store = LogStore::new(tmp.path().to_path_buf());
    let dir = store.create_job_dir("inv1", "bg-job").unwrap();
    let finished = chrono::Utc::now();
    let meta = JobMeta {
        name: "bg-job".to_string(),
        hook_type: "worktree-post-create".to_string(),
        worktree: "feature/x".to_string(),
        command: "echo hi".to_string(),
        working_dir: "/tmp".to_string(),
        env: HashMap::new(),
        started_at: finished - chrono::Duration::seconds(5),
        status: JobStatus::Completed,
        exit_code: Some(0),
        pid: Some(1234),
        background: true,
        finished_at: Some(finished),
    };
    store.write_meta(&dir, &meta).unwrap();
    let loaded = store.read_meta(&dir).unwrap();
    assert!(loaded.background);
    assert!(loaded.finished_at.is_some());
}
```

Run: `mise run test:unit -- --lib coordinator::log_store` Expected: Fails to
compile (fields do not exist).

- [ ] **Step 2: Add the fields to `JobMeta`**

In `src/coordinator/log_store.rs`, add two fields after `pid` in the `JobMeta`
struct (after line 28):

```rust
pub background: bool,
pub finished_at: Option<chrono::DateTime<chrono::Utc>>,
```

- [ ] **Step 3: Fix all existing `JobMeta` constructors in tests**

Update every `JobMeta { ... }` literal in the project to include the new fields.
Search all files for `JobMeta {` and add `background: false, finished_at: None,`
to each. Key locations:

- `src/coordinator/log_store.rs` tests (4 existing constructors)
- `src/coordinator/process.rs` line 152 (`run_single_background_job`)
- `src/coordinator/process.rs` tests (~3 constructors)
- `src/commands/hooks/jobs.rs` line 167 (synthetic meta in `list_jobs`)

For the production code at `process.rs:152`, set `background: true` and
`finished_at: None` (will be set to `Some(Utc::now())` in Task 4).

For the synthetic meta at `jobs.rs:167`, set
`background: true, finished_at: None`.

- [ ] **Step 4: Run tests, clippy, fmt**

Run: `mise run fmt && mise run clippy && mise run test:unit` Expected: All tests
pass. No clippy warnings.

- [ ] **Step 5: Commit**

```bash
git add src/coordinator/log_store.rs src/coordinator/process.rs src/commands/hooks/jobs.rs
git commit -m "feat(jobs): add background and finished_at fields to JobMeta"
```

---

### Task 3: Add metadata fields to `CoordinatorState` with `with_metadata()` builder

**Files:**

- Modify: `src/coordinator/process.rs`

Three new fields on `CoordinatorState` and a builder method. This is a pure
struct change with no behavior yet.

- [ ] **Step 1: Write a failing test for `with_metadata()`**

Add to the test module in `src/coordinator/process.rs`:

```rust
#[test]
fn test_coordinator_state_with_metadata() {
    let state = CoordinatorState::new("test-repo", "inv-1")
        .with_metadata("worktree-post-create", "worktree-post-create", "feature/tax-calc");
    assert_eq!(state.trigger_command, "worktree-post-create");
    assert_eq!(state.hook_type, "worktree-post-create");
    assert_eq!(state.worktree, "feature/tax-calc");
}
```

Run: `mise run test:unit -- --lib coordinator::process` Expected: Fails to
compile.

- [ ] **Step 2: Add fields and builder to `CoordinatorState`**

In `src/coordinator/process.rs`, update the `CoordinatorState` struct (lines
30-34) and its `new()` constructor:

```rust
pub struct CoordinatorState {
    pub repo_hash: String,
    pub invocation_id: String,
    pub jobs: Vec<JobSpec>,
    pub trigger_command: String,
    pub hook_type: String,
    pub worktree: String,
}

impl CoordinatorState {
    pub fn new(repo_hash: &str, invocation_id: &str) -> Self {
        Self {
            repo_hash: repo_hash.to_string(),
            invocation_id: invocation_id.to_string(),
            jobs: Vec::new(),
            trigger_command: String::new(),
            hook_type: String::new(),
            worktree: String::new(),
        }
    }

    /// Set invocation metadata. Returns `self` for chaining.
    pub fn with_metadata(
        mut self,
        trigger_command: &str,
        hook_type: &str,
        worktree: &str,
    ) -> Self {
        self.trigger_command = trigger_command.to_string();
        self.hook_type = hook_type.to_string();
        self.worktree = worktree.to_string();
        self
    }

    pub fn add_job(&mut self, job: JobSpec) {
        self.jobs.push(job);
    }
    // ... rest unchanged
}
```

- [ ] **Step 3: Run tests, clippy, fmt**

Run:
`mise run fmt && mise run clippy && mise run test:unit -- --lib coordinator::process`
Expected: All tests pass including the new one.

- [ ] **Step 4: Commit**

```bash
git add src/coordinator/process.rs
git commit -m "feat(jobs): add trigger_command/hook_type/worktree to CoordinatorState"
```

---

### Task 4: Propagate metadata through the coordinator pipeline

**Files:**

- Modify: `src/coordinator/process.rs`

This task wires the metadata fields into the execution pipeline:
`fork_coordinator` writes `invocation.json`, and `run_single_background_job`
populates `hook_type`/`worktree` on `JobMeta` and sets `finished_at`.

- [ ] **Step 1: Write a failing test for invocation.json being written**

Add to the test module in `src/coordinator/process.rs`:

```rust
#[test]
fn test_run_all_writes_invocation_meta() {
    let tmp = TempDir::new().unwrap();
    let store = LogStore::new(tmp.path().to_path_buf());
    let mut state = CoordinatorState::new("test-repo", "inv-meta-1")
        .with_metadata("worktree-post-create", "worktree-post-create", "feature/x");
    state.add_job(JobSpec {
        name: "echo-job".to_string(),
        command: "echo hello".to_string(),
        working_dir: std::env::temp_dir(),
        background: true,
        ..Default::default()
    });

    state.run_all(&store).unwrap();

    // Verify invocation.json was written
    let inv_meta = store.read_invocation_meta("inv-meta-1").unwrap();
    assert_eq!(inv_meta.trigger_command, "worktree-post-create");
    assert_eq!(inv_meta.worktree, "feature/x");
}

#[test]
fn test_run_all_populates_job_hook_type_and_worktree() {
    let tmp = TempDir::new().unwrap();
    let store = LogStore::new(tmp.path().to_path_buf());
    let mut state = CoordinatorState::new("test-repo", "inv-pop-1")
        .with_metadata("worktree-post-create", "worktree-post-create", "feature/y");
    state.add_job(JobSpec {
        name: "check-job".to_string(),
        command: "echo ok".to_string(),
        working_dir: std::env::temp_dir(),
        background: true,
        ..Default::default()
    });

    state.run_all(&store).unwrap();

    let meta = store
        .read_meta(&tmp.path().join("inv-pop-1").join("check-job"))
        .unwrap();
    assert_eq!(meta.hook_type, "worktree-post-create");
    assert_eq!(meta.worktree, "feature/y");
    assert!(meta.background);
    assert!(meta.finished_at.is_some());
}
```

Run: `mise run test:unit -- --lib coordinator::process` Expected: Tests fail
(invocation.json not written, fields still empty).

- [ ] **Step 2: Write `invocation.json` at start of `run_all_with_cancel`**

In `src/coordinator/process.rs`, add at the beginning of `run_all_with_cancel()`
(after line 69, before `let mut handles`):

```rust
// Write invocation metadata if we have any.
if !self.trigger_command.is_empty() {
    let inv_meta = super::log_store::InvocationMeta {
        invocation_id: self.invocation_id.clone(),
        trigger_command: self.trigger_command.clone(),
        hook_type: self.hook_type.clone(),
        worktree: self.worktree.clone(),
        created_at: chrono::Utc::now(),
    };
    let _ = store.write_invocation_meta(&self.invocation_id, &inv_meta);
}
```

- [ ] **Step 3: Pass metadata to `run_single_background_job`**

The `run_single_background_job` function needs to know `hook_type` and
`worktree`. Since it runs in a thread, clone the values. In the thread spawn
loop inside `run_all_with_cancel`, add clones before the `thread::spawn`:

```rust
let hook_type = self.hook_type.clone();
let worktree = self.worktree.clone();
```

Then change the `run_single_background_job` call to pass them:

```rust
run_single_background_job(
    &job,
    &inv_id,
    &local_store,
    &results,
    &child_pids,
    &cancel_all,
    &hook_type,
    &worktree,
);
```

Update the `run_single_background_job` signature to accept the new parameters:

```rust
fn run_single_background_job(
    job: &JobSpec,
    invocation_id: &str,
    store: &LogStore,
    results: &Arc<Mutex<Vec<JobResult>>>,
    child_pids: &ChildPidMap,
    cancel_all: &Arc<AtomicBool>,
    hook_type: &str,
    worktree: &str,
)
```

- [ ] **Step 4: Populate `hook_type`, `worktree`, `background` on initial
      JobMeta write**

In `run_single_background_job`, update the `JobMeta` construction (around
line 152) to use the passed parameters:

```rust
let mut meta = JobMeta {
    name: job.name.clone(),
    hook_type: hook_type.to_string(),
    worktree: worktree.to_string(),
    command: job.command.clone(),
    working_dir: job.working_dir.display().to_string(),
    env: job.env.clone(),
    started_at: chrono::Utc::now(),
    status: JobStatus::Running,
    exit_code: None,
    pid: Some(std::process::id()),
    background: job.background,
    finished_at: None,
};
```

- [ ] **Step 5: Set `finished_at` on the final meta write**

In `run_single_background_job`, just before the final `store.write_meta` call
(around line 222), add:

```rust
meta.finished_at = Some(chrono::Utc::now());
```

This line goes right after `meta.exit_code = exit_code;` and before
`if let Err(e) = store.write_meta(...)`.

- [ ] **Step 6: Run tests, clippy, fmt**

Run:
`mise run fmt && mise run clippy && mise run test:unit -- --lib coordinator::process`
Expected: All tests pass including the 2 new ones.

- [ ] **Step 7: Commit**

```bash
git add src/coordinator/process.rs
git commit -m "feat(jobs): propagate metadata to invocation.json and JobMeta during execution"
```

---

### Task 5: Wire metadata at the dispatch site

**Files:**

- Modify: `src/hooks/yaml_executor/mod.rs`
- Modify: `src/commands/hooks/jobs.rs` (retry path)

This task plumbs `trigger_command`, `hook_type`, and `worktree` into
`CoordinatorState` at the two places where coordinators are created: the main
dispatch site and the retry path.

- [ ] **Step 1: Update the dispatch site in `yaml_executor/mod.rs`**

In `src/hooks/yaml_executor/mod.rs`, replace lines 275-276 (the
`CoordinatorState::new` and loop):

```rust
let repo_hash = compute_repo_hash(&hook_env_obj);
let invocation_id = generate_invocation_id();
let store = crate::coordinator::log_store::LogStore::for_repo(&repo_hash)?;

// Derive trigger_command: manual runs use "hooks run {hook_name}",
// automatic hooks use the hook_name directly.
let trigger_command = if ctx.command == "hooks-run" {
    format!("hooks run {}", hook_name)
} else {
    hook_name.to_string()
};

let mut coord_state =
    crate::coordinator::process::CoordinatorState::new(&repo_hash, &invocation_id)
        .with_metadata(&trigger_command, hook_name, &ctx.branch_name);
for spec in bg_specs {
    coord_state.add_job(spec);
}
```

The rest of the dispatch block (lines 281-288) stays unchanged.

- [ ] **Step 2: Update the retry path in `jobs.rs`**

In `src/commands/hooks/jobs.rs`, in `retry_job()` (around lines 423-426), update
the CoordinatorState creation to include metadata:

```rust
let invocation_id = generate_invocation_id();
let retry_store = LogStore::for_repo(&repo_hash)?;
let trigger_command = format!("hooks jobs retry {}", meta.name);
let mut coord_state =
    crate::coordinator::process::CoordinatorState::new(&repo_hash, &invocation_id)
        .with_metadata(&trigger_command, &meta.hook_type, &meta.worktree);
coord_state.add_job(job_spec);
```

- [ ] **Step 3: Run full test suite, clippy, fmt**

Run: `mise run fmt && mise run clippy && mise run test:unit` Expected: All tests
pass. No clippy warnings.

- [ ] **Step 4: Commit**

```bash
git add src/hooks/yaml_executor/mod.rs src/commands/hooks/jobs.rs
git commit -m "feat(jobs): plumb metadata at dispatch site and retry path"
```

---

### Task 6: `JobAddress` parsing and resolution

**Files:**

- Modify: `src/commands/hooks/jobs.rs`

This task adds the composite address parser (`worktree:invocation:job`) and a
resolver that maps addresses to concrete job directories. It is pure logic with
no I/O beyond the LogStore, so it is straightforward to test.

- [ ] **Step 1: Write failing tests for address parsing**

Add a new `#[cfg(test)] mod tests` block at the bottom of
`src/commands/hooks/jobs.rs` (or inside an existing one):

```rust
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_parse_job_address_name_only() {
        let addr = JobAddress::parse("db-migrate");
        assert_eq!(addr.job_name, "db-migrate");
        assert!(addr.invocation_prefix.is_none());
        assert!(addr.worktree.is_none());
    }

    #[test]
    fn test_parse_job_address_invocation_and_name() {
        let addr = JobAddress::parse("c9d4:db-migrate");
        assert_eq!(addr.job_name, "db-migrate");
        assert_eq!(addr.invocation_prefix.as_deref(), Some("c9d4"));
        assert!(addr.worktree.is_none());
    }

    #[test]
    fn test_parse_job_address_full() {
        let addr = JobAddress::parse("feat/tax-calc:c9d4:db-migrate");
        assert_eq!(addr.job_name, "db-migrate");
        assert_eq!(addr.invocation_prefix.as_deref(), Some("c9d4"));
        assert_eq!(addr.worktree.as_deref(), Some("feat/tax-calc"));
    }

    #[test]
    fn test_parse_job_address_worktree_with_slash() {
        let addr = JobAddress::parse("feature/auth/v2:a3f2:warm-build");
        assert_eq!(addr.worktree.as_deref(), Some("feature/auth/v2"));
        assert_eq!(addr.invocation_prefix.as_deref(), Some("a3f2"));
        assert_eq!(addr.job_name, "warm-build");
    }

    #[test]
    fn test_resolve_job_address_name_only_finds_most_recent() {
        use crate::coordinator::log_store::{InvocationMeta, LogStore, JobMeta, JobStatus};
        use tempfile::TempDir;
        use std::collections::HashMap;

        let tmp = TempDir::new().unwrap();
        let store = LogStore::new(tmp.path().to_path_buf());

        // Create two invocations for the same worktree
        let now = chrono::Utc::now();
        for (inv_id, offset) in &[("0001000000000000", 100i64), ("0002000000000000", 50)] {
            std::fs::create_dir_all(tmp.path().join(inv_id)).unwrap();
            let inv_meta = InvocationMeta {
                invocation_id: inv_id.to_string(),
                trigger_command: "worktree-post-create".to_string(),
                hook_type: "worktree-post-create".to_string(),
                worktree: "feature/x".to_string(),
                created_at: now - chrono::Duration::seconds(*offset),
            };
            store.write_invocation_meta(inv_id, &inv_meta).unwrap();

            let dir = store.create_job_dir(inv_id, "db-migrate").unwrap();
            let meta = JobMeta {
                name: "db-migrate".to_string(),
                hook_type: "worktree-post-create".to_string(),
                worktree: "feature/x".to_string(),
                command: "echo".to_string(),
                working_dir: "/tmp".to_string(),
                env: HashMap::new(),
                started_at: now - chrono::Duration::seconds(*offset),
                status: JobStatus::Completed,
                exit_code: Some(0),
                pid: None,
                background: true,
                finished_at: Some(now - chrono::Duration::seconds(offset - 3)),
            };
            store.write_meta(&dir, &meta).unwrap();
        }

        let addr = JobAddress::parse("db-migrate");
        let result = resolve_job_address(&addr, &store, "feature/x").unwrap();
        // Should resolve to the most recent invocation (0002...)
        assert!(result.invocation_id.starts_with("0002"));
        assert_eq!(result.job_name, "db-migrate");
    }

    #[test]
    fn test_resolve_job_address_with_prefix() {
        use crate::coordinator::log_store::{InvocationMeta, LogStore, JobMeta, JobStatus};
        use tempfile::TempDir;
        use std::collections::HashMap;

        let tmp = TempDir::new().unwrap();
        let store = LogStore::new(tmp.path().to_path_buf());
        let now = chrono::Utc::now();

        let inv_id = "c9d4e7f2a3b10000";
        std::fs::create_dir_all(tmp.path().join(inv_id)).unwrap();
        let inv_meta = InvocationMeta {
            invocation_id: inv_id.to_string(),
            trigger_command: "worktree-post-create".to_string(),
            hook_type: "worktree-post-create".to_string(),
            worktree: "feature/x".to_string(),
            created_at: now,
        };
        store.write_invocation_meta(inv_id, &inv_meta).unwrap();

        let dir = store.create_job_dir(inv_id, "db-migrate").unwrap();
        let meta = JobMeta {
            name: "db-migrate".to_string(),
            hook_type: "worktree-post-create".to_string(),
            worktree: "feature/x".to_string(),
            command: "echo".to_string(),
            working_dir: "/tmp".to_string(),
            env: HashMap::new(),
            started_at: now,
            status: JobStatus::Completed,
            exit_code: Some(0),
            pid: None,
            background: true,
            finished_at: Some(now),
        };
        store.write_meta(&dir, &meta).unwrap();

        let addr = JobAddress::parse("c9d4:db-migrate");
        let result = resolve_job_address(&addr, &store, "feature/x").unwrap();
        assert_eq!(result.invocation_id, inv_id);
    }
}
```

Run: `mise run test:unit -- --lib commands::hooks::jobs` Expected: Fails to
compile.

- [ ] **Step 2: Implement `JobAddress` struct and `parse()`**

Add to `src/commands/hooks/jobs.rs`, after the existing imports (before
`pub fn run`):

```rust
/// Parsed composite job address: `[worktree:][invocation:]job_name`.
#[derive(Debug, Clone)]
pub struct JobAddress {
    pub worktree: Option<String>,
    pub invocation_prefix: Option<String>,
    pub job_name: String,
}

impl JobAddress {
    /// Parse a composite address string.
    ///
    /// Forms:
    /// - `job_name`
    /// - `invocation:job_name`
    /// - `worktree:invocation:job_name`
    pub fn parse(input: &str) -> Self {
        let parts: Vec<&str> = input.rsplitn(3, ':').collect();
        match parts.len() {
            1 => Self {
                worktree: None,
                invocation_prefix: None,
                job_name: parts[0].to_string(),
            },
            2 => Self {
                worktree: None,
                invocation_prefix: Some(parts[1].to_string()),
                job_name: parts[0].to_string(),
            },
            3 => Self {
                worktree: Some(parts[2].to_string()),
                invocation_prefix: Some(parts[1].to_string()),
                job_name: parts[0].to_string(),
            },
            _ => unreachable!(),
        }
    }

    /// Apply `--inv` override: if both inline and flag are present, flag wins.
    pub fn with_inv_override(mut self, inv: Option<&str>) -> Self {
        if let Some(prefix) = inv {
            self.invocation_prefix = Some(prefix.to_string());
        }
        self
    }
}
```

- [ ] **Step 3: Implement `ResolvedAddress` and `resolve_job_address()`**

Add after the `JobAddress` implementation:

```rust
/// A fully resolved job address: concrete invocation ID, job name, and job directory.
#[derive(Debug)]
pub struct ResolvedAddress {
    pub invocation_id: String,
    pub job_name: String,
    pub job_dir: std::path::PathBuf,
}

/// Resolve a `JobAddress` against the log store.
///
/// - Missing worktree defaults to `current_worktree`.
/// - Missing invocation: uses the most recent invocation containing the named job.
/// - Invocation prefix: matches IDs starting with the prefix; errors on ambiguity.
fn resolve_job_address(
    addr: &JobAddress,
    store: &LogStore,
    current_worktree: &str,
) -> Result<ResolvedAddress> {
    let worktree = addr.worktree.as_deref().unwrap_or(current_worktree);

    let invocations = store.list_invocations_for_worktree(worktree)?;

    if invocations.is_empty() {
        anyhow::bail!("No invocations found for worktree '{worktree}'.");
    }

    match &addr.invocation_prefix {
        Some(prefix) => {
            // Find invocations matching the prefix
            let matches: Vec<&crate::coordinator::log_store::InvocationMeta> = invocations
                .iter()
                .filter(|inv| inv.invocation_id.starts_with(prefix.as_str()))
                .collect();

            match matches.len() {
                0 => anyhow::bail!(
                    "No invocation matching prefix '{prefix}' in worktree '{worktree}'."
                ),
                1 => {
                    let inv = matches[0];
                    let job_dir = store
                        .base_dir
                        .join(&inv.invocation_id)
                        .join(&addr.job_name);
                    if !job_dir.exists() {
                        let available = list_job_names_in_invocation(store, &inv.invocation_id)?;
                        anyhow::bail!(
                            "No job named '{}' found in invocation '{}'.\nAvailable jobs: {}",
                            addr.job_name,
                            &inv.invocation_id[..4],
                            available.join(", ")
                        );
                    }
                    Ok(ResolvedAddress {
                        invocation_id: inv.invocation_id.clone(),
                        job_name: addr.job_name.clone(),
                        job_dir,
                    })
                }
                _ => {
                    use crate::output::format::shorthand_from_seconds;
                    let now = chrono::Utc::now();
                    let lines: Vec<String> = matches
                        .iter()
                        .map(|inv| {
                            let ago = shorthand_from_seconds(
                                now.signed_duration_since(inv.created_at).num_seconds(),
                            );
                            format!(
                                "  {}  {} -- {} ago",
                                &inv.invocation_id[..4],
                                inv.trigger_command,
                                ago
                            )
                        })
                        .collect();
                    anyhow::bail!(
                        "Ambiguous invocation ID '{}' -- matches:\n{}\nUse more characters to disambiguate.",
                        prefix,
                        lines.join("\n")
                    );
                }
            }
        }
        None => {
            // No invocation prefix: find the most recent invocation containing the job.
            // Iterate from newest to oldest (list_invocations returns ascending).
            for inv in invocations.iter().rev() {
                let job_dir = store
                    .base_dir
                    .join(&inv.invocation_id)
                    .join(&addr.job_name);
                if job_dir.exists() {
                    return Ok(ResolvedAddress {
                        invocation_id: inv.invocation_id.clone(),
                        job_name: addr.job_name.clone(),
                        job_dir,
                    });
                }
            }
            // Job not found in any invocation
            let all_job_names = collect_all_job_names(store, &invocations)?;
            anyhow::bail!(
                "No job named '{}' found in worktree '{}'.\nAvailable jobs: {}",
                addr.job_name,
                worktree,
                all_job_names.join(", ")
            );
        }
    }
}

/// List job names in a specific invocation.
fn list_job_names_in_invocation(store: &LogStore, invocation_id: &str) -> Result<Vec<String>> {
    let dirs = store.list_jobs_in_invocation(invocation_id)?;
    Ok(dirs
        .iter()
        .filter_map(|d| d.file_name().map(|n| n.to_string_lossy().to_string()))
        .collect())
}

/// Collect unique job names across a set of invocations.
fn collect_all_job_names(
    store: &LogStore,
    invocations: &[crate::coordinator::log_store::InvocationMeta],
) -> Result<Vec<String>> {
    let mut names = std::collections::BTreeSet::new();
    for inv in invocations {
        for dir in store.list_jobs_in_invocation(&inv.invocation_id)? {
            if let Some(n) = dir.file_name() {
                names.insert(n.to_string_lossy().to_string());
            }
        }
    }
    Ok(names.into_iter().collect())
}
```

- [ ] **Step 4: Run tests, clippy, fmt**

Run:
`mise run fmt && mise run clippy && mise run test:unit -- --lib commands::hooks::jobs`
Expected: All tests pass.

- [ ] **Step 5: Commit**

```bash
git add src/commands/hooks/jobs.rs
git commit -m "feat(jobs): add JobAddress parser and resolver for composite addressing"
```

---

### Task 7: CLI interface changes (`--all`, `--inv`, subcommand args)

**Files:**

- Modify: `src/commands/hooks/jobs.rs`

Replace `--all-repos` with `--all`. Add `--inv` flag to `Logs`, `Cancel`, and
`Retry` subcommands. Update `show_logs`, `cancel_job`, and `retry_job` to use
`resolve_job_address`.

- [ ] **Step 1: Update `JobsArgs` and `JobsCommand` enums**

Replace the entire `JobsArgs` and `JobsCommand` definitions (lines 12-53 of
`src/commands/hooks/jobs.rs`):

```rust
#[derive(Parser, Debug)]
#[command(about = "Manage background hook jobs")]
pub struct JobsArgs {
    #[command(subcommand)]
    command: Option<JobsCommand>,

    /// Show jobs across all worktrees.
    #[arg(long)]
    all: bool,

    /// Output in JSON format.
    #[arg(long)]
    json: bool,
}

#[derive(Subcommand, Debug)]
enum JobsCommand {
    /// View output log for a background job.
    Logs {
        /// Job address: name, inv:name, or worktree:inv:name.
        job: String,
        /// Invocation ID prefix (overrides inline prefix).
        #[arg(long)]
        inv: Option<String>,
    },
    /// Cancel a running background job.
    Cancel {
        /// Job address (omit for --all).
        job: Option<String>,
        /// Cancel all running jobs.
        #[arg(long)]
        all: bool,
        /// Invocation ID prefix.
        #[arg(long)]
        inv: Option<String>,
    },
    /// Re-run a failed background job.
    Retry {
        /// Job address.
        job: String,
        /// Invocation ID prefix.
        #[arg(long)]
        inv: Option<String>,
    },
    /// Remove logs older than the retention period.
    Clean,
}
```

- [ ] **Step 2: Update `run()` dispatch to pass new args**

Update the `run()` function to match the new enum shapes:

```rust
pub fn run(args: JobsArgs, path: &Path, output: &mut dyn Output) -> Result<()> {
    match args.command {
        None => list_jobs(&args, path, output),
        Some(JobsCommand::Logs { ref job, ref inv }) => {
            show_logs(job, inv.as_deref(), &args, path, output)
        }
        Some(JobsCommand::Cancel {
            ref job,
            all,
            ref inv,
        }) => {
            if all || job.is_none() {
                cancel_all(path, output)
            } else {
                cancel_job(job.as_ref().unwrap(), inv.as_deref(), path, output)
            }
        }
        Some(JobsCommand::Retry { ref job, ref inv }) => {
            retry_job(job, inv.as_deref(), path, output)
        }
        Some(JobsCommand::Clean) => clean_logs(&args, path, output),
    }
}
```

- [ ] **Step 3: Update `show_logs` to use address resolution**

Replace the `show_logs` function signature and first section. The function
should resolve the address, then read meta and display the new format from the
spec:

```rust
fn show_logs(
    job: &str,
    inv: Option<&str>,
    _args: &JobsArgs,
    path: &Path,
    output: &mut dyn Output,
) -> Result<()> {
    let repo_hash = compute_repo_hash_from_path(path)?;
    let store = LogStore::for_repo(&repo_hash)?;

    let current_worktree = crate::core::repo::get_current_branch().unwrap_or_default();
    let addr = JobAddress::parse(job).with_inv_override(inv);
    let resolved = resolve_job_address(&addr, &store, &current_worktree)?;

    let meta = store.read_meta(&resolved.job_dir)?;
    let log_path = LogStore::log_path(&resolved.job_dir);

    // Read invocation meta for context
    let inv_meta = store.read_invocation_meta(&resolved.invocation_id).ok();
    let short_id = &resolved.invocation_id[..4.min(resolved.invocation_id.len())];

    // Header: STATUS  job_name  [short_id]
    output.info(&format!(
        "{}  {}{}",
        format_status(&meta.status),
        bold(&meta.name),
        dim(&format!("  [{short_id}]")),
    ));

    // Metadata lines
    if !meta.worktree.is_empty() {
        output.info(&format!("worktree:  {}", meta.worktree));
    }
    if let Some(ref inv) = inv_meta {
        output.info(&format!("trigger:   {}", inv.trigger_command));
    }

    let now = chrono::Utc::now();
    let ago = crate::output::format::shorthand_from_seconds(
        now.signed_duration_since(meta.started_at).num_seconds(),
    );
    let started_str = meta.started_at.with_timezone(&chrono::Local).format("%Y-%m-%d %H:%M:%S");
    output.info(&format!("started:   {ago} ago ({started_str})"));

    // Duration
    let duration_secs = match meta.finished_at {
        Some(finished) => finished.signed_duration_since(meta.started_at).num_seconds(),
        None => now.signed_duration_since(meta.started_at).num_seconds(),
    };
    output.info(&format!("duration:  {}s", duration_secs));

    if !meta.command.is_empty() {
        output.info(&format!("command:   {}", meta.command));
    }

    output.info("");
    output.info(&dim("--- output ---"));

    if log_path.exists() {
        let contents = std::fs::read_to_string(&log_path)
            .with_context(|| format!("Failed to read log file: {}", log_path.display()))?;
        output.info(&contents);
    } else {
        output.info(&dim("(no output)"));
    }

    output.info("");
    output.info(&dim(&format!("Full log: {}", log_path.display())));

    Ok(())
}
```

- [ ] **Step 4: Update `cancel_job` to use address resolution**

```rust
fn cancel_job(job: &str, inv: Option<&str>, path: &Path, output: &mut dyn Output) -> Result<()> {
    let repo_hash = compute_repo_hash_from_path(path)?;
    let store = LogStore::for_repo(&repo_hash)?;
    let current_worktree = crate::core::repo::get_current_branch().unwrap_or_default();
    let addr = JobAddress::parse(job).with_inv_override(inv);
    let resolved = resolve_job_address(&addr, &store, &current_worktree)?;

    match CoordinatorClient::connect(&repo_hash)? {
        Some(mut client) => {
            let msg = client.cancel_job(&resolved.job_name)?;
            output.success(&msg);
        }
        None => {
            anyhow::bail!("No coordinator running for this repository. Is the job still active?");
        }
    }

    Ok(())
}
```

- [ ] **Step 5: Update `retry_job` to use address resolution**

```rust
fn retry_job(job: &str, inv: Option<&str>, path: &Path, output: &mut dyn Output) -> Result<()> {
    let repo_hash = compute_repo_hash_from_path(path)?;
    let store = LogStore::for_repo(&repo_hash)?;
    let current_worktree = crate::core::repo::get_current_branch().unwrap_or_default();
    let addr = JobAddress::parse(job).with_inv_override(inv);
    let resolved = resolve_job_address(&addr, &store, &current_worktree)?;

    let meta = store.read_meta(&resolved.job_dir)?;

    if !matches!(meta.status, JobStatus::Failed) {
        anyhow::bail!(
            "Job '{}' has status '{}'. Only failed jobs can be retried.",
            meta.name,
            match meta.status {
                JobStatus::Running => "running",
                JobStatus::Completed => "completed",
                JobStatus::Cancelled => "cancelled",
                JobStatus::Failed => unreachable!(),
            }
        );
    }

    if meta.command.is_empty() {
        anyhow::bail!(
            "Cannot retry job '{}': no command recorded in metadata.",
            meta.name
        );
    }

    let working_dir = std::path::PathBuf::from(&meta.working_dir);
    if !working_dir.exists() {
        anyhow::bail!(
            "Cannot retry job '{}': working directory '{}' no longer exists.",
            meta.name,
            meta.working_dir
        );
    }

    output.info(&format!("Retrying job: {}", bold(&meta.name)));
    output.info(&format!("  command:  {}", dim(&meta.command)));
    output.info(&format!("  workdir:  {}", dim(&meta.working_dir)));

    let job_spec = crate::executor::JobSpec {
        name: meta.name.clone(),
        command: meta.command.clone(),
        working_dir,
        env: meta.env.clone(),
        background: true,
        ..Default::default()
    };

    let invocation_id = generate_invocation_id();
    let retry_store = LogStore::for_repo(&repo_hash)?;
    let trigger_command = format!("hooks jobs retry {}", meta.name);
    let mut coord_state =
        crate::coordinator::process::CoordinatorState::new(&repo_hash, &invocation_id)
            .with_metadata(&trigger_command, &meta.hook_type, &meta.worktree);
    coord_state.add_job(job_spec);

    #[cfg(unix)]
    {
        crate::coordinator::process::fork_coordinator(coord_state, retry_store)?;
        output.success(&format!("Job '{}' re-dispatched to background.", meta.name));
    }

    #[cfg(not(unix))]
    {
        let _ = (coord_state, retry_store);
        anyhow::bail!("Background job retry is only supported on Unix systems.");
    }

    Ok(())
}
```

- [ ] **Step 6: Update `clean_logs` to respect `--all` scope**

```rust
fn clean_logs(args: &JobsArgs, path: &Path, output: &mut dyn Output) -> Result<()> {
    let repo_hashes = if args.all {
        list_all_repo_hashes()?
    } else {
        vec![compute_repo_hash_from_path(path)?]
    };

    let mut total_removed = 0;
    for repo_hash in &repo_hashes {
        let store = LogStore::for_repo(repo_hash)?;
        total_removed += store.clean(chrono::Duration::days(7))?;
    }

    if total_removed > 0 {
        output.success(&format!("Removed {total_removed} old job log(s)."));
    } else {
        output.info("No old logs to clean.");
    }

    Ok(())
}
```

- [ ] **Step 7: Update `list_jobs` to use `args.all` instead of
      `args.all_repos`**

In `list_jobs()`, replace `args.all_repos` with `args.all` (lines 125 and 193).
Remove the `args.worktree` filter logic (the worktree filter now comes from the
display redesign in Task 8). For now the listing behavior can remain the same
structurally -- it will be fully replaced in Task 8.

- [ ] **Step 8: Run tests, clippy, fmt**

Run: `mise run fmt && mise run clippy && mise run test:unit` Expected: All tests
pass.

- [ ] **Step 9: Commit**

```bash
git add src/commands/hooks/jobs.rs
git commit -m "feat(jobs): update CLI interface with --all, --inv, and address-based subcommands"
```

---

### Task 8: Grouped display redesign for `list_jobs`

**Files:**

- Modify: `src/commands/hooks/jobs.rs`

This is the core display rewrite. The output groups jobs by worktree (when
`--all`) and invocation, using `tabled::Builder` for aligned columns.

- [ ] **Step 1: Add imports for tabled and formatting**

At the top of `src/commands/hooks/jobs.rs`, add/update imports:

```rust
use crate::coordinator::log_store::{InvocationMeta, JobStatus, LogStore};
use crate::output::format::shorthand_from_seconds;
use crate::styles::{blue, bold, dim, dim_underline, green, red, yellow};
use tabled::{
    builder::Builder,
    settings::{Padding, Style, object::Columns},
};
```

Remove the unused `use crate::coordinator::client::CoordinatorClient;` from the
list_jobs path (it is still needed for cancel).

- [ ] **Step 2: Add stale-detection helper**

```rust
/// Check if a coordinator is currently running for this repo.
fn is_coordinator_running(repo_hash: &str) -> bool {
    crate::coordinator::coordinator_socket_path(repo_hash)
        .map(|p| p.exists())
        .unwrap_or(false)
}
```

- [ ] **Step 3: Rewrite `list_jobs` for grouped display**

Replace the entire `list_jobs` function. This is the largest single change:

```rust
fn list_jobs(args: &JobsArgs, path: &Path, output: &mut dyn Output) -> Result<()> {
    let repo_hash = compute_repo_hash_from_path(path)?;
    let store = LogStore::for_repo(&repo_hash)?;

    let current_worktree = crate::core::repo::get_current_branch().ok();
    let coordinator_alive = is_coordinator_running(&repo_hash);

    // Load invocations, filtered by worktree scope.
    let invocations = if args.all {
        store.list_invocations()?
    } else {
        match &current_worktree {
            Some(wt) => store.list_invocations_for_worktree(wt)?,
            None => {
                output.warning(
                    "Could not determine current worktree. Showing all worktrees.",
                );
                store.list_invocations()?
            }
        }
    };

    if invocations.is_empty() {
        output.info(
            "No background job history for this worktree.\nUse --all to see jobs across all worktrees.",
        );
        return Ok(());
    }

    if args.json {
        // Implemented in Task 9. Stub here to keep Task 8 compilable.
        return print_json_output(&invocations, &store, coordinator_alive, output);
    }

    // Group invocations by worktree for --all display.
    let show_worktree_headers = args.all;
    let mut grouped: std::collections::BTreeMap<String, Vec<&InvocationMeta>> =
        std::collections::BTreeMap::new();
    for inv in &invocations {
        grouped.entry(inv.worktree.clone()).or_default().push(inv);
    }

    let now = chrono::Utc::now();

    for (worktree, worktree_invocations) in &grouped {
        if show_worktree_headers {
            output.info(&bold(worktree));
        }

        let indent = if show_worktree_headers { "  " } else { "" };

        for inv in worktree_invocations {
            // Invocation header
            let age_secs = now.signed_duration_since(inv.created_at).num_seconds();
            let ago = shorthand_from_seconds(age_secs);
            let short_id = &inv.invocation_id[..4.min(inv.invocation_id.len())];

            output.info(&format!(
                "{indent}{} -- {}{}",
                dim(&format!("{ago} ago")),
                inv.trigger_command,
                dim(&format!("  [{short_id}]")),
            ));

            // Load jobs for this invocation
            let job_dirs = store.list_jobs_in_invocation(&inv.invocation_id)?;
            if job_dirs.is_empty() {
                continue;
            }

            let table_indent = format!("{indent}  ");

            // Build table
            let mut builder = Builder::new();
            builder.push_record([
                dim_underline("Job"),
                dim_underline("Status"),
                dim_underline("Started"),
                dim_underline("Duration"),
            ]);

            for dir in &job_dirs {
                if let Ok(meta) = store.read_meta(dir) {
                    let prefix = if meta.background {
                        blue("\u{21bb} ")
                    } else {
                        "  ".to_string()
                    };

                    let status_str = format_status_inline(&meta.status, coordinator_alive);

                    let started_str = meta
                        .started_at
                        .with_timezone(&chrono::Local)
                        .format("%H:%M:%S")
                        .to_string();

                    let duration_secs = match meta.finished_at {
                        Some(finished) => {
                            finished.signed_duration_since(meta.started_at).num_seconds()
                        }
                        None => now.signed_duration_since(meta.started_at).num_seconds(),
                    };
                    let duration_str = format!("{}s", duration_secs);

                    builder.push_record([
                        format!("{prefix}{}", meta.name),
                        status_str,
                        started_str,
                        duration_str,
                    ]);
                }
            }

            let mut table = builder.build();
            table.with(Style::blank());
            table.modify(Columns::first(), Padding::new(0, 1, 0, 0));

            // Print each line with the appropriate indent
            for line in table.to_string().lines() {
                output.info(&format!("{table_indent}{line}"));
            }

            output.info("");
        }
    }

    Ok(())
}

/// Format status with icon and color for inline display.
fn format_status_inline(status: &JobStatus, coordinator_alive: bool) -> String {
    match status {
        JobStatus::Completed => green("\u{2713} completed"),
        JobStatus::Failed => red("\u{2717} failed"),
        JobStatus::Running => {
            if coordinator_alive {
                yellow("\u{27f3} running")
            } else {
                yellow("\u{27f3} running (stale)")
            }
        }
        JobStatus::Cancelled => dim("\u{2014} cancelled"),
    }
}
```

- [ ] **Step 4: Add stub for `print_json_output` (implemented in Task 9)**

This stub keeps Task 8 compilable before Task 9 fills in the real
implementation:

```rust
fn print_json_output(
    _invocations: &[InvocationMeta],
    _store: &LogStore,
    _coordinator_alive: bool,
    output: &mut dyn Output,
) -> Result<()> {
    output.info("JSON output not yet implemented.");
    Ok(())
}
```

- [ ] **Step 5: Run tests, clippy, fmt**

Run: `mise run fmt && mise run clippy && mise run test:unit` Expected: All tests
pass. No clippy warnings.

- [ ] **Step 6: Commit**

```bash
git add src/commands/hooks/jobs.rs
git commit -m "feat(jobs): rewrite list_jobs with invocation-grouped display"
```

---

### Task 9: JSON output

**Files:**

- Modify: `src/commands/hooks/jobs.rs`

Replace the `print_json_output` stub from Task 8 with the full implementation
that produces the nested hierarchy matching the spec.

- [ ] **Step 1: Add serde structs for JSON output**

Add after the existing imports in `src/commands/hooks/jobs.rs`:

```rust
#[derive(serde::Serialize)]
struct JsonOutput {
    worktrees: Vec<JsonWorktree>,
}

#[derive(serde::Serialize)]
struct JsonWorktree {
    name: String,
    invocations: Vec<JsonInvocation>,
}

#[derive(serde::Serialize)]
struct JsonInvocation {
    id: String,
    short_id: String,
    trigger_command: String,
    hook_type: String,
    created_at: String,
    jobs: Vec<JsonJob>,
}

#[derive(serde::Serialize)]
struct JsonJob {
    name: String,
    background: bool,
    status: String,
    exit_code: Option<i32>,
    started_at: String,
    finished_at: Option<String>,
    duration_secs: i64,
    command: String,
}
```

- [ ] **Step 2: Replace the stub with the full `print_json_output`**

```rust
fn print_json_output(
    invocations: &[InvocationMeta],
    store: &LogStore,
    coordinator_alive: bool,
    output: &mut dyn Output,
) -> Result<()> {
    let now = chrono::Utc::now();

    // Group by worktree
    let mut grouped: std::collections::BTreeMap<String, Vec<&InvocationMeta>> =
        std::collections::BTreeMap::new();
    for inv in invocations {
        grouped.entry(inv.worktree.clone()).or_default().push(inv);
    }

    let worktrees: Vec<JsonWorktree> = grouped
        .into_iter()
        .map(|(worktree, invs)| {
            let json_invocations: Vec<JsonInvocation> = invs
                .iter()
                .filter_map(|inv| {
                    let job_dirs = store.list_jobs_in_invocation(&inv.invocation_id).ok()?;
                    let jobs: Vec<JsonJob> = job_dirs
                        .iter()
                        .filter_map(|dir| {
                            let meta = store.read_meta(dir).ok()?;
                            let duration_secs = match meta.finished_at {
                                Some(finished) => {
                                    finished.signed_duration_since(meta.started_at).num_seconds()
                                }
                                None => now.signed_duration_since(meta.started_at).num_seconds(),
                            };
                            let status_str = match &meta.status {
                                JobStatus::Running if !coordinator_alive => {
                                    "running (stale)".to_string()
                                }
                                JobStatus::Running => "running".to_string(),
                                JobStatus::Completed => "completed".to_string(),
                                JobStatus::Failed => "failed".to_string(),
                                JobStatus::Cancelled => "cancelled".to_string(),
                            };
                            Some(JsonJob {
                                name: meta.name,
                                background: meta.background,
                                status: status_str,
                                exit_code: meta.exit_code,
                                started_at: meta.started_at.to_rfc3339(),
                                finished_at: meta.finished_at.map(|f| f.to_rfc3339()),
                                duration_secs,
                                command: meta.command,
                            })
                        })
                        .collect();
                    Some(JsonInvocation {
                        id: inv.invocation_id.clone(),
                        short_id: inv.invocation_id[..4.min(inv.invocation_id.len())]
                            .to_string(),
                        trigger_command: inv.trigger_command.clone(),
                        hook_type: inv.hook_type.clone(),
                        created_at: inv.created_at.to_rfc3339(),
                        jobs,
                    })
                })
                .collect();
            JsonWorktree {
                name: worktree,
                invocations: json_invocations,
            }
        })
        .collect();

    let json_output = JsonOutput { worktrees };
    let json_str = serde_json::to_string_pretty(&json_output)?;
    output.info(&json_str);

    Ok(())
}
```

- [ ] **Step 3: Run tests, clippy, fmt**

Run: `mise run fmt && mise run clippy && mise run test:unit` Expected: All tests
pass.

- [ ] **Step 4: Commit**

```bash
git add src/commands/hooks/jobs.rs
git commit -m "feat(jobs): add nested JSON output for hooks jobs --json"
```

---

### Task 10: Shell completions -- `complete.rs` handler

**Files:**

- Modify: `src/commands/complete.rs`

Add the `hooks-jobs-job` completion handler that provides context-aware
completions based on colon position.

- [ ] **Step 1: Add match arm in `complete()`**

In `src/commands/complete.rs`, add a new arm in the `match (command, position)`
block (before the `_ => Ok(vec![])` default, around line 120):

```rust
// hooks jobs: complete job addresses (names, invocation IDs, composite)
("hooks-jobs-job", 1) => complete_job_addresses(word),
```

- [ ] **Step 2: Implement `complete_job_addresses`**

Add the function at the bottom of the file (before `#[cfg(test)]`):

```rust
/// Complete job addresses for `hooks jobs logs|retry|cancel`.
///
/// Adapts to the colon level in the current word:
/// - No colon: job names from latest invocation + invocation short IDs
/// - After `inv:`: jobs within that invocation
/// - After `wt:`: invocation IDs for that worktree
/// - After `wt:inv:`: jobs within that worktree+invocation
fn complete_job_addresses(prefix: &str) -> Result<Vec<String>> {
    use crate::coordinator::log_store::LogStore;

    let repo_hash = find_project_root()
        .ok()
        .and_then(|root| {
            use std::collections::hash_map::DefaultHasher;
            use std::hash::{Hash, Hasher};
            let mut hasher = DefaultHasher::new();
            root.display().to_string().hash(&mut hasher);
            Some(format!("{:016x}", hasher.finish()))
        });
    let repo_hash = match repo_hash {
        Some(h) => h,
        None => return Ok(vec![]),
    };

    let store = match LogStore::for_repo(&repo_hash) {
        Ok(s) => s,
        Err(_) => return Ok(vec![]),
    };

    let current_worktree = crate::core::repo::get_current_branch().unwrap_or_default();
    let now = chrono::Utc::now();

    let colon_count = prefix.matches(':').count();

    match colon_count {
        0 => {
            // First level: job names + invocation short IDs
            let invocations = store
                .list_invocations_for_worktree(&current_worktree)
                .unwrap_or_default();
            let mut entries = Vec::new();

            // Job names from the latest invocation
            if let Some(latest) = invocations.last() {
                let job_dirs = store
                    .list_jobs_in_invocation(&latest.invocation_id)
                    .unwrap_or_default();
                for dir in &job_dirs {
                    if let Ok(meta) = store.read_meta(dir) {
                        let name = &meta.name;
                        if name.starts_with(prefix) {
                            let status_icon = match meta.status {
                                crate::coordinator::log_store::JobStatus::Completed => {
                                    "\u{2713} completed"
                                }
                                crate::coordinator::log_store::JobStatus::Failed => {
                                    "\u{2717} failed"
                                }
                                crate::coordinator::log_store::JobStatus::Running => {
                                    "\u{27f3} running"
                                }
                                crate::coordinator::log_store::JobStatus::Cancelled => {
                                    "\u{2014} cancelled"
                                }
                            };
                            let short_id =
                                &latest.invocation_id[..4.min(latest.invocation_id.len())];
                            let ago = crate::output::format::shorthand_from_seconds(
                                now.signed_duration_since(latest.created_at).num_seconds(),
                            );
                            entries.push(format!(
                                "{name}\t{status_icon} -- {ago} ago [{short_id}]"
                            ));
                        }
                    }
                }
            }

            // Invocation short IDs
            for inv in &invocations {
                let short_id = &inv.invocation_id[..4.min(inv.invocation_id.len())];
                if short_id.starts_with(prefix) {
                    let ago = crate::output::format::shorthand_from_seconds(
                        now.signed_duration_since(inv.created_at).num_seconds(),
                    );
                    let job_count = store
                        .list_jobs_in_invocation(&inv.invocation_id)
                        .map(|d| d.len())
                        .unwrap_or(0);
                    entries.push(format!(
                        "{short_id}\t{} -- {ago} ago ({job_count} job{})",
                        inv.trigger_command,
                        if job_count == 1 { "" } else { "s" },
                    ));
                }
            }

            Ok(entries)
        }
        1 => {
            // After one colon: could be inv:job or worktree:inv
            let (before, after) = prefix.split_once(':').unwrap_or(("", ""));

            // Try as invocation prefix first
            let invocations = store
                .list_invocations_for_worktree(&current_worktree)
                .unwrap_or_default();
            let matching: Vec<_> = invocations
                .iter()
                .filter(|inv| inv.invocation_id.starts_with(before))
                .collect();

            if matching.len() == 1 {
                // inv:job completions
                let inv = matching[0];
                let short_id = &inv.invocation_id[..4.min(inv.invocation_id.len())];
                let job_dirs = store
                    .list_jobs_in_invocation(&inv.invocation_id)
                    .unwrap_or_default();
                let mut entries = Vec::new();
                for dir in &job_dirs {
                    if let Ok(meta) = store.read_meta(dir) {
                        if meta.name.starts_with(after) {
                            let status_icon = match meta.status {
                                crate::coordinator::log_store::JobStatus::Completed => {
                                    "\u{2713} completed"
                                }
                                crate::coordinator::log_store::JobStatus::Failed => {
                                    "\u{2717} failed"
                                }
                                crate::coordinator::log_store::JobStatus::Running => {
                                    "\u{27f3} running"
                                }
                                crate::coordinator::log_store::JobStatus::Cancelled => {
                                    "\u{2014} cancelled"
                                }
                            };
                            entries.push(format!("{short_id}:{}\t{status_icon}", meta.name));
                        }
                    }
                }
                return Ok(entries);
            }

            // Try as worktree: prefix (complete invocation IDs for that worktree)
            let wt_invocations = store
                .list_invocations_for_worktree(before)
                .unwrap_or_default();
            let mut entries = Vec::new();
            for inv in &wt_invocations {
                let short_id = &inv.invocation_id[..4.min(inv.invocation_id.len())];
                if short_id.starts_with(after) {
                    let ago = crate::output::format::shorthand_from_seconds(
                        now.signed_duration_since(inv.created_at).num_seconds(),
                    );
                    entries.push(format!(
                        "{before}:{short_id}\t{} -- {ago} ago",
                        inv.trigger_command,
                    ));
                }
            }
            Ok(entries)
        }
        2 => {
            // worktree:inv:job
            let parts: Vec<&str> = prefix.rsplitn(3, ':').collect();
            let (job_prefix, inv_prefix, wt) = (parts[0], parts[1], parts[2]);
            let invocations = store.list_invocations_for_worktree(wt).unwrap_or_default();
            let matching: Vec<_> = invocations
                .iter()
                .filter(|inv| inv.invocation_id.starts_with(inv_prefix))
                .collect();

            if matching.len() != 1 {
                return Ok(vec![]);
            }
            let inv = matching[0];
            let short_id = &inv.invocation_id[..4.min(inv.invocation_id.len())];
            let job_dirs = store
                .list_jobs_in_invocation(&inv.invocation_id)
                .unwrap_or_default();
            let mut entries = Vec::new();
            for dir in &job_dirs {
                if let Ok(meta) = store.read_meta(dir) {
                    if meta.name.starts_with(job_prefix) {
                        let status_icon = match meta.status {
                            crate::coordinator::log_store::JobStatus::Completed => {
                                "\u{2713} completed"
                            }
                            crate::coordinator::log_store::JobStatus::Failed => "\u{2717} failed",
                            crate::coordinator::log_store::JobStatus::Running => "\u{27f3} running",
                            crate::coordinator::log_store::JobStatus::Cancelled => {
                                "\u{2014} cancelled"
                            }
                        };
                        entries.push(format!(
                            "{wt}:{short_id}:{}\t{status_icon}",
                            meta.name
                        ));
                    }
                }
            }
            Ok(entries)
        }
        _ => Ok(vec![]),
    }
}
```

- [ ] **Step 3: Run tests, clippy, fmt**

Run: `mise run fmt && mise run clippy && mise run test:unit` Expected: All tests
pass.

- [ ] **Step 4: Commit**

```bash
git add src/commands/complete.rs
git commit -m "feat(jobs): add hooks-jobs-job completion handler for composite addresses"
```

---

### Task 11: Shell completion script wiring (bash, zsh, fish)

**Files:**

- Modify: `src/commands/completions/bash.rs`
- Modify: `src/commands/completions/zsh.rs`
- Modify: `src/commands/completions/fish.rs`

Wire the `hooks-jobs-job` completion for `logs`, `retry`, and `cancel`
subcommands. Update `--all-repos` to `--all`.

- [ ] **Step 1: Update bash completions**

In `src/commands/completions/bash.rs`, replace the `jobs)` case (lines 225-234)
with:

```bash
            jobs)
                if [[ $cword -eq 3 ]]; then
                    COMPREPLY=( $(compgen -W "logs cancel retry clean" -- "$cur") )
                    return 0
                fi
                case "${words[3]}" in
                    logs|retry|cancel)
                        if [[ "$cur" == -* ]]; then
                            COMPREPLY=( $(compgen -W "--inv -h --help" -- "$cur") )
                            return 0
                        fi
                        local completions
                        completions=$(daft __complete hooks-jobs-job "$cur" 2>/dev/null)
                        if [[ -n "$completions" ]]; then
                            while IFS=$'\n' read -r line; do
                                local val="${line%%	*}"
                                COMPREPLY+=( "$val" )
                            done <<< "$completions"
                        fi
                        return 0
                        ;;
                esac
                if [[ "$cur" == -* ]]; then
                    COMPREPLY=( $(compgen -W "--all --json -h --help" -- "$cur") )
                    return 0
                fi
                return 0
                ;;
```

- [ ] **Step 2: Update zsh completions**

In `src/commands/completions/zsh.rs`, replace the `jobs)` case (lines 313-322)
with:

```zsh
            jobs)
                if (( CURRENT == 4 )); then
                    compadd logs cancel retry clean
                    return
                fi
                case "$words[4]" in
                    logs|retry|cancel)
                        if [[ "$curword" == -* ]]; then
                            compadd -- --inv -h --help
                            return
                        fi
                        local -a completions
                        completions=("${(@f)$(daft __complete hooks-jobs-job "$curword" 2>/dev/null)}")
                        local -a vals descs
                        for line in $completions; do
                            vals+=("${line%%	*}")
                            descs+=("$line")
                        done
                        _describe 'job' descs
                        return
                        ;;
                esac
                if [[ "$curword" == -* ]]; then
                    compadd -- --all --json -h --help
                fi
                return
                ;;
```

- [ ] **Step 3: Update fish completions**

In `src/commands/completions/fish.rs`, replace the hooks jobs lines (lines
287-290) with:

```fish
# hooks jobs: sub-subcommands and flags
complete -c daft -n '__fish_seen_subcommand_from hooks; and __fish_seen_subcommand_from jobs; and not __fish_seen_subcommand_from logs cancel retry clean' -f -a 'logs cancel retry clean'
complete -c daft -n '__fish_seen_subcommand_from hooks; and __fish_seen_subcommand_from jobs' -l all -d 'Show jobs from all worktrees'
complete -c daft -n '__fish_seen_subcommand_from hooks; and __fish_seen_subcommand_from jobs' -l json -d 'Output as JSON'
complete -c daft -n '__fish_seen_subcommand_from hooks; and __fish_seen_subcommand_from jobs; and __fish_seen_subcommand_from logs cancel retry' -l inv -d 'Invocation ID prefix'
complete -c daft -n '__fish_seen_subcommand_from hooks; and __fish_seen_subcommand_from jobs; and __fish_seen_subcommand_from logs cancel retry' -f -a "(daft __complete hooks-jobs-job (commandline -ct) 2>/dev/null)"
```

- [ ] **Step 4: Run tests, clippy, fmt**

Run: `mise run fmt && mise run clippy && mise run test:unit` Expected: All tests
pass. No clippy warnings.

- [ ] **Step 5: Commit**

```bash
git add src/commands/completions/bash.rs src/commands/completions/zsh.rs src/commands/completions/fish.rs
git commit -m "feat(jobs): wire hooks-jobs-job completions into bash/zsh/fish scripts"
```

---

### Critical Files for Implementation

- /Users/avihu/Projects/daft/feat/background-hook-jobs/src/coordinator/log_store.rs
- /Users/avihu/Projects/daft/feat/background-hook-jobs/src/coordinator/process.rs
- /Users/avihu/Projects/daft/feat/background-hook-jobs/src/commands/hooks/jobs.rs
- /Users/avihu/Projects/daft/feat/background-hook-jobs/src/commands/complete.rs
- /Users/avihu/Projects/daft/feat/background-hook-jobs/src/hooks/yaml_executor/mod.rs
