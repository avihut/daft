# OS-Variant Run Map Implementation Plan

> **For Claude:** REQUIRED SUB-SKILL: Use superpowers:executing-plans to
> implement this plan task-by-task.

**Goal:** Replace per-OS job duplication with polymorphic `run`/`skip`/`only`
fields that accept OS-keyed maps, silently skipping jobs with no matching
platform variant.

**Architecture:** Make `run`, `skip`, and `only` on `JobDef` polymorphic via
serde `untagged` enums. Resolve the active variant early in execution (before
skip/only evaluation). Remove the top-level `os` field. Platform-skipped jobs
are invisible in output but count as satisfied for `needs` dependencies.

**Tech Stack:** Rust, serde_yaml, serde (untagged enums), HashMap

---

### Task 1: Add `RunCommand` enum and make `JobDef.run` polymorphic

**Files:**

- Modify: `src/hooks/yaml_config.rs`

**Step 1: Write failing tests for the new `run` forms**

Add these tests at the end of the `mod tests` block in `yaml_config.rs` (before
the closing `}`):

```rust
#[test]
fn test_run_simple_string() {
    let yaml = r#"
hooks:
  post-clone:
    jobs:
      - name: test
        run: echo hello
"#;
    let config: YamlConfig = serde_yaml::from_str(yaml).unwrap();
    let job = &config.hooks["post-clone"].jobs.as_ref().unwrap()[0];
    match &job.run {
        Some(RunCommand::Simple(s)) => assert_eq!(s, "echo hello"),
        other => panic!("Expected Simple, got {other:?}"),
    }
}

#[test]
fn test_run_os_map() {
    let yaml = r#"
hooks:
  post-clone:
    jobs:
      - name: install-mise
        run:
          macos: brew install mise
          linux: curl https://mise.run | sh
"#;
    let config: YamlConfig = serde_yaml::from_str(yaml).unwrap();
    let job = &config.hooks["post-clone"].jobs.as_ref().unwrap()[0];
    match &job.run {
        Some(RunCommand::Platform(map)) => {
            assert_eq!(map.len(), 2);
            match &map[&TargetOs::Macos] {
                PlatformRunCommand::Simple(s) => assert_eq!(s, "brew install mise"),
                other => panic!("Expected Simple, got {other:?}"),
            }
        }
        other => panic!("Expected Platform, got {other:?}"),
    }
}

#[test]
fn test_run_os_map_single_os() {
    let yaml = r#"
hooks:
  post-clone:
    jobs:
      - name: install-brew
        run:
          macos: /bin/bash -c "$(curl -fsSL https://example.com)"
"#;
    let config: YamlConfig = serde_yaml::from_str(yaml).unwrap();
    let job = &config.hooks["post-clone"].jobs.as_ref().unwrap()[0];
    match &job.run {
        Some(RunCommand::Platform(map)) => {
            assert_eq!(map.len(), 1);
            assert!(map.contains_key(&TargetOs::Macos));
        }
        other => panic!("Expected Platform, got {other:?}"),
    }
}
```

**Step 2: Run tests to verify they fail**

Run: `mise run test:unit -- --test yaml_config`

Expected: compilation errors because `RunCommand` and `PlatformRunCommand` don't
exist yet.

**Step 3: Add `Hash` derive to `TargetOs`**

In `src/hooks/yaml_config.rs`, change the `TargetOs` derive line from:

```rust
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
```

to:

```rust
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, Hash)]
```

**Step 4: Define `RunCommand` and `PlatformRunCommand` enums**

Add after the `PlatformConstraint` impl block (after line 150):

```rust
/// A run command that can be a simple string or OS-keyed map.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(untagged)]
pub enum RunCommand {
    /// Simple string command (runs on all platforms).
    Simple(String),
    /// OS-keyed map of commands.
    Platform(HashMap<TargetOs, PlatformRunCommand>),
}

/// A platform-specific run command (string or list).
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(untagged)]
pub enum PlatformRunCommand {
    /// Single command string.
    Simple(String),
    /// List of commands joined with " && ".
    List(Vec<String>),
}
```

**Step 5: Change `JobDef.run` type**

Change `pub run: Option<String>` to `pub run: Option<RunCommand>`.

**Step 6: Remove `os` field from `JobDef`**

Delete `pub os: Option<PlatformConstraint<TargetOs>>` from `JobDef`.

**Step 7: Add helper methods to `RunCommand`**

Add an impl block for `RunCommand`:

```rust
impl RunCommand {
    /// Resolve the command for the current OS.
    ///
    /// Returns `None` if this is a platform map with no matching OS (silent skip).
    /// Returns `Some(command_string)` for the resolved command.
    pub fn resolve_for_current_os(&self) -> Option<String> {
        match self {
            RunCommand::Simple(s) => Some(s.clone()),
            RunCommand::Platform(map) => {
                let current_os = Self::current_target_os()?;
                map.get(&current_os).map(|cmd| cmd.to_command_string())
            }
        }
    }

    /// Returns true if this is a platform map (OS-keyed).
    pub fn is_platform(&self) -> bool {
        matches!(self, RunCommand::Platform(_))
    }

    /// Get the TargetOs for the current platform.
    fn current_target_os() -> Option<TargetOs> {
        match std::env::consts::OS {
            "macos" => Some(TargetOs::Macos),
            "linux" => Some(TargetOs::Linux),
            "windows" => Some(TargetOs::Windows),
            _ => None,
        }
    }
}

impl PlatformRunCommand {
    /// Convert to a single command string.
    pub fn to_command_string(&self) -> String {
        match self {
            PlatformRunCommand::Simple(s) => s.clone(),
            PlatformRunCommand::List(cmds) => cmds.join(" && "),
        }
    }
}
```

**Step 8: Fix existing tests that use `job.run` as `Option<String>`**

Update these existing tests in `yaml_config.rs`:

- `test_minimal_config`: change `assert_eq!(jobs[0].run.as_deref(), ...)` to
  match on `RunCommand::Simple`.
- `test_command_def_to_job_def`: `CommandDef.run` stays as `Option<String>`, but
  `to_job_def()` needs to wrap it in `RunCommand::Simple`. Update
  `CommandDef::to_job_def()` accordingly.

In `CommandDef::to_job_def()`, change `run: self.run.clone()` to
`run: self.run.as_ref().map(|r| RunCommand::Simple(r.clone()))`.

Remove the old `test_os_single`, `test_os_list`, `test_os_and_arch_combined`
tests (they test the removed `os` field).

**Step 9: Run tests**

Run: `mise run test:unit -- --test yaml_config`

Expected: all tests pass.

**Step 10: Commit**

```
feat(hooks): add RunCommand enum for polymorphic run field

Replace Option<String> with Option<RunCommand> on JobDef, supporting
both simple string commands and OS-keyed maps. Remove the top-level
os field from JobDef.
```

---

### Task 2: Add `Platform` variant to `SkipCondition` and `OnlyCondition`

**Files:**

- Modify: `src/hooks/yaml_config.rs`

**Step 1: Write failing tests for OS-keyed skip/only**

```rust
#[test]
fn test_skip_platform_map() {
    let yaml = r#"
hooks:
  post-clone:
    jobs:
      - name: install-mise
        run:
          macos: brew install mise
          linux: curl https://mise.run | sh
        skip:
          macos:
            - run: "brew list mise"
              desc: mise is already installed via brew
          linux:
            - run: "command -v mise"
              desc: mise is already installed
"#;
    let config: YamlConfig = serde_yaml::from_str(yaml).unwrap();
    let job = &config.hooks["post-clone"].jobs.as_ref().unwrap()[0];
    match &job.skip {
        Some(SkipCondition::Platform(map)) => {
            assert_eq!(map.len(), 2);
            assert!(map.contains_key(&TargetOs::Macos));
            assert!(map.contains_key(&TargetOs::Linux));
        }
        other => panic!("Expected Platform, got {other:?}"),
    }
}

#[test]
fn test_only_platform_map() {
    let yaml = r#"
hooks:
  post-clone:
    jobs:
      - name: setup
        run:
          macos: echo mac
          linux: echo linux
        only:
          macos:
            - run: "test -f Brewfile"
              desc: Only when Brewfile exists
"#;
    let config: YamlConfig = serde_yaml::from_str(yaml).unwrap();
    let job = &config.hooks["post-clone"].jobs.as_ref().unwrap()[0];
    match &job.only {
        Some(OnlyCondition::Platform(map)) => {
            assert_eq!(map.len(), 1);
            assert!(map.contains_key(&TargetOs::Macos));
        }
        other => panic!("Expected Platform, got {other:?}"),
    }
}
```

**Step 2: Run tests to verify they fail**

Run: `mise run test:unit -- --test yaml_config`

**Step 3: Add `Platform` variant to `SkipCondition`**

```rust
pub enum SkipCondition {
    Bool(bool),
    EnvVar(String),
    Rules(Vec<SkipRule>),
    Platform(HashMap<TargetOs, Vec<SkipRule>>),
}
```

Important: `Platform` must come BEFORE `Rules` in the enum because serde
`untagged` tries variants in order. A HashMap will not accidentally match a Vec,
but it _could_ conflict with the string variant. The HashMap keys are objects
(TargetOs), not strings, so `Platform` should come after `EnvVar` but before
`Rules` to avoid ambiguity. Actually, since `Rules` is `Vec<SkipRule>` and
`Platform` is `HashMap<TargetOs, Vec<SkipRule>>`, serde will try each in order
-- a YAML map won't deserialize as a Vec, so the order
`Bool, EnvVar, Platform, Rules` is safe.

**Step 4: Add `Platform` variant to `OnlyCondition`**

Same pattern:

```rust
pub enum OnlyCondition {
    Bool(bool),
    EnvVar(String),
    Platform(HashMap<TargetOs, Vec<OnlyRule>>),
    Rules(Vec<OnlyRule>),
}
```

**Step 5: Run tests**

Run: `mise run test:unit -- --test yaml_config`

Expected: all pass.

**Step 6: Commit**

```
feat(hooks): add Platform variant to SkipCondition and OnlyCondition
```

---

### Task 3: Update condition evaluation for platform-aware skip/only

**Files:**

- Modify: `src/hooks/conditions.rs`

**Step 1: Write failing tests for platform-aware skip evaluation**

Add in `conditions.rs` tests:

```rust
#[test]
fn test_should_skip_platform_matching_os() {
    use super::super::yaml_config::TargetOs;
    let current_os = match std::env::consts::OS {
        "macos" => TargetOs::Macos,
        "linux" => TargetOs::Linux,
        _ => return,
    };
    let mut map = std::collections::HashMap::new();
    map.insert(
        current_os,
        vec![SkipRule::Structured(SkipRuleStructured {
            ref_pattern: None,
            env: None,
            run: Some("true".to_string()),
            desc: Some("already installed".to_string()),
        })],
    );
    let cond = SkipCondition::Platform(map);
    let info = should_skip(&cond, Path::new(".")).unwrap();
    assert_eq!(info.reason, "already installed");
}

#[test]
fn test_should_skip_platform_non_matching_os() {
    use super::super::yaml_config::TargetOs;
    let other_os = if std::env::consts::OS == "macos" {
        TargetOs::Linux
    } else {
        TargetOs::Macos
    };
    let mut map = std::collections::HashMap::new();
    map.insert(
        other_os,
        vec![SkipRule::Structured(SkipRuleStructured {
            ref_pattern: None,
            env: None,
            run: Some("true".to_string()),
            desc: Some("already installed".to_string()),
        })],
    );
    let cond = SkipCondition::Platform(map);
    // No matching OS key => no skip rules apply => should NOT skip
    assert!(should_skip(&cond, Path::new(".")).is_none());
}
```

**Step 2: Run tests to verify they fail**

Run: `mise run test:unit -- --test conditions`

**Step 3: Update `should_skip` to handle `Platform` variant**

Add a match arm to `should_skip`:

```rust
SkipCondition::Platform(map) => {
    let current_os = resolve_current_target_os();
    if let Some(os) = current_os {
        if let Some(rules) = map.get(&os) {
            for rule in rules {
                if let Some(info) = eval_skip_rule(rule, worktree) {
                    return Some(info);
                }
            }
        }
    }
    None
}
```

Add a helper at the top of `conditions.rs`:

```rust
use super::yaml_config::TargetOs;

fn resolve_current_target_os() -> Option<TargetOs> {
    match std::env::consts::OS {
        "macos" => Some(TargetOs::Macos),
        "linux" => Some(TargetOs::Linux),
        "windows" => Some(TargetOs::Windows),
        _ => None,
    }
}
```

**Step 4: Update `should_only_skip` to handle `Platform` variant**

Same pattern:

```rust
OnlyCondition::Platform(map) => {
    let current_os = resolve_current_target_os();
    if let Some(os) = current_os {
        if let Some(rules) = map.get(&os) {
            for rule in rules {
                if let Some(info) = eval_only_rule(rule, worktree) {
                    return Some(info);
                }
            }
        }
    }
    None
}
```

**Step 5: Remove `check_platform_constraints` function**

Delete the `check_platform_constraints` function entirely (lines 297-340). The
OS check is now done via `RunCommand::resolve_for_current_os()`. The arch check
remains -- extract just the arch part into a new function:

```rust
/// Check arch constraint for a job.
///
/// Returns `Some(reason)` if the current arch does not match.
pub fn check_arch_constraint(job: &JobDef) -> Option<String> {
    if let Some(ref arch_constraint) = job.arch {
        let current_arch = std::env::consts::ARCH;
        let matches = arch_constraint
            .as_slice()
            .iter()
            .any(|target| target.as_str() == current_arch);
        if !matches {
            let allowed: Vec<&str> = arch_constraint
                .as_slice()
                .iter()
                .map(|t| t.as_str())
                .collect();
            return Some(format!(
                "not on {} (current: {current_arch})",
                allowed.join("/")
            ));
        }
    }
    None
}
```

Update the tests: remove the `test_check_platform_constraints_*` tests that test
OS constraints. Keep/adapt any arch-only tests.

**Step 6: Run tests**

Run: `mise run test:unit -- --test conditions`

Expected: all pass.

**Step 7: Commit**

```
feat(hooks): platform-aware skip/only evaluation and remove OS constraint check
```

---

### Task 4: Update execution layer to use `RunCommand` resolution

**Files:**

- Modify: `src/hooks/yaml_executor/mod.rs`
- Modify: `src/hooks/yaml_executor/parallel.rs`
- Modify: `src/hooks/yaml_executor/dependency.rs`

**Step 1: Update `resolve_command` in `mod.rs`**

Change `resolve_command` to handle `RunCommand`:

```rust
pub(crate) fn resolve_command(
    job: &JobDef,
    ctx: &HookContext,
    job_name: Option<&str>,
    source_dir: &str,
) -> String {
    if let Some(ref run) = job.run {
        match run.resolve_for_current_os() {
            Some(cmd) => template::substitute(&cmd, ctx, job_name),
            None => String::new(), // platform skip handled by caller
        }
    } else if let Some(ref script) = job.script {
        // ... existing script logic unchanged ...
    } else {
        String::new()
    }
}
```

**Step 2: Add `is_platform_skip` helper**

Add a helper function in `mod.rs`:

```rust
/// Check if a job should be silently skipped due to platform mismatch.
///
/// Returns true if the job's `run` field is an OS-keyed map and the current
/// OS has no matching key. These jobs are completely invisible in output.
pub(crate) fn is_platform_skip(job: &JobDef) -> bool {
    match &job.run {
        Some(RunCommand::Platform(map)) => {
            RunCommand::current_target_os()
                .map(|os| !map.contains_key(&os))
                .unwrap_or(true)
        }
        _ => false,
    }
}
```

Add `use super::yaml_config::RunCommand;` to the imports.

**Step 3: Update `check_skip_conditions`**

Replace the `check_platform_constraints` call with `check_arch_constraint`:

```rust
pub(crate) fn check_skip_conditions(
    job: &JobDef,
    working_dir: &Path,
) -> Option<super::conditions::SkipInfo> {
    // Note: platform (OS) skip is handled separately via is_platform_skip()
    // and is completely silent. Only arch constraint produces a visible skip.
    if let Some(reason) = super::conditions::check_arch_constraint(job) {
        return Some(super::conditions::SkipInfo {
            reason,
            ran_command: false,
        });
    }
    if let Some(ref skip) = job.skip {
        if let Some(info) = super::conditions::should_skip(skip, working_dir) {
            return Some(info);
        }
    }
    if let Some(ref only) = job.only {
        if let Some(info) = super::conditions::should_only_skip(only, working_dir) {
            return Some(info);
        }
    }
    None
}
```

**Step 4: Update `execute_single_job`**

Replace the platform constraints check with the new pattern:

```rust
// Platform skip (OS-keyed run with no matching variant) — silent, invisible
if is_platform_skip(job) {
    output.debug(&format!("Platform skip job '{job_name}': no variant for current OS"));
    return Ok(HookResult::skipped("platform skip"));
}

// Arch constraint
if let Some(reason) = super::conditions::check_arch_constraint(job) {
    output.debug(&format!("Skipping job '{job_name}': {reason}"));
    return Ok(HookResult::skipped(reason));
}
```

**Step 5: Update `parallel.rs`**

In `execute_parallel`, the parallel job data collection calls
`resolve_command()` which now handles `RunCommand`. But we need to filter out
platform-skipped jobs before spawning threads. Add a pre-filter:

Before the `for job in &parallel` loop that collects `job_data`, add platform
skip filtering. Actually, the existing code doesn't handle skip at all for
parallel jobs at the job_data level -- it happens in `execute_single_job`. But
for platform skips we want them invisible. The simplest approach: in
`execute_parallel`, the jobs already go through `execute_single_job` for
interactive jobs. For parallel jobs, the skip happens when `resolve_command`
returns empty -- it's already handled by the `cmd.is_empty()` check in
`execute_single_job`. However, parallel.rs builds `ParallelJobData` directly
without going through `execute_single_job`. We need to add a platform skip check
there too.

Add at the top of the parallel job collection loop:

```rust
for job in &parallel {
    // Skip platform-mismatched jobs silently
    if is_platform_skip(job) {
        continue;
    }
    // ... existing code ...
}
```

Import `is_platform_skip` at the top of parallel.rs:

```rust
use super::{execute_single_job, is_platform_skip, resolve_command, ExecContext, ParallelJobData};
```

**Step 6: Update `dependency.rs`**

In `execute_dag_parallel` and `execute_dag_sequential`, platform-skipped jobs
need to be marked as `Skipped` in the DAG state so dependents proceed. The
existing code handles this through `check_skip_conditions` which is called from
`execute_single_job`. Since we changed `execute_single_job` to return
`HookResult::skipped("platform skip")` for platform skips, the DAG executor
already treats skipped results as "satisfied" (the dependent's in-degree
decreases). However, we need platform skips to be silent in output.

In the DAG executor's result reporting, platform skips will show up as "(skip)
platform skip". To make them silent, we need to distinguish platform skips from
regular skips. Add a `platform_skip` flag to `HookResult`:

Actually, the simplest approach: check `is_platform_skip(job)` in the DAG
executor before running the job, and if so, mark it `Skipped` without emitting
any output. Let me trace the code path more carefully.

In `execute_dag_parallel` (dependency.rs), jobs are dispatched via
`execute_single_job`. The result is collected and reported. For skipped jobs,
the renderer's `finish_job_skipped` is called. We need platform-skipped jobs to
not call `finish_job_skipped` at all.

The cleanest approach: add a `platform_skip: bool` field to `HookResult`. When
true, the renderers skip output entirely.

In `src/hooks/executor.rs`, add to `HookResult`:

```rust
pub platform_skip: bool,
```

Default it to `false` in all existing constructors. Add a new constructor:

```rust
pub fn platform_skipped() -> Self {
    Self {
        skipped: true,
        skip_reason: Some("platform skip".to_string()),
        platform_skip: true,
        ..Default::default()
    }
}
```

Update `execute_single_job` to use `HookResult::platform_skipped()` instead of
`HookResult::skipped("platform skip")`.

Then in `sequential.rs`, `parallel.rs`, and `dependency.rs`, check
`result.platform_skip` and skip output entirely when true.

**Step 7: Run all unit tests**

Run: `mise run test:unit`

Expected: all pass (fix any compilation errors from references to the removed
`os` field or `check_platform_constraints`).

**Step 8: Commit**

```
feat(hooks): integrate RunCommand resolution into execution layer

Platform-skipped jobs (no OS variant) are now completely invisible
in output. Arch constraints continue to show skip messages.
```

---

### Task 5: Update `hooks run --list` display and validation

**Files:**

- Modify: `src/commands/hooks/run_cmd.rs`
- Modify: `src/hooks/yaml_config_validate.rs`

**Step 1: Update `run_cmd.rs` display**

In `run_cmd.rs` around lines 153-164, the code displays `job.os` and `job.run`.
Replace the `os` display with showing the platform keys from the run map, and
update the run display:

```rust
// Replace the os display block with:
if let Some(ref run) = job.run {
    match run {
        RunCommand::Simple(s) => {
            output.info(&format!("     {}: {}", dim("run"), s));
        }
        RunCommand::Platform(map) => {
            let os_list: Vec<&str> = map.keys().map(|o| o.as_str()).collect();
            output.info(&format!("     {}: {}", dim("os"), os_list.join(", ")));
            for (os, cmd) in map {
                output.info(&format!(
                    "     {}.{}: {}",
                    dim("run"),
                    os.as_str(),
                    cmd.to_command_string()
                ));
            }
        }
    }
} else if let Some(ref script) = job.script {
    // ... existing script display ...
}
```

Remove the old `job.os` display block.

Add necessary imports:
`use crate::hooks::yaml_config::{RunCommand, PlatformRunCommand};`

**Step 2: Update validation**

In `yaml_config_validate.rs`, the `validate_job` function checks
`job.run.is_some()`. Since `run` is now `Option<RunCommand>`, `is_some()` still
works. The check `!has_run && !has_script && !has_group` still validates
correctly. No changes needed for basic validation.

However, if you want to validate that OS-keyed maps only contain valid
`TargetOs` keys, serde already handles that (invalid enum variants cause parse
errors).

**Step 3: Run tests**

Run: `mise run test:unit`

Expected: all pass.

**Step 4: Commit**

```
refactor(hooks): update hooks run --list display for RunCommand
```

---

### Task 6: Update project `daft.yml` to use new format

**Files:**

- Modify: `daft.yml`

**Step 1: Rewrite `daft.yml` using the new syntax**

```yaml
hooks:
  post-clone:
    jobs:
      - name: install-brew
        description: Install Homebrew package manager
        run:
          macos:
            /bin/bash -c "$(curl -fsSL
            https://raw.githubusercontent.com/Homebrew/install/HEAD/install.sh)"
        skip:
          - run: "command -v brew"
            desc: Brew is already installed
      - name: install-mise
        description: Install mise
        run:
          macos: brew install mise
          linux: curl https://mise.run | sh
        needs: [install-brew]
        skip:
          - run: "command -v mise"
            desc: mise is already installed
      - name: mise-install
        description: Install tools from mise.toml (Rust, lefthook, bun, etc.)
        run: mise install
        needs: [install-mise]
      - name: install-lefthook-in-repo
        description: Set up lefthook git hooks in the repository
        run: lefthook install
        needs: [mise-install]
        skip:
          - run: "lefthook check-install"
            desc: Lefthook hooks are already installed

  worktree-post-create:
    jobs:
      - name: mise trust
        run: mise trust
      - name: mise install
        run: mise install
        needs: [mise trust]
      - name: bun install
        run: bun install
        needs: [mise install]

  worktree-pre-remove:
    jobs: []
```

Key changes from old format:

- `install-brew`: `os: macos` + `run: ...` becomes `run: { macos: ... }`
- `install-mise-macos` and `install-mise-linux` merged into single
  `install-mise` with `run: { macos: ..., linux: ... }`
- `mise-install` `needs` simplified from
  `[install-mise-macos, install-mise-linux]` to `[install-mise]`

**Step 2: Verify it parses**

Run: `cargo run -- hooks run post-clone --list`

Expected: shows the jobs with OS info derived from the run map.

**Step 3: Commit**

```
refactor: migrate daft.yml to OS-variant run map syntax
```

---

### Task 7: Full build verification and cleanup

**Files:**

- All modified files

**Step 1: Run formatting**

Run: `mise run fmt`

**Step 2: Run clippy**

Run: `mise run clippy`

Fix any warnings.

**Step 3: Run all unit tests**

Run: `mise run test:unit`

Expected: all pass.

**Step 4: Run integration tests**

Run: `mise run test:integration`

Expected: all pass.

**Step 5: Verify man pages**

Run: `mise run man:verify`

If verification fails (unlikely since we didn't change command help text), run
`mise run man:gen` to regenerate.

**Step 6: Commit any remaining fixes**

```
chore: fix clippy warnings and formatting
```

---

## Files Modified Summary

| File                                    | Change                                             |
| --------------------------------------- | -------------------------------------------------- |
| `src/hooks/yaml_config.rs`              | Add `RunCommand`, `PlatformRunCommand` enums.      |
|                                         | Make `JobDef.run` use `RunCommand`. Remove `os`.   |
|                                         | Add `Platform` to `SkipCondition`/`OnlyCondition`. |
|                                         | Add `Hash` to `TargetOs`.                          |
| `src/hooks/conditions.rs`               | Handle `Platform` variants in `should_skip`/       |
|                                         | `should_only_skip`. Replace                        |
|                                         | `check_platform_constraints` with                  |
|                                         | `check_arch_constraint`.                           |
| `src/hooks/yaml_executor/mod.rs`        | Update `resolve_command`, `check_skip_conditions`, |
|                                         | `execute_single_job`. Add `is_platform_skip`.      |
| `src/hooks/yaml_executor/parallel.rs`   | Filter platform-skipped jobs before threading.     |
| `src/hooks/yaml_executor/dependency.rs` | Handle silent platform skips in DAG executor.      |
| `src/hooks/executor.rs`                 | Add `platform_skip` field to `HookResult`.         |
| `src/commands/hooks/run_cmd.rs`         | Update `--list` display for `RunCommand`.          |
| `src/hooks/yaml_config_validate.rs`     | Minor: validation still works, may need            |
|                                         | adjustments for `RunCommand`.                      |
| `daft.yml`                              | Migrate to new syntax.                             |
