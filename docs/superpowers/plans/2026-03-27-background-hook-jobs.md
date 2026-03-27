# Background Hook Jobs Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use
> superpowers:subagent-driven-development (recommended) or
> superpowers:executing-plans to implement this plan task-by-task. Steps use
> checkbox (`- [ ]`) syntax for tracking.

**Goal:** Allow hook jobs to run in the background via a coordinator process, so
long-running tasks (like builds) don't block the user from starting work.

**Architecture:** Jobs marked `background: true` are partitioned out of the
foreground DAG and handed to a forked coordinator process that manages them as
threads. The coordinator communicates via a Unix domain socket, writes logs to
the XDG state directory, and exposes a `daft hooks jobs` CLI for management.

**Tech Stack:** Rust, Unix domain sockets (`std::os::unix::net`), `serde_json`
for IPC, `dirs` crate for XDG paths, existing DAG executor infrastructure.

---

## File Structure

### New Files

| File                                               | Responsibility                                              |
| -------------------------------------------------- | ----------------------------------------------------------- |
| `src/coordinator/mod.rs`                           | Coordinator types, state, IPC protocol, public API          |
| `src/coordinator/process.rs`                       | Fork, daemonize, socket listener, main loop                 |
| `src/coordinator/client.rs`                        | Client connection to coordinator socket (for CLI + removal) |
| `src/coordinator/log_store.rs`                     | Log file management, retention, cleanup                     |
| `src/commands/hooks/jobs.rs`                       | `daft hooks jobs` CLI command and subcommands               |
| `tests/manual/scenarios/hooks/background-jobs.yml` | YAML integration test scenarios                             |

### Modified Files

| File                                | Change                                                         |
| ----------------------------------- | -------------------------------------------------------------- |
| `src/lib.rs`                        | Add `daft_state_dir()`, `pub mod coordinator`                  |
| `src/hooks/yaml_config.rs`          | Add `background`, `background_output`, `log` fields            |
| `src/hooks/yaml_config_validate.rs` | Validate new fields, foreground promotion warning              |
| `src/hooks/yaml_config_loader.rs`   | Merge `log` config across layers                               |
| `src/hooks/yaml_executor/mod.rs`    | DAG partitioning, coordinator handoff                          |
| `src/hooks/job_adapter.rs`          | Pass through `background` and `log` fields to JobSpec          |
| `src/executor/mod.rs`               | Add `background`, `background_output`, `log_config` to JobSpec |
| `src/hooks/executor.rs`             | Post-hook summary line, promotion warnings                     |
| `src/commands/hooks/mod.rs`         | Add `Jobs` variant to `HooksCommand`, route to `jobs.rs`       |
| `src/main.rs`                       | Add `DAFT_IS_COORDINATOR` guard alongside `__` prefix guard    |
| `Cargo.toml`                        | Add `chrono` dep for timestamps (Unix sockets are in std)      |

---

## Task 1: Add `daft_state_dir()` to `src/lib.rs`

**Files:**

- Modify: `src/lib.rs:22-76`

- [ ] **Step 1: Write the failing test**

```rust
// In src/lib.rs, inside the existing #[cfg(test)] mod tests block:

#[test]
#[serial]
fn test_daft_state_dir_default() {
    env::remove_var("DAFT_STATE_DIR");
    let dir = daft_state_dir().unwrap();
    assert!(dir.ends_with("daft"));
}

#[test]
#[serial]
fn test_daft_state_dir_override() {
    env::set_var("DAFT_STATE_DIR", "/tmp/test-daft-state");
    let dir = daft_state_dir().unwrap();
    assert_eq!(dir, PathBuf::from("/tmp/test-daft-state"));
    env::remove_var("DAFT_STATE_DIR");
}

#[test]
#[serial]
fn test_daft_state_dir_rejects_relative_path() {
    env::set_var("DAFT_STATE_DIR", "relative/path");
    let result = daft_state_dir();
    assert!(result.is_err());
    assert!(result.unwrap_err().to_string().contains("must be an absolute path"));
    env::remove_var("DAFT_STATE_DIR");
}
```

- [ ] **Step 2: Run test to verify it fails**

Run: `cargo test --lib test_daft_state_dir` Expected: FAIL — `daft_state_dir`
not found

- [ ] **Step 3: Implement `daft_state_dir()`**

Add to `src/lib.rs` after the `DATA_DIR_ENV` constant (line 30):

```rust
/// Environment variable to override the state directory path.
///
/// When set, coordinator sockets, background job logs, and other runtime
/// state are stored in this directory instead of the XDG state directory
/// (`~/.local/state/daft/`).
///
/// Only honored in dev builds (same policy as `DAFT_CONFIG_DIR`).
pub const STATE_DIR_ENV: &str = "DAFT_STATE_DIR";
```

Add after `daft_data_dir()` (after line 76):

```rust
/// Returns the daft state directory path.
///
/// In dev builds, when `DAFT_STATE_DIR` is set to a non-empty absolute path,
/// uses that path directly (no `daft/` suffix appended). In release builds the
/// env var is ignored. Always falls back to `dirs::state_dir()/daft`
/// (macOS: `~/.local/state/daft`, Linux: `$XDG_STATE_HOME/daft`).
pub fn daft_state_dir() -> anyhow::Result<std::path::PathBuf> {
    use std::path::PathBuf;
    if cfg!(daft_dev_build) {
        if let Ok(dir) = env::var(STATE_DIR_ENV) {
            if !dir.is_empty() {
                let path = PathBuf::from(&dir);
                if path.is_relative() {
                    anyhow::bail!("DAFT_STATE_DIR must be an absolute path, got: {dir}");
                }
                return Ok(path);
            }
        }
    }
    // dirs::state_dir() returns None on macOS (no native equivalent).
    // Fall back to ~/.local/state which is the XDG convention.
    let state_dir = dirs::state_dir().unwrap_or_else(|| {
        dirs::home_dir()
            .expect("Could not determine home directory")
            .join(".local")
            .join("state")
    });
    Ok(state_dir.join("daft"))
}
```

- [ ] **Step 4: Run tests to verify they pass**

Run: `cargo test --lib test_daft_state_dir` Expected: PASS (all 3 tests)

- [ ] **Step 5: Run existing tests to verify no regression**

Run: `cargo test --lib` Expected: All existing tests still pass

- [ ] **Step 6: Commit**

```bash
git add src/lib.rs
git commit -m "feat(coordinator): add daft_state_dir() for XDG state directory"
```

---

## Task 2: Add `background` and `log` fields to YAML config schema

**Files:**

- Modify: `src/hooks/yaml_config.rs:83-110` (HookDef),
  `src/hooks/yaml_config.rs:225-280` (JobDef)

- [ ] **Step 1: Write failing test for deserialization**

Add to the test module in `src/hooks/yaml_config.rs`:

```rust
#[test]
fn test_deserialize_background_job() {
    let yaml = r#"
hooks:
  worktree-post-create:
    background: true
    jobs:
      - name: warm build
        run: cargo build
      - name: install deps
        run: pnpm install
        background: false
        log:
          retention: "14d"
          path: "./build-logs/install.log"
      - name: silent job
        run: echo hello
        background_output: silent
"#;
    let config: YamlConfig = serde_yaml::from_str(yaml).unwrap();
    let hook = config.hooks.get("worktree-post-create").unwrap();
    assert_eq!(hook.background, Some(true));

    let jobs = hook.jobs.as_ref().unwrap();
    // Job 0: inherits hook-level background (no override)
    assert_eq!(jobs[0].background, None);
    // Job 1: explicit override
    assert_eq!(jobs[1].background, Some(false));
    assert_eq!(jobs[1].log.as_ref().unwrap().retention, Some("14d".to_string()));
    assert_eq!(jobs[1].log.as_ref().unwrap().path, Some("./build-logs/install.log".to_string()));
    // Job 2: background_output
    assert_eq!(jobs[2].background_output, Some(BackgroundOutput::Silent));
}

#[test]
fn test_deserialize_top_level_log_config() {
    let yaml = r#"
log:
  retention: "30d"
hooks:
  worktree-post-create:
    jobs:
      - name: test
        run: echo hi
"#;
    let config: YamlConfig = serde_yaml::from_str(yaml).unwrap();
    assert_eq!(config.log.as_ref().unwrap().retention, Some("30d".to_string()));
}
```

- [ ] **Step 2: Run test to verify it fails**

Run: `cargo test --lib test_deserialize_background_job` Expected: FAIL — fields
don't exist yet

- [ ] **Step 3: Add types and fields**

Add new types (before `HookDef`):

```rust
/// Output behavior for background jobs.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum BackgroundOutput {
    /// Always write to log file; terminal notification on failure.
    Log,
    /// Write to log file only on failure; no terminal notification.
    Silent,
}

/// Log configuration, applicable at top-level, hook-level, or job-level.
#[derive(Debug, Clone, Default, Deserialize, Serialize)]
pub struct LogConfig {
    /// Log retention duration (e.g., "7d", "24h", "30m").
    pub retention: Option<String>,
    /// Override log file path. Absolute or relative to worktree root.
    pub path: Option<String>,
}
```

Add to `YamlConfig` struct:

```rust
/// Log configuration (retention, etc.).
#[serde(default)]
pub log: Option<LogConfig>,
```

Add to `HookDef` struct:

```rust
/// Whether jobs in this hook default to background execution.
pub background: Option<bool>,
```

Add to `JobDef` struct:

```rust
/// Run this job in the background (overrides hook-level default).
pub background: Option<bool>,
/// Output behavior for background execution.
pub background_output: Option<BackgroundOutput>,
/// Log configuration for this job.
pub log: Option<LogConfig>,
```

- [ ] **Step 4: Run tests to verify they pass**

Run: `cargo test --lib test_deserialize_background` Expected: PASS

- [ ] **Step 5: Run full test suite to verify no regression**

Run: `mise run test:unit` Expected: All existing tests still pass

- [ ] **Step 6: Commit**

```bash
git add src/hooks/yaml_config.rs
git commit -m "feat(config): add background, background_output, and log fields to YAML schema"
```

---

## Task 3: Add `background` and `log` fields to `JobSpec`

**Files:**

- Modify: `src/executor/mod.rs:23-63` (JobSpec)
- Modify: `src/hooks/job_adapter.rs:31-91` (yaml_jobs_to_specs)

- [ ] **Step 1: Write failing test**

Add to the test module in `src/hooks/job_adapter.rs` (or create one if it
doesn't exist):

```rust
#[test]
fn test_background_fields_pass_through() {
    use crate::hooks::yaml_config::{BackgroundOutput, LogConfig};

    let jobs = vec![JobDef {
        name: Some("bg-job".to_string()),
        run: Some(RunCommand::Simple("echo hi".to_string())),
        background: Some(true),
        background_output: Some(BackgroundOutput::Silent),
        log: Some(LogConfig {
            retention: Some("14d".to_string()),
            path: Some("./logs/bg.log".to_string()),
        }),
        ..Default::default()
    }];

    let env = HookEnvironment::default();
    let specs = yaml_jobs_to_specs(&jobs, &env, None, Path::new("/tmp"));
    assert_eq!(specs.len(), 1);
    assert_eq!(specs[0].background, true);
    assert_eq!(specs[0].background_output, Some(BackgroundOutput::Silent));
    assert_eq!(specs[0].log_config.as_ref().unwrap().retention, Some("14d".to_string()));
}
```

- [ ] **Step 2: Run test to verify it fails**

Run: `cargo test --lib test_background_fields_pass_through` Expected: FAIL —
`background` field doesn't exist on JobSpec

- [ ] **Step 3: Add fields to `JobSpec`**

In `src/executor/mod.rs`, add to the `JobSpec` struct:

```rust
/// Whether this job should run in the background.
pub background: bool,
/// Output behavior for background execution.
pub background_output: Option<BackgroundOutput>,
/// Log configuration for this job.
pub log_config: Option<LogConfig>,
```

Update the `Default` impl to include:

```rust
background: false,
background_output: None,
log_config: None,
```

- [ ] **Step 4: Update `yaml_jobs_to_specs` to pass through fields**

In `src/hooks/job_adapter.rs`, in the `yaml_jobs_to_specs` function, when
building each `JobSpec`, add:

```rust
background: job.background.unwrap_or(false),
background_output: job.background_output.clone(),
log_config: job.log.clone(),
```

- [ ] **Step 5: Run tests to verify they pass**

Run: `cargo test --lib test_background_fields_pass_through` Expected: PASS

- [ ] **Step 6: Run full unit tests**

Run: `mise run test:unit` Expected: All tests pass

- [ ] **Step 7: Commit**

```bash
git add src/executor/mod.rs src/hooks/job_adapter.rs
git commit -m "feat(executor): add background and log fields to JobSpec"
```

---

## Task 4: Validate background config and detect foreground promotion

**Files:**

- Modify: `src/hooks/yaml_config_validate.rs`

- [ ] **Step 1: Write failing tests**

Add to the test module in `src/hooks/yaml_config_validate.rs`:

```rust
#[test]
fn test_warn_background_job_promoted_to_foreground() {
    let yaml = r#"
hooks:
  worktree-post-create:
    jobs:
      - name: bg-dep
        run: echo dep
        background: true
      - name: fg-consumer
        run: echo consume
        needs: [bg-dep]
"#;
    let config: YamlConfig = serde_yaml::from_str(yaml).unwrap();
    let result = validate_config(&config).unwrap();
    assert!(!result.warnings.is_empty());
    assert!(result.warnings.iter().any(|w|
        w.message.contains("promoted to foreground")
    ));
}

#[test]
fn test_warn_interactive_job_cannot_be_background() {
    let yaml = r#"
hooks:
  worktree-post-create:
    jobs:
      - name: interactive-bg
        run: vim file.txt
        background: true
        interactive: true
"#;
    let config: YamlConfig = serde_yaml::from_str(yaml).unwrap();
    let result = validate_config(&config).unwrap();
    assert!(!result.warnings.is_empty());
    assert!(result.warnings.iter().any(|w|
        w.message.contains("interactive") && w.message.contains("background")
    ));
}

#[test]
fn test_valid_background_output_values() {
    let yaml = r#"
hooks:
  worktree-post-create:
    jobs:
      - name: job1
        run: echo hi
        background: true
        background_output: log
      - name: job2
        run: echo hi
        background: true
        background_output: silent
"#;
    let config: YamlConfig = serde_yaml::from_str(yaml).unwrap();
    let result = validate_config(&config).unwrap();
    assert!(result.is_ok());
}

#[test]
fn test_warn_background_output_on_foreground_job() {
    let yaml = r#"
hooks:
  worktree-post-create:
    jobs:
      - name: fg-job
        run: echo hi
        background_output: silent
"#;
    let config: YamlConfig = serde_yaml::from_str(yaml).unwrap();
    let result = validate_config(&config).unwrap();
    assert!(result.warnings.iter().any(|w|
        w.message.contains("background_output") && w.message.contains("foreground")
    ));
}
```

- [ ] **Step 2: Run tests to verify they fail**

Run: `cargo test --lib test_warn_background` Expected: FAIL — validation logic
doesn't exist yet

- [ ] **Step 3: Implement validation rules**

In `src/hooks/yaml_config_validate.rs`, add to `validate_job()` (or a new
`validate_background_fields()` called from it):

```rust
fn validate_background_fields(
    job: &JobDef,
    hook_def: &HookDef,
    all_jobs: &[JobDef],
    path: &str,
    result: &mut ValidationResult,
) {
    let is_bg = job.background.or(hook_def.background).unwrap_or(false);

    // Interactive jobs cannot be background
    if is_bg && job.interactive == Some(true) {
        result.warnings.push(ValidationWarning {
            path: path.to_string(),
            message: format!(
                "Job '{}' is marked as both interactive and background; \
                 interactive jobs require a terminal and will be promoted to foreground",
                job.name.as_deref().unwrap_or("<unnamed>")
            ),
        });
    }

    // background_output on a foreground job is meaningless
    if !is_bg && job.background_output.is_some() {
        result.warnings.push(ValidationWarning {
            path: path.to_string(),
            message: format!(
                "Job '{}' has background_output set but is a foreground job; \
                 background_output only applies to background jobs",
                job.name.as_deref().unwrap_or("<unnamed>")
            ),
        });
    }
}
```

Add foreground promotion detection (at the hook level, after all jobs are
validated):

```rust
fn detect_foreground_promotions(
    hook_name: &str,
    hook_def: &HookDef,
    result: &mut ValidationResult,
) {
    let jobs = match &hook_def.jobs {
        Some(jobs) => jobs,
        None => return,
    };

    // Build a map of job name -> is_background
    let bg_map: HashMap<&str, bool> = jobs
        .iter()
        .filter_map(|j| {
            let name = j.name.as_deref()?;
            let is_bg = j.background.or(hook_def.background).unwrap_or(false);
            Some((name, is_bg))
        })
        .collect();

    // For each foreground job, walk its `needs` transitively
    // and warn if any dependency is background
    for job in jobs {
        let is_bg = job.background.or(hook_def.background).unwrap_or(false);
        if is_bg {
            continue; // Only check foreground jobs
        }
        for dep_name in &job.needs {
            if bg_map.get(dep_name.as_str()) == Some(&true) {
                result.warnings.push(ValidationWarning {
                    path: format!("hooks.{}.jobs", hook_name),
                    message: format!(
                        "Background job '{}' will be promoted to foreground \
                         (required by '{}')",
                        dep_name,
                        job.name.as_deref().unwrap_or("<unnamed>")
                    ),
                });
            }
        }
    }
}
```

Wire both functions into the existing `validate_hook_def()` call chain.

- [ ] **Step 4: Run tests to verify they pass**

Run: `cargo test --lib test_warn_background` Expected: PASS (all 4 tests)

- [ ] **Step 5: Run full unit tests**

Run: `mise run test:unit` Expected: All tests pass

- [ ] **Step 6: Commit**

```bash
git add src/hooks/yaml_config_validate.rs
git commit -m "feat(config): validate background job fields and detect foreground promotion"
```

---

## Task 5: DAG partitioning — split jobs into foreground and background phases

**Files:**

- Modify: `src/hooks/yaml_executor/mod.rs`

- [ ] **Step 1: Write failing test**

Add to the test module in `src/hooks/yaml_executor/mod.rs` (or a new
`src/hooks/yaml_executor/partition.rs`):

```rust
#[cfg(test)]
mod partition_tests {
    use super::*;
    use crate::executor::JobSpec;

    fn spec(name: &str, background: bool, needs: Vec<&str>) -> JobSpec {
        JobSpec {
            name: name.to_string(),
            background,
            needs: needs.into_iter().map(String::from).collect(),
            ..Default::default()
        }
    }

    #[test]
    fn test_partition_no_background_jobs() {
        let jobs = vec![
            spec("a", false, vec![]),
            spec("b", false, vec!["a"]),
        ];
        let (fg, bg) = partition_foreground_background(&jobs);
        assert_eq!(fg.len(), 2);
        assert_eq!(bg.len(), 0);
    }

    #[test]
    fn test_partition_independent_background_jobs() {
        let jobs = vec![
            spec("fg", false, vec![]),
            spec("bg1", true, vec![]),
            spec("bg2", true, vec![]),
        ];
        let (fg, bg) = partition_foreground_background(&jobs);
        assert_eq!(fg.len(), 1);
        assert_eq!(fg[0].name, "fg");
        assert_eq!(bg.len(), 2);
    }

    #[test]
    fn test_partition_background_promoted_by_foreground_dependency() {
        let jobs = vec![
            spec("bg-dep", true, vec![]),
            spec("fg-consumer", false, vec!["bg-dep"]),
        ];
        let (fg, bg) = partition_foreground_background(&jobs);
        assert_eq!(fg.len(), 2); // bg-dep promoted
        assert_eq!(bg.len(), 0);
    }

    #[test]
    fn test_partition_transitive_promotion() {
        let jobs = vec![
            spec("bg1", true, vec![]),
            spec("bg2", true, vec!["bg1"]),
            spec("fg", false, vec!["bg2"]),
        ];
        let (fg, bg) = partition_foreground_background(&jobs);
        assert_eq!(fg.len(), 3); // both bg jobs promoted
        assert_eq!(bg.len(), 0);
    }

    #[test]
    fn test_partition_mixed() {
        let jobs = vec![
            spec("install", false, vec![]),
            spec("build", true, vec!["install"]),
            spec("assets", true, vec!["install"]),
            spec("types", false, vec!["build"]),
        ];
        let (fg, bg) = partition_foreground_background(&jobs);
        // install: foreground
        // build: background, but types depends on it -> promoted
        // types: foreground
        // assets: background, no foreground dependents -> stays background
        assert_eq!(fg.len(), 3);
        assert!(fg.iter().any(|j| j.name == "install"));
        assert!(fg.iter().any(|j| j.name == "build"));
        assert!(fg.iter().any(|j| j.name == "types"));
        assert_eq!(bg.len(), 1);
        assert_eq!(bg[0].name, "assets");
    }
}
```

- [ ] **Step 2: Run tests to verify they fail**

Run: `cargo test --lib partition_tests` Expected: FAIL —
`partition_foreground_background` doesn't exist

- [ ] **Step 3: Implement partitioning function**

Add to `src/hooks/yaml_executor/mod.rs` (or a new `partition.rs` submodule):

```rust
/// Partition jobs into foreground and background phases.
///
/// Background jobs that are transitively depended on by any foreground job
/// are promoted to the foreground phase. Returns `(foreground, background)`.
pub fn partition_foreground_background(jobs: &[JobSpec]) -> (Vec<JobSpec>, Vec<JobSpec>) {
    if jobs.is_empty() {
        return (Vec::new(), Vec::new());
    }

    // Build name -> index map
    let name_to_idx: HashMap<&str, usize> = jobs
        .iter()
        .enumerate()
        .map(|(i, j)| (j.name.as_str(), i))
        .collect();

    // Build reverse dependency map: job -> jobs that depend on it
    let mut dependents: HashMap<usize, Vec<usize>> = HashMap::new();
    for (i, job) in jobs.iter().enumerate() {
        for dep_name in &job.needs {
            if let Some(&dep_idx) = name_to_idx.get(dep_name.as_str()) {
                dependents.entry(dep_idx).or_default().push(i);
            }
        }
    }

    // Start with all foreground jobs as "must be foreground"
    let mut must_fg: Vec<bool> = jobs.iter().map(|j| !j.background).collect();

    // Walk backwards from foreground jobs through dependencies,
    // promoting any background dependency to foreground
    let mut stack: Vec<usize> = must_fg
        .iter()
        .enumerate()
        .filter(|(_, &is_fg)| is_fg)
        .map(|(i, _)| i)
        .collect();

    while let Some(idx) = stack.pop() {
        for dep_name in &jobs[idx].needs {
            if let Some(&dep_idx) = name_to_idx.get(dep_name.as_str()) {
                if !must_fg[dep_idx] {
                    must_fg[dep_idx] = true;
                    stack.push(dep_idx); // Recurse into this dep's deps
                }
            }
        }
    }

    // Partition
    let mut foreground = Vec::new();
    let mut background = Vec::new();
    for (i, job) in jobs.iter().enumerate() {
        if must_fg[i] {
            foreground.push(job.clone());
        } else {
            background.push(job.clone());
        }
    }

    (foreground, background)
}
```

- [ ] **Step 4: Run tests to verify they pass**

Run: `cargo test --lib partition_tests` Expected: PASS (all 5 tests)

- [ ] **Step 5: Commit**

```bash
git add src/hooks/yaml_executor/
git commit -m "feat(executor): add foreground/background job partitioning"
```

---

## Task 6: Log store — write and read background job logs

**Files:**

- Create: `src/coordinator/log_store.rs`
- Create: `src/coordinator/mod.rs`
- Modify: `src/lib.rs` (add `pub mod coordinator`)

- [ ] **Step 1: Write failing tests**

Create `src/coordinator/mod.rs`:

```rust
pub mod log_store;
```

Create `src/coordinator/log_store.rs` with tests at the bottom:

```rust
#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    #[test]
    fn test_create_job_log_dir() {
        let tmp = TempDir::new().unwrap();
        let store = LogStore::new(tmp.path().to_path_buf());
        let dir = store.create_job_dir("abc123", "warm-build").unwrap();
        assert!(dir.exists());
        assert!(dir.ends_with("abc123/warm-build"));
    }

    #[test]
    fn test_write_and_read_meta() {
        let tmp = TempDir::new().unwrap();
        let store = LogStore::new(tmp.path().to_path_buf());
        let dir = store.create_job_dir("inv1", "job1").unwrap();
        let meta = JobMeta {
            name: "job1".to_string(),
            hook_type: "worktree-post-create".to_string(),
            worktree: "/path/to/wt".to_string(),
            command: "cargo build".to_string(),
            working_dir: "/path/to/wt".to_string(),
            env: HashMap::new(),
            started_at: chrono::Utc::now(),
            status: JobStatus::Running,
            exit_code: None,
            pid: Some(12345),
        };
        store.write_meta(&dir, &meta).unwrap();
        let loaded = store.read_meta(&dir).unwrap();
        assert_eq!(loaded.name, "job1");
        assert!(matches!(loaded.status, JobStatus::Running));
    }

    #[test]
    fn test_list_jobs_for_repo() {
        let tmp = TempDir::new().unwrap();
        let store = LogStore::new(tmp.path().to_path_buf());
        store.create_job_dir("inv1", "job-a").unwrap();
        store.create_job_dir("inv1", "job-b").unwrap();
        store.create_job_dir("inv2", "job-c").unwrap();
        let jobs = store.list_job_dirs().unwrap();
        assert_eq!(jobs.len(), 3);
    }

    #[test]
    fn test_clean_old_logs() {
        let tmp = TempDir::new().unwrap();
        let store = LogStore::new(tmp.path().to_path_buf());
        let dir = store.create_job_dir("old-inv", "old-job").unwrap();
        let meta = JobMeta {
            name: "old-job".to_string(),
            hook_type: "post-clone".to_string(),
            worktree: "/tmp/wt".to_string(),
            command: "echo old".to_string(),
            working_dir: "/tmp/wt".to_string(),
            env: HashMap::new(),
            started_at: chrono::Utc::now() - chrono::Duration::days(30),
            status: JobStatus::Completed,
            exit_code: Some(0),
            pid: None,
        };
        store.write_meta(&dir, &meta).unwrap();
        let removed = store.clean(chrono::Duration::days(7)).unwrap();
        assert_eq!(removed, 1);
        assert!(!dir.exists());
    }
}
```

- [ ] **Step 2: Run tests to verify they fail**

Run: `cargo test --lib log_store::tests` Expected: FAIL — types don't exist

- [ ] **Step 3: Implement `LogStore`**

In `src/coordinator/log_store.rs`:

````rust
use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::fs;
use std::path::{Path, PathBuf};

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum JobStatus {
    Running,
    Completed,
    Failed,
    Cancelled,
}

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
}

/// Manages background job log storage on disk.
///
/// Directory structure:
/// ```text
/// <base_dir>/
///   <invocation-id>/
///     <job-name>/
///       meta.json
///       output.log
/// ```
pub struct LogStore {
    base_dir: PathBuf,
}

impl LogStore {
    pub fn new(base_dir: PathBuf) -> Self {
        Self { base_dir }
    }

    /// Returns the log store for a specific repository.
    pub fn for_repo(repo_hash: &str) -> Result<Self> {
        let base = crate::daft_state_dir()?.join("jobs").join(repo_hash);
        Ok(Self::new(base))
    }

    pub fn create_job_dir(&self, invocation_id: &str, job_name: &str) -> Result<PathBuf> {
        let dir = self.base_dir.join(invocation_id).join(job_name);
        fs::create_dir_all(&dir)
            .with_context(|| format!("Failed to create job log dir: {}", dir.display()))?;
        Ok(dir)
    }

    pub fn write_meta(&self, job_dir: &Path, meta: &JobMeta) -> Result<()> {
        let path = job_dir.join("meta.json");
        let content = serde_json::to_string_pretty(meta)?;
        fs::write(&path, content)?;
        Ok(())
    }

    pub fn read_meta(&self, job_dir: &Path) -> Result<JobMeta> {
        let path = job_dir.join("meta.json");
        let content = fs::read_to_string(&path)?;
        let meta: JobMeta = serde_json::from_str(&content)?;
        Ok(meta)
    }

    pub fn log_path(job_dir: &Path) -> PathBuf {
        job_dir.join("output.log")
    }

    pub fn list_job_dirs(&self) -> Result<Vec<PathBuf>> {
        let mut dirs = Vec::new();
        if !self.base_dir.exists() {
            return Ok(dirs);
        }
        for inv_entry in fs::read_dir(&self.base_dir)? {
            let inv_entry = inv_entry?;
            if inv_entry.file_type()?.is_dir() {
                for job_entry in fs::read_dir(inv_entry.path())? {
                    let job_entry = job_entry?;
                    if job_entry.file_type()?.is_dir() {
                        dirs.push(job_entry.path());
                    }
                }
            }
        }
        Ok(dirs)
    }

    pub fn clean(&self, max_age: chrono::Duration) -> Result<usize> {
        let cutoff = chrono::Utc::now() - max_age;
        let mut removed = 0;

        for job_dir in self.list_job_dirs()? {
            if let Ok(meta) = self.read_meta(&job_dir) {
                if meta.started_at < cutoff && !matches!(meta.status, JobStatus::Running) {
                    fs::remove_dir_all(&job_dir)?;
                    removed += 1;

                    // Clean up empty invocation dir
                    if let Some(parent) = job_dir.parent() {
                        let _ = fs::remove_dir(parent); // Only succeeds if empty
                    }
                }
            }
        }

        Ok(removed)
    }
}
````

Add `pub mod coordinator` to `src/lib.rs`.

- [ ] **Step 4: Check if `chrono` dependency is needed**

Run: `grep chrono Cargo.toml`

If not present, add to `Cargo.toml`:

```toml
chrono = { version = "0.4", features = ["serde"] }
```

- [ ] **Step 5: Run tests to verify they pass**

Run: `cargo test --lib log_store::tests` Expected: PASS (all 4 tests)

- [ ] **Step 6: Commit**

```bash
git add src/coordinator/ src/lib.rs Cargo.toml Cargo.lock
git commit -m "feat(coordinator): add log store for background job persistence"
```

---

## Task 7: Coordinator process — fork, run background jobs, manage lifecycle

**Files:**

- Create: `src/coordinator/process.rs`
- Modify: `src/coordinator/mod.rs`

- [ ] **Step 1: Write failing tests for coordinator state management**

Add to `src/coordinator/process.rs`:

```rust
#[cfg(test)]
mod tests {
    use super::*;
    use crate::executor::JobSpec;
    use tempfile::TempDir;

    fn test_job(name: &str) -> JobSpec {
        JobSpec {
            name: name.to_string(),
            command: format!("echo {name}"),
            working_dir: std::env::temp_dir(),
            background: true,
            ..Default::default()
        }
    }

    #[test]
    fn test_coordinator_state_new() {
        let state = CoordinatorState::new("test-repo", "inv-1");
        assert!(state.jobs.is_empty());
        assert_eq!(state.repo_hash, "test-repo");
    }

    #[test]
    fn test_coordinator_state_add_jobs() {
        let mut state = CoordinatorState::new("test-repo", "inv-1");
        state.add_job(test_job("job-a"));
        state.add_job(test_job("job-b"));
        assert_eq!(state.jobs.len(), 2);
    }

    #[test]
    fn test_coordinator_run_jobs_to_completion() {
        let tmp = TempDir::new().unwrap();
        let store = LogStore::new(tmp.path().to_path_buf());
        let mut state = CoordinatorState::new("test-repo", "inv-1");
        state.add_job(JobSpec {
            name: "echo-job".to_string(),
            command: "echo hello".to_string(),
            working_dir: std::env::temp_dir(),
            background: true,
            ..Default::default()
        });

        let results = state.run_all(&store).unwrap();
        assert_eq!(results.len(), 1);
        assert!(results[0].status.is_terminal());

        // Verify log was written
        let meta = store.read_meta(
            &store.base_dir.join("inv-1").join("echo-job")
        ).unwrap();
        assert!(matches!(meta.status, JobStatus::Completed));
    }
}
```

- [ ] **Step 2: Run tests to verify they fail**

Run: `cargo test --lib coordinator::process::tests` Expected: FAIL — types don't
exist

- [ ] **Step 3: Implement `CoordinatorState` and job execution**

In `src/coordinator/process.rs`:

```rust
use super::log_store::{JobMeta, JobStatus, LogStore};
use crate::executor::command::run_command;
use crate::executor::{JobResult, JobSpec, NodeStatus};
use anyhow::Result;
use std::collections::HashMap;
use std::fs::OpenOptions;
use std::io::Write;
use std::sync::mpsc;
use std::time::Instant;

/// State for a coordinator process managing background jobs.
pub struct CoordinatorState {
    pub repo_hash: String,
    pub invocation_id: String,
    pub jobs: Vec<JobSpec>,
}

impl CoordinatorState {
    pub fn new(repo_hash: &str, invocation_id: &str) -> Self {
        Self {
            repo_hash: repo_hash.to_string(),
            invocation_id: invocation_id.to_string(),
            jobs: Vec::new(),
        }
    }

    pub fn add_job(&mut self, job: JobSpec) {
        self.jobs.push(job);
    }

    /// Run all background jobs, writing logs and metadata to the store.
    /// Jobs run as threads within this process.
    pub fn run_all(&self, store: &LogStore) -> Result<Vec<JobResult>> {
        let mut handles = Vec::new();
        let results = std::sync::Arc::new(std::sync::Mutex::new(Vec::new()));

        for job in &self.jobs {
            let job = job.clone();
            let inv_id = self.invocation_id.clone();
            let store_base = store.base_dir.clone();
            let results = std::sync::Arc::clone(&results);

            let handle = std::thread::spawn(move || {
                let local_store = LogStore::new(store_base);
                run_single_background_job(&job, &inv_id, &local_store, &results);
            });
            handles.push(handle);
        }

        for handle in handles {
            handle.join().ok();
        }

        let results = std::sync::Arc::try_unwrap(results)
            .unwrap_or_else(|arc| arc.lock().unwrap().clone())
            .into_inner()
            .unwrap_or_default();

        Ok(results)
    }
}

fn run_single_background_job(
    job: &JobSpec,
    invocation_id: &str,
    store: &LogStore,
    results: &std::sync::Arc<std::sync::Mutex<Vec<JobResult>>>,
) {
    let job_dir = match store.create_job_dir(invocation_id, &job.name) {
        Ok(d) => d,
        Err(e) => {
            eprintln!("Failed to create log dir for '{}': {e}", job.name);
            return;
        }
    };

    // Write initial meta
    let meta = JobMeta {
        name: job.name.clone(),
        hook_type: String::new(),
        worktree: job.working_dir.display().to_string(),
        command: job.command.clone(),
        working_dir: job.working_dir.display().to_string(),
        env: job.env.clone(),
        started_at: chrono::Utc::now(),
        status: JobStatus::Running,
        exit_code: None,
        pid: None,
    };
    store.write_meta(&job_dir, &meta).ok();

    // Set up line sender to write output to log file
    let (tx, rx) = mpsc::channel::<String>();
    let log_path = LogStore::log_path(&job_dir);
    let log_writer = std::thread::spawn(move || {
        let mut file = OpenOptions::new()
            .create(true)
            .append(true)
            .open(&log_path)
            .ok();
        for line in rx {
            if let Some(ref mut f) = file {
                writeln!(f, "{line}").ok();
            }
        }
    });

    let start = Instant::now();
    let cmd_result = run_command(
        &job.command,
        &job.env,
        &job.working_dir,
        job.timeout,
        Some(tx),
    );

    // Wait for log writer to drain
    log_writer.join().ok();

    let duration = start.elapsed();
    let (status, exit_code, node_status) = match &cmd_result {
        Ok(r) if r.success => (JobStatus::Completed, r.exit_code, NodeStatus::Succeeded),
        Ok(r) => (JobStatus::Failed, r.exit_code, NodeStatus::Failed),
        Err(_) => (JobStatus::Failed, None, NodeStatus::Failed),
    };

    // Update meta with final status
    let final_meta = JobMeta {
        status,
        exit_code,
        ..meta
    };
    store.write_meta(&job_dir, &final_meta).ok();

    // Report failure to terminal
    if matches!(node_status, NodeStatus::Failed) {
        // Best-effort write to inherited stderr
        eprintln!(
            "\x1b[31m✗\x1b[0m Background job '{}' failed (exit {}) — daft hooks jobs logs {}",
            job.name,
            exit_code.map(|c| c.to_string()).unwrap_or_else(|| "?".to_string()),
            job.name,
        );
    }

    results.lock().unwrap().push(JobResult {
        name: job.name.clone(),
        status: node_status,
        duration,
        exit_code,
        stdout: cmd_result.as_ref().map(|r| r.stdout.clone()).unwrap_or_default(),
        stderr: cmd_result.as_ref().map(|r| r.stderr.clone()).unwrap_or_default(),
    });
}

/// Fork the current process into a coordinator that runs background jobs.
///
/// The parent process returns `Ok(None)` immediately.
/// The child process runs all jobs and returns `Ok(Some(results))` when done.
///
/// # Safety
/// Uses `libc::fork()`. Only call from a single-threaded context (before
/// the thread pool is created) or after all foreground work is complete.
#[cfg(unix)]
pub fn fork_coordinator(
    state: CoordinatorState,
    store: LogStore,
) -> Result<Option<Vec<JobResult>>> {
    use std::process;

    // Safety: fork() is called after foreground work is complete,
    // so no thread-safety concerns with in-flight threads.
    let pid = unsafe { libc::fork() };

    match pid {
        -1 => anyhow::bail!("fork() failed: {}", std::io::Error::last_os_error()),
        0 => {
            // Child: become the coordinator
            // Create a new session so we don't get killed when the terminal closes
            unsafe { libc::setsid() };

            // Set the guard env var to prevent recursive background spawning
            std::env::set_var("DAFT_IS_COORDINATOR", "1");

            let results = state.run_all(&store)?;
            process::exit(0);
        }
        child_pid => {
            // Parent: report and return
            Ok(None)
        }
    }
}
```

Update `src/coordinator/mod.rs`:

```rust
pub mod log_store;
pub mod process;
```

- [ ] **Step 4: Run tests to verify they pass**

Run: `cargo test --lib coordinator::process::tests` Expected: PASS (all 3 tests)

- [ ] **Step 5: Commit**

```bash
git add src/coordinator/
git commit -m "feat(coordinator): implement coordinator state and fork-based background execution"
```

---

## Task 8: Coordinator client — Unix socket IPC for CLI commands

**Files:**

- Create: `src/coordinator/client.rs`
- Modify: `src/coordinator/mod.rs`
- Modify: `src/coordinator/process.rs` (add socket listener)

- [ ] **Step 1: Define IPC protocol types**

Add to `src/coordinator/mod.rs`:

```rust
pub mod client;
pub mod log_store;
pub mod process;

use serde::{Deserialize, Serialize};

/// Request from CLI to coordinator.
#[derive(Debug, Serialize, Deserialize)]
pub enum CoordinatorRequest {
    /// List all jobs and their current status.
    ListJobs,
    /// Cancel a specific job by name.
    CancelJob { name: String },
    /// Cancel all running jobs.
    CancelAll,
    /// Graceful shutdown of the coordinator.
    Shutdown,
}

/// Response from coordinator to CLI.
#[derive(Debug, Serialize, Deserialize)]
pub enum CoordinatorResponse {
    /// List of job statuses.
    Jobs(Vec<JobInfo>),
    /// Acknowledgement with optional message.
    Ack { message: String },
    /// Error response.
    Error { message: String },
}

/// Summary info about a background job.
#[derive(Debug, Serialize, Deserialize)]
pub struct JobInfo {
    pub name: String,
    pub hook_type: String,
    pub worktree: String,
    pub status: log_store::JobStatus,
    pub elapsed_secs: Option<u64>,
    pub exit_code: Option<i32>,
}

/// Returns the socket path for a coordinator.
pub fn coordinator_socket_path(repo_hash: &str) -> anyhow::Result<std::path::PathBuf> {
    Ok(crate::daft_state_dir()?.join(format!("coordinator-{repo_hash}.sock")))
}

/// Returns the PID file path for a coordinator.
pub fn coordinator_pid_path(repo_hash: &str) -> anyhow::Result<std::path::PathBuf> {
    Ok(crate::daft_state_dir()?.join(format!("coordinator-{repo_hash}.pid")))
}
```

- [ ] **Step 2: Implement the client**

In `src/coordinator/client.rs`:

```rust
use super::{CoordinatorRequest, CoordinatorResponse, coordinator_socket_path};
use anyhow::{Context, Result};
use std::io::{BufRead, BufReader, Write};
use std::os::unix::net::UnixStream;
use std::time::Duration;

/// Client for communicating with a running coordinator.
pub struct CoordinatorClient {
    stream: UnixStream,
}

impl CoordinatorClient {
    /// Connect to the coordinator for the given repo.
    /// Returns `None` if no coordinator is running.
    pub fn connect(repo_hash: &str) -> Result<Option<Self>> {
        let socket_path = coordinator_socket_path(repo_hash)?;
        if !socket_path.exists() {
            return Ok(None);
        }

        match UnixStream::connect(&socket_path) {
            Ok(stream) => {
                stream.set_read_timeout(Some(Duration::from_secs(5)))?;
                stream.set_write_timeout(Some(Duration::from_secs(5)))?;
                Ok(Some(Self { stream }))
            }
            Err(e) if e.kind() == std::io::ErrorKind::ConnectionRefused => {
                // Stale socket file — clean it up
                std::fs::remove_file(&socket_path).ok();
                Ok(None)
            }
            Err(e) => Err(e).context("Failed to connect to coordinator"),
        }
    }

    /// Send a request and receive the response.
    pub fn send(&mut self, request: &CoordinatorRequest) -> Result<CoordinatorResponse> {
        let mut msg = serde_json::to_string(request)?;
        msg.push('\n');
        self.stream.write_all(msg.as_bytes())?;

        let mut reader = BufReader::new(&self.stream);
        let mut response_line = String::new();
        reader.read_line(&mut response_line)?;

        let response: CoordinatorResponse = serde_json::from_str(&response_line)?;
        Ok(response)
    }

    pub fn list_jobs(&mut self) -> Result<Vec<super::JobInfo>> {
        match self.send(&CoordinatorRequest::ListJobs)? {
            CoordinatorResponse::Jobs(jobs) => Ok(jobs),
            CoordinatorResponse::Error { message } => anyhow::bail!(message),
            _ => anyhow::bail!("Unexpected response from coordinator"),
        }
    }

    pub fn cancel_job(&mut self, name: &str) -> Result<String> {
        match self.send(&CoordinatorRequest::CancelJob { name: name.to_string() })? {
            CoordinatorResponse::Ack { message } => Ok(message),
            CoordinatorResponse::Error { message } => anyhow::bail!(message),
            _ => anyhow::bail!("Unexpected response from coordinator"),
        }
    }

    pub fn cancel_all(&mut self) -> Result<String> {
        match self.send(&CoordinatorRequest::CancelAll)? {
            CoordinatorResponse::Ack { message } => Ok(message),
            CoordinatorResponse::Error { message } => anyhow::bail!(message),
            _ => anyhow::bail!("Unexpected response from coordinator"),
        }
    }
}
```

- [ ] **Step 3: Add socket listener to coordinator process**

In `src/coordinator/process.rs`, add a socket listener that runs in a separate
thread alongside job execution. The listener handles incoming
`CoordinatorRequest` messages and responds with `CoordinatorResponse`.

Add to `fork_coordinator()` child branch (after `setsid`):

```rust
// Write PID file
let pid_path = crate::coordinator::coordinator_pid_path(&state.repo_hash)?;
std::fs::create_dir_all(pid_path.parent().unwrap())?;
std::fs::write(&pid_path, process::id().to_string())?;

// Start socket listener in a separate thread
let socket_path = crate::coordinator::coordinator_socket_path(&state.repo_hash)?;
let listener_handle = start_socket_listener(&socket_path, /* shared state */);

let results = state.run_all(&store)?;

// Clean up socket and PID file
std::fs::remove_file(&socket_path).ok();
std::fs::remove_file(&pid_path).ok();
```

Implement `start_socket_listener()`:

```rust
fn start_socket_listener(
    socket_path: &Path,
    // Add shared state reference for querying job status
) -> std::thread::JoinHandle<()> {
    // Remove stale socket
    std::fs::remove_file(socket_path).ok();

    let listener = std::os::unix::net::UnixListener::bind(socket_path)
        .expect("Failed to bind coordinator socket");
    listener.set_nonblocking(true)
        .expect("Failed to set socket non-blocking");

    let socket_path = socket_path.to_path_buf();
    std::thread::spawn(move || {
        loop {
            match listener.accept() {
                Ok((stream, _)) => {
                    handle_client_connection(stream);
                }
                Err(ref e) if e.kind() == std::io::ErrorKind::WouldBlock => {
                    std::thread::sleep(std::time::Duration::from_millis(100));
                }
                Err(_) => break,
            }
        }
    })
}
```

- [ ] **Step 4: Run all coordinator tests**

Run: `cargo test --lib coordinator` Expected: PASS

- [ ] **Step 5: Commit**

```bash
git add src/coordinator/
git commit -m "feat(coordinator): add IPC client and socket listener"
```

---

## Task 9: Integrate coordinator into hook execution pipeline

**Files:**

- Modify: `src/hooks/yaml_executor/mod.rs`
- Modify: `src/hooks/executor.rs`

- [ ] **Step 1: Write integration test**

Add to `src/hooks/yaml_executor/mod.rs` tests:

```rust
#[test]
fn test_execute_with_background_jobs_partitions_correctly() {
    // This is a unit test for the integration point.
    // Verify that execute_yaml_hook_with_rc correctly partitions
    // jobs and returns a HookResult that includes background job info.
    //
    // Use a mock presenter and verify:
    // 1. Foreground jobs are executed via run_jobs()
    // 2. Background jobs are collected into a BackgroundBatch
    // 3. The HookResult indicates background jobs were dispatched
}
```

- [ ] **Step 2: Modify `execute_yaml_hook_with_rc` to partition and dispatch**

In `src/hooks/yaml_executor/mod.rs`, after line 203 (where `yaml_jobs_to_specs`
is called), insert the partitioning logic:

```rust
// Partition into foreground and background phases
let (fg_specs, bg_specs) = partition_foreground_background(&specs);

// Log promotion warnings
for bg_job in &specs {
    if bg_job.background && fg_specs.iter().any(|fg| fg.name == bg_job.name) {
        presenter.on_message(&format!(
            "⚠ Job '{}' promoted to foreground (required by a foreground job)",
            bg_job.name,
        ));
    }
}

// Run foreground jobs as before
let fg_results = crate::executor::runner::run_jobs(&fg_specs, exec_mode, presenter)?;

// If there are background jobs, dispatch to coordinator
if !bg_specs.is_empty() {
    let repo_hash = compute_repo_hash(&hook_env);
    let invocation_id = generate_invocation_id();

    // Check for DAFT_NO_BACKGROUND_JOBS — run inline instead
    if std::env::var("DAFT_NO_BACKGROUND_JOBS").is_ok() {
        let bg_results = crate::executor::runner::run_jobs(&bg_specs, exec_mode, presenter)?;
        // Merge results
        let mut all_results = fg_results;
        all_results.extend(bg_results);
        return job_results_to_hook_result(&all_results);
    }

    let store = LogStore::for_repo(&repo_hash)?;
    let mut coord_state = CoordinatorState::new(&repo_hash, &invocation_id);
    for spec in bg_specs {
        coord_state.add_job(spec);
    }

    let bg_count = coord_state.jobs.len();
    fork_coordinator(coord_state, store)?;

    presenter.on_message(&format!(
        "⟳ {bg_count} background job{} running — daft hooks jobs to manage",
        if bg_count == 1 { "" } else { "s" },
    ));
}
```

- [ ] **Step 3: Add helper functions**

```rust
fn compute_repo_hash(env: &HookEnvironment) -> String {
    use std::collections::hash_map::DefaultHasher;
    use std::hash::{Hash, Hasher};
    let project_root = env.vars().get("DAFT_PROJECT_ROOT").cloned().unwrap_or_default();
    let mut hasher = DefaultHasher::new();
    project_root.hash(&mut hasher);
    format!("{:016x}", hasher.finish())
}

fn generate_invocation_id() -> String {
    use std::time::{SystemTime, UNIX_EPOCH};
    let ts = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_millis();
    format!("{ts:016x}")
}
```

- [ ] **Step 4: Run full unit tests**

Run: `mise run test:unit` Expected: All tests pass

- [ ] **Step 5: Commit**

```bash
git add src/hooks/yaml_executor/ src/hooks/executor.rs
git commit -m "feat(hooks): integrate background job partitioning and coordinator dispatch"
```

---

## Task 10: Add `DAFT_IS_COORDINATOR` guard to `main.rs`

**Files:**

- Modify: `src/main.rs:32-65`

- [ ] **Step 1: Write failing test**

This is best tested via integration test. Add to
`tests/manual/scenarios/hooks/background-jobs.yml` (started in Task 12). For
now, verify manually:

- [ ] **Step 2: Add the guard**

In `src/main.rs`, extend the existing background task detection (line 42):

```rust
let is_background_task = std::env::args()
    .nth(1)
    .is_some_and(|a| a.starts_with("__"));

// Also skip background spawning if we're inside a coordinator process
let is_coordinator = std::env::var("DAFT_IS_COORDINATOR").is_ok();

let skip_background = is_background_task || is_coordinator;
```

Replace references to `is_background_task` with `skip_background` on lines
56-65:

```rust
let update_notification = if !skip_background {
    daft::update_check::maybe_check_for_update()
} else {
    None
};

if !skip_background {
    daft::trust_prune::maybe_prune_trust();
}
```

- [ ] **Step 3: Run clippy and tests**

Run: `mise run clippy && mise run test:unit` Expected: No warnings, all tests
pass

- [ ] **Step 4: Commit**

```bash
git add src/main.rs
git commit -m "fix(coordinator): add DAFT_IS_COORDINATOR guard to prevent recursive background spawning"
```

---

## Task 11: `daft hooks jobs` CLI command

**Files:**

- Create: `src/commands/hooks/jobs.rs`
- Modify: `src/commands/hooks/mod.rs`

- [ ] **Step 1: Add `Jobs` variant to `HooksCommand`**

In `src/commands/hooks/mod.rs`, add to the `HooksCommand` enum:

```rust
/// Manage background hook jobs.
#[command(name = "jobs")]
Jobs(JobsArgs),
```

Add to the dispatch in `run()`:

```rust
Some(HooksCommand::Jobs(args)) => jobs::run(args, &path),
```

Add module declaration:

```rust
mod jobs;
```

- [ ] **Step 2: Implement `jobs.rs` with argument parsing**

Create `src/commands/hooks/jobs.rs`:

```rust
use anyhow::Result;
use clap::{Parser, Subcommand};
use std::path::Path;

use crate::coordinator::client::CoordinatorClient;
use crate::coordinator::log_store::{JobStatus, LogStore};

#[derive(Parser, Debug)]
#[command(about = "Manage background hook jobs")]
pub struct JobsArgs {
    #[command(subcommand)]
    command: Option<JobsCommand>,

    /// Show jobs across all repositories.
    #[arg(long)]
    all_repos: bool,

    /// Filter to a specific worktree.
    #[arg(long)]
    worktree: Option<String>,

    /// Output in JSON format.
    #[arg(long)]
    json: bool,
}

#[derive(Subcommand, Debug)]
enum JobsCommand {
    /// View output log for a background job.
    Logs {
        /// Job name.
        job: String,
    },
    /// Cancel a running background job.
    Cancel {
        /// Job name (omit for --all).
        job: Option<String>,
        /// Cancel all running jobs.
        #[arg(long)]
        all: bool,
    },
    /// Re-run a failed background job.
    Retry {
        /// Job name.
        job: String,
    },
    /// Remove logs older than the retention period.
    Clean,
}

pub fn run(args: JobsArgs, repo_path: &Path) -> Result<()> {
    let repo_hash = compute_repo_hash_from_path(repo_path)?;

    match args.command {
        None => list_jobs(&repo_hash, &args),
        Some(JobsCommand::Logs { job }) => show_logs(&repo_hash, &job),
        Some(JobsCommand::Cancel { job, all }) => {
            if all {
                cancel_all(&repo_hash)
            } else if let Some(name) = job {
                cancel_job(&repo_hash, &name)
            } else {
                anyhow::bail!("Specify a job name or --all")
            }
        }
        Some(JobsCommand::Retry { job }) => retry_job(&repo_hash, &job),
        Some(JobsCommand::Clean) => clean_logs(&repo_hash),
    }
}

fn list_jobs(repo_hash: &str, args: &JobsArgs) -> Result<()> {
    let store = LogStore::for_repo(repo_hash)?;

    // First try to get live status from coordinator
    if let Some(mut client) = CoordinatorClient::connect(repo_hash)? {
        let live_jobs = client.list_jobs()?;
        // Print running jobs from coordinator
        if !live_jobs.is_empty() {
            println!("RUNNING");
            for job in &live_jobs {
                if matches!(job.status, JobStatus::Running) {
                    println!(
                        "  {:<24} {:<24} {:<16} {}",
                        job.name, job.hook_type, job.worktree,
                        format_elapsed(job.elapsed_secs),
                    );
                }
            }
            println!();
        }
    }

    // Then show historical data from log store
    let job_dirs = store.list_job_dirs()?;
    let mut completed = Vec::new();
    let mut failed = Vec::new();

    for dir in &job_dirs {
        if let Ok(meta) = store.read_meta(dir) {
            match meta.status {
                JobStatus::Completed => completed.push(meta),
                JobStatus::Failed => failed.push(meta),
                JobStatus::Cancelled => completed.push(meta),
                JobStatus::Running => {} // Handled above via coordinator
            }
        }
    }

    if !completed.is_empty() {
        println!("COMPLETED (last 24h)");
        for meta in &completed {
            println!("  {:<24} {:<24} {}", meta.name, meta.hook_type, meta.worktree);
        }
        println!();
    }

    if !failed.is_empty() {
        println!("FAILED (last 24h)");
        for meta in &failed {
            println!(
                "  {:<24} {:<24} {:<16} exit {}",
                meta.name, meta.hook_type, meta.worktree,
                meta.exit_code.map(|c| c.to_string()).unwrap_or_else(|| "?".to_string()),
            );
        }
    }

    if completed.is_empty() && failed.is_empty() {
        println!("No background jobs found.");
    }

    Ok(())
}

fn show_logs(repo_hash: &str, job_name: &str) -> Result<()> {
    let store = LogStore::for_repo(repo_hash)?;
    // Find the most recent job dir matching the name
    let job_dirs = store.list_job_dirs()?;
    let matching = job_dirs
        .iter()
        .filter(|d| d.file_name().map(|n| n.to_string_lossy().as_ref() == job_name).unwrap_or(false))
        .last();

    match matching {
        Some(dir) => {
            let log_path = LogStore::log_path(dir);
            if log_path.exists() {
                let content = std::fs::read_to_string(&log_path)?;
                print!("{content}");
            } else {
                println!("No output log found for '{job_name}'.");
            }
        }
        None => println!("No job found matching '{job_name}'."),
    }
    Ok(())
}

fn cancel_job(repo_hash: &str, name: &str) -> Result<()> {
    if let Some(mut client) = CoordinatorClient::connect(repo_hash)? {
        let msg = client.cancel_job(name)?;
        println!("{msg}");
    } else {
        println!("No coordinator running.");
    }
    Ok(())
}

fn cancel_all(repo_hash: &str) -> Result<()> {
    if let Some(mut client) = CoordinatorClient::connect(repo_hash)? {
        let msg = client.cancel_all()?;
        println!("{msg}");
    } else {
        println!("No coordinator running.");
    }
    Ok(())
}

fn retry_job(repo_hash: &str, job_name: &str) -> Result<()> {
    // Read the failed job's meta to reconstruct the job spec
    let store = LogStore::for_repo(repo_hash)?;
    let job_dirs = store.list_job_dirs()?;
    let matching = job_dirs
        .iter()
        .filter(|d| d.file_name().map(|n| n.to_string_lossy().as_ref() == job_name).unwrap_or(false))
        .last();

    match matching {
        Some(dir) => {
            let meta = store.read_meta(dir)?;
            if !matches!(meta.status, JobStatus::Failed) {
                anyhow::bail!("Job '{}' is not in failed state (status: {:?})", job_name, meta.status);
            }
            // Archive old log
            let old_log = LogStore::log_path(dir);
            if old_log.exists() {
                let archive = dir.join("output.log.prev");
                std::fs::rename(&old_log, &archive)?;
            }

            // Reconstruct JobSpec from stored meta
            let job_spec = JobSpec {
                name: meta.name.clone(),
                command: meta.command.clone(),
                working_dir: std::path::PathBuf::from(&meta.working_dir),
                env: meta.env.clone(),
                background: true,
                ..Default::default()
            };

            // Spawn a new coordinator for the retry
            let invocation_id = generate_invocation_id();
            let mut coord_state = CoordinatorState::new(repo_hash, &invocation_id);
            coord_state.add_job(job_spec);
            fork_coordinator(coord_state, store)?;
            println!("Retrying '{job_name}' — daft hooks jobs to monitor");
            Ok(())
        }
        None => anyhow::bail!("No job found matching '{job_name}'"),
    }
}

fn clean_logs(repo_hash: &str) -> Result<()> {
    let store = LogStore::for_repo(repo_hash)?;
    let removed = store.clean(chrono::Duration::days(7))?;
    println!("Removed {removed} old job log(s).");
    Ok(())
}

fn format_elapsed(secs: Option<u64>) -> String {
    match secs {
        Some(s) => format!("{}m {}s", s / 60, s % 60),
        None => "—".to_string(),
    }
}

fn compute_repo_hash_from_path(path: &Path) -> Result<String> {
    use std::collections::hash_map::DefaultHasher;
    use std::hash::{Hash, Hasher};
    let common_dir = crate::get_git_common_dir(path)?;
    let mut hasher = DefaultHasher::new();
    common_dir.to_string_lossy().hash(&mut hasher);
    Ok(format!("{:016x}", hasher.finish()))
}
```

- [ ] **Step 3: Run clippy**

Run: `mise run clippy` Expected: No warnings

- [ ] **Step 4: Run full unit tests**

Run: `mise run test:unit` Expected: All tests pass

- [ ] **Step 5: Commit**

```bash
git add src/commands/hooks/jobs.rs src/commands/hooks/mod.rs
git commit -m "feat(cli): add daft hooks jobs command for background job management"
```

---

## Task 12: Config merging for `log` section across layers

**Files:**

- Modify: `src/hooks/yaml_config_loader.rs`

- [ ] **Step 1: Write failing test**

Add to tests in `src/hooks/yaml_config_loader.rs`:

```rust
#[test]
fn test_merge_log_config() {
    let base = YamlConfig {
        log: Some(LogConfig {
            retention: Some("7d".to_string()),
            path: None,
        }),
        ..Default::default()
    };
    let overlay = YamlConfig {
        log: Some(LogConfig {
            retention: Some("14d".to_string()),
            path: None,
        }),
        ..Default::default()
    };
    let merged = merge_configs(base, overlay);
    assert_eq!(merged.log.unwrap().retention, Some("14d".to_string()));
}

#[test]
fn test_merge_log_config_overlay_partial() {
    let base = YamlConfig {
        log: Some(LogConfig {
            retention: Some("7d".to_string()),
            path: Some("/base/path".to_string()),
        }),
        ..Default::default()
    };
    let overlay = YamlConfig {
        log: Some(LogConfig {
            retention: Some("14d".to_string()),
            path: None, // Should not override base path
        }),
        ..Default::default()
    };
    let merged = merge_configs(base, overlay);
    let log = merged.log.unwrap();
    assert_eq!(log.retention, Some("14d".to_string()));
    assert_eq!(log.path, Some("/base/path".to_string()));
}
```

- [ ] **Step 2: Run tests to verify they fail**

Run: `cargo test --lib test_merge_log_config` Expected: FAIL — merge_configs
doesn't handle `log` field

- [ ] **Step 3: Add log merging to `merge_configs`**

In `src/hooks/yaml_config_loader.rs`, in the `merge_configs` function, add
handling for the `log` field:

```rust
// Merge log config (field-level merge)
log: match (base.log, overlay.log) {
    (Some(b), Some(o)) => Some(LogConfig {
        retention: o.retention.or(b.retention),
        path: o.path.or(b.path),
    }),
    (base_log, overlay_log) => overlay_log.or(base_log),
},
```

- [ ] **Step 4: Run tests to verify they pass**

Run: `cargo test --lib test_merge_log_config` Expected: PASS

- [ ] **Step 5: Commit**

```bash
git add src/hooks/yaml_config_loader.rs
git commit -m "feat(config): merge log config across configuration layers"
```

---

## Task 13: Worktree removal — cancel background jobs

**Files:**

- Modify: `src/hooks/executor.rs` (or the command that handles removal)

- [ ] **Step 1: Write test scenario**

This is best tested via a YAML integration scenario. Add cancellation logic to
the removal flow.

- [ ] **Step 2: Add cancellation to worktree removal**

Wherever the `worktree-pre-remove` hook is triggered, add before hook execution:

```rust
// Cancel any running background jobs for this worktree
fn cancel_background_jobs_for_worktree(
    repo_path: &Path,
    worktree_path: &Path,
    grace_period: Duration,
) -> Result<()> {
    let repo_hash = compute_repo_hash_from_path(repo_path)?;

    if let Some(mut client) = CoordinatorClient::connect(&repo_hash)? {
        let jobs = client.list_jobs()?;
        let wt_str = worktree_path.to_string_lossy();

        for job in jobs {
            if matches!(job.status, JobStatus::Running)
                && job.worktree == wt_str.as_ref()
            {
                eprintln!("Stopping background job '{}'...", job.name);
                client.cancel_job(&job.name)?;
            }
        }
    }

    Ok(())
}
```

Call this function in the removal command before proceeding with
`git worktree remove`.

- [ ] **Step 3: Run full unit tests**

Run: `mise run test:unit` Expected: All tests pass

- [ ] **Step 4: Commit**

```bash
git add src/hooks/executor.rs src/commands/
git commit -m "feat(hooks): cancel background jobs when removing a worktree"
```

---

## Task 14: YAML integration test scenarios

**Files:**

- Create: `tests/manual/scenarios/hooks/background-jobs.yml`

- [ ] **Step 1: Create test scenario file**

```yaml
name: background-jobs
description: Background hook jobs execute via coordinator and don't block
repos:
  - name: test-background
    default_branch: main
    branches:
      - name: main
        files:
          - path: daft.yml
            content: |
              hooks:
                worktree-post-create:
                  jobs:
                    - name: foreground-setup
                      run: echo "fg-done" > "$DAFT_WORKTREE_PATH/.fg-marker"
                    - name: background-build
                      run: |
                        sleep 1
                        echo "bg-done" > "$DAFT_WORKTREE_PATH/.bg-marker"
                      background: true
                      needs: [foreground-setup]
          - path: README.md
            content: "test repo"
      - name: feat/test-bg
        files: []

steps:
  - name: Clone the repository
    run: git-worktree-clone --layout contained $REMOTE
    expect:
      exit_code: 0
      dirs_exist:
        - $REPO_DIR

  - name: Trust the repository
    run: daft hooks trust --force
    working_dir: $REPO_DIR/main
    expect:
      exit_code: 0

  - name: Create worktree with background hooks
    run: git-worktree-checkout feat/test-bg
    working_dir: $REPO_DIR/main
    expect:
      exit_code: 0
      stdout_contains:
        - "background job"
      files_exist:
        - $REPO_DIR/feat/test-bg/.fg-marker

  - name: Wait for background job to complete
    run: |
      for i in $(seq 1 10); do
        if [ -f "$REPO_DIR/feat/test-bg/.bg-marker" ]; then
          echo "bg-marker found"
          exit 0
        fi
        sleep 1
      done
      echo "bg-marker not found after 10s"
      exit 1
    expect:
      exit_code: 0
      stdout_contains:
        - "bg-marker found"

  - name: Check hooks jobs shows history
    run: daft hooks jobs
    working_dir: $REPO_DIR/main
    expect:
      exit_code: 0
      stdout_contains:
        - "background-build"
```

- [ ] **Step 2: Run the test scenario**

Run: `mise run test:manual -- --ci background-jobs` Expected: All steps pass

- [ ] **Step 3: Commit**

```bash
git add tests/manual/scenarios/hooks/background-jobs.yml
git commit -m "test: add YAML integration tests for background hook jobs"
```

---

## Task 15: Add `DAFT_NO_BACKGROUND_JOBS` scenario test

**Files:**

- Create: `tests/manual/scenarios/hooks/background-jobs-disabled.yml`

- [ ] **Step 1: Create scenario for disabled background jobs**

```yaml
name: background-jobs-disabled
description: DAFT_NO_BACKGROUND_JOBS promotes all background jobs to foreground
repos:
  - name: test-bg-disabled
    default_branch: main
    branches:
      - name: main
        files:
          - path: daft.yml
            content: |
              hooks:
                worktree-post-create:
                  jobs:
                    - name: bg-job
                      run: echo "bg-ran" > "$DAFT_WORKTREE_PATH/.bg-ran"
                      background: true
          - path: README.md
            content: "test repo"
      - name: feat/no-bg
        files: []

steps:
  - name: Clone the repository
    run: git-worktree-clone --layout contained $REMOTE
    expect:
      exit_code: 0

  - name: Trust the repository
    run: daft hooks trust --force
    working_dir: $REPO_DIR/main
    expect:
      exit_code: 0

  - name: Create worktree with DAFT_NO_BACKGROUND_JOBS
    run: git-worktree-checkout feat/no-bg
    working_dir: $REPO_DIR/main
    env:
      DAFT_NO_BACKGROUND_JOBS: "1"
    expect:
      exit_code: 0
      # bg-job ran in foreground so marker exists immediately
      files_exist:
        - $REPO_DIR/feat/no-bg/.bg-ran
```

- [ ] **Step 2: Run the test scenario**

Run: `mise run test:manual -- --ci background-jobs-disabled` Expected: All steps
pass

- [ ] **Step 3: Commit**

```bash
git add tests/manual/scenarios/hooks/background-jobs-disabled.yml
git commit -m "test: add scenario for DAFT_NO_BACKGROUND_JOBS escape hatch"
```

---

## Task 16: Update shell completions and help output

**Files:**

- Modify: `src/commands/completions/bash.rs`
- Modify: `src/commands/completions/zsh.rs`
- Modify: `src/commands/completions/fish.rs`
- Modify: `src/commands/completions/mod.rs`
- Modify: `src/commands/docs.rs`

- [ ] **Step 1: Add `jobs` to hooks subcommand completions**

In each completion file, find the hooks subcommand completion section and add
`jobs` alongside `trust`, `status`, `validate`, `run`, etc.

In `mod.rs`, if `get_command_for_name()` lists hooks subcommands, add `jobs`.

In `docs.rs`, update `get_command_categories()` to include `jobs` under the
hooks section description.

- [ ] **Step 2: Verify completions work**

Run: `daft completions bash | grep -A5 hooks` Expected: `jobs` appears in the
completions

- [ ] **Step 3: Run clippy and format**

Run: `mise run fmt && mise run clippy` Expected: Clean

- [ ] **Step 4: Commit**

```bash
git add src/commands/completions/ src/commands/docs.rs
git commit -m "feat(completions): add hooks jobs subcommand to shell completions"
```

---

## Task 17: Regenerate man pages

**Files:**

- Modify: `man/` (generated)

- [ ] **Step 1: Regenerate man pages**

Run: `mise run man:gen`

- [ ] **Step 2: Verify man pages are up to date**

Run: `mise run man:verify` Expected: PASS

- [ ] **Step 3: Commit**

```bash
git add man/
git commit -m "docs: regenerate man pages for hooks jobs command"
```

---

## Task 18: Final verification

- [ ] **Step 1: Run formatter**

Run: `mise run fmt`

- [ ] **Step 2: Run clippy**

Run: `mise run clippy` Expected: Zero warnings

- [ ] **Step 3: Run all unit tests**

Run: `mise run test:unit` Expected: All pass

- [ ] **Step 4: Run integration tests**

Run: `mise run test:integration` Expected: All pass

- [ ] **Step 5: Run full CI simulation**

Run: `mise run ci` Expected: All checks pass
