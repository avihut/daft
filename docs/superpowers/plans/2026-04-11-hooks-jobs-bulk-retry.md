# Hooks Jobs Bulk Retry — Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use
> superpowers:subagent-driven-development (recommended) or
> superpowers:executing-plans to implement this plan task-by-task. Steps use
> checkbox (`- [ ]`) syntax for tracking.

**Goal:** Replace the single-job `daft hooks jobs retry <job>` with bulk retry
forms (`retry`, `retry <hook>`, `retry <inv-prefix>`, `retry <job>`) that select
and re-dispatch all failed/cancelled jobs from an invocation, preserving DAG
relationships.

**Architecture:** Parse the retry positional into a `RetryTarget` enum, resolve
it to an invocation, compute a retry set of failed/cancelled jobs with pruned
`needs:` edges, split by fg/bg, execute fg inline then fork bg to the
coordinator — all within a single new invocation. Also fix the two-segment
`JobAddress` parser to support `worktree:job` addressing.

**Tech Stack:** Rust, clap, serde, chrono, tabled, anyhow

---

## File Structure

| File                                              | Responsibility                                                                             |
| ------------------------------------------------- | ------------------------------------------------------------------------------------------ |
| `src/coordinator/log_store.rs`                    | Add `needs` field to `JobMeta`, add `find_invocations_by_prefix` method                    |
| `src/executor/log_sink.rs`                        | Persist `spec.needs` into `JobMeta.needs` in `on_job_complete`                             |
| `src/coordinator/process.rs`                      | Persist `spec.needs` into `JobMeta.needs` in `run_single_background_job`                   |
| `src/commands/hooks/jobs.rs`                      | `RetryTarget` enum, `retry_command` orchestration, `JobAddress` B-1 fix, updated clap args |
| `src/commands/complete.rs`                        | New `complete_retry_targets` function with three helpers                                   |
| `src/commands/completions/{bash,zsh,fish,fig}.rs` | Wire `retry` to new completion dispatch + new flags                                        |
| `tests/manual/scenarios/hooks/retry-*.yml`        | 7 new integration scenarios                                                                |

---

### Task 1: Add `needs` field to `JobMeta`

**Files:**

- Modify: `src/coordinator/log_store.rs:17-31`
- Test: `src/coordinator/log_store.rs` (existing tests + 1 new)

- [ ] **Step 1: Write the failing test**

Add this test at the bottom of the `#[cfg(test)] mod tests` block in
`src/coordinator/log_store.rs`:

```rust
#[test]
fn job_meta_needs_round_trips_through_json() {
    let tmp = TempDir::new().unwrap();
    let store = LogStore::new(tmp.path().to_path_buf());
    let dir = store.create_job_dir("inv-needs", "seeder").unwrap();
    let meta = JobMeta {
        name: "seeder".to_string(),
        hook_type: "worktree-post-create".to_string(),
        worktree: "feature/x".to_string(),
        command: "echo seed".to_string(),
        working_dir: "/tmp".to_string(),
        env: HashMap::new(),
        started_at: chrono::Utc::now(),
        status: JobStatus::Completed,
        exit_code: Some(0),
        pid: None,
        background: false,
        finished_at: None,
        needs: vec!["migrator".to_string()],
    };
    store.write_meta(&dir, &meta).unwrap();
    let loaded = store.read_meta(&dir).unwrap();
    assert_eq!(loaded.needs, vec!["migrator".to_string()]);
}
```

- [ ] **Step 2: Run test to verify it fails**

Run: `mise run test:unit -- --test job_meta_needs_round_trips`

Expected: Compile error — `JobMeta` has no field named `needs`.

- [ ] **Step 3: Add the `needs` field to `JobMeta`**

In `src/coordinator/log_store.rs`, add the field to the `JobMeta` struct (after
the `finished_at` field at line 30):

```rust
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct JobMeta {
    pub name: String,
    pub hook_type: String,
    pub worktree: String,
    pub command: String,
    pub working_dir: String,
    pub env: HashMap<String, String>,
    pub started_at: chrono::DateTime<chrono::Utc>,
    pub status: JobStatus,
    pub exit_code: Option<i32>,
    pub pid: Option<u32>,
    pub background: bool,
    pub finished_at: Option<chrono::DateTime<chrono::Utc>>,
    #[serde(default)]
    pub needs: Vec<String>,
}
```

`#[serde(default)]` ensures back-compat: old JSON without `needs` deserializes
as `vec![]`.

- [ ] **Step 4: Fix all existing `JobMeta` construction sites**

Every place that constructs a `JobMeta` literal must now include `needs`. Search
for `JobMeta {` across the codebase. Add `needs: vec![],` to each:

- `src/executor/log_sink.rs` — `on_job_complete` (line 111-124) and
  `on_job_runner_skipped` (line 141-154)
- `src/coordinator/process.rs` — `run_single_background_job` (line 179-192)
- `src/coordinator/log_store.rs` — all test functions that construct `JobMeta`
  (`test_write_and_read_meta`, `test_clean_old_logs`,
  `test_job_meta_background_and_finished_at`,
  `skipped_status_round_trips_through_json`,
  `write_job_record_creates_meta_and_log_atomically`)
- `src/hooks/yaml_executor/mod.rs` — skipped-job `JobMeta` construction sites
  (search for `JobMeta {` in that file)

For each site, add `needs: vec![],` after the `finished_at` field.

- [ ] **Step 5: Run all tests to verify everything compiles and passes**

Run: `mise run test:unit`

Expected: All tests pass, including the new
`job_meta_needs_round_trips_through_json`.

- [ ] **Step 6: Also verify back-compat — old JSON without `needs` still loads**

Add this test in `src/coordinator/log_store.rs`:

```rust
#[test]
fn job_meta_without_needs_field_deserializes_to_empty_vec() {
    let json = r#"{
        "name": "old-job",
        "hook_type": "worktree-post-create",
        "worktree": "feature/x",
        "command": "echo hi",
        "working_dir": "/tmp",
        "env": {},
        "started_at": "2026-04-11T12:00:00Z",
        "status": "completed",
        "exit_code": 0,
        "pid": null,
        "background": false,
        "finished_at": null
    }"#;
    let meta: JobMeta = serde_json::from_str(json).unwrap();
    assert!(meta.needs.is_empty());
}
```

Run: `mise run test:unit -- --test job_meta_without_needs`

Expected: PASS.

- [ ] **Step 7: Run clippy and fmt**

Run: `mise run fmt && mise run clippy`

Expected: Zero warnings, zero errors.

- [ ] **Step 8: Commit**

```bash
git add src/coordinator/log_store.rs src/executor/log_sink.rs \
  src/coordinator/process.rs src/hooks/yaml_executor/mod.rs
git commit -m "feat(jobs): add needs field to JobMeta for DAG-aware retry"
```

---

### Task 2: Persist `spec.needs` into `JobMeta` at write sites

**Files:**

- Modify: `src/executor/log_sink.rs:111-124` (`on_job_complete`)
- Modify: `src/coordinator/process.rs:179-192` (`run_single_background_job`)
- Test: `src/executor/log_sink.rs` (update existing test)

- [ ] **Step 1: Update the `on_job_complete` implementation in
      `BufferingLogSink`**

In `src/executor/log_sink.rs`, in the `on_job_complete` method (line 104-132),
change the `JobMeta` construction to set `needs` from the spec:

Replace:

```rust
        let meta = JobMeta {
            name: spec.name.clone(),
            hook_type: self.hook_type.clone(),
            worktree: self.worktree.clone(),
            command: spec.command.clone(),
            working_dir: spec.working_dir.to_string_lossy().into_owned(),
            env: spec.env.clone(),
            started_at: buf.started_at,
            status: Self::node_to_job_status(result.status),
            exit_code: result.exit_code,
            pid: None,
            background: false,
            finished_at: Some(chrono::Utc::now()),
            needs: vec![],
        };
```

With:

```rust
        let meta = JobMeta {
            name: spec.name.clone(),
            hook_type: self.hook_type.clone(),
            worktree: self.worktree.clone(),
            command: spec.command.clone(),
            working_dir: spec.working_dir.to_string_lossy().into_owned(),
            env: spec.env.clone(),
            started_at: buf.started_at,
            status: Self::node_to_job_status(result.status),
            exit_code: result.exit_code,
            pid: None,
            background: false,
            finished_at: Some(chrono::Utc::now()),
            needs: spec.needs.clone(),
        };
```

- [ ] **Step 2: Update `on_job_runner_skipped` similarly**

In the `on_job_runner_skipped` method (line 134-162), change `needs: vec![]` to
`needs: spec.needs.clone()`.

- [ ] **Step 3: Update `run_single_background_job` in `process.rs`**

In `src/coordinator/process.rs`, in the `run_single_background_job` function
(line 179-192 `JobMeta` construction), change `needs: vec![]` to
`needs: job.needs.clone()`.

- [ ] **Step 4: Update existing sink test to verify `needs` is persisted**

In `src/executor/log_sink.rs`, update
`buffering_sink_writes_meta_and_log_on_complete` (line 193-223). After line
`let spec = make_spec("pnpm-install", false);`, add needs to the spec:

```rust
let mut spec = make_spec("pnpm-install", false);
spec.needs = vec!["db-migrate".to_string()];
```

And after the assertions, add:

```rust
assert_eq!(loaded.needs, vec!["db-migrate".to_string()]);
```

- [ ] **Step 5: Run tests**

Run: `mise run test:unit`

Expected: All tests pass.

- [ ] **Step 6: Run clippy and fmt**

Run: `mise run fmt && mise run clippy`

Expected: Clean.

- [ ] **Step 7: Commit**

```bash
git add src/executor/log_sink.rs src/coordinator/process.rs
git commit -m "feat(jobs): persist spec.needs into JobMeta at fg and bg write sites"
```

---

### Task 3: Add `find_invocations_by_prefix` to `LogStore`

**Files:**

- Modify: `src/coordinator/log_store.rs:187-190`
- Test: `src/coordinator/log_store.rs` (new tests)

- [ ] **Step 1: Write the failing test**

Add in the test module of `src/coordinator/log_store.rs`:

```rust
#[test]
fn find_invocations_by_prefix_returns_matching() {
    let tmp = TempDir::new().unwrap();
    let store = LogStore::new(tmp.path().to_path_buf());

    let now = chrono::Utc::now();
    for (inv_id, wt, offset) in &[
        ("a3f200000000", "feature/a", 100i64),
        ("a3f200000001", "feature/a", 50),
        ("b7c100000000", "feature/a", 10),
    ] {
        std::fs::create_dir_all(tmp.path().join(inv_id)).unwrap();
        let meta = InvocationMeta {
            invocation_id: inv_id.to_string(),
            trigger_command: "worktree-post-create".to_string(),
            hook_type: "worktree-post-create".to_string(),
            worktree: wt.to_string(),
            created_at: now - chrono::Duration::seconds(*offset),
        };
        store.write_invocation_meta(inv_id, &meta).unwrap();
    }

    let matches = store
        .find_invocations_by_prefix("feature/a", "a3f2")
        .unwrap();
    assert_eq!(matches.len(), 2);
    assert!(matches.iter().all(|m| m.invocation_id.starts_with("a3f2")));

    let matches = store
        .find_invocations_by_prefix("feature/a", "b7c1")
        .unwrap();
    assert_eq!(matches.len(), 1);

    let matches = store
        .find_invocations_by_prefix("feature/a", "zzzz")
        .unwrap();
    assert!(matches.is_empty());
}
```

- [ ] **Step 2: Run test to verify it fails**

Run: `mise run test:unit -- --test find_invocations_by_prefix`

Expected: Compile error — no method `find_invocations_by_prefix` on `LogStore`.

- [ ] **Step 3: Implement `find_invocations_by_prefix`**

Add this method to the `impl LogStore` block in `src/coordinator/log_store.rs`,
after `list_invocations_for_worktree`:

```rust
pub fn find_invocations_by_prefix(
    &self,
    worktree: &str,
    prefix: &str,
) -> Result<Vec<InvocationMeta>> {
    let all = self.list_invocations_for_worktree(worktree)?;
    Ok(all
        .into_iter()
        .filter(|m| m.invocation_id.starts_with(prefix))
        .collect())
}
```

- [ ] **Step 4: Run tests**

Run: `mise run test:unit -- --test find_invocations_by_prefix`

Expected: PASS.

- [ ] **Step 5: Run clippy and fmt**

Run: `mise run fmt && mise run clippy`

- [ ] **Step 6: Commit**

```bash
git add src/coordinator/log_store.rs
git commit -m "feat(jobs): add find_invocations_by_prefix to LogStore"
```

---

### Task 4: Fix `JobAddress` two-segment parsing (B-1 fold-in)

**Files:**

- Modify: `src/commands/hooks/jobs.rs:114-153` (JobAddress)
- Modify: `src/commands/hooks/jobs.rs:162-248` (resolve_job_address)
- Test: `src/commands/hooks/jobs.rs` (existing + new tests)

- [ ] **Step 1: Write failing tests for the new `WorktreeJob` variant**

Add these tests in the `#[cfg(test)] mod tests` block of
`src/commands/hooks/jobs.rs`:

```rust
#[test]
fn test_parse_job_address_worktree_job_two_segment() {
    let addr = JobAddress::parse("feature/auth:db-migrate");
    assert_eq!(addr.worktree.as_deref(), Some("feature/auth"));
    assert!(addr.invocation_prefix.is_none());
    assert_eq!(addr.job_name, "db-migrate");
}

#[test]
fn test_parse_job_address_two_segment_inv_job_no_slash() {
    let addr = JobAddress::parse("c9d4:db-migrate");
    assert!(addr.worktree.is_none());
    assert_eq!(addr.invocation_prefix.as_deref(), Some("c9d4"));
    assert_eq!(addr.job_name, "db-migrate");
}
```

- [ ] **Step 2: Run tests to verify the first one fails**

Run:
`mise run test:unit -- --test test_parse_job_address_worktree_job_two_segment`

Expected: FAIL — `addr.worktree` is `None` (current parser puts the first
segment into `invocation_prefix`).

- [ ] **Step 3: Fix the two-segment parse case in `JobAddress::parse`**

In `src/commands/hooks/jobs.rs`, change the `parse` method (lines 122-145).
Replace the `2 =>` match arm:

```rust
            2 => {
                // Disambiguate: if the left segment contains '/', it's a
                // worktree name (branch names always have '/'); otherwise
                // it's an invocation ID prefix (hex-only, no '/').
                let left = parts[1];
                if left.contains('/') {
                    Self {
                        worktree: Some(left.to_string()),
                        invocation_prefix: None,
                        job_name: parts[0].to_string(),
                    }
                } else {
                    Self {
                        worktree: None,
                        invocation_prefix: Some(left.to_string()),
                        job_name: parts[0].to_string(),
                    }
                }
            },
```

- [ ] **Step 4: Update `resolve_job_address` to handle worktree-only (no
      invocation)**

In `src/commands/hooks/jobs.rs`, the `resolve_job_address` function (line
162-248) currently branches on `addr.invocation_prefix`. After the fix, when
`addr.worktree` is `Some(...)` but `addr.invocation_prefix` is `None`, we need
the "find most recent invocation containing the job" path — which is exactly the
`None =>` arm at line 227-247. This already works: `worktree` defaults to
`addr.worktree.as_deref().unwrap_or(current_worktree)` (line 167), and the
`None =>` arm iterates invocations in that worktree.

Verify by reading the code — no change needed in `resolve_job_address`. The fix
is entirely in the parser.

- [ ] **Step 5: Update help text for `logs`, `cancel`, and `retry` subcommands**

In `src/commands/hooks/jobs.rs`, update the doc comment for the `job` field in
the `Logs` variant (line 85):

Replace:

```rust
        /// Job address: name, inv:name, or worktree:inv:name.
```

With:

```rust
        /// Job address: name, inv:name, worktree:name, or worktree:inv:name.
```

Apply the same update to the `Cancel` variant's `job` field (line 93-94) and the
`Retry` variant's `job` field (line 104-105).

- [ ] **Step 6: Run all tests**

Run: `mise run test:unit`

Expected: All pass, including the two new tests and all existing address tests.

- [ ] **Step 7: Run clippy and fmt**

Run: `mise run fmt && mise run clippy`

- [ ] **Step 8: Commit**

```bash
git add src/commands/hooks/jobs.rs
git commit -m "fix(jobs): support worktree:job two-segment address (B-1)"
```

---

### Task 5: Implement `RetryTarget` enum and shape parser

**Files:**

- Modify: `src/commands/hooks/jobs.rs` (add `RetryTarget`,
  `retry_target_from_arg`)
- Test: `src/commands/hooks/jobs.rs` (new unit tests)

- [ ] **Step 1: Write failing tests**

Add in the `#[cfg(test)] mod tests` block of `src/commands/hooks/jobs.rs`:

```rust
#[test]
fn test_retry_target_empty_is_latest() {
    let target = retry_target_from_arg(None, &RetryFlags::default());
    assert!(matches!(target, RetryTarget::LatestInvocation));
}

#[test]
fn test_retry_target_known_hook_type() {
    let target =
        retry_target_from_arg(Some("worktree-post-create"), &RetryFlags::default());
    assert!(matches!(target, RetryTarget::HookType(ref h) if h == "worktree-post-create"));
}

#[test]
fn test_retry_target_hex_prefix() {
    let target = retry_target_from_arg(Some("a3f2"), &RetryFlags::default());
    assert!(matches!(target, RetryTarget::InvocationPrefix(ref p) if p == "a3f2"));
}

#[test]
fn test_retry_target_job_name() {
    let target = retry_target_from_arg(Some("db-migrate"), &RetryFlags::default());
    assert!(matches!(target, RetryTarget::JobName(ref n) if n == "db-migrate"));
}

#[test]
fn test_retry_target_flag_overrides_shape() {
    let flags = RetryFlags { hook: Some("worktree-post-create".into()), ..Default::default() };
    let target = retry_target_from_arg(None, &flags);
    assert!(matches!(target, RetryTarget::HookType(ref h) if h == "worktree-post-create"));

    let flags = RetryFlags { inv: Some("a3f2".into()), ..Default::default() };
    let target = retry_target_from_arg(None, &flags);
    assert!(matches!(target, RetryTarget::InvocationPrefix(ref p) if p == "a3f2"));

    let flags = RetryFlags { job: Some("db-migrate".into()), ..Default::default() };
    let target = retry_target_from_arg(None, &flags);
    assert!(matches!(target, RetryTarget::JobName(ref n) if n == "db-migrate"));
}

#[test]
fn test_retry_target_post_clone_is_hook_not_job() {
    let target = retry_target_from_arg(Some("post-clone"), &RetryFlags::default());
    assert!(matches!(target, RetryTarget::HookType(ref h) if h == "post-clone"));
}

#[test]
fn test_retry_target_8char_hex_is_invocation() {
    let target = retry_target_from_arg(Some("deadbeef"), &RetryFlags::default());
    assert!(matches!(target, RetryTarget::InvocationPrefix(_)));
}

#[test]
fn test_retry_target_9char_hex_is_job() {
    // Hex prefixes are 2-8 chars; 9+ chars fall through to job name
    let target = retry_target_from_arg(Some("deadbeef0"), &RetryFlags::default());
    assert!(matches!(target, RetryTarget::JobName(_)));
}
```

- [ ] **Step 2: Run tests to verify compile failure**

Run: `mise run test:unit -- --test test_retry_target`

Expected: Compile error — `RetryTarget`, `retry_target_from_arg`, `RetryFlags`
not found.

- [ ] **Step 3: Implement the types and parser**

Add the following above the `run()` function in `src/commands/hooks/jobs.rs`
(before line 273):

```rust
use crate::hooks::HookType;

const KNOWN_HOOK_TYPES: &[&str] = &[
    "post-clone",
    "worktree-pre-create",
    "worktree-post-create",
    "worktree-pre-remove",
    "worktree-post-remove",
];

#[derive(Debug, PartialEq)]
enum RetryTarget {
    LatestInvocation,
    HookType(String),
    InvocationPrefix(String),
    JobName(String),
}

#[derive(Debug, Default)]
struct RetryFlags {
    hook: Option<String>,
    inv: Option<String>,
    job: Option<String>,
}

fn retry_target_from_arg(arg: Option<&str>, flags: &RetryFlags) -> RetryTarget {
    // Explicit flags take priority.
    if let Some(ref h) = flags.hook {
        return RetryTarget::HookType(h.clone());
    }
    if let Some(ref i) = flags.inv {
        return RetryTarget::InvocationPrefix(i.clone());
    }
    if let Some(ref j) = flags.job {
        return RetryTarget::JobName(j.clone());
    }

    match arg {
        None => RetryTarget::LatestInvocation,
        Some(a) => {
            if KNOWN_HOOK_TYPES.contains(&a) {
                RetryTarget::HookType(a.to_string())
            } else if a.len() >= 2
                && a.len() <= 8
                && a.chars().all(|c| c.is_ascii_hexdigit())
            {
                RetryTarget::InvocationPrefix(a.to_string())
            } else {
                RetryTarget::JobName(a.to_string())
            }
        }
    }
}
```

- [ ] **Step 4: Run tests**

Run: `mise run test:unit -- --test test_retry_target`

Expected: All 8 tests pass.

- [ ] **Step 5: Run clippy and fmt**

Run: `mise run fmt && mise run clippy`

Note: `HookType` import may trigger an unused warning for now; if so, prefix
with `#[allow(unused_imports)]` temporarily — it will be used in later tasks.
Actually, remove the `use crate::hooks::HookType;` import for now since
`KNOWN_HOOK_TYPES` is a string slice. Only add it when needed.

- [ ] **Step 6: Commit**

```bash
git add src/commands/hooks/jobs.rs
git commit -m "feat(jobs): add RetryTarget enum and shape-based disambiguator"
```

---

### Task 6: Implement retry set computation (`build_retry_set`)

**Files:**

- Modify: `src/commands/hooks/jobs.rs` (add `build_retry_set`)
- Test: `src/commands/hooks/jobs.rs` (new unit tests)

- [ ] **Step 1: Write failing tests**

Add in the test module of `src/commands/hooks/jobs.rs`:

```rust
#[test]
fn test_build_retry_set_picks_failed_and_cancelled() {
    use crate::coordinator::log_store::{JobMeta, JobStatus};
    use std::collections::HashMap;

    let metas = vec![
        make_test_job_meta("a", JobStatus::Completed, vec![]),
        make_test_job_meta("b", JobStatus::Failed, vec!["a".into()]),
        make_test_job_meta("c", JobStatus::Cancelled, vec!["b".into()]),
        make_test_job_meta("d", JobStatus::Skipped, vec![]),
    ];
    let (specs, _) = build_retry_set(&metas);
    let names: Vec<&str> = specs.iter().map(|s| s.name.as_str()).collect();
    assert_eq!(names, vec!["b", "c"]);
    // b's needs should be pruned (a is not in retry set)
    assert!(specs[0].needs.is_empty());
    // c's needs should point to b (b IS in retry set)
    assert_eq!(specs[1].needs, vec!["b".to_string()]);
}

#[test]
fn test_build_retry_set_all_green_returns_empty() {
    let metas = vec![
        make_test_job_meta("a", JobStatus::Completed, vec![]),
        make_test_job_meta("b", JobStatus::Completed, vec!["a".into()]),
    ];
    let (specs, _) = build_retry_set(&metas);
    assert!(specs.is_empty());
}

#[test]
fn test_build_retry_set_single_failed() {
    let metas = vec![
        make_test_job_meta("only", JobStatus::Failed, vec![]),
    ];
    let (specs, _) = build_retry_set(&metas);
    assert_eq!(specs.len(), 1);
    assert_eq!(specs[0].name, "only");
}
```

Also add this helper function in the test module:

```rust
fn make_test_job_meta(
    name: &str,
    status: crate::coordinator::log_store::JobStatus,
    needs: Vec<String>,
) -> crate::coordinator::log_store::JobMeta {
    crate::coordinator::log_store::JobMeta {
        name: name.to_string(),
        hook_type: "worktree-post-create".to_string(),
        worktree: "feature/x".to_string(),
        command: format!("echo {name}"),
        working_dir: "/tmp".to_string(),
        env: std::collections::HashMap::new(),
        started_at: chrono::Utc::now(),
        status,
        exit_code: None,
        pid: None,
        background: false,
        finished_at: None,
        needs,
    }
}
```

- [ ] **Step 2: Run tests to verify compile failure**

Run: `mise run test:unit -- --test test_build_retry_set`

Expected: Compile error — `build_retry_set` not found.

- [ ] **Step 3: Implement `build_retry_set`**

Add this function in `src/commands/hooks/jobs.rs` (near the `RetryTarget`
definitions):

```rust
fn build_retry_set(
    metas: &[crate::coordinator::log_store::JobMeta],
) -> (Vec<crate::executor::JobSpec>, Vec<String>) {
    let retry_names: std::collections::HashSet<String> = metas
        .iter()
        .filter(|m| matches!(m.status, JobStatus::Failed | JobStatus::Cancelled))
        .map(|m| m.name.clone())
        .collect();

    let mut specs: Vec<crate::executor::JobSpec> = metas
        .iter()
        .filter(|m| retry_names.contains(&m.name))
        .map(|m| {
            let needs: Vec<String> = m
                .needs
                .iter()
                .filter(|n| retry_names.contains(n.as_str()))
                .cloned()
                .collect();
            crate::executor::JobSpec {
                name: m.name.clone(),
                command: m.command.clone(),
                working_dir: std::path::PathBuf::from(&m.working_dir),
                env: m.env.clone(),
                background: m.background,
                needs,
                ..Default::default()
            }
        })
        .collect();

    let names: Vec<String> = specs.iter().map(|s| s.name.clone()).collect();
    (specs, names)
}
```

- [ ] **Step 4: Run tests**

Run: `mise run test:unit -- --test test_build_retry_set`

Expected: All 3 tests pass.

- [ ] **Step 5: Run clippy and fmt**

Run: `mise run fmt && mise run clippy`

- [ ] **Step 6: Commit**

```bash
git add src/commands/hooks/jobs.rs
git commit -m "feat(jobs): add build_retry_set for DAG-aware retry subset computation"
```

---

### Task 7: Update clap `Retry` variant with new args and flags

**Files:**

- Modify: `src/commands/hooks/jobs.rs:81-112` (JobsCommand enum)
- Modify: `src/commands/hooks/jobs.rs:273-294` (run function)

- [ ] **Step 1: Update the `Retry` variant in the `JobsCommand` enum**

In `src/commands/hooks/jobs.rs`, replace the `Retry` variant (lines 102-109):

```rust
    /// Re-run failed jobs from an invocation.
    Retry {
        /// Target: hook name, invocation prefix, or job name.
        /// Empty = retry all failed from most recent invocation.
        target: Option<String>,
        /// Force interpretation as a hook name.
        #[arg(long, conflicts_with_all = ["inv_flag", "job_flag"])]
        hook: Option<String>,
        /// Force interpretation as an invocation prefix.
        #[arg(long = "inv", conflicts_with_all = ["hook", "job_flag"])]
        inv_flag: Option<String>,
        /// Force interpretation as a job name.
        #[arg(long = "job", conflicts_with_all = ["hook", "inv_flag"])]
        job_flag: Option<String>,
    },
```

- [ ] **Step 2: Update the `run()` function dispatch**

In `src/commands/hooks/jobs.rs`, update the `Retry` arm in the `run()` function
(line 290-292):

```rust
        Some(JobsCommand::Retry {
            ref target,
            ref hook,
            ref inv_flag,
            ref job_flag,
        }) => retry_command(target.as_deref(), hook, inv_flag, job_flag, path, output),
```

- [ ] **Step 3: Create the `retry_command` stub**

Add a placeholder function (we'll fill it in the next task):

```rust
fn retry_command(
    target: Option<&str>,
    hook_flag: &Option<String>,
    inv_flag: &Option<String>,
    job_flag: &Option<String>,
    path: &Path,
    output: &mut dyn Output,
) -> Result<()> {
    let flags = RetryFlags {
        hook: hook_flag.clone(),
        inv: inv_flag.clone(),
        job: job_flag.clone(),
    };
    let parsed = retry_target_from_arg(target, &flags);
    output.info(&format!("Retry target: {parsed:?}"));
    Ok(())
}
```

- [ ] **Step 4: Verify compilation and existing tests pass**

Run: `mise run test:unit`

Expected: All pass. The old `retry_job` function still exists but is now
unreachable (the dispatch goes to `retry_command` instead). Leave `retry_job` in
place for now — we'll incorporate its logic in the next task.

- [ ] **Step 5: Run clippy and fmt**

Run: `mise run fmt && mise run clippy`

If clippy warns about `retry_job` being dead code, add `#[allow(dead_code)]`
above it temporarily.

- [ ] **Step 6: Commit**

```bash
git add src/commands/hooks/jobs.rs
git commit -m "feat(jobs): update Retry clap variant with bulk retry args and flags"
```

---

### Task 8: Implement `retry_command` — the full orchestration

**Files:**

- Modify: `src/commands/hooks/jobs.rs` (replace stub `retry_command`, remove old
  `retry_job`)

This is the core task. The function:

1. Resolves `RetryTarget` to an invocation.
2. Loads all jobs, computes the retry set.
3. Validates (no running jobs, non-empty set, etc.).
4. Splits by fg/bg.
5. Runs fg inline with `BufferingLogSink`.
6. Forks bg to coordinator.
7. Prints summary.

- [ ] **Step 1: Implement the resolve-to-invocation logic**

Replace the `retry_command` stub with the full implementation. Add a helper that
resolves `RetryTarget` + store + worktree → `InvocationMeta`:

```rust
fn resolve_retry_invocation(
    target: &RetryTarget,
    store: &LogStore,
    current_worktree: &str,
) -> Result<InvocationMeta> {
    match target {
        RetryTarget::LatestInvocation => {
            let invocations = store.list_invocations_for_worktree(current_worktree)?;
            invocations.into_iter().last().ok_or_else(|| {
                anyhow::anyhow!(
                    "No invocations found in worktree '{current_worktree}'. Run a hook first."
                )
            })
        }
        RetryTarget::HookType(hook) => {
            let invocations = store.list_invocations_for_worktree(current_worktree)?;
            invocations
                .into_iter()
                .filter(|inv| inv.hook_type == *hook)
                .last()
                .ok_or_else(|| {
                    anyhow::anyhow!(
                        "No invocations of '{hook}' in worktree '{current_worktree}'."
                    )
                })
        }
        RetryTarget::InvocationPrefix(prefix) => {
            let matches = store.find_invocations_by_prefix(current_worktree, prefix)?;
            match matches.len() {
                0 => anyhow::bail!(
                    "No invocation matching prefix '{prefix}' in worktree '{current_worktree}'."
                ),
                1 => Ok(matches.into_iter().next().unwrap()),
                _ => {
                    let now = chrono::Utc::now();
                    let lines: Vec<String> = matches
                        .iter()
                        .map(|inv| {
                            let ago = shorthand_from_seconds(
                                now.signed_duration_since(inv.created_at).num_seconds(),
                            );
                            let short =
                                &inv.invocation_id[..4.min(inv.invocation_id.len())];
                            format!("  {short}  {} -- {ago} ago", inv.trigger_command)
                        })
                        .collect();
                    anyhow::bail!(
                        "Ambiguous invocation prefix '{prefix}' -- matches:\n{}\n\
                         Use more characters to disambiguate.",
                        lines.join("\n")
                    );
                }
            }
        }
        RetryTarget::JobName(name) => {
            let invocations = store.list_invocations_for_worktree(current_worktree)?;
            for inv in invocations.iter().rev() {
                let job_dirs = store.list_jobs_in_invocation(&inv.invocation_id)?;
                for dir in &job_dirs {
                    if let Ok(meta) = store.read_meta(dir) {
                        if meta.name == *name
                            && matches!(meta.status, JobStatus::Failed | JobStatus::Cancelled)
                        {
                            return Ok(inv.clone());
                        }
                    }
                }
            }
            anyhow::bail!(
                "No failed job named '{name}' in worktree '{current_worktree}'."
            )
        }
    }
}
```

- [ ] **Step 2: Implement the full `retry_command`**

```rust
fn retry_command(
    target: Option<&str>,
    hook_flag: &Option<String>,
    inv_flag: &Option<String>,
    job_flag: &Option<String>,
    path: &Path,
    output: &mut dyn Output,
) -> Result<()> {
    let repo_hash = compute_repo_hash_from_path(path)?;
    let store = LogStore::for_repo(&repo_hash)?;
    let current_worktree = crate::core::repo::get_current_branch().unwrap_or_default();

    let flags = RetryFlags {
        hook: hook_flag.clone(),
        inv: inv_flag.clone(),
        job: job_flag.clone(),
    };
    let parsed = retry_target_from_arg(target, &flags);

    // For single-job form, tighten: must be Failed or Cancelled.
    let is_single_job = matches!(parsed, RetryTarget::JobName(_));

    // Edge case: if the user passed a composite address like
    // "feature/x:db-migrate", it will be classified as JobName.
    // Parse it through JobAddress to detect cross-worktree usage and
    // give a clear error. If the worktree matches current, extract
    // just the job name.
    let parsed = if let RetryTarget::JobName(ref name) = parsed {
        if name.contains(':') {
            let addr = JobAddress::parse(name);
            if let Some(ref wt) = addr.worktree {
                if wt != &current_worktree {
                    anyhow::bail!(
                        "Cross-worktree retry is not yet supported. \
                         Run this command from inside '{wt}', or see \
                         'daft help hooks jobs retry'."
                    );
                }
            }
            RetryTarget::JobName(addr.job_name)
        } else {
            parsed
        }
    } else {
        parsed
    };

    let source_inv = resolve_retry_invocation(&parsed, &store, &current_worktree)?;
    let short_id = &source_inv.invocation_id[..4.min(source_inv.invocation_id.len())];

    // Load all jobs in the source invocation.
    let job_dirs = store.list_jobs_in_invocation(&source_inv.invocation_id)?;
    let mut all_metas = Vec::new();
    for dir in &job_dirs {
        if let Ok(meta) = store.read_meta(dir) {
            all_metas.push(meta);
        }
    }

    // Check for running/pending jobs.
    if all_metas.iter().any(|m| matches!(m.status, JobStatus::Running)) {
        anyhow::bail!(
            "Invocation {short_id} still has running jobs. \
             Wait for it to finish, or cancel it."
        );
    }

    // For single-job: filter to just that job, validate state.
    let metas_for_retry = if is_single_job {
        if let RetryTarget::JobName(ref name) = parsed {
            let job_meta = all_metas
                .iter()
                .find(|m| m.name == *name)
                .ok_or_else(|| anyhow::anyhow!("No job named '{name}' in invocation {short_id}."))?;
            if !matches!(job_meta.status, JobStatus::Failed | JobStatus::Cancelled) {
                anyhow::bail!(
                    "Job '{}' in {} is not in a retryable state (status: {:?}). \
                     Use 'daft hooks run' to re-fire the full hook.",
                    name,
                    short_id,
                    job_meta.status
                );
            }
            vec![job_meta.clone()]
        } else {
            unreachable!()
        }
    } else {
        all_metas.clone()
    };

    let (retry_specs, retry_names) = build_retry_set(&metas_for_retry);

    if retry_specs.is_empty() {
        let ago = shorthand_from_seconds(
            chrono::Utc::now()
                .signed_duration_since(source_inv.created_at)
                .num_seconds(),
        );
        output.info(&format!(
            "No failed jobs in invocation {short_id} ({}, {ago} ago). Nothing to retry.",
            source_inv.trigger_command
        ));
        return Ok(());
    }

    // Validate working dirs exist.
    for spec in &retry_specs {
        if !spec.working_dir.exists() {
            anyhow::bail!(
                "Cannot retry job '{}': working directory '{}' no longer exists.",
                spec.name,
                spec.working_dir.display()
            );
        }
        if spec.command.is_empty() {
            anyhow::bail!(
                "Cannot retry job '{}': no command recorded in metadata.",
                spec.name
            );
        }
    }

    // Split into fg and bg sets.
    let (fg_specs, bg_specs): (Vec<_>, Vec<_>) =
        retry_specs.into_iter().partition(|s| !s.background);

    // Create the new invocation.
    let new_invocation_id = generate_invocation_id();
    let new_short_id = &new_invocation_id[..4.min(new_invocation_id.len())];

    let trigger_form = match &parsed {
        RetryTarget::LatestInvocation => "hooks jobs retry".to_string(),
        RetryTarget::HookType(h) => format!("hooks jobs retry {h}"),
        RetryTarget::InvocationPrefix(p) => format!("hooks jobs retry {p}"),
        RetryTarget::JobName(n) => format!("hooks jobs retry {n}"),
    };

    let new_inv_meta = InvocationMeta {
        invocation_id: new_invocation_id.clone(),
        trigger_command: trigger_form,
        hook_type: source_inv.hook_type.clone(),
        worktree: current_worktree.clone(),
        created_at: chrono::Utc::now(),
    };
    let retry_store = LogStore::for_repo(&repo_hash)?;
    retry_store.write_invocation_meta(&new_invocation_id, &new_inv_meta)?;

    // Run foreground phase.
    let mut fg_count = 0;
    if !fg_specs.is_empty() {
        let arc_store = std::sync::Arc::new(LogStore::for_repo(&repo_hash)?);
        let fg_sink: std::sync::Arc<dyn crate::executor::log_sink::LogSink> =
            std::sync::Arc::new(crate::executor::BufferingLogSink::new(
                arc_store,
                new_invocation_id.clone(),
                source_inv.hook_type.clone(),
                current_worktree.clone(),
            ));
        let presenter: std::sync::Arc<dyn crate::executor::presenter::JobPresenter> =
            std::sync::Arc::new(crate::executor::cli_presenter::CliPresenter::new());
        let exec_mode = if fg_specs.iter().any(|s| !s.needs.is_empty()) {
            crate::executor::ExecutionMode::Parallel
        } else {
            crate::executor::ExecutionMode::Parallel
        };
        let _fg_results =
            crate::executor::runner::run_jobs(&fg_specs, exec_mode, &presenter, Some(&fg_sink))?;
        fg_count = fg_specs.len();
    }

    // Run background phase.
    let mut bg_count = 0;
    #[cfg(unix)]
    if !bg_specs.is_empty() {
        let bg_store = LogStore::for_repo(&repo_hash)?;
        let mut coord_state =
            crate::coordinator::process::CoordinatorState::new(&repo_hash, &new_invocation_id)
                .with_metadata(
                    &new_inv_meta.trigger_command,
                    &source_inv.hook_type,
                    &current_worktree,
                );
        for spec in &bg_specs {
            coord_state.add_job(spec.clone());
        }
        bg_count = bg_specs.len();
        crate::coordinator::process::fork_coordinator(coord_state, bg_store)?;
    }

    #[cfg(not(unix))]
    if !bg_specs.is_empty() {
        anyhow::bail!("Background job retry is only supported on Unix systems.");
    }

    // Print summary.
    let total = fg_count + bg_count;
    let mut parts = Vec::new();
    if fg_count > 0 {
        parts.push(format!("{fg_count} foreground done"));
    }
    if bg_count > 0 {
        parts.push(format!("{bg_count} background running"));
    }
    output.success(&format!(
        "Retried {} job{} in invocation {new_short_id} ({}). Check status: daft hooks jobs",
        total,
        if total == 1 { "" } else { "s" },
        parts.join(", "),
    ));

    Ok(())
}
```

- [ ] **Step 3: Remove the old `retry_job` function**

Delete the old `retry_job` function (lines 820-888) and its
`#[allow(dead_code)]` attribute if present.

- [ ] **Step 4: Verify compilation**

Run: `mise run test:unit`

Expected: All pass.

- [ ] **Step 5: Run clippy and fmt**

Run: `mise run fmt && mise run clippy`

- [ ] **Step 6: Commit**

```bash
git add src/commands/hooks/jobs.rs
git commit -m "feat(jobs): implement bulk retry command with fg/bg split and DAG pruning"
```

---

### Task 9: Shell completion — `complete_retry_targets`

**Files:**

- Modify: `src/commands/complete.rs`

- [ ] **Step 1: Add the new completion dispatch arm**

In `src/commands/complete.rs`, add a new match arm after the existing
`("hooks-jobs-job", 1)` arm (line 121):

```rust
        // hooks jobs retry: complete retry targets (hooks, invocations, jobs with failures)
        ("hooks-jobs-retry", 1) => complete_retry_targets(word),
```

- [ ] **Step 2: Implement `complete_retry_targets`**

Add this function in `src/commands/complete.rs`:

```rust
fn complete_retry_targets(prefix: &str) -> Result<Vec<String>> {
    use crate::coordinator::log_store::{JobStatus, LogStore};
    use std::collections::hash_map::DefaultHasher;
    use std::hash::{Hash, Hasher};

    let repo_hash = find_project_root().ok().map(|root| {
        let mut hasher = DefaultHasher::new();
        root.display().to_string().hash(&mut hasher);
        format!("{:016x}", hasher.finish())
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
    let invocations = store
        .list_invocations_for_worktree(&current_worktree)
        .unwrap_or_default();

    let mut entries = Vec::new();

    // 1. Hook types with failures
    let mut hooks_with_failures: std::collections::HashMap<String, (usize, usize)> =
        std::collections::HashMap::new();
    for inv in &invocations {
        let job_dirs = store
            .list_jobs_in_invocation(&inv.invocation_id)
            .unwrap_or_default();
        let failed_count = job_dirs
            .iter()
            .filter_map(|d| store.read_meta(d).ok())
            .filter(|m| matches!(m.status, JobStatus::Failed | JobStatus::Cancelled))
            .count();
        if failed_count > 0 {
            let entry = hooks_with_failures
                .entry(inv.hook_type.clone())
                .or_insert((0, 0));
            entry.0 += failed_count;
            entry.1 += 1;
        }
    }
    for (hook, (failed, inv_count)) in &hooks_with_failures {
        if hook.starts_with(prefix) {
            entries.push(format!(
                "{hook}\thook -- {failed} failed across {inv_count} invocation{}",
                if *inv_count == 1 { "" } else { "s" },
            ));
        }
    }

    // 2. Invocation short IDs with failures
    for inv in &invocations {
        let short_id = &inv.invocation_id[..4.min(inv.invocation_id.len())];
        if !short_id.starts_with(prefix) {
            continue;
        }
        let job_dirs = store
            .list_jobs_in_invocation(&inv.invocation_id)
            .unwrap_or_default();
        let failed_count = job_dirs
            .iter()
            .filter_map(|d| store.read_meta(d).ok())
            .filter(|m| matches!(m.status, JobStatus::Failed | JobStatus::Cancelled))
            .count();
        if failed_count > 0 {
            let ago = crate::output::format::shorthand_from_seconds(
                now.signed_duration_since(inv.created_at).num_seconds(),
            );
            entries.push(format!(
                "{short_id}\tinvocation -- {}, {failed_count} failed, {ago} ago",
                inv.trigger_command,
            ));
        }
    }

    // 3. Job names from latest invocation with failures
    for inv in invocations.iter().rev() {
        let job_dirs = store
            .list_jobs_in_invocation(&inv.invocation_id)
            .unwrap_or_default();
        let failed_jobs: Vec<_> = job_dirs
            .iter()
            .filter_map(|d| store.read_meta(d).ok())
            .filter(|m| matches!(m.status, JobStatus::Failed | JobStatus::Cancelled))
            .collect();
        if !failed_jobs.is_empty() {
            let short_id = &inv.invocation_id[..4.min(inv.invocation_id.len())];
            let ago = crate::output::format::shorthand_from_seconds(
                now.signed_duration_since(inv.created_at).num_seconds(),
            );
            for meta in &failed_jobs {
                if meta.name.starts_with(prefix) {
                    entries.push(format!(
                        "{}\tjob -- failed in {short_id}, {ago} ago",
                        meta.name,
                    ));
                }
            }
            break; // Only show jobs from the most recent failing invocation
        }
    }

    Ok(entries)
}
```

- [ ] **Step 3: Verify compilation**

Run: `mise run test:unit`

Expected: All pass.

- [ ] **Step 4: Run clippy and fmt**

Run: `mise run fmt && mise run clippy`

- [ ] **Step 5: Commit**

```bash
git add src/commands/complete.rs
git commit -m "feat(jobs): add retry target completion (hooks, invocations, jobs)"
```

---

### Task 10: Wire completion scripts for retry

**Files:**

- Modify: `src/commands/completions/bash.rs`
- Modify: `src/commands/completions/zsh.rs`
- Modify: `src/commands/completions/fish.rs`
- Modify: `src/commands/completions/fig.rs`

- [ ] **Step 1: Update bash completions**

In `src/commands/completions/bash.rs`, find the `logs|retry|cancel)` case (line
231). We need to split `retry` into its own case with the new flags. Replace the
combined arm with separate arms:

Change:

```bash
                    logs|retry|cancel)
                        if [[ "$cur" == -* ]]; then
                            COMPREPLY=( $(compgen -W "--inv -h --help" -- "$cur") )
                            return 0
                        fi
                        local completions
                        completions=$(daft __complete hooks-jobs-job "$cur" 2>/dev/null)
```

To:

```bash
                    logs|cancel)
                        if [[ "$cur" == -* ]]; then
                            COMPREPLY=( $(compgen -W "--inv -h --help" -- "$cur") )
                            return 0
                        fi
                        local completions
                        completions=$(daft __complete hooks-jobs-job "$cur" 2>/dev/null)
```

And add a new `retry)` arm right after the `;;` that closes `logs|cancel)`:

```bash
                    retry)
                        if [[ "$cur" == -* ]]; then
                            COMPREPLY=( $(compgen -W "--hook --inv --job -h --help" -- "$cur") )
                            return 0
                        fi
                        local completions
                        completions=$(daft __complete hooks-jobs-retry "$cur" 2>/dev/null)
                        if [[ -n "$completions" ]]; then
                            while IFS=$'\n' read -r line; do
                                local val="${line%%	*}"
                                COMPREPLY+=("$val")
                            done <<< "$completions"
                        fi
                        return 0
                        ;;
```

- [ ] **Step 2: Update zsh completions**

In `src/commands/completions/zsh.rs`, find the `logs|retry|cancel)` case (line
319). Split similarly:

Change `logs|retry|cancel)` to `logs|cancel)`.

Add a new `retry)` case after the `;;` closing `logs|cancel)`:

```zsh
                    retry)
                        if [[ "$curword" == -* ]]; then
                            compadd -- --hook --inv --job -h --help
                            return
                        fi
                        local -a _vals _descs
                        while IFS='' read -r _line; do
                            _vals+=("${_line%%$'\t'*}")
                            _descs+=("${_line//$'\t'/  }")
                        done < <(daft __complete hooks-jobs-retry "$curword" 2>/dev/null)
                        compadd -l -d _descs -a _vals
                        return
                        ;;
```

- [ ] **Step 3: Update fish completions**

In `src/commands/completions/fish.rs`, find the line that wires `hooks-jobs-job`
for `logs cancel retry` (line 291). Split:

Replace line 291:

```fish
complete -c daft -n '__fish_seen_subcommand_from hooks; and __fish_seen_subcommand_from jobs; and __fish_seen_subcommand_from logs cancel retry' -f -a "(daft __complete hooks-jobs-job (commandline -ct) 2>/dev/null)"
```

With two lines:

```fish
complete -c daft -n '__fish_seen_subcommand_from hooks; and __fish_seen_subcommand_from jobs; and __fish_seen_subcommand_from logs cancel' -f -a "(daft __complete hooks-jobs-job (commandline -ct) 2>/dev/null)"
complete -c daft -n '__fish_seen_subcommand_from hooks; and __fish_seen_subcommand_from jobs; and __fish_seen_subcommand_from retry' -f -a "(daft __complete hooks-jobs-retry (commandline -ct) 2>/dev/null)"
```

Also update line 290 (the `--inv` flag line) to add the new flags for retry.
Replace:

```fish
complete -c daft -n '__fish_seen_subcommand_from hooks; and __fish_seen_subcommand_from jobs; and __fish_seen_subcommand_from logs cancel retry' -l inv -d 'Invocation ID prefix'
```

With:

```fish
complete -c daft -n '__fish_seen_subcommand_from hooks; and __fish_seen_subcommand_from jobs; and __fish_seen_subcommand_from logs cancel' -l inv -d 'Invocation ID prefix'
complete -c daft -n '__fish_seen_subcommand_from hooks; and __fish_seen_subcommand_from jobs; and __fish_seen_subcommand_from retry' -l hook -d 'Force hook name interpretation'
complete -c daft -n '__fish_seen_subcommand_from hooks; and __fish_seen_subcommand_from jobs; and __fish_seen_subcommand_from retry' -l inv -d 'Force invocation prefix interpretation'
complete -c daft -n '__fish_seen_subcommand_from hooks; and __fish_seen_subcommand_from jobs; and __fish_seen_subcommand_from retry' -l job -d 'Force job name interpretation'
```

- [ ] **Step 4: Update fig completions**

In `src/commands/completions/fig.rs`, find the
`fig_subcommand("retry", "Retry a failed job")` line (line 371). Change the
description:

```rust
            fig_subcommand("retry", "Re-run failed jobs from an invocation"),
```

- [ ] **Step 5: Verify compilation**

Run: `mise run test:unit`

Expected: All pass.

- [ ] **Step 6: Run clippy and fmt**

Run: `mise run fmt && mise run clippy`

- [ ] **Step 7: Commit**

```bash
git add src/commands/completions/bash.rs src/commands/completions/zsh.rs \
  src/commands/completions/fish.rs src/commands/completions/fig.rs \
  src/commands/complete.rs
git commit -m "feat(jobs): wire retry shell completions with hook/inv/job categories"
```

---

### Task 11: Integration scenario — `retry-empty.yml`

**Files:**

- Create: `tests/manual/scenarios/hooks/retry-empty.yml`

- [ ] **Step 1: Write the scenario**

```yaml
name: Empty retry retries all failed jobs from the most recent invocation
description:
  "Core bulk retry scenario. A hook with a guaranteed-fail job fires during
  worktree creation, then 'daft hooks jobs retry' with no arguments re-runs the
  failed job from the most recent invocation. Verifies: new invocation appears,
  retry fails again, fix the job, retry succeeds on third pass. Listing shows
  three chronological invocations."

repos:
  - name: test-retry-empty
    default_branch: main
    branches:
      - name: main
        files:
          - path: README.md
            content: "# Retry test"
          - path: fail-once.sh
            content: |
              #!/usr/bin/env bash
              if [ ! -f /tmp/daft-retry-test-pass ]; then
                echo "failing on purpose"
                exit 1
              fi
              echo "passing now"
              exit 0
        commits:
          - message: "Initial commit"
      - name: feature/retry
        from: main
    daft_yml: |
      hooks:
        worktree-post-create:
          jobs:
            - name: might-fail
              run: bash fail-once.sh
            - name: always-ok
              run: echo works

steps:
  - name: Clone the repository
    run: git-worktree-clone --layout contained $REMOTE_TEST_RETRY_EMPTY
    expect:
      exit_code: 0

  - name: Trust the repository
    run: daft hooks trust --force 2>&1
    cwd: "$WORK_DIR/test-retry-empty/main"
    expect:
      exit_code: 0

  - name: Checkout feature branch (might-fail will fail)
    run: env -u DAFT_TESTING git-worktree-checkout feature/retry 2>&1
    cwd: "$WORK_DIR/test-retry-empty/main"
    expect:
      exit_code: 0
      output_contains:
        - "failing on purpose"

  - name: List jobs — should show one failed, one completed
    run: daft hooks jobs 2>&1
    cwd: "$WORK_DIR/test-retry-empty/feature/retry"
    expect:
      exit_code: 0
      output_contains:
        - "might-fail"
        - "failed"
        - "always-ok"
        - "completed"

  - name: Retry with no args — retries the failed job, fails again
    run: daft hooks jobs retry 2>&1
    cwd: "$WORK_DIR/test-retry-empty/feature/retry"
    expect:
      exit_code: 0
      output_contains:
        - "Retried"
        - "hooks jobs retry"

  - name: List jobs after first retry — should show two invocations
    run: daft hooks jobs 2>&1
    cwd: "$WORK_DIR/test-retry-empty/feature/retry"
    expect:
      exit_code: 0
      output_contains:
        - "worktree-post-create"
        - "hooks jobs retry"
        - "might-fail"

  - name: Fix the job by creating the sentinel file
    run: touch /tmp/daft-retry-test-pass
    expect:
      exit_code: 0

  - name: Retry again — should pass now
    run: daft hooks jobs retry 2>&1
    cwd: "$WORK_DIR/test-retry-empty/feature/retry"
    expect:
      exit_code: 0
      output_contains:
        - "Retried"

  - name: Clean up sentinel
    run: rm -f /tmp/daft-retry-test-pass
    expect:
      exit_code: 0
```

- [ ] **Step 2: Run the scenario**

Run: `mise run test:manual -- --ci retry-empty`

Expected: All steps pass.

- [ ] **Step 3: Commit**

```bash
git add tests/manual/scenarios/hooks/retry-empty.yml
git commit -m "test(hooks): add retry-empty integration scenario"
```

---

### Task 12: Integration scenario — `retry-job-name.yml`

**Files:**

- Create: `tests/manual/scenarios/hooks/retry-job-name.yml`

- [ ] **Step 1: Write the scenario**

```yaml
name: Single-job retry works and rejects non-failed jobs
description:
  "Verifies that 'retry <job-name>' retries exactly one job, and that attempting
  to retry a completed or skipped job produces an error."

repos:
  - name: test-retry-job
    default_branch: main
    branches:
      - name: main
        files:
          - path: README.md
            content: "# Single-job retry test"
        commits:
          - message: "Initial commit"
      - name: feature/rj
        from: main
    daft_yml: |
      hooks:
        worktree-post-create:
          jobs:
            - name: fails-here
              run: "exit 1"
            - name: works-fine
              run: echo ok
            - name: skipped-job
              run: echo never
              skip: true

steps:
  - name: Clone the repository
    run: git-worktree-clone --layout contained $REMOTE_TEST_RETRY_JOB
    expect:
      exit_code: 0

  - name: Trust the repository
    run: daft hooks trust --force 2>&1
    cwd: "$WORK_DIR/test-retry-job/main"
    expect:
      exit_code: 0

  - name: Checkout feature branch
    run: env -u DAFT_TESTING git-worktree-checkout feature/rj 2>&1
    cwd: "$WORK_DIR/test-retry-job/main"
    expect:
      exit_code: 0

  - name: Retry the failed job by name
    run: daft hooks jobs retry fails-here 2>&1
    cwd: "$WORK_DIR/test-retry-job/feature/rj"
    expect:
      exit_code: 0
      output_contains:
        - "Retried 1 job"

  - name: Retry a completed job — should fail
    run: daft hooks jobs retry works-fine 2>&1
    cwd: "$WORK_DIR/test-retry-job/feature/rj"
    expect:
      exit_code: 1
      output_contains:
        - "not in a retryable state"

  - name: Retry a skipped job — should fail (not found as failed)
    run: daft hooks jobs retry skipped-job 2>&1
    cwd: "$WORK_DIR/test-retry-job/feature/rj"
    expect:
      exit_code: 1
      output_contains:
        - "No failed job named"
```

- [ ] **Step 2: Run the scenario**

Run: `mise run test:manual -- --ci retry-job-name`

Expected: All steps pass.

- [ ] **Step 3: Commit**

```bash
git add tests/manual/scenarios/hooks/retry-job-name.yml
git commit -m "test(hooks): add retry-job-name integration scenario"
```

---

### Task 13: Integration scenario — `retry-hook-name.yml`

**Files:**

- Create: `tests/manual/scenarios/hooks/retry-hook-name.yml`

- [ ] **Step 1: Write the scenario**

```yaml
name: Retry by hook name targets the most recent invocation of that hook type
description:
  "Verifies that 'retry worktree-post-create' selects the most recent invocation
  of that hook type, including manual 'hooks run' invocations."

repos:
  - name: test-retry-hook
    default_branch: main
    branches:
      - name: main
        files:
          - path: README.md
            content: "# Hook-name retry test"
        commits:
          - message: "Initial commit"
      - name: feature/rh
        from: main
    daft_yml: |
      hooks:
        worktree-post-create:
          jobs:
            - name: hook-job
              run: "exit 1"

steps:
  - name: Clone the repository
    run: git-worktree-clone --layout contained $REMOTE_TEST_RETRY_HOOK
    expect:
      exit_code: 0

  - name: Trust the repository
    run: daft hooks trust --force 2>&1
    cwd: "$WORK_DIR/test-retry-hook/main"
    expect:
      exit_code: 0

  - name: Checkout feature branch (automatic hook fires, job fails)
    run: env -u DAFT_TESTING git-worktree-checkout feature/rh 2>&1
    cwd: "$WORK_DIR/test-retry-hook/main"
    expect:
      exit_code: 0

  - name: List jobs — should show one invocation with a failed job
    run: daft hooks jobs 2>&1
    cwd: "$WORK_DIR/test-retry-hook/feature/rh"
    expect:
      exit_code: 0
      output_contains:
        - "hook-job"
        - "failed"

  - name: Retry by hook name
    run: daft hooks jobs retry worktree-post-create 2>&1
    cwd: "$WORK_DIR/test-retry-hook/feature/rh"
    expect:
      exit_code: 0
      output_contains:
        - "Retried"

  - name: List jobs — should show original + retry invocation
    run: daft hooks jobs 2>&1
    cwd: "$WORK_DIR/test-retry-hook/feature/rh"
    expect:
      exit_code: 0
      output_contains:
        - "worktree-post-create"
        - "hooks jobs retry worktree-post-create"
```

- [ ] **Step 2: Run the scenario**

Run: `mise run test:manual -- --ci retry-hook-name`

Expected: All steps pass.

- [ ] **Step 3: Commit**

```bash
git add tests/manual/scenarios/hooks/retry-hook-name.yml
git commit -m "test(hooks): add retry-hook-name integration scenario"
```

---

### Task 14: Integration scenario — `retry-needs-pruning.yml`

**Files:**

- Create: `tests/manual/scenarios/hooks/retry-needs-pruning.yml`

- [ ] **Step 1: Write the scenario**

```yaml
name: Retry preserves DAG needs within the retry set and prunes outside edges
description:
  "Verifies DAG-aware retry. Hook declares a->b->c chain. First run: a succeeds,
  b fails, c is cancelled. Retry picks up b and c, prunes b's dep on a,
  preserves c's dep on b. After fixing b, c should run."

repos:
  - name: test-retry-dag
    default_branch: main
    branches:
      - name: main
        files:
          - path: README.md
            content: "# DAG retry test"
          - path: maybe-fail.sh
            content: |
              #!/usr/bin/env bash
              if [ ! -f /tmp/daft-dag-test-pass ]; then
                echo "b fails"
                exit 1
              fi
              echo "b passes"
        commits:
          - message: "Initial commit"
      - name: feature/dag
        from: main
    daft_yml: |
      hooks:
        worktree-post-create:
          jobs:
            - name: step-a
              run: echo a-ok
            - name: step-b
              run: bash maybe-fail.sh
              needs: [step-a]
            - name: step-c
              run: echo c-runs
              needs: [step-b]

steps:
  - name: Clone the repository
    run: git-worktree-clone --layout contained $REMOTE_TEST_RETRY_DAG
    expect:
      exit_code: 0

  - name: Trust the repository
    run: daft hooks trust --force 2>&1
    cwd: "$WORK_DIR/test-retry-dag/main"
    expect:
      exit_code: 0

  - name:
      Checkout feature branch (a passes, b fails, c should be skipped/cancelled)
    run: env -u DAFT_TESTING git-worktree-checkout feature/dag 2>&1
    cwd: "$WORK_DIR/test-retry-dag/main"
    expect:
      exit_code: 0

  - name: List jobs — a completed, b failed, c skipped
    run: daft hooks jobs 2>&1
    cwd: "$WORK_DIR/test-retry-dag/feature/dag"
    expect:
      exit_code: 0
      output_contains:
        - "step-a"
        - "completed"
        - "step-b"
        - "failed"

  - name: Retry — should retry b and c but not a
    run: daft hooks jobs retry 2>&1
    cwd: "$WORK_DIR/test-retry-dag/feature/dag"
    expect:
      exit_code: 0
      output_contains:
        - "Retried"
      output_not_contains:
        - "step-a"

  - name: Fix step-b
    run: touch /tmp/daft-dag-test-pass
    expect:
      exit_code: 0

  - name: Retry again — b passes, c should run
    run: daft hooks jobs retry 2>&1
    cwd: "$WORK_DIR/test-retry-dag/feature/dag"
    expect:
      exit_code: 0
      output_contains:
        - "Retried"

  - name: Clean up sentinel
    run: rm -f /tmp/daft-dag-test-pass
    expect:
      exit_code: 0
```

- [ ] **Step 2: Run the scenario**

Run: `mise run test:manual -- --ci retry-needs-pruning`

Expected: All steps pass.

- [ ] **Step 3: Commit**

```bash
git add tests/manual/scenarios/hooks/retry-needs-pruning.yml
git commit -m "test(hooks): add retry-needs-pruning integration scenario (DAG)"
```

---

### Task 15: Integration scenario — `retry-invocation-prefix.yml`

**Files:**

- Create: `tests/manual/scenarios/hooks/retry-invocation-prefix.yml`

- [ ] **Step 1: Write the scenario**

```yaml
name: Retry by invocation prefix targets a specific invocation
description:
  "Verifies that 'retry <prefix>' works and that ambiguous prefixes produce a
  helpful error."

repos:
  - name: test-retry-inv
    default_branch: main
    branches:
      - name: main
        files:
          - path: README.md
            content: "# Invocation prefix retry test"
        commits:
          - message: "Initial commit"
      - name: feature/inv
        from: main
    daft_yml: |
      hooks:
        worktree-post-create:
          jobs:
            - name: inv-job
              run: "exit 1"

steps:
  - name: Clone the repository
    run: git-worktree-clone --layout contained $REMOTE_TEST_RETRY_INV
    expect:
      exit_code: 0

  - name: Trust the repository
    run: daft hooks trust --force 2>&1
    cwd: "$WORK_DIR/test-retry-inv/main"
    expect:
      exit_code: 0

  - name: Checkout feature branch (job fails)
    run: env -u DAFT_TESTING git-worktree-checkout feature/inv 2>&1
    cwd: "$WORK_DIR/test-retry-inv/main"
    expect:
      exit_code: 0

  - name: Get the invocation short ID from JSON output
    run: daft hooks jobs --json 2>&1
    cwd: "$WORK_DIR/test-retry-inv/feature/inv"
    expect:
      exit_code: 0
      output_contains:
        - "short_id"
      capture:
        SHORT_ID:
          json_path: "$.worktrees[0].invocations[0].short_id"

  - name: Retry by the captured invocation prefix
    run: daft hooks jobs retry $SHORT_ID 2>&1
    cwd: "$WORK_DIR/test-retry-inv/feature/inv"
    expect:
      exit_code: 0
      output_contains:
        - "Retried"
```

- [ ] **Step 2: Run the scenario**

Run: `mise run test:manual -- --ci retry-invocation-prefix`

Expected: All steps pass. Note: this scenario depends on the test harness
supporting JSON-path capture from `daft hooks jobs --json` output. If the
harness doesn't support `capture` with `json_path`, this step may need to be
restructured to use grep/sed in a shell step. Check the test harness
documentation in `tests/README.md` before implementing.

- [ ] **Step 3: Commit**

```bash
git add tests/manual/scenarios/hooks/retry-invocation-prefix.yml
git commit -m "test(hooks): add retry-invocation-prefix integration scenario"
```

---

### Task 16: Integration scenario — `retry-address-two-segment.yml`

**Files:**

- Create: `tests/manual/scenarios/hooks/retry-address-two-segment.yml`

- [ ] **Step 1: Write the scenario**

```yaml
name:
  Two-segment worktree:job address works for logs, errors for cross-worktree
  retry
description:
  "Verifies the B-1 fold-in fix: 'daft hooks jobs logs worktree:job' resolves
  the worktree:job two-segment form. Also verifies that retry with a
  cross-worktree address is rejected until sub-project C."

repos:
  - name: test-retry-addr
    default_branch: main
    branches:
      - name: main
        files:
          - path: README.md
            content: "# Address test"
        commits:
          - message: "Initial commit"
      - name: feature/addr
        from: main
    daft_yml: |
      hooks:
        worktree-post-create:
          jobs:
            - name: addr-job
              run: echo address-output

steps:
  - name: Clone the repository
    run: git-worktree-clone --layout contained $REMOTE_TEST_RETRY_ADDR
    expect:
      exit_code: 0

  - name: Trust the repository
    run: daft hooks trust --force 2>&1
    cwd: "$WORK_DIR/test-retry-addr/main"
    expect:
      exit_code: 0

  - name: Checkout feature branch
    run: env -u DAFT_TESTING git-worktree-checkout feature/addr 2>&1
    cwd: "$WORK_DIR/test-retry-addr/main"
    expect:
      exit_code: 0

  - name: View log via two-segment worktree:job from inside the worktree
    run: daft hooks jobs logs feature/addr:addr-job 2>&1
    cwd: "$WORK_DIR/test-retry-addr/feature/addr"
    expect:
      exit_code: 0
      output_contains:
        - "addr-job"
        - "address-output"

  - name: View log via two-segment worktree:job from the main worktree
    run: daft hooks jobs logs feature/addr:addr-job 2>&1
    cwd: "$WORK_DIR/test-retry-addr/main"
    expect:
      exit_code: 0
      output_contains:
        - "addr-job"
        - "address-output"
```

- [ ] **Step 2: Run the scenario**

Run: `mise run test:manual -- --ci retry-address-two-segment`

Expected: All steps pass.

- [ ] **Step 3: Commit**

```bash
git add tests/manual/scenarios/hooks/retry-address-two-segment.yml
git commit -m "test(hooks): add two-segment address integration scenario (B-1)"
```

---

### Task 17: Integration scenario — `retry-mixed-fg-bg.yml`

**Files:**

- Create: `tests/manual/scenarios/hooks/retry-mixed-fg-bg.yml`

- [ ] **Step 1: Write the scenario**

```yaml
name: Retry re-dispatches fg jobs inline and bg jobs to coordinator
description:
  "Verifies the fg-first/bg-second execution model. A hook with one fg job
  (fails) and one bg job (fails) and one fg job (passes). Retry picks up only
  the two failures. The fg failure runs inline, the bg failure dispatches to the
  coordinator."

repos:
  - name: test-retry-mixed
    default_branch: main
    branches:
      - name: main
        files:
          - path: README.md
            content: "# Mixed retry test"
        commits:
          - message: "Initial commit"
      - name: feature/mix
        from: main
    daft_yml: |
      hooks:
        worktree-post-create:
          jobs:
            - name: fg-ok
              run: echo fg-ok-output
            - name: fg-fail
              run: "exit 1"
            - name: bg-fail
              run: "exit 1"
              background: true

steps:
  - name: Clone the repository
    run: git-worktree-clone --layout contained $REMOTE_TEST_RETRY_MIXED
    expect:
      exit_code: 0

  - name: Trust the repository
    run: daft hooks trust --force 2>&1
    cwd: "$WORK_DIR/test-retry-mixed/main"
    expect:
      exit_code: 0

  - name: Checkout feature branch (fg-fail and bg-fail will fail)
    run: env -u DAFT_TESTING git-worktree-checkout feature/mix 2>&1
    cwd: "$WORK_DIR/test-retry-mixed/main"
    expect:
      exit_code: 0

  - name: Wait for bg job to finish
    run: |
      for i in $(seq 1 20); do
        if daft hooks jobs --json 2>/dev/null | grep -q '"failed"'; then
          break
        fi
        sleep 0.5
      done
      daft hooks jobs 2>&1
    cwd: "$WORK_DIR/test-retry-mixed/feature/mix"
    expect:
      exit_code: 0
      output_contains:
        - "fg-ok"
        - "completed"
        - "fg-fail"
        - "failed"
        - "bg-fail"

  - name: Retry — should re-dispatch fg-fail inline and bg-fail to coordinator
    run: daft hooks jobs retry 2>&1
    cwd: "$WORK_DIR/test-retry-mixed/feature/mix"
    expect:
      exit_code: 0
      output_contains:
        - "Retried"
        - "foreground"
      output_not_contains:
        - "fg-ok"
```

- [ ] **Step 2: Run the scenario**

Run: `mise run test:manual -- --ci retry-mixed-fg-bg`

Expected: All steps pass.

- [ ] **Step 3: Commit**

```bash
git add tests/manual/scenarios/hooks/retry-mixed-fg-bg.yml
git commit -m "test(hooks): add retry-mixed-fg-bg integration scenario"
```

---

### Task 18: Final verification and cleanup

**Files:**

- All files modified in Tasks 1-17

- [ ] **Step 1: Run the full test suite**

Run: `mise run test:unit`

Expected: All unit tests pass.

- [ ] **Step 2: Run clippy**

Run: `mise run clippy`

Expected: Zero warnings.

- [ ] **Step 3: Run fmt**

Run: `mise run fmt:check`

Expected: All files formatted.

- [ ] **Step 4: Run all integration scenarios**

Run: `mise run test:manual -- --ci`

Expected: All scenarios pass, including the 7 existing hooks scenarios and the 7
new retry scenarios.

- [ ] **Step 5: Remove any dead code**

Check for:

- The old `retry_job` function — should have been removed in Task 8.
- Any `#[allow(dead_code)]` annotations added temporarily.
- Any unused imports.

- [ ] **Step 6: Regenerate man pages**

Run: `mise run man:gen`

The `retry` subcommand's help text changed (new flags `--hook`, `--inv`,
`--job`; updated description). The man page must reflect this.

- [ ] **Step 7: Commit if any changes from cleanup**

```bash
git add -A
git commit -m "chore(jobs): cleanup dead code and regenerate man pages after bulk retry"
```
