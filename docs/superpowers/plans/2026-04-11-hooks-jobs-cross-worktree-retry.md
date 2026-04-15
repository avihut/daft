# Hooks Jobs Cross-Worktree Retry — Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use
> superpowers:subagent-driven-development (recommended) or
> superpowers:executing-plans to implement this plan task-by-task. Steps use
> checkbox (`- [ ]`) syntax for tracking.

**Goal:** Enable retrying failed jobs from any worktree (including deleted
ones), add listing filters (`--worktree`, `--status`, `--hook`), and fold in all
remaining cleanup items from sub-projects A and B.

**Architecture:** Extend `retry_command` with `--worktree` and `--cwd` flags,
lift B's cross-worktree guard, add filter flags to `list_jobs`, build log-store
completion helpers following the rich completion format, and clean up 5 deferred
items. Each change is independently testable and committable.

**Tech Stack:** Rust, clap, serde, chrono, anyhow

---

## File Structure

| File                                              | Responsibility                                                                                        |
| ------------------------------------------------- | ----------------------------------------------------------------------------------------------------- |
| `src/coordinator/log_store.rs`                    | C-3 fix, `JobMeta::skipped` constructor (B-3), `list_distinct_worktrees`                              |
| `src/executor/log_sink.rs`                        | Use `JobMeta::skipped` constructor (B-3)                                                              |
| `src/executor/runner.rs`                          | LogSink use-path cleanup (C-4)                                                                        |
| `src/hooks/yaml_executor/mod.rs`                  | Non-Unix sink fix (C-2), use `JobMeta::skipped` (B-3)                                                 |
| `src/commands/hooks/jobs.rs`                      | `--worktree`, `--cwd` on Retry; `--worktree`/`--status`/`--hook` on listing; lift guard; filter logic |
| `src/commands/complete.rs`                        | `complete_retry_worktrees`, `complete_listing_worktrees`, `complete_hook_types`                       |
| `src/commands/completions/{bash,zsh,fish,fig}.rs` | Wire new flags and dispatch arms                                                                      |
| `tests/manual/scenarios/hooks/*.yml`              | 4 new integration scenarios                                                                           |

---

### Task 1: C-3 — Add `with_context` to `write_invocation_meta`

**Files:**

- Modify: `src/coordinator/log_store.rs:159`

- [ ] **Step 1: Fix the bare `fs::write` call**

In `src/coordinator/log_store.rs`, find the `write_invocation_meta` method (line
153). Change the bare `fs::write`:

```rust
        fs::write(&path, content)?;
```

To:

```rust
        fs::write(&path, content)
            .with_context(|| format!("Failed to write invocation meta: {}", path.display()))?;
```

- [ ] **Step 2: Run tests and clippy**

Run: `mise run test:unit && mise run clippy`

Expected: All pass, zero warnings.

- [ ] **Step 3: Commit**

```bash
git add src/coordinator/log_store.rs
git commit -m "fix(log_store): add with_context to write_invocation_meta fs::write"
```

---

### Task 2: C-4 — Clean up fully-qualified LogSink path in runner.rs

**Files:**

- Modify: `src/executor/runner.rs`

- [ ] **Step 1: Add the use import**

At the top of `src/executor/runner.rs`, find the existing imports. Add:

```rust
use super::log_sink::LogSink;
```

- [ ] **Step 2: Replace all fully-qualified occurrences**

Search for `crate::executor::log_sink::LogSink` in the file and replace every
occurrence with just `LogSink`. There are ~13 occurrences across function
signatures and parameter types. Each one looks like:

```rust
sink: Option<&Arc<dyn crate::executor::log_sink::LogSink>>,
```

Change to:

```rust
sink: Option<&Arc<dyn LogSink>>,
```

- [ ] **Step 3: Run tests and clippy**

Run: `mise run test:unit && mise run clippy`

Expected: All pass.

- [ ] **Step 4: Commit**

```bash
git add src/executor/runner.rs
git commit -m "refactor(runner): use LogSink import instead of fully-qualified path"
```

---

### Task 3: C-2 — Fix non-Unix bg fallback to pass sink

**Files:**

- Modify: `src/hooks/yaml_executor/mod.rs:359-365`

- [ ] **Step 1: Fix the `None` sink in the non-Unix fallback**

In `src/hooks/yaml_executor/mod.rs`, find the `#[cfg(not(unix))]` block (around
line 359). Change:

```rust
    #[cfg(not(unix))]
    {
        let bg_results = crate::executor::runner::run_jobs(&bg_specs, exec_mode, presenter, None)?;
```

To:

```rust
    #[cfg(not(unix))]
    {
        let bg_results = crate::executor::runner::run_jobs(&bg_specs, exec_mode, presenter, Some(&fg_sink))?;
```

The `fg_sink` variable is already in scope (created at line 296-302).

- [ ] **Step 2: Run tests and clippy**

Run: `mise run test:unit && mise run clippy`

Expected: All pass.

- [ ] **Step 3: Commit**

```bash
git add src/hooks/yaml_executor/mod.rs
git commit -m "fix(yaml_executor): pass sink to run_jobs in non-Unix bg fallback"
```

---

### Task 4: B-3 — Extract `JobMeta::skipped` constructor

**Files:**

- Modify: `src/coordinator/log_store.rs` (add constructor)
- Modify: `src/executor/log_sink.rs` (use constructor)
- Modify: `src/hooks/yaml_executor/mod.rs` (use constructor)
- Test: `src/coordinator/log_store.rs`

- [ ] **Step 1: Write the failing test**

Add in the test module of `src/coordinator/log_store.rs`:

```rust
#[test]
fn job_meta_skipped_constructor_matches_inline() {
    let inline = JobMeta {
        name: "test-job".to_string(),
        hook_type: "worktree-post-create".to_string(),
        worktree: "feature/x".to_string(),
        command: "echo test".to_string(),
        working_dir: String::new(),
        env: HashMap::new(),
        started_at: chrono::Utc::now(),
        status: JobStatus::Skipped,
        exit_code: None,
        pid: None,
        background: false,
        finished_at: None,
        needs: vec!["dep".to_string()],
    };
    let constructed = JobMeta::skipped(
        "test-job",
        "worktree-post-create",
        "feature/x",
        "echo test",
        false,
        vec!["dep".to_string()],
    );
    assert_eq!(inline.name, constructed.name);
    assert_eq!(inline.hook_type, constructed.hook_type);
    assert_eq!(inline.worktree, constructed.worktree);
    assert_eq!(inline.command, constructed.command);
    assert_eq!(inline.status, constructed.status);
    assert_eq!(inline.needs, constructed.needs);
    assert_eq!(inline.background, constructed.background);
    assert!(constructed.exit_code.is_none());
    assert!(constructed.pid.is_none());
    assert!(constructed.finished_at.is_none());
}
```

- [ ] **Step 2: Run test to verify it fails**

Run: `mise run test:unit -- --test job_meta_skipped_constructor`

Expected: Compile error — no method `skipped` on `JobMeta`.

- [ ] **Step 3: Add the constructor to `JobMeta`**

In `src/coordinator/log_store.rs`, add an `impl JobMeta` block after the struct
definition (after line 33):

```rust
impl JobMeta {
    /// Construct a sparse `JobMeta` for a skipped job.
    ///
    /// Used by both `BufferingLogSink::on_job_runner_skipped` and
    /// `yaml_executor` skipped-job recording to avoid drift.
    pub fn skipped(
        name: &str,
        hook_type: &str,
        worktree: &str,
        command: &str,
        background: bool,
        needs: Vec<String>,
    ) -> Self {
        Self {
            name: name.to_string(),
            hook_type: hook_type.to_string(),
            worktree: worktree.to_string(),
            command: command.to_string(),
            working_dir: String::new(),
            env: HashMap::new(),
            started_at: chrono::Utc::now(),
            status: JobStatus::Skipped,
            exit_code: None,
            pid: None,
            background,
            finished_at: None,
            needs,
        }
    }
}
```

- [ ] **Step 4: Use the constructor in
      `BufferingLogSink::on_job_runner_skipped`**

In `src/executor/log_sink.rs`, replace the inline `JobMeta` construction in
`on_job_runner_skipped` (lines 141-155) with:

```rust
    fn on_job_runner_skipped(&self, spec: &JobSpec, reason: &str) {
        {
            let mut buffers = self.buffers.lock().unwrap();
            buffers.remove(&spec.name);
        }

        let meta = JobMeta::skipped(
            &spec.name,
            &self.hook_type,
            &self.worktree,
            &spec.command,
            false,
            spec.needs.clone(),
        );

        if let Err(e) = self
            .store
            .write_job_record(&self.invocation_id, &meta, reason.as_bytes())
        {
            eprintln!("daft: failed to write job record for '{}': {e}", spec.name);
        }
    }
```

- [ ] **Step 5: Use the constructor in `yaml_executor` skipped-job recording**

In `src/hooks/yaml_executor/mod.rs`, find the skipped-job `JobMeta` construction
(around line 251-265). Replace with:

```rust
        let meta = crate::coordinator::log_store::JobMeta::skipped(
            &sj.name,
            hook_name,
            &ctx.branch_name,
            "",
            sj.background,
            vec![],
        );
```

- [ ] **Step 6: Run tests**

Run: `mise run test:unit`

Expected: All pass.

- [ ] **Step 7: Run clippy and fmt**

Run: `mise run fmt && mise run clippy`

- [ ] **Step 8: Commit**

```bash
git add src/coordinator/log_store.rs src/executor/log_sink.rs \
  src/hooks/yaml_executor/mod.rs
git commit -m "refactor(jobs): extract JobMeta::skipped constructor (B-3)"
```

---

### Task 5: Add `list_distinct_worktrees` to LogStore

**Files:**

- Modify: `src/coordinator/log_store.rs`
- Test: `src/coordinator/log_store.rs`

- [ ] **Step 1: Write the failing test**

Add in the test module of `src/coordinator/log_store.rs`:

```rust
#[test]
fn list_distinct_worktrees_returns_unique_names() {
    let tmp = TempDir::new().unwrap();
    let store = LogStore::new(tmp.path().to_path_buf());

    let now = chrono::Utc::now();
    for (inv_id, wt, offset) in &[
        ("inv1", "feature/a", 100i64),
        ("inv2", "feature/b", 50),
        ("inv3", "feature/a", 10),
        ("inv4", "feature/c", 5),
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

    let worktrees = store.list_distinct_worktrees().unwrap();
    assert_eq!(worktrees.len(), 3);
    assert!(worktrees.contains(&"feature/a".to_string()));
    assert!(worktrees.contains(&"feature/b".to_string()));
    assert!(worktrees.contains(&"feature/c".to_string()));
}
```

- [ ] **Step 2: Implement the method**

Add to the `impl LogStore` block in `src/coordinator/log_store.rs`:

```rust
pub fn list_distinct_worktrees(&self) -> Result<Vec<String>> {
    let invocations = self.list_invocations()?;
    let mut seen = std::collections::BTreeSet::new();
    for inv in &invocations {
        seen.insert(inv.worktree.clone());
    }
    Ok(seen.into_iter().collect())
}
```

- [ ] **Step 3: Run tests**

Run: `mise run test:unit -- --test list_distinct_worktrees`

Expected: PASS.

- [ ] **Step 4: Commit**

```bash
git add src/coordinator/log_store.rs
git commit -m "feat(log_store): add list_distinct_worktrees method"
```

---

### Task 6: Add `--worktree` and `--cwd` flags to Retry, lift cross-worktree guard

**Files:**

- Modify: `src/commands/hooks/jobs.rs`

- [ ] **Step 1: Add flags to the `Retry` variant**

In `src/commands/hooks/jobs.rs`, find the `Retry` variant in `JobsCommand`
(around line 102). Add `--worktree` and `--cwd` fields:

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
        /// Retry jobs from a specific worktree (can be deleted).
        #[arg(long)]
        worktree: Option<String>,
        /// Override working directory for all retried jobs.
        #[arg(long)]
        cwd: Option<String>,
    },
```

- [ ] **Step 2: Update the `run()` dispatch**

Update the `Retry` arm in the `run()` function to pass the new fields:

```rust
        Some(JobsCommand::Retry {
            ref target,
            ref hook,
            ref inv_flag,
            ref job_flag,
            ref worktree,
            ref cwd,
        }) => retry_command(
            target.as_deref(),
            hook,
            inv_flag,
            job_flag,
            worktree.as_deref(),
            cwd.as_deref(),
            path,
            output,
        ),
```

- [ ] **Step 3: Update `retry_command` signature and body**

Change the function signature to accept the new parameters:

```rust
fn retry_command(
    target: Option<&str>,
    hook_flag: &Option<String>,
    inv_flag: &Option<String>,
    job_flag: &Option<String>,
    worktree_flag: Option<&str>,
    cwd_flag: Option<&str>,
    path: &Path,
    output: &mut dyn Output,
) -> Result<()> {
```

Inside the function body, make these changes:

**a)** After computing `current_worktree`, determine the effective worktree:

```rust
    let current_worktree = crate::core::repo::get_current_branch().unwrap_or_default();
    let effective_worktree = worktree_flag.unwrap_or(&current_worktree);
```

**b)** Replace the cross-worktree guard block (lines 1013-1023). Currently it
bails on cross-worktree. Change it to: detect conflict between `--worktree` and
composite address, then use the composite worktree if no `--worktree` flag:

```rust
    let effective_worktree = if let RetryTarget::JobName(ref name) = parsed {
        if name.contains(':') {
            let addr = JobAddress::parse(name);
            if let Some(ref wt) = addr.worktree {
                if let Some(flag_wt) = worktree_flag {
                    if flag_wt != wt.as_str() {
                        anyhow::bail!(
                            "Conflicting worktree: --worktree says '{}' but address says '{}'.",
                            flag_wt,
                            wt
                        );
                    }
                }
                parsed = RetryTarget::JobName(addr.job_name.clone());
                wt.as_str()
            } else {
                effective_worktree
            }
        } else {
            effective_worktree
        }
    } else {
        effective_worktree
    };
```

Note: `effective_worktree` needs to be a `String` owned value to handle the
lifetime from `addr.worktree`. Adjust the types accordingly — use
`let mut effective_worktree = worktree_flag.unwrap_or(&current_worktree).to_string();`
and reassign in the composite address branch.

**c)** Replace all uses of `current_worktree` in the resolution path with
`effective_worktree` — specifically in the call to `resolve_retry_invocation`.

**d)** Update the working_dir validation to support `--cwd`:

```rust
    // Validate --cwd if provided.
    if let Some(cwd) = cwd_flag {
        let cwd_path = std::path::Path::new(cwd);
        if !cwd_path.exists() || !cwd_path.is_dir() {
            anyhow::bail!("--cwd path '{}' does not exist or is not a directory.", cwd);
        }
    }

    for spec in &retry_specs {
        if !spec.working_dir.exists() && cwd_flag.is_none() {
            anyhow::bail!(
                "Cannot retry job '{}': working directory '{}' no longer exists. \
                 Use --cwd to specify an alternative.",
                spec.name,
                spec.working_dir.display()
            );
        }
    }
```

**e)** Before execution, apply `--cwd` override to all specs:

```rust
    if let Some(cwd) = cwd_flag {
        let cwd_path = std::path::PathBuf::from(cwd);
        for spec in &mut fg_specs {
            spec.working_dir = cwd_path.clone();
        }
        for spec in &mut bg_specs {
            spec.working_dir = cwd_path.clone();
        }
    }
```

Note: the `partition` that splits fg/bg needs to produce mutable vecs. Change:

```rust
let (mut fg_specs, mut bg_specs): (Vec<_>, Vec<_>) =
    retry_specs.into_iter().partition(|s| !s.background);
```

- [ ] **Step 4: Run tests and clippy**

Run: `mise run test:unit && mise run fmt && mise run clippy`

Expected: All pass.

- [ ] **Step 5: Commit**

```bash
git add src/commands/hooks/jobs.rs
git commit -m "feat(jobs): add --worktree and --cwd flags to retry, lift cross-worktree guard"
```

---

### Task 7: Add listing filter flags (`--worktree`, `--status`, `--hook`)

**Files:**

- Modify: `src/commands/hooks/jobs.rs` (JobsArgs struct, list_jobs function)

- [ ] **Step 1: Add filter flags to `JobsArgs`**

In `src/commands/hooks/jobs.rs`, extend the `JobsArgs` struct (around line 66):

```rust
#[derive(Parser, Debug)]
#[command(about = "Manage background hook jobs")]
pub struct JobsArgs {
    #[command(subcommand)]
    command: Option<JobsCommand>,

    /// Show jobs across all worktrees.
    #[arg(long, conflicts_with = "worktree")]
    all: bool,

    /// Output in JSON format.
    #[arg(long)]
    json: bool,

    /// Filter to a specific worktree (can be deleted).
    #[arg(long, conflicts_with = "all")]
    worktree: Option<String>,

    /// Filter to invocations containing jobs with this status.
    #[arg(long)]
    status: Option<String>,

    /// Filter to invocations of this hook type.
    #[arg(long = "hook")]
    hook_filter: Option<String>,
}
```

- [ ] **Step 2: Update `list_jobs` to apply filters**

In the `list_jobs` function (around line 545), update the invocation selection
logic. Currently:

```rust
let invocations = if args.all {
    store.list_invocations()?
} else {
    store.list_invocations_for_worktree(&current_worktree)?
};
```

Change to:

```rust
let invocations = if args.all {
    store.list_invocations()?
} else if let Some(ref wt) = args.worktree {
    store.list_invocations_for_worktree(wt)?
} else {
    store.list_invocations_for_worktree(&current_worktree)?
};

// Apply --hook filter.
let invocations: Vec<_> = if let Some(ref hook) = args.hook_filter {
    invocations
        .into_iter()
        .filter(|inv| inv.hook_type == *hook)
        .collect()
} else {
    invocations
};

// Apply --status filter (invocation-level: keep if any job matches).
let invocations: Vec<_> = if let Some(ref status_str) = args.status {
    let target_status = match status_str.as_str() {
        "failed" => JobStatus::Failed,
        "completed" => JobStatus::Completed,
        "running" => JobStatus::Running,
        "cancelled" => JobStatus::Cancelled,
        "skipped" => JobStatus::Skipped,
        other => anyhow::bail!(
            "Unknown status '{}'. Valid values: failed, completed, running, cancelled, skipped.",
            other
        ),
    };
    invocations
        .into_iter()
        .filter(|inv| {
            store
                .list_jobs_in_invocation(&inv.invocation_id)
                .unwrap_or_default()
                .iter()
                .any(|dir| {
                    store
                        .read_meta(dir)
                        .map(|m| m.status == target_status)
                        .unwrap_or(false)
                })
        })
        .collect()
} else {
    invocations
};
```

- [ ] **Step 3: Update the JSON output path similarly**

The `print_json_output` function and the `list_jobs` JSON branch also need to
receive the filtered invocations. Check that the same `invocations` variable is
passed to both the table rendering and JSON paths.

- [ ] **Step 4: Run tests and clippy**

Run: `mise run test:unit && mise run fmt && mise run clippy`

Expected: All pass.

- [ ] **Step 5: Commit**

```bash
git add src/commands/hooks/jobs.rs
git commit -m "feat(jobs): add --worktree, --status, --hook listing filters"
```

---

### Task 8: Add unit tests for listing filters and cross-worktree retry

**Files:**

- Modify: `src/commands/hooks/jobs.rs` (test module)

- [ ] **Step 1: Add filter parsing tests**

Add in the test module of `src/commands/hooks/jobs.rs`:

```rust
#[test]
fn test_retry_target_with_worktree_flag_uses_effective_worktree() {
    // This is a behavioral test — verify resolve_retry_invocation
    // uses the provided worktree, not the current one.
    use crate::coordinator::log_store::{InvocationMeta, LogStore};
    use tempfile::TempDir;

    let tmp = TempDir::new().unwrap();
    let store = LogStore::new(tmp.path().to_path_buf());
    let now = chrono::Utc::now();

    // Create an invocation in feature/other (not current worktree)
    std::fs::create_dir_all(tmp.path().join("inv1")).unwrap();
    let inv_meta = InvocationMeta {
        invocation_id: "inv1".to_string(),
        trigger_command: "worktree-post-create".to_string(),
        hook_type: "worktree-post-create".to_string(),
        worktree: "feature/other".to_string(),
        created_at: now,
    };
    store.write_invocation_meta("inv1", &inv_meta).unwrap();

    // resolve_retry_invocation with "feature/other" should find it
    let result = resolve_retry_invocation(
        &RetryTarget::LatestInvocation,
        &store,
        "feature/other",
    );
    assert!(result.is_ok());
    assert_eq!(result.unwrap().worktree, "feature/other");

    // With "feature/current" should find nothing
    let result = resolve_retry_invocation(
        &RetryTarget::LatestInvocation,
        &store,
        "feature/current",
    );
    assert!(result.is_err());
}
```

- [ ] **Step 2: Run tests**

Run: `mise run test:unit`

Expected: All pass.

- [ ] **Step 3: Commit**

```bash
git add src/commands/hooks/jobs.rs
git commit -m "test(jobs): add cross-worktree retry and listing filter tests"
```

---

### Task 9: Shell completion — `complete_retry_worktrees` and `complete_hook_types`

**Files:**

- Modify: `src/commands/complete.rs`

- [ ] **Step 1: Add dispatch arms**

In `src/commands/complete.rs`, add new dispatch arms in the match block (after
the existing `("hooks-jobs-retry", 1)` arm):

```rust
        // hooks jobs retry --worktree: worktrees with failures
        ("hooks-jobs-retry-worktree", 1) => complete_retry_worktrees(word),

        // hooks jobs --worktree: all worktrees
        ("hooks-jobs-worktree", 1) => complete_listing_worktrees(word),

        // hooks jobs --hook / hooks jobs retry --hook filter: hook types
        ("hooks-jobs-hook-filter", 1) => complete_hook_types(word),
```

- [ ] **Step 2: Implement `complete_retry_worktrees`**

Filtered to worktrees with failures. Uses `name\tgroup\tdescription` format:

```rust
fn complete_retry_worktrees(prefix: &str) -> Result<Vec<String>> {
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

    let now = chrono::Utc::now();
    let invocations = store.list_invocations().unwrap_or_default();

    // Group by worktree: (failed_count, latest_created_at)
    let mut worktree_stats: std::collections::HashMap<String, (usize, chrono::DateTime<chrono::Utc>)> =
        std::collections::HashMap::new();

    for inv in &invocations {
        let job_dirs = store
            .list_jobs_in_invocation(&inv.invocation_id)
            .unwrap_or_default();
        let failed = job_dirs
            .iter()
            .filter_map(|d| store.read_meta(d).ok())
            .filter(|m| matches!(m.status, JobStatus::Failed | JobStatus::Cancelled))
            .count();
        let entry = worktree_stats
            .entry(inv.worktree.clone())
            .or_insert((0, inv.created_at));
        entry.0 += failed;
        if inv.created_at > entry.1 {
            entry.1 = inv.created_at;
        }
    }

    let mut entries = Vec::new();
    for (wt, (failed, latest)) in &worktree_stats {
        if *failed == 0 {
            continue; // Only show worktrees with failures for retry
        }
        if wt.starts_with(prefix) {
            let ago = crate::output::format::shorthand_from_seconds(
                now.signed_duration_since(*latest).num_seconds(),
            );
            entries.push(format!(
                "{wt}\tworktree\t{failed} failed, {ago} ago",
            ));
        }
    }

    Ok(entries)
}
```

- [ ] **Step 3: Implement `complete_listing_worktrees`**

Same as above but unfiltered (includes worktrees with zero failures):

```rust
fn complete_listing_worktrees(prefix: &str) -> Result<Vec<String>> {
    use crate::coordinator::log_store::LogStore;
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

    let now = chrono::Utc::now();
    let worktrees = store.list_distinct_worktrees().unwrap_or_default();
    let invocations = store.list_invocations().unwrap_or_default();

    let mut entries = Vec::new();
    for wt in &worktrees {
        if !wt.starts_with(prefix) {
            continue;
        }
        let wt_invs: Vec<_> = invocations.iter().filter(|i| i.worktree == *wt).collect();
        let latest = wt_invs.iter().map(|i| i.created_at).max();
        let ago = latest
            .map(|t| {
                crate::output::format::shorthand_from_seconds(
                    now.signed_duration_since(t).num_seconds(),
                )
            })
            .unwrap_or_else(|| "unknown".to_string());
        let inv_count = wt_invs.len();
        entries.push(format!(
            "{wt}\tworktree\t{inv_count} invocation{}, {ago} ago",
            if inv_count == 1 { "" } else { "s" },
        ));
    }

    Ok(entries)
}
```

- [ ] **Step 4: Implement `complete_hook_types`**

```rust
fn complete_hook_types(prefix: &str) -> Result<Vec<String>> {
    use crate::coordinator::log_store::LogStore;
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

    let invocations = store.list_invocations().unwrap_or_default();

    let mut hook_counts: std::collections::HashMap<String, usize> =
        std::collections::HashMap::new();
    for inv in &invocations {
        *hook_counts.entry(inv.hook_type.clone()).or_insert(0) += 1;
    }

    let mut entries = Vec::new();
    for (hook, count) in &hook_counts {
        if hook.starts_with(prefix) {
            entries.push(format!(
                "{hook}\thook\t{count} invocation{}",
                if *count == 1 { "" } else { "s" },
            ));
        }
    }

    Ok(entries)
}
```

- [ ] **Step 5: Run tests and clippy**

Run: `mise run test:unit && mise run fmt && mise run clippy`

- [ ] **Step 6: Commit**

```bash
git add src/commands/complete.rs
git commit -m "feat(jobs): add worktree and hook-type completion helpers"
```

---

### Task 10: Wire completion scripts for new flags

**Files:**

- Modify: `src/commands/completions/bash.rs`
- Modify: `src/commands/completions/zsh.rs`
- Modify: `src/commands/completions/fish.rs`
- Modify: `src/commands/completions/fig.rs`

- [ ] **Step 1: Update bash completions**

In `src/commands/completions/bash.rs`:

**a)** In the `retry)` case, add `--worktree` and `--cwd` to the flag list:

Change:

```bash
COMPREPLY=( $(compgen -W "--hook --inv --job -h --help" -- "$cur") )
```

To:

```bash
COMPREPLY=( $(compgen -W "--hook --inv --job --worktree --cwd -h --help" -- "$cur") )
```

**b)** Add dynamic completion for `--worktree` value in the `retry)` case. After
the flag check, add:

```bash
                        if [[ "${words[*]}" == *"--worktree"* && "${prev}" == "--worktree" ]]; then
                            local completions
                            completions=$(daft __complete hooks-jobs-retry-worktree "$cur" 2>/dev/null)
                            if [[ -n "$completions" ]]; then
                                while IFS=$'\n' read -r line; do
                                    local val="${line%%	*}"
                                    COMPREPLY+=("$val")
                                done <<< "$completions"
                            fi
                            return 0
                        fi
```

**c)** In the `jobs)` section (the parent level, where `--all` and `--json` are
handled), add the new listing flags. Find where `--all` and `--json` are offered
and add `--worktree`, `--status`, `--hook`:

```bash
COMPREPLY=( $(compgen -W "--all --json --worktree --status --hook -h --help" -- "$cur") )
```

And add dynamic dispatch for `--worktree` and `--hook` values, and static
completion for `--status`:

```bash
                        if [[ "${prev}" == "--worktree" ]]; then
                            local completions
                            completions=$(daft __complete hooks-jobs-worktree "$cur" 2>/dev/null)
                            if [[ -n "$completions" ]]; then
                                while IFS=$'\n' read -r line; do
                                    local val="${line%%	*}"
                                    COMPREPLY+=("$val")
                                done <<< "$completions"
                            fi
                            return 0
                        fi
                        if [[ "${prev}" == "--status" ]]; then
                            COMPREPLY=( $(compgen -W "failed completed running cancelled skipped" -- "$cur") )
                            return 0
                        fi
                        if [[ "${prev}" == "--hook" ]]; then
                            local completions
                            completions=$(daft __complete hooks-jobs-hook-filter "$cur" 2>/dev/null)
                            if [[ -n "$completions" ]]; then
                                while IFS=$'\n' read -r line; do
                                    local val="${line%%	*}"
                                    COMPREPLY+=("$val")
                                done <<< "$completions"
                            fi
                            return 0
                        fi
```

- [ ] **Step 2: Update zsh completions**

Apply the same pattern in `src/commands/completions/zsh.rs`:

**a)** Add `--worktree --cwd` to the retry flag list.

**b)** Add value dispatch for `--worktree` in retry (check `$prev` or
`$words[-2]`).

**c)** Add `--worktree --status --hook` to the jobs-level flag list.

**d)** Add value dispatch for each.

- [ ] **Step 3: Update fish completions**

In `src/commands/completions/fish.rs`:

**a)** Add flag registrations for retry:

```fish
complete -c daft -n '...; and __fish_seen_subcommand_from retry' -l worktree -d 'Retry from specific worktree'
complete -c daft -n '...; and __fish_seen_subcommand_from retry' -l cwd -d 'Override working directory'
```

**b)** Add dynamic completion for `--worktree` value on retry.

**c)** Add flag registrations for the listing:

```fish
complete -c daft -n '...; and __fish_seen_subcommand_from jobs' -l worktree -d 'Filter by worktree'
complete -c daft -n '...; and __fish_seen_subcommand_from jobs' -l status -d 'Filter by job status'
complete -c daft -n '...; and __fish_seen_subcommand_from jobs' -l hook -d 'Filter by hook type'
```

**d)** Add static `--status` values and dynamic `--hook` dispatch.

- [ ] **Step 4: Update fig completions**

In `src/commands/completions/fig.rs`, update the `hooks_jobs` subcommand
description to mention filters, and add `--worktree`/`--status`/`--hook` to the
options if fig supports them.

- [ ] **Step 5: Run tests and clippy**

Run: `mise run test:unit && mise run fmt && mise run clippy`

- [ ] **Step 6: Commit**

```bash
git add src/commands/completions/bash.rs src/commands/completions/zsh.rs \
  src/commands/completions/fish.rs src/commands/completions/fig.rs
git commit -m "feat(jobs): wire --worktree, --status, --hook completions for retry and listing"
```

---

### Task 11: Integration scenario — `cross-worktree-retry.yml`

**Files:**

- Create: `tests/manual/scenarios/hooks/cross-worktree-retry.yml`

- [ ] **Step 1: Write the scenario**

```yaml
name: Cross-worktree retry via --worktree flag and composite address
description:
  "Verifies that 'retry --worktree feature/x' works from a different worktree,
  and that the composite address form 'retry feature/x:job-name' also works
  after C lifts B's guard."

repos:
  - name: test-cross-retry
    default_branch: main
    branches:
      - name: main
        files:
          - path: README.md
            content: "# Cross-worktree retry test"
        commits:
          - message: "Initial commit"
      - name: feature/target
        from: main
    daft_yml: |
      hooks:
        worktree-post-create:
          jobs:
            - name: cross-job
              run: "exit 1"

steps:
  - name: Clone the repository
    run: git-worktree-clone --layout contained $REMOTE_TEST_CROSS_RETRY
    expect:
      exit_code: 0

  - name: Trust the repository
    run: daft hooks trust --force 2>&1
    cwd: "$WORK_DIR/test-cross-retry/main"
    expect:
      exit_code: 0

  - name: Checkout feature branch (hook fires, job fails)
    run: env -u DAFT_TESTING git-worktree-checkout feature/target 2>&1
    cwd: "$WORK_DIR/test-cross-retry/main"
    expect:
      exit_code: 0

  - name: Retry from main worktree using --worktree flag
    run: daft hooks jobs retry --worktree feature/target 2>&1
    cwd: "$WORK_DIR/test-cross-retry/main"
    expect:
      exit_code: 0
      output_contains:
        - "Retried"

  - name: View jobs of feature/target from main using --worktree
    run: daft hooks jobs --worktree feature/target 2>&1
    cwd: "$WORK_DIR/test-cross-retry/main"
    expect:
      exit_code: 0
      output_contains:
        - "cross-job"
        - "hooks jobs retry"

  - name: Retry from main using composite address
    run: daft hooks jobs retry feature/target:cross-job 2>&1
    cwd: "$WORK_DIR/test-cross-retry/main"
    expect:
      exit_code: 0
      output_contains:
        - "Retried 1 job"
```

- [ ] **Step 2: Commit**

```bash
git add tests/manual/scenarios/hooks/cross-worktree-retry.yml
git commit -m "test(hooks): add cross-worktree-retry integration scenario"
```

---

### Task 12: Integration scenario — `deleted-worktree-retry.yml`

**Files:**

- Create: `tests/manual/scenarios/hooks/deleted-worktree-retry.yml`

- [ ] **Step 1: Write the scenario**

```yaml
name: Retry jobs for a deleted worktree with --cwd override
description:
  "Verifies that retry for a removed worktree refuses by default when the
  working directory is gone, and succeeds when --cwd is provided."

repos:
  - name: test-deleted-retry
    default_branch: main
    branches:
      - name: main
        files:
          - path: README.md
            content: "# Deleted worktree retry test"
        commits:
          - message: "Initial commit"
      - name: feature/ephemeral
        from: main
    daft_yml: |
      hooks:
        worktree-post-create:
          jobs:
            - name: setup-env
              run: "exit 1"

steps:
  - name: Clone the repository
    run: git-worktree-clone --layout contained $REMOTE_TEST_DELETED_RETRY
    expect:
      exit_code: 0

  - name: Trust the repository
    run: daft hooks trust --force 2>&1
    cwd: "$WORK_DIR/test-deleted-retry/main"
    expect:
      exit_code: 0

  - name: Checkout feature branch (hook fires, job fails)
    run: env -u DAFT_TESTING git-worktree-checkout feature/ephemeral 2>&1
    cwd: "$WORK_DIR/test-deleted-retry/main"
    expect:
      exit_code: 0

  - name: Remove the worktree
    run: daft remove feature/ephemeral --force 2>&1
    cwd: "$WORK_DIR/test-deleted-retry/main"
    expect:
      exit_code: 0

  - name: Retry without --cwd should fail (working dir gone)
    run: daft hooks jobs retry --worktree feature/ephemeral 2>&1
    cwd: "$WORK_DIR/test-deleted-retry/main"
    expect:
      exit_code: 1
      output_contains:
        - "no longer exists"
        - "--cwd"

  - name: Retry with --cwd should succeed
    run: daft hooks jobs retry --worktree feature/ephemeral --cwd /tmp 2>&1
    cwd: "$WORK_DIR/test-deleted-retry/main"
    expect:
      exit_code: 0
      output_contains:
        - "Retried"
```

- [ ] **Step 2: Commit**

```bash
git add tests/manual/scenarios/hooks/deleted-worktree-retry.yml
git commit -m "test(hooks): add deleted-worktree-retry integration scenario"
```

---

### Task 13: Integration scenario — `listing-filters.yml`

**Files:**

- Create: `tests/manual/scenarios/hooks/listing-filters.yml`

- [ ] **Step 1: Write the scenario**

```yaml
name: Listing filters --status, --hook, --worktree work and combine
description:
  "Verifies that daft hooks jobs supports --status, --hook, and --worktree
  filters, and that they AND together correctly."

repos:
  - name: test-listing-filters
    default_branch: main
    branches:
      - name: main
        files:
          - path: README.md
            content: "# Listing filters test"
        commits:
          - message: "Initial commit"
      - name: feature/filters
        from: main
    daft_yml: |
      hooks:
        worktree-post-create:
          jobs:
            - name: good-job
              run: echo success
            - name: bad-job
              run: "exit 1"

steps:
  - name: Clone the repository
    run: git-worktree-clone --layout contained $REMOTE_TEST_LISTING_FILTERS
    expect:
      exit_code: 0

  - name: Trust the repository
    run: daft hooks trust --force 2>&1
    cwd: "$WORK_DIR/test-listing-filters/main"
    expect:
      exit_code: 0

  - name: Checkout feature branch
    run: env -u DAFT_TESTING git-worktree-checkout feature/filters 2>&1
    cwd: "$WORK_DIR/test-listing-filters/main"
    expect:
      exit_code: 0

  - name: Filter by --status failed shows the invocation
    run: daft hooks jobs --status failed 2>&1
    cwd: "$WORK_DIR/test-listing-filters/feature/filters"
    expect:
      exit_code: 0
      output_contains:
        - "bad-job"
        - "failed"

  - name: Filter by --status running shows nothing
    run: daft hooks jobs --status running 2>&1
    cwd: "$WORK_DIR/test-listing-filters/feature/filters"
    expect:
      exit_code: 0
      output_not_contains:
        - "bad-job"

  - name: Filter by --hook worktree-post-create shows the invocation
    run: daft hooks jobs --hook worktree-post-create 2>&1
    cwd: "$WORK_DIR/test-listing-filters/feature/filters"
    expect:
      exit_code: 0
      output_contains:
        - "worktree-post-create"

  - name: Filter by --hook worktree-pre-remove shows nothing
    run: daft hooks jobs --hook worktree-pre-remove 2>&1
    cwd: "$WORK_DIR/test-listing-filters/feature/filters"
    expect:
      exit_code: 0
      output_not_contains:
        - "worktree-post-create"

  - name: Filter by --worktree from main shows feature/filters jobs
    run: daft hooks jobs --worktree feature/filters 2>&1
    cwd: "$WORK_DIR/test-listing-filters/main"
    expect:
      exit_code: 0
      output_contains:
        - "bad-job"
        - "good-job"

  - name: --all and --worktree together should error
    run: daft hooks jobs --all --worktree feature/filters 2>&1
    cwd: "$WORK_DIR/test-listing-filters/main"
    expect:
      exit_code: 2
```

- [ ] **Step 2: Commit**

```bash
git add tests/manual/scenarios/hooks/listing-filters.yml
git commit -m "test(hooks): add listing-filters integration scenario"
```

---

### Task 14: Integration scenario — `post-clone-visibility.yml` (B-2)

**Files:**

- Create: `tests/manual/scenarios/hooks/post-clone-visibility.yml`

- [ ] **Step 1: Write the scenario**

```yaml
name: Post-clone hook invocation appears in daft hooks jobs listing
description:
  "Regression coverage for post-clone hooks. All other scenarios test
  worktree-post-create or worktree-pre-remove. This verifies that a post-clone
  hook fired during git-worktree-clone also writes an invocation record visible
  in the listing."

repos:
  - name: test-post-clone-vis
    default_branch: main
    branches:
      - name: main
        files:
          - path: README.md
            content: "# Post-clone visibility test"
        commits:
          - message: "Initial commit"
    daft_yml: |
      hooks:
        post-clone:
          jobs:
            - name: clone-setup
              run: echo clone-setup-output

steps:
  - name: Clone the repository (triggers post-clone hook)
    run:
      env -u DAFT_TESTING git-worktree-clone --layout contained
      $REMOTE_TEST_POST_CLONE_VIS
    expect:
      exit_code: 0
      output_contains:
        - "clone-setup-output"

  - name: Trust the repository
    run: daft hooks trust --force 2>&1
    cwd: "$WORK_DIR/test-post-clone-vis/main"
    expect:
      exit_code: 0

  - name: List jobs — should show post-clone invocation
    run: daft hooks jobs 2>&1
    cwd: "$WORK_DIR/test-post-clone-vis/main"
    expect:
      exit_code: 0
      output_contains:
        - "post-clone"
        - "clone-setup"
        - "completed"
```

Note: The post-clone hook fires during `git-worktree-clone`. The repo must be
trusted before the hook will execute. The implementer should verify whether
trust needs to happen before or after the clone in the test harness. If the
clone command auto-trusts in the test fixture (via `DAFT_TESTING`), the
`env -u DAFT_TESTING` prefix should handle it. If trust is needed before clone,
the scenario steps may need reordering — the implementer should check the
existing `foreground-only-hook.yml` pattern and adapt.

- [ ] **Step 2: Commit**

```bash
git add tests/manual/scenarios/hooks/post-clone-visibility.yml
git commit -m "test(hooks): add post-clone-visibility integration scenario (B-2)"
```

---

### Task 15: Final verification and cleanup

**Files:**

- All files modified in Tasks 1-14

- [ ] **Step 1: Run the full unit test suite**

Run: `mise run test:unit`

Expected: All tests pass.

- [ ] **Step 2: Run clippy**

Run: `mise run clippy`

Expected: Zero warnings.

- [ ] **Step 3: Run fmt check**

Run: `mise run fmt:check`

Expected: All files formatted.

- [ ] **Step 4: Run all integration scenarios**

Run: `mise run test:manual -- --ci`

Expected: All scenarios pass — the 13 existing + 4 new.

- [ ] **Step 5: Remove any dead code or temporary annotations**

Check for unused `#[allow(dead_code)]`, unused imports, or dead functions.

- [ ] **Step 6: Regenerate man pages**

Run: `mise run man:gen`

The `retry` subcommand gained `--worktree` and `--cwd` flags. The `hooks jobs`
command gained `--worktree`, `--status`, `--hook` flags. Man pages must reflect
these.

- [ ] **Step 7: Update the basket overview doc**

In `docs/superpowers/specs/2026-04-11-hooks-jobs-basket-overview.md`, update
sub-project C's status from "deferred" to "complete" with the commit range and
test count.

- [ ] **Step 8: Commit if any changes from cleanup**

```bash
git add -A
git commit -m "chore(jobs): cleanup and man page regen after cross-worktree retry"
```
