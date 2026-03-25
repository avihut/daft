# Move Hooks Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use
> superpowers:subagent-driven-development (recommended) or
> superpowers:executing-plans to implement this plan task-by-task. Steps use
> checkbox (`- [ ]`) syntax for tracking.

**Goal:** When worktrees are moved (rename, layout transform, adopt),
selectively re-run identity-sensitive hook jobs — teardown with the old
identity, setup with the new identity — based on a `tracks` field that declares
which worktree attributes each job depends on.

**Architecture:** Add a `tracks: [path, branch]` field to `JobDef` with implicit
detection from template variable usage. Extend `HookContext` with move-specific
fields (`is_move`, `old_worktree_path`, `old_branch_name`). Build a
`MoveHookRunner` that filters tracked jobs across hook entry points and runs
teardown/setup around move operations. Integrate into rename, layout transform
executor, and adopt.

**Tech Stack:** Rust, serde (YAML deserialization), clap

**Spec:** `docs/superpowers/specs/2026-03-25-move-hooks-design.md`

---

## File Structure

### New files

| File                                                     | Responsibility                                                                                                         |
| -------------------------------------------------------- | ---------------------------------------------------------------------------------------------------------------------- |
| `src/hooks/tracking.rs`                                  | `TrackedAttribute` enum, implicit detection from template vars, effective tracking set computation                     |
| `src/hooks/move_hooks.rs`                                | `MoveHookRunner` — orchestrates the four-phase move hook flow (pre-remove, post-remove, move, pre-create, post-create) |
| `tests/manual/scenarios/rename/with-hooks.yml`           | YAML test: rename triggers tracked hook teardown/setup                                                                 |
| `tests/manual/scenarios/rename/with-hooks-branch.yml`    | YAML test: branch-tracked hooks fire on rename                                                                         |
| `tests/manual/scenarios/layout/transform-with-hooks.yml` | YAML test: layout transform triggers path-tracked hooks                                                                |

### Modified files

| File                                   | Change                                                                                    |
| -------------------------------------- | ----------------------------------------------------------------------------------------- |
| `src/hooks/yaml_config.rs`             | Add `tracks` field to `JobDef`                                                            |
| `src/hooks/yaml_config_validate.rs`    | Validate `tracks` values                                                                  |
| `src/hooks/environment.rs`             | Add `is_move`, `old_worktree_path`, `old_branch_name` to `HookContext`; emit new env vars |
| `src/hooks/template.rs`                | Add `{old_worktree_path}`, `{old_branch}` template variables                              |
| `src/hooks/executor.rs`                | Handle `is_move` in `get_hook_source_worktree`                                            |
| `src/hooks/yaml_executor/mod.rs`       | Add job filtering by tracked attributes                                                   |
| `src/hooks/mod.rs`                     | Add `pub mod tracking;` and `pub mod move_hooks;`                                         |
| `src/core/worktree/rename.rs`          | Accept `HookRunner`, call `MoveHookRunner` around filesystem move                         |
| `src/commands/worktree_branch.rs`      | Pass `HookRunner` to rename                                                               |
| `src/core/layout/transform/execute.rs` | Call `MoveHookRunner` around `MoveWorktree` ops                                           |
| `src/core/layout/transform/legacy.rs`  | Call `MoveHookRunner` in `convert_to_bare` worktree relocation                            |

---

## Task 1: `TrackedAttribute` enum and `tracks` field on `JobDef`

**Files:**

- Create: `src/hooks/tracking.rs`
- Modify: `src/hooks/yaml_config.rs:214-267` (JobDef struct)
- Modify: `src/hooks/mod.rs` (add module)
- Test: `src/hooks/yaml_config.rs` (inline unit tests)

- [ ] **Step 1: Write failing test — `tracks` field deserializes from YAML**

In `src/hooks/yaml_config.rs` tests section, add:

```rust
#[test]
fn test_tracks_field_deserializes() {
    let yaml = r#"
hooks:
  worktree-post-create:
    jobs:
      - name: mise-trust
        run: mise trust
        tracks: [path]
      - name: docker-up
        run: ./scripts/docker-up.sh
        tracks: [path, branch]
      - name: bun-install
        run: bun install
"#;
    let config: YamlConfig = serde_yaml::from_str(yaml).unwrap();
    let hook = config.hooks.get("worktree-post-create").unwrap();
    let jobs = hook.jobs.as_ref().unwrap();

    // mise-trust tracks path
    assert_eq!(
        jobs[0].tracks.as_ref().unwrap(),
        &[TrackedAttribute::Path]
    );
    // docker-up tracks both
    assert_eq!(
        jobs[1].tracks.as_ref().unwrap(),
        &[TrackedAttribute::Path, TrackedAttribute::Branch]
    );
    // bun-install has no tracks
    assert!(jobs[2].tracks.is_none());
}
```

- [ ] **Step 2: Run test to verify it fails**

Run: `cargo test test_tracks_field_deserializes -- --nocapture` Expected: FAIL —
`TrackedAttribute` does not exist

- [ ] **Step 3: Create `src/hooks/tracking.rs` with `TrackedAttribute` enum**

```rust
use serde::{Deserialize, Serialize};

/// Worktree attributes that a hook job can track.
/// When a tracked attribute changes (e.g., during rename or layout transform),
/// the job is re-run with teardown/setup semantics.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, Hash)]
#[serde(rename_all = "lowercase")]
pub enum TrackedAttribute {
    Path,
    Branch,
}
```

- [ ] **Step 4: Add `tracks` field to `JobDef` in `yaml_config.rs`**

Add to the `JobDef` struct after the `group` field (line 266):

```rust
pub tracks: Option<Vec<TrackedAttribute>>,
```

Add the import at the top of `yaml_config.rs`:

```rust
use super::tracking::TrackedAttribute;
```

- [ ] **Step 5: Register the module in `src/hooks/mod.rs`**

Add `pub mod tracking;` alongside the existing module declarations.

- [ ] **Step 6: Run test to verify it passes**

Run: `cargo test test_tracks_field_deserializes -- --nocapture` Expected: PASS

- [ ] **Step 7: Run `mise run fmt` and `mise run clippy`**

- [ ] **Step 8: Commit**

```bash
git add src/hooks/tracking.rs src/hooks/yaml_config.rs src/hooks/mod.rs
git commit -m "feat(hooks): add TrackedAttribute enum and tracks field to JobDef"
```

---

## Task 2: Validate `tracks` field

**Files:**

- Modify: `src/hooks/yaml_config_validate.rs:133` (validate_job function)
- Test: `src/hooks/yaml_config_validate.rs` (inline unit tests)

- [ ] **Step 1: Write failing test — invalid tracks value rejected**

In `src/hooks/yaml_config_validate.rs` tests section, add:

```rust
#[test]
fn test_invalid_tracks_value_rejected() {
    let yaml = r#"
hooks:
  worktree-post-create:
    jobs:
      - name: bad-job
        run: echo hello
        tracks: [path, invalid]
"#;
    // serde should reject "invalid" since TrackedAttribute only accepts path/branch
    assert!(serde_yaml::from_str::<YamlConfig>(yaml).is_err());
}

#[test]
fn test_valid_tracks_accepted() {
    let yaml = r#"
hooks:
  worktree-post-create:
    jobs:
      - name: good-job
        run: echo hello
        tracks: [path, branch]
"#;
    let config: YamlConfig = serde_yaml::from_str(yaml).unwrap();
    let result = validate_config(&config).unwrap();
    assert!(result.errors.is_empty());
}
```

- [ ] **Step 2: Run tests to verify behavior**

Run:
`cargo test test_invalid_tracks_value_rejected test_valid_tracks_accepted -- --nocapture`
Expected: Both should PASS — serde's `rename_all = "lowercase"` enum already
rejects unknown variants. If the first test fails because serde accepts unknown
values, we need explicit validation.

- [ ] **Step 3: If serde rejects automatically, commit as-is. If not, add
      validation.**

If explicit validation needed, add to `validate_job()` in
`yaml_config_validate.rs` after the existing checks:

```rust
// tracks is validated by serde deserialization (enum variants are exhaustive)
// No additional validation needed here
```

- [ ] **Step 4: Run `mise run fmt` and `mise run clippy`**

- [ ] **Step 5: Commit**

```bash
git add src/hooks/yaml_config_validate.rs
git commit -m "test(hooks): add validation tests for tracks field"
```

---

## Task 3: Implicit tracking detection from template variables

**Files:**

- Modify: `src/hooks/tracking.rs`
- Test: `src/hooks/tracking.rs` (inline unit tests)

- [ ] **Step 1: Write failing tests for implicit detection**

Add to `src/hooks/tracking.rs`:

```rust
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_detect_path_from_worktree_path_template() {
        let result = detect_tracked_attributes("mise trust {worktree_path}");
        assert!(result.contains(&TrackedAttribute::Path));
        assert!(!result.contains(&TrackedAttribute::Branch));
    }

    #[test]
    fn test_detect_branch_from_branch_template() {
        let result = detect_tracked_attributes("docker run --name {branch}");
        assert!(result.contains(&TrackedAttribute::Branch));
        assert!(!result.contains(&TrackedAttribute::Path));
    }

    #[test]
    fn test_detect_branch_from_worktree_branch_template() {
        let result = detect_tracked_attributes("echo {worktree_branch}");
        assert!(result.contains(&TrackedAttribute::Branch));
    }

    #[test]
    fn test_detect_both() {
        let result = detect_tracked_attributes("setup {worktree_path} {branch}");
        assert!(result.contains(&TrackedAttribute::Path));
        assert!(result.contains(&TrackedAttribute::Branch));
    }

    #[test]
    fn test_detect_none() {
        let result = detect_tracked_attributes("bun install");
        assert!(result.is_empty());
    }
}
```

- [ ] **Step 2: Run tests to verify they fail**

Run: `cargo test -p daft tracking::tests -- --nocapture` Expected: FAIL —
`detect_tracked_attributes` does not exist

- [ ] **Step 3: Implement `detect_tracked_attributes`**

Add to `src/hooks/tracking.rs`:

```rust
use std::collections::HashSet;

/// Scan a command string for template variables that imply tracking.
pub fn detect_tracked_attributes(command: &str) -> HashSet<TrackedAttribute> {
    let mut result = HashSet::new();

    if command.contains("{worktree_path}") {
        result.insert(TrackedAttribute::Path);
    }
    if command.contains("{branch}") || command.contains("{worktree_branch}") {
        result.insert(TrackedAttribute::Branch);
    }

    result
}
```

- [ ] **Step 4: Run tests to verify they pass**

Run: `cargo test -p daft tracking::tests -- --nocapture` Expected: PASS

- [ ] **Step 5: Add `effective_tracks` function that unions explicit and
      implicit**

```rust
use super::yaml_config::{JobDef, RunCommand, PlatformRunCommand};

/// Compute the effective tracking set for a job: union of explicit `tracks`
/// field and implicitly detected template variables in `run` strings.
pub fn effective_tracks(job: &JobDef) -> HashSet<TrackedAttribute> {
    let mut result: HashSet<TrackedAttribute> = job
        .tracks
        .as_ref()
        .map(|t| t.iter().cloned().collect())
        .unwrap_or_default();

    if let Some(ref run) = job.run {
        for command_str in run_command_strings(run) {
            result.extend(detect_tracked_attributes(&command_str));
        }
    }

    result
}

/// Extract all command strings from a RunCommand (across all platform variants).
fn run_command_strings(run: &RunCommand) -> Vec<String> {
    match run {
        RunCommand::Simple(s) => vec![s.clone()],
        RunCommand::Platform(map) => map
            .values()
            .flat_map(|prc| match prc {
                PlatformRunCommand::Simple(s) => vec![s.clone()],
                PlatformRunCommand::List(list) => list.clone(),
            })
            .collect(),
    }
}
```

- [ ] **Step 6: Write test for `effective_tracks`**

```rust
#[test]
fn test_effective_tracks_unions_explicit_and_implicit() {
    let job = JobDef {
        name: Some("test".to_string()),
        run: Some(RunCommand::Simple("setup {worktree_path}".to_string())),
        tracks: Some(vec![TrackedAttribute::Branch]),
        ..Default::default()
    };
    let result = effective_tracks(&job);
    assert!(result.contains(&TrackedAttribute::Path));   // implicit
    assert!(result.contains(&TrackedAttribute::Branch));  // explicit
}

#[test]
fn test_effective_tracks_platform_variants() {
    use std::collections::HashMap;
    use super::super::yaml_config::TargetOs;

    let mut platform = HashMap::new();
    platform.insert(
        TargetOs::Macos,
        PlatformRunCommand::List(vec![
            "docker stop {branch}".to_string(),
            "docker rm {branch}".to_string(),
        ]),
    );
    let job = JobDef {
        name: Some("docker".to_string()),
        run: Some(RunCommand::Platform(platform)),
        ..Default::default()
    };
    let result = effective_tracks(&job);
    assert!(result.contains(&TrackedAttribute::Branch));
    assert!(!result.contains(&TrackedAttribute::Path));
}
```

- [ ] **Step 7: Run all tracking tests**

Run: `cargo test -p daft tracking -- --nocapture` Expected: PASS

- [ ] **Step 8: Run `mise run fmt` and `mise run clippy`**

- [ ] **Step 9: Commit**

```bash
git add src/hooks/tracking.rs
git commit -m "feat(hooks): implicit tracking detection from template variables"
```

---

## Task 4: Extend `HookContext` with move-specific fields

**Files:**

- Modify: `src/hooks/environment.rs:10-54` (HookContext struct)
- Modify: `src/hooks/environment.rs:150-190` (from_context)
- Test: `src/hooks/environment.rs` (inline unit tests)

- [ ] **Step 1: Write failing test — move env vars present**

Add to `src/hooks/environment.rs` tests section:

```rust
#[test]
fn test_move_env_vars_set() {
    let ctx = HookContext {
        hook_type: HookType::PostCreate,
        command: "rename".to_string(),
        project_root: PathBuf::from("/project"),
        git_dir: PathBuf::from("/project/.git"),
        remote: "origin".to_string(),
        source_worktree: PathBuf::from("/project/old-wt"),
        worktree_path: PathBuf::from("/project/new-wt"),
        branch_name: "feat/new-name".to_string(),
        is_new_branch: false,
        base_branch: None,
        repository_url: None,
        default_branch: None,
        removal_reason: None,
        is_move: true,
        old_worktree_path: Some(PathBuf::from("/project/old-wt")),
        old_branch_name: Some("feat/old-name".to_string()),
    };
    let env = HookEnvironment::from_context(&ctx);
    assert_eq!(env.vars.get("DAFT_IS_MOVE").unwrap(), "true");
    assert_eq!(
        env.vars.get("DAFT_OLD_WORKTREE_PATH").unwrap(),
        "/project/old-wt"
    );
    assert_eq!(
        env.vars.get("DAFT_OLD_BRANCH_NAME").unwrap(),
        "feat/old-name"
    );
}

#[test]
fn test_non_move_has_no_move_vars() {
    let ctx = HookContext {
        hook_type: HookType::PostCreate,
        command: "checkout".to_string(),
        project_root: PathBuf::from("/project"),
        git_dir: PathBuf::from("/project/.git"),
        remote: "origin".to_string(),
        source_worktree: PathBuf::from("/project/src-wt"),
        worktree_path: PathBuf::from("/project/new-wt"),
        branch_name: "feat/new".to_string(),
        is_new_branch: true,
        base_branch: None,
        repository_url: None,
        default_branch: None,
        removal_reason: None,
        is_move: false,
        old_worktree_path: None,
        old_branch_name: None,
    };
    let env = HookEnvironment::from_context(&ctx);
    assert!(!env.vars.contains_key("DAFT_IS_MOVE"));
    assert!(!env.vars.contains_key("DAFT_OLD_WORKTREE_PATH"));
    assert!(!env.vars.contains_key("DAFT_OLD_BRANCH_NAME"));
}
```

- [ ] **Step 2: Run tests to verify they fail**

Run:
`cargo test test_move_env_vars_set test_non_move_has_no_move_vars -- --nocapture`
Expected: FAIL — fields don't exist on `HookContext`

- [ ] **Step 3: Add fields to `HookContext`**

Add after `removal_reason` (line 53) in the `HookContext` struct:

```rust
/// Whether this hook is executing as part of a move operation.
pub is_move: bool,
/// The worktree path before the move (set in all four move phases).
pub old_worktree_path: Option<PathBuf>,
/// The branch name before the move (set in all four move phases).
pub old_branch_name: Option<String>,
/// During move hooks, the set of changed attributes for job filtering.
/// Set by MoveHookRunner; None during normal (non-move) hook execution.
pub changed_attributes: Option<HashSet<TrackedAttribute>>,
```

Add the import: `use crate::hooks::tracking::TrackedAttribute;` and
`use std::collections::HashSet;`.

- [ ] **Step 4: Update `HookContext::new()` and all construction sites**

First, update the `HookContext::new()` constructor method to initialize the new
fields with defaults (`is_move: false`, `old_worktree_path: None`,
`old_branch_name: None`). Then update every place that creates a `HookContext`
via struct literal `HookContext { ... }` to include:

```rust
is_move: false,
old_worktree_path: None,
old_branch_name: None,
changed_attributes: None,
```

Key locations (use `grep -rn "HookContext" src/` to find all sites):

- `HookContext::new()` in `src/hooks/environment.rs`
- `src/core/worktree/checkout_branch.rs` (lines ~116, ~173)
- `src/core/worktree/prune.rs` (multiple sites)
- `src/core/worktree/branch_delete.rs` (multiple sites)
- `src/core/layout/transform/legacy.rs` (lines ~256, ~266)
- `src/hooks/executor.rs` (test code)
- `src/hooks/environment.rs` (test code)

Note: All `HookContext` struct literals in this plan's test code also need
`changed_attributes: None`. The code snippets in later tasks omit it for brevity
— add it when implementing.

- [ ] **Step 5: Emit new env vars in `HookEnvironment::from_context`**

Add after the existing optional var handling (around line 190):

```rust
if ctx.is_move {
    vars.insert("DAFT_IS_MOVE".to_string(), "true".to_string());
    if let Some(ref old_path) = ctx.old_worktree_path {
        vars.insert(
            "DAFT_OLD_WORKTREE_PATH".to_string(),
            old_path.display().to_string(),
        );
    }
    if let Some(ref old_branch) = ctx.old_branch_name {
        vars.insert(
            "DAFT_OLD_BRANCH_NAME".to_string(),
            old_branch.clone(),
        );
    }
}
```

- [ ] **Step 6: Run tests to verify they pass**

Run: `cargo test -p daft environment -- --nocapture` Expected: PASS

- [ ] **Step 7: Run `mise run fmt` and `mise run clippy`**

- [ ] **Step 8: Commit**

```bash
git add src/hooks/environment.rs src/core/worktree/checkout_branch.rs \
  src/core/worktree/prune.rs src/core/worktree/branch_delete.rs \
  src/core/layout/transform/legacy.rs src/hooks/executor.rs
git commit -m "feat(hooks): add move-specific fields to HookContext"
```

---

## Task 5: Add move template variables

**Files:**

- Modify: `src/hooks/template.rs:22-50` (substitute function)
- Test: `src/hooks/template.rs` (inline unit tests)

- [ ] **Step 1: Write failing test — old template vars substituted**

Add to `src/hooks/template.rs` tests section:

```rust
#[test]
fn test_old_template_vars_during_move() {
    let ctx = HookContext {
        hook_type: HookType::PostCreate,
        command: "rename".to_string(),
        project_root: PathBuf::from("/project"),
        git_dir: PathBuf::from("/project/.git"),
        remote: "origin".to_string(),
        source_worktree: PathBuf::from("/project/old-wt"),
        worktree_path: PathBuf::from("/project/new-wt"),
        branch_name: "feat/new".to_string(),
        is_new_branch: false,
        base_branch: None,
        repository_url: None,
        default_branch: None,
        removal_reason: None,
        is_move: true,
        old_worktree_path: Some(PathBuf::from("/project/old-wt")),
        old_branch_name: Some("feat/old".to_string()),
    };
    let result = substitute(
        "from {old_worktree_path} to {worktree_path} branch {old_branch}",
        &ctx,
        None,
    );
    assert_eq!(result, "from /project/old-wt to /project/new-wt branch feat/old");
}

#[test]
fn test_old_template_vars_empty_when_not_move() {
    let ctx = HookContext {
        hook_type: HookType::PostCreate,
        command: "checkout".to_string(),
        project_root: PathBuf::from("/project"),
        git_dir: PathBuf::from("/project/.git"),
        remote: "origin".to_string(),
        source_worktree: PathBuf::from("/project/src"),
        worktree_path: PathBuf::from("/project/wt"),
        branch_name: "feat/x".to_string(),
        is_new_branch: true,
        base_branch: None,
        repository_url: None,
        default_branch: None,
        removal_reason: None,
        is_move: false,
        old_worktree_path: None,
        old_branch_name: None,
    };
    let result = substitute("old={old_worktree_path} branch={old_branch}", &ctx, None);
    assert_eq!(result, "old= branch=");
}
```

- [ ] **Step 2: Run tests to verify they fail**

Run: `cargo test test_old_template_vars -- --nocapture` Expected: FAIL —
templates not substituted

- [ ] **Step 3: Add template substitutions to `substitute()`**

Add to the `substitute` function after the existing replacements:

```rust
// Move-specific templates
let old_path = ctx
    .old_worktree_path
    .as_ref()
    .map(|p| p.display().to_string())
    .unwrap_or_default();
let old_branch = ctx.old_branch_name.as_deref().unwrap_or_default();
result = result.replace("{old_worktree_path}", &old_path);
result = result.replace("{old_branch}", old_branch);
```

- [ ] **Step 4: Run tests to verify they pass**

Run: `cargo test -p daft template -- --nocapture` Expected: PASS

- [ ] **Step 5: Run `mise run fmt` and `mise run clippy`**

- [ ] **Step 6: Commit**

```bash
git add src/hooks/template.rs
git commit -m "feat(hooks): add old_worktree_path and old_branch template vars"
```

---

## Task 6: Handle `is_move` in hook executor's source worktree resolution

**Files:**

- Modify: `src/hooks/executor.rs:440-451` (get_hook_source_worktree)
- Modify: `src/hooks/environment.rs:220-226` (working_directory)
- Test: `src/hooks/executor.rs` (inline unit tests)

Note: `get_hook_source_worktree` is currently an instance method on
`HookExecutor` (`fn get_hook_source_worktree(&self, ctx: &HookContext)`). Since
it only reads `ctx` and does not use `self`, extract it to a
`pub(crate) fn get_hook_source_worktree(ctx: &HookContext) -> PathBuf` free
function in `executor.rs`. Update the one call site inside
`HookExecutor::execute` to call the free function instead. This makes the logic
testable without constructing a `HookExecutor`.

- [ ] **Step 1: Extract `get_hook_source_worktree` to a free function**

Move the method body out of `impl HookExecutor` and make it
`pub(crate) fn get_hook_source_worktree(ctx: &HookContext) -> PathBuf`. Update
the call inside `execute()` from `self.get_hook_source_worktree(ctx)` to
`get_hook_source_worktree(ctx)`.

- [ ] **Step 2: Write failing test — move post-remove uses worktree_path**

Add to `src/hooks/executor.rs` tests section:

```rust
#[test]
fn test_move_post_remove_uses_worktree_path() {
    let ctx = HookContext {
        hook_type: HookType::PostRemove,
        command: "rename".to_string(),
        project_root: PathBuf::from("/project"),
        git_dir: PathBuf::from("/project/.git"),
        remote: "origin".to_string(),
        source_worktree: PathBuf::from("/project/source"),
        worktree_path: PathBuf::from("/project/old-wt"),
        branch_name: "feat/old".to_string(),
        is_new_branch: false,
        base_branch: None,
        repository_url: None,
        default_branch: None,
        removal_reason: None,
        is_move: true,
        old_worktree_path: Some(PathBuf::from("/project/old-wt")),
        old_branch_name: Some("feat/old".to_string()),
    };
    let result = get_hook_source_worktree(&ctx);
    // During move, post-remove should read from worktree_path (still exists)
    assert_eq!(result, PathBuf::from("/project/old-wt"));
}

#[test]
fn test_non_move_post_remove_uses_source_worktree() {
    let ctx = HookContext {
        hook_type: HookType::PostRemove,
        command: "prune".to_string(),
        project_root: PathBuf::from("/project"),
        git_dir: PathBuf::from("/project/.git"),
        remote: "origin".to_string(),
        source_worktree: PathBuf::from("/project/source"),
        worktree_path: PathBuf::from("/project/deleted-wt"),
        branch_name: "feat/gone".to_string(),
        is_new_branch: false,
        base_branch: None,
        repository_url: None,
        default_branch: None,
        removal_reason: None,
        is_move: false,
        old_worktree_path: None,
        old_branch_name: None,
    };
    let result = get_hook_source_worktree(&ctx);
    // Normal remove: target is deleted, fall back to source
    assert_eq!(result, PathBuf::from("/project/source"));
}
```

- [ ] **Step 3: Run tests to verify the first fails**

Run: `cargo test test_move_post_remove test_non_move_post_remove -- --nocapture`
Expected: First FAIL (returns source_worktree), second PASS

- [ ] **Step 4: Update `get_hook_source_worktree`**

Replace the `PostRemove` arm:

```rust
HookType::PostRemove => {
    if ctx.is_move {
        // During move, worktree still exists at old path
        ctx.worktree_path.clone()
    } else {
        // Normal remove: target is deleted, read from source
        ctx.source_worktree.clone()
    }
}
```

- [ ] **Step 5: Run tests to verify both pass**

Run: `cargo test test_move_post_remove test_non_move_post_remove -- --nocapture`
Expected: PASS

- [ ] **Step 6: Update `working_directory` for move pre-create**

In `src/hooks/environment.rs`, the `working_directory` method (lines 220-226)
returns `&ctx.source_worktree` for `PreCreate`. During a move, the new worktree
path already exists (it was just moved there), so `working_directory` should
return `&ctx.worktree_path` when `ctx.is_move` is true:

```rust
pub fn working_directory<'a>(ctx: &'a HookContext) -> &'a Path {
    match ctx.hook_type {
        HookType::PreCreate if !ctx.is_move => &ctx.source_worktree,
        _ => &ctx.worktree_path,
    }
}
```

- [ ] **Step 7: Run `mise run fmt` and `mise run clippy`**

- [ ] **Step 8: Commit**

```bash
git add src/hooks/executor.rs src/hooks/environment.rs
git commit -m "feat(hooks): handle is_move in get_hook_source_worktree and working_directory"
```

---

## Task 7: Job filtering by tracked attributes in YAML executor

**Files:**

- Modify: `src/hooks/yaml_executor/mod.rs`
- Test: `src/hooks/yaml_executor/mod.rs` (inline unit tests)

- [ ] **Step 1: Write failing test — filter jobs by changed attributes**

Add to `src/hooks/yaml_executor/mod.rs` tests (or a new test module):

```rust
#[cfg(test)]
mod tracking_filter_tests {
    use super::*;
    use crate::hooks::tracking::TrackedAttribute;
    use crate::hooks::yaml_config::{JobDef, RunCommand};
    use std::collections::HashSet;

    #[test]
    fn test_filter_jobs_by_changed_path() {
        let jobs = vec![
            JobDef {
                name: Some("path-job".to_string()),
                run: Some(RunCommand::Simple("mise trust".to_string())),
                tracks: Some(vec![TrackedAttribute::Path]),
                ..Default::default()
            },
            JobDef {
                name: Some("branch-job".to_string()),
                run: Some(RunCommand::Simple("docker up".to_string())),
                tracks: Some(vec![TrackedAttribute::Branch]),
                ..Default::default()
            },
            JobDef {
                name: Some("untracked".to_string()),
                run: Some(RunCommand::Simple("bun install".to_string())),
                ..Default::default()
            },
        ];

        let changed = HashSet::from([TrackedAttribute::Path]);
        let filtered = filter_tracked_jobs(&jobs, &changed);
        assert_eq!(filtered.len(), 1);
        assert_eq!(filtered[0].name.as_deref(), Some("path-job"));
    }

    #[test]
    fn test_filter_includes_implicit_tracking() {
        let jobs = vec![
            JobDef {
                name: Some("implicit-path".to_string()),
                run: Some(RunCommand::Simple("direnv allow {worktree_path}".to_string())),
                ..Default::default()
            },
        ];

        let changed = HashSet::from([TrackedAttribute::Path]);
        let filtered = filter_tracked_jobs(&jobs, &changed);
        assert_eq!(filtered.len(), 1);
    }

    #[test]
    fn test_filter_pulls_in_needs_dependencies() {
        let jobs = vec![
            JobDef {
                name: Some("dep".to_string()),
                run: Some(RunCommand::Simple("mise install".to_string())),
                ..Default::default()
            },
            JobDef {
                name: Some("tracked".to_string()),
                run: Some(RunCommand::Simple("mise trust".to_string())),
                tracks: Some(vec![TrackedAttribute::Path]),
                needs: Some(vec!["dep".to_string()]),
                ..Default::default()
            },
        ];

        let changed = HashSet::from([TrackedAttribute::Path]);
        let filtered = filter_tracked_jobs(&jobs, &changed);
        assert_eq!(filtered.len(), 2);
        let names: Vec<_> = filtered.iter().map(|j| j.name.as_deref().unwrap()).collect();
        assert!(names.contains(&"dep"));
        assert!(names.contains(&"tracked"));
    }
}
```

- [ ] **Step 2: Run tests to verify they fail**

Run: `cargo test tracking_filter_tests -- --nocapture` Expected: FAIL —
`filter_tracked_jobs` does not exist

- [ ] **Step 3: Implement `filter_tracked_jobs`**

Add to `src/hooks/yaml_executor/mod.rs`:

```rust
use crate::hooks::tracking::{effective_tracks, TrackedAttribute};
use std::collections::HashSet;

/// Filter jobs to those whose effective tracking set intersects with the
/// changed attributes, plus any jobs they depend on via `needs`.
pub fn filter_tracked_jobs(
    jobs: &[JobDef],
    changed: &HashSet<TrackedAttribute>,
) -> Vec<JobDef> {
    // 1. Find directly tracked jobs
    let mut selected_names: HashSet<String> = HashSet::new();
    for job in jobs {
        let tracks = effective_tracks(job);
        if !tracks.is_disjoint(changed) {
            if let Some(ref name) = job.name {
                selected_names.insert(name.clone());
            }
        }
    }

    // 2. Pull in needs dependencies (transitive)
    let mut made_progress = true;
    while made_progress {
        made_progress = false;
        for job in jobs {
            if let Some(ref name) = job.name {
                if selected_names.contains(name) {
                    if let Some(ref needs) = job.needs {
                        for dep in needs {
                            if selected_names.insert(dep.clone()) {
                                made_progress = true;
                            }
                        }
                    }
                }
            }
        }
    }

    // 3. Return selected jobs in original order
    jobs.iter()
        .filter(|job| {
            job.name
                .as_ref()
                .map(|n| selected_names.contains(n))
                .unwrap_or(false)
        })
        .cloned()
        .collect()
}
```

- [ ] **Step 4: Run tests to verify they pass**

Run: `cargo test tracking_filter_tests -- --nocapture` Expected: PASS

- [ ] **Step 5: Run `mise run fmt` and `mise run clippy`**

- [ ] **Step 6: Commit**

```bash
git add src/hooks/yaml_executor/mod.rs
git commit -m "feat(hooks): add filter_tracked_jobs for move hook job selection"
```

---

## Task 8: `MoveHookRunner` — orchestrate the four-phase move hook flow

**Files:**

- Create: `src/hooks/move_hooks.rs`
- Modify: `src/hooks/mod.rs` (add module)
- Test: `src/hooks/move_hooks.rs` (inline unit tests)

- [ ] **Step 1: Define the `MoveHookRunner` interface**

Create `src/hooks/move_hooks.rs`:

```rust
use std::collections::HashSet;
use std::path::{Path, PathBuf};

use crate::core::{HookRunner, ProgressSink};
use crate::hooks::environment::{HookContext, HookType};
use crate::hooks::tracking::TrackedAttribute;

/// Parameters describing a worktree move for hook purposes.
pub struct MoveHookParams {
    pub old_worktree_path: PathBuf,
    pub new_worktree_path: PathBuf,
    pub old_branch_name: String,
    pub new_branch_name: String,
    pub project_root: PathBuf,
    pub git_dir: PathBuf,
    pub remote: String,
    pub source_worktree: PathBuf,
    pub command: String,
    pub changed_attributes: HashSet<TrackedAttribute>,
}

/// Run teardown hooks (pre-remove + post-remove) for tracked jobs with old identity.
pub fn run_teardown_hooks(
    params: &MoveHookParams,
    sink: &mut (impl ProgressSink + HookRunner),
) {
    for hook_type in [HookType::PreRemove, HookType::PostRemove] {
        let ctx = HookContext {
            hook_type,
            command: params.command.clone(),
            project_root: params.project_root.clone(),
            git_dir: params.git_dir.clone(),
            remote: params.remote.clone(),
            source_worktree: params.source_worktree.clone(),
            worktree_path: params.old_worktree_path.clone(),
            branch_name: params.old_branch_name.clone(),
            is_new_branch: false,
            base_branch: None,
            repository_url: None,
            default_branch: None,
            removal_reason: None,
            is_move: true,
            old_worktree_path: Some(params.old_worktree_path.clone()),
            old_branch_name: Some(params.old_branch_name.clone()),
        };
        if let Err(e) = sink.run_hook(&ctx) {
            sink.on_warning(&format!(
                "Move {} hook failed: {e}",
                hook_type.yaml_name()
            ));
        }
    }
}

/// Run setup hooks (pre-create + post-create) for tracked jobs with new identity.
pub fn run_setup_hooks(
    params: &MoveHookParams,
    sink: &mut (impl ProgressSink + HookRunner),
) {
    for hook_type in [HookType::PreCreate, HookType::PostCreate] {
        let ctx = HookContext {
            hook_type,
            command: params.command.clone(),
            project_root: params.project_root.clone(),
            git_dir: params.git_dir.clone(),
            remote: params.remote.clone(),
            source_worktree: params.source_worktree.clone(),
            worktree_path: params.new_worktree_path.clone(),
            branch_name: params.new_branch_name.clone(),
            is_new_branch: false,
            base_branch: None,
            repository_url: None,
            default_branch: None,
            removal_reason: None,
            is_move: true,
            old_worktree_path: Some(params.old_worktree_path.clone()),
            old_branch_name: Some(params.old_branch_name.clone()),
        };
        if let Err(e) = sink.run_hook(&ctx) {
            sink.on_warning(&format!(
                "Move {} hook failed: {e}",
                hook_type.yaml_name()
            ));
        }
    }
}
```

- [ ] **Step 2: Register module in `src/hooks/mod.rs`**

Add: `pub mod move_hooks;`

- [ ] **Step 3: Run `mise run clippy` to verify it compiles**

- [ ] **Step 4: Wire tracked job filtering into the executor**

The `HookExecutor` needs to filter jobs when `ctx.is_move` is true. In
`src/hooks/executor.rs`, in the `try_yaml_hook` method (around line 295 where
`execute_yaml_hook_with_rc` is called), the `changed_attributes` need to be
passed through. The simplest approach: add the changed attributes to
`HookContext` or pass them as a separate parameter.

Add to `HookContext`:

```rust
/// During move hooks, the set of changed attributes for job filtering.
pub changed_attributes: Option<HashSet<TrackedAttribute>>,
```

Update all existing construction sites to add `changed_attributes: None`.

In `execute_yaml_hook_with_rc` in `yaml_executor/mod.rs`, before building
`JobSpec`s (around line 193), add filtering logic:

```rust
let effective_jobs = if let Some(ref changed) = ctx.changed_attributes {
    filter_tracked_jobs(&jobs, changed)
} else {
    jobs
};
```

Then use `effective_jobs` instead of `jobs` for the rest of the function.

- [ ] **Step 5: Update `MoveHookParams` to pass `changed_attributes` through
      context**

Update `run_teardown_hooks` and `run_setup_hooks` to set
`changed_attributes: Some(params.changed_attributes.clone())` on the
`HookContext`.

- [ ] **Step 6: Run `mise run fmt` and `mise run clippy`**

- [ ] **Step 7: Commit**

```bash
git add src/hooks/move_hooks.rs src/hooks/mod.rs src/hooks/executor.rs \
  src/hooks/yaml_executor/mod.rs src/hooks/environment.rs
git commit -m "feat(hooks): add MoveHookRunner for four-phase move hook flow"
```

---

## Task 9: Integrate move hooks into rename

**Files:**

- Modify: `src/core/worktree/rename.rs:70` (execute function signature)
- Modify: `src/commands/worktree_branch.rs:440` (caller)
- Test: `tests/manual/scenarios/rename/with-hooks.yml`
- Test: `tests/manual/scenarios/rename/with-hooks-branch.yml`

- [ ] **Step 1: Write YAML test scenario — rename with path-tracked hooks**

Create `tests/manual/scenarios/rename/with-hooks.yml`:

```yaml
name: Rename with path-tracked hooks
description: Path-tracked hooks run teardown/setup when worktree is renamed

repos:
  - name: test-hooks-rename
    default_branch: main
    branches:
      - name: main
        files:
          - path: README.md
            content: "# Hooks rename test"
        commits:
          - message: "Initial commit"
      - name: feature/old-name
        from: main
    daft_yml: |
      hooks:
        worktree-post-create:
          jobs:
            - name: mark-path
              run: echo "{worktree_path}" > "{worktree_path}/.path-marker"
        worktree-pre-remove:
          jobs:
            - name: clean-marker
              run: rm -f "{worktree_path}/.path-marker"
              tracks: [path]

steps:
  - name: Clone and trust hooks
    run: |
      git-worktree-clone --layout contained $REMOTE_TEST_HOOKS_RENAME
      cd $WORK_DIR/test-hooks-rename/main
      daft hooks trust --force
    expect:
      exit_code: 0

  - name: Checkout feature branch (hooks run)
    run: git-worktree-checkout feature/old-name 2>&1
    cwd: "$WORK_DIR/test-hooks-rename/main"
    expect:
      exit_code: 0
      files_exist:
        - "$WORK_DIR/test-hooks-rename/feature/old-name/.path-marker"

  - name: Rename the branch
    run: git-worktree-branch -m feature/old-name feature/new-name 2>&1
    cwd: "$WORK_DIR/test-hooks-rename/main"
    expect:
      exit_code: 0
      dirs_exist:
        - "$WORK_DIR/test-hooks-rename/feature/new-name"
      files_exist:
        - "$WORK_DIR/test-hooks-rename/feature/new-name/.path-marker"
```

- [ ] **Step 2: Run the test to verify it fails**

Run: `mise run test:manual -- --ci rename:with-hooks` Expected: FAIL — rename
doesn't run hooks yet

- [ ] **Step 3: Update `rename::execute` to accept `HookRunner`**

In `src/core/worktree/rename.rs`, change the `execute` function signature:

```rust
pub fn execute(
    params: &RenameParams,
    sink: &mut (impl ProgressSink + HookRunner),
) -> Result<RenameResult> {
```

Add imports:

```rust
use crate::core::HookRunner;
use crate::hooks::move_hooks::{run_teardown_hooks, run_setup_hooks, MoveHookParams};
use crate::hooks::tracking::TrackedAttribute;
use std::collections::HashSet;
```

- [ ] **Step 4: Inject move hooks around the filesystem move**

In the `execute` function, between the branch rename (step 7, ~line 187) and the
worktree move (~line 208), determine changed attributes and run teardown:

**Important:** The variable names below are illustrative — you MUST read
`rename::execute` and use the actual local variable names. Key mappings to look
for:

- Old branch name: the resolved branch (result of `resolve_source()`), NOT
  `params.source` which may be a filesystem path
- Git dir: bound as `_git_dir` in current code — rename to `git_dir` (remove
  underscore prefix since it's now used)
- Source worktree: not currently in scope — use the old worktree path (the
  worktree being moved is the source context for hook execution)
- Project root: look for `get_git_common_dir()` or similar

```rust
// Determine what changed for move hooks
let mut changed_attributes = HashSet::new();
if old_path != new_path {
    changed_attributes.insert(TrackedAttribute::Path);
}
changed_attributes.insert(TrackedAttribute::Branch); // branch always changes in rename

let move_params = MoveHookParams {
    old_worktree_path: old_path.clone(),
    new_worktree_path: new_path.clone(),
    old_branch_name: old_branch.clone(),   // resolved branch name, NOT params.source
    new_branch_name: params.new_branch.clone(),
    project_root: project_root.clone(),
    git_dir: git_dir.clone(),              // rename _git_dir to git_dir
    remote: remote_name.to_string(),
    source_worktree: old_path.clone(),     // old worktree is the source context
    command: "rename".to_string(),
    changed_attributes: changed_attributes.clone(),
};
run_teardown_hooks(&move_params, sink);
```

After the worktree move completes, run setup:

```rust
run_setup_hooks(&move_params, sink);
```

- [ ] **Step 5: Update the caller in `worktree_branch.rs`**

In `src/commands/worktree_branch.rs`, the `run_rename_inner` function creates an
`OutputSink` and passes it to `rename::execute`. Since `OutputSink` does not
implement `HookRunner`, switch to `CommandBridge` (same pattern used in
`run_checkout_inner` in the checkout command). `CommandBridge` wraps an
`HookExecutor` and implements both `ProgressSink + HookRunner`. Find an existing
example by searching for `CommandBridge::new` in `src/commands/` and replicate
the pattern:

```rust
let executor = HookExecutor::new(/* trust_db, settings */);
let mut bridge = CommandBridge::new(output, executor);
rename::execute(&params, &mut bridge)?;
```

The exact constructor args depend on how other commands create `CommandBridge` —
match the checkout or prune command pattern.

- [ ] **Step 6: Run the YAML test**

Run: `mise run test:manual -- --ci rename:with-hooks` Expected: PASS

- [ ] **Step 7: Write branch-tracked hook test**

Create `tests/manual/scenarios/rename/with-hooks-branch.yml`:

```yaml
name: Rename with branch-tracked hooks
description: Branch-tracked hooks fire teardown and setup on rename

repos:
  - name: test-branch-hooks
    default_branch: main
    branches:
      - name: main
        files:
          - path: README.md
            content: "# Branch hooks test"
        commits:
          - message: "Initial commit"
      - name: feature/alpha
        from: main
    daft_yml: |
      hooks:
        worktree-post-create:
          jobs:
            - name: record-branch
              run: echo "{branch}" > "{worktree_path}/.branch-marker"
              tracks: [branch]
        worktree-pre-remove:
          jobs:
            - name: clean-branch
              run: rm -f "{worktree_path}/.branch-marker"
              tracks: [branch]

steps:
  - name: Clone and trust hooks
    run: |
      git-worktree-clone --layout contained $REMOTE_TEST_BRANCH_HOOKS
      cd $WORK_DIR/test-branch-hooks/main
      daft hooks trust --force
    expect:
      exit_code: 0

  - name: Checkout and verify marker
    run: git-worktree-checkout feature/alpha 2>&1
    cwd: "$WORK_DIR/test-branch-hooks/main"
    expect:
      exit_code: 0

  - name: Rename and verify new branch marker
    run: |
      git-worktree-branch -m feature/alpha feature/beta 2>&1
      cat "$WORK_DIR/test-branch-hooks/feature/beta/.branch-marker"
    cwd: "$WORK_DIR/test-branch-hooks/main"
    expect:
      exit_code: 0
      output_contains:
        - "feature/beta"
```

- [ ] **Step 8: Run test to verify it passes**

Run: `mise run test:manual -- --ci rename:with-hooks-branch` Expected: PASS

- [ ] **Step 9: Run `mise run fmt` and `mise run clippy`**

- [ ] **Step 10: Run full test suite**

Run: `mise run test:unit` and `mise run test:manual -- --ci rename` Expected:
All PASS

- [ ] **Step 11: Commit**

```bash
git add src/core/worktree/rename.rs src/commands/worktree_branch.rs \
  tests/manual/scenarios/rename/with-hooks.yml \
  tests/manual/scenarios/rename/with-hooks-branch.yml
git commit -m "feat(hooks): integrate move hooks into rename command"
```

---

## Task 10: Integrate move hooks into layout transform executor

**Files:**

- Modify: `src/core/layout/transform/execute.rs:77-125` (execute_op)
- Test: `tests/manual/scenarios/layout/transform-with-hooks.yml`

- [ ] **Step 1: Write YAML test scenario**

Create `tests/manual/scenarios/layout/transform-with-hooks.yml`:

```yaml
name: Layout transform with path-tracked hooks
description:
  Path-tracked hooks run teardown/setup when layout changes worktree paths

repos:
  - name: test-transform-hooks
    default_branch: main
    branches:
      - name: main
        files:
          - path: README.md
            content: "# Transform hooks test"
        commits:
          - message: "Initial commit"
      - name: feature/work
        from: main
    daft_yml: |
      hooks:
        worktree-post-create:
          jobs:
            - name: mark-path
              run: echo "{worktree_path}" > "{worktree_path}/.path-marker"
              tracks: [path]

steps:
  - name: Clone contained layout and trust
    run: |
      git-worktree-clone --layout contained $REMOTE_TEST_TRANSFORM_HOOKS
      cd $WORK_DIR/test-transform-hooks/main
      daft hooks trust --force
      git-worktree-checkout feature/work 2>&1
    expect:
      exit_code: 0
      files_exist:
        - "$WORK_DIR/test-transform-hooks/feature/work/.path-marker"

  - name: Transform to sibling layout
    run: daft layout transform --layout sibling --all 2>&1
    cwd: "$WORK_DIR/test-transform-hooks/main"
    expect:
      exit_code: 0
      files_exist:
        - "$WORK_DIR/test-transform-hooks-feature-work/.path-marker"
```

- [ ] **Step 2: Run test to verify it fails**

Run: `mise run test:manual -- --ci layout:transform-with-hooks` Expected: FAIL —
transform doesn't run move hooks

- [ ] **Step 3: Add move hook integration to `execute_op` for `MoveWorktree`**

In `src/core/layout/transform/execute.rs`, the `execute_op` function handles
`TransformOp::MoveWorktree { branch, from, to }`. Before calling
`exec_move_worktree`, run teardown hooks; after, run setup hooks.

The `execute_plan` function needs access to a `HookRunner`. Update its signature
to accept `sink: &mut (impl ProgressSink + HookRunner)` instead of just
`progress: &mut dyn ProgressSink`.

In the `MoveWorktree` arm:

```rust
TransformOp::MoveWorktree { branch, from, to } => {
    // Run teardown hooks before move
    let move_params = MoveHookParams {
        old_worktree_path: from.clone(),
        new_worktree_path: to.clone(),
        old_branch_name: branch.clone(),
        new_branch_name: branch.clone(), // branch unchanged in transform
        project_root: /* derive from plan context */,
        git_dir: /* derive from plan context */,
        remote: "origin".to_string(),
        source_worktree: /* current worktree */,
        command: "layout-transform".to_string(),
        changed_attributes: HashSet::from([TrackedAttribute::Path]),
    };
    run_teardown_hooks(&move_params, sink);

    exec_move_worktree(from, to, git)?;

    run_setup_hooks(&move_params, sink);
}
```

`execute_plan` needs additional context to construct `MoveHookParams`. Create an
`ExecutionContext` struct:

```rust
pub struct ExecutionContext {
    pub project_root: PathBuf,
    pub git_dir: PathBuf,
    pub remote: String,
    pub source_worktree: PathBuf,
}
```

Update `execute_plan` signature to:

```rust
pub fn execute_plan(
    plan: &TransformPlan,
    git: &GitCommand,
    ctx: &ExecutionContext,
    sink: &mut (impl ProgressSink + HookRunner),
) -> Result<ExecuteResult>
```

Note: changing from `progress: &mut dyn ProgressSink` (trait object) to a
generic `sink: &mut (impl ProgressSink + HookRunner)` changes the calling
convention. The primary caller is in `src/commands/layout.rs` (search for
`execute_plan(`). That caller currently uses `OutputSink` — switch it to
`CommandBridge` (same pattern as Task 9 Step 5). Construct the
`ExecutionContext` from the layout command's available state (project root, git
dir, etc. are already computed there).

- [ ] **Step 4: Update all callers of `execute_plan`**

Search for `execute_plan(` in `src/commands/` and update callers to:

1. Switch from `OutputSink` to `CommandBridge`
2. Pass the new `ExecutionContext` parameter
3. Construct context from the available layout/git state

- [ ] **Step 5: Run the YAML test**

Run: `mise run test:manual -- --ci layout:transform-with-hooks` Expected: PASS

- [ ] **Step 6: Run `mise run fmt` and `mise run clippy`**

- [ ] **Step 7: Run full test suite**

Run: `mise run test:unit` and `mise run test:manual -- --ci layout` Expected:
All PASS

- [ ] **Step 8: Commit**

```bash
git add src/core/layout/transform/execute.rs \
  tests/manual/scenarios/layout/transform-with-hooks.yml
git commit -m "feat(hooks): integrate move hooks into layout transform executor"
```

---

## Task 11: Integrate move hooks into adopt (convert_to_bare)

**Files:**

- Modify: `src/core/layout/transform/legacy.rs` (convert_to_bare)

- [ ] **Step 1: Identify the worktree relocation site in `convert_to_bare`**

In `legacy.rs`, `convert_to_bare` calls `move_files_to_worktree` (around
line 95) to relocate the project root contents into a worktree subdirectory.
This is where the path changes.

- [ ] **Step 2: Add move hook calls around the relocation**

Before `move_files_to_worktree`, call `run_teardown_hooks` with the old path
(project root). After, call `run_setup_hooks` with the new path (worktree
subdirectory).

`convert_to_bare` currently takes `progress: &mut dyn ProgressSink` (a trait
object). It needs `HookRunner` too. Change the signature to a generic:
`sink: &mut (impl ProgressSink + HookRunner)`. This requires updating all
callers — the main one is in `src/commands/flow_adopt.rs` and possibly in
`src/core/worktree/flow_adopt.rs`. Follow the same `CommandBridge` pattern from
Tasks 9 and 10. Also update `convert_to_non_bare` if it already takes
`impl ProgressSink + HookRunner` for consistency.

```rust
// Before move_files_to_worktree
let move_params = MoveHookParams {
    old_worktree_path: project_root.clone(),
    new_worktree_path: worktree_path.clone(),
    old_branch_name: current_branch.clone(),
    new_branch_name: current_branch.clone(), // branch unchanged
    project_root: project_root.clone(),
    git_dir: git_dir.clone(),
    remote: remote_name.to_string(),
    source_worktree: project_root.clone(),
    command: "adopt".to_string(),
    changed_attributes: HashSet::from([TrackedAttribute::Path]),
};
run_teardown_hooks(&move_params, sink);

move_files_to_worktree(/* ... */)?;

run_setup_hooks(&move_params, sink);
```

- [ ] **Step 3: Run `mise run fmt` and `mise run clippy`**

- [ ] **Step 4: Run existing adopt tests**

Run: `mise run test:manual -- --ci flow` Expected: All PASS (existing tests
unaffected)

- [ ] **Step 5: Commit**

```bash
git add src/core/layout/transform/legacy.rs
git commit -m "feat(hooks): integrate move hooks into adopt (convert_to_bare)"
```

---

## Task 12: Final integration tests and cleanup

**Files:**

- All modified files
- Tests

- [ ] **Step 1: Run full test suite**

Run: `mise run ci` Expected: All PASS

- [ ] **Step 2: Run clippy with zero warnings**

Run: `mise run clippy` Expected: Zero warnings

- [ ] **Step 3: Verify formatting**

Run: `mise run fmt:check` Expected: No formatting issues

- [ ] **Step 4: Run the full manual test suite**

Run: `mise run test:manual -- --ci` Expected: All scenarios pass

- [ ] **Step 5: Final commit if any cleanup needed**

```bash
git add -A
git commit -m "chore: final cleanup for move hooks feature"
```
