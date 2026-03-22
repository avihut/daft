# Centralized Layout XDG Data Dir Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use
> superpowers:subagent-driven-development (recommended) or
> superpowers:executing-plans to implement this plan task-by-task. Steps use
> checkbox (`- [ ]`) syntax for tracking.

**Goal:** Change the centralized layout to store worktrees in the XDG data
directory (`~/.local/share/daft/worktrees/`) instead of `~/worktrees/`.

**Architecture:** Add a `daft_data_dir()` function mirroring the existing
`daft_config_dir()`, expose it as a `{{ daft_data_dir }}` template variable, and
update the centralized layout template to use it.

**Tech Stack:** Rust, `dirs` crate (already a dependency), existing template
engine.

**Spec:** `docs/superpowers/specs/2026-03-22-centralized-layout-xdg-data-dir.md`

---

### Task 1: Add `daft_data_dir()` function and unit tests

**Files:**

- Modify: `src/lib.rs:12-45` (constants and `daft_config_dir` area)

- [ ] **Step 1: Write failing tests**

Add after the existing `test_daft_config_dir_*` tests (line ~171):

```rust
#[test]
#[serial]
fn test_daft_data_dir_default() {
    env::remove_var(DATA_DIR_ENV);
    let dir = daft_data_dir().unwrap();
    assert!(dir.ends_with("daft"));
}

#[test]
#[serial]
fn test_daft_data_dir_override() {
    env::set_var(DATA_DIR_ENV, "/tmp/test-daft-data");
    let dir = daft_data_dir().unwrap();
    assert_eq!(dir, PathBuf::from("/tmp/test-daft-data"));
    env::remove_var(DATA_DIR_ENV);
}

#[test]
#[serial]
fn test_daft_data_dir_override_no_suffix() {
    env::set_var(DATA_DIR_ENV, "/tmp/my-custom-data");
    let dir = daft_data_dir().unwrap();
    assert_eq!(dir, PathBuf::from("/tmp/my-custom-data"));
    assert!(!dir.ends_with("daft"));
    env::remove_var(DATA_DIR_ENV);
}

#[test]
#[serial]
fn test_daft_data_dir_empty_falls_back() {
    env::set_var(DATA_DIR_ENV, "");
    let dir = daft_data_dir().unwrap();
    assert!(dir.ends_with("daft"));
    env::remove_var(DATA_DIR_ENV);
}

#[test]
#[serial]
fn test_daft_data_dir_rejects_relative_path() {
    env::set_var(DATA_DIR_ENV, "relative/path");
    let result = daft_data_dir();
    assert!(result.is_err());
    assert!(result
        .unwrap_err()
        .to_string()
        .contains("must be an absolute path"));
    env::remove_var(DATA_DIR_ENV);
}
```

- [ ] **Step 2: Run tests to verify they fail**

```bash
mise run test:unit
```

Expected: FAIL — `DATA_DIR_ENV` and `daft_data_dir` not defined.

- [ ] **Step 3: Add `DATA_DIR_ENV` constant and `daft_data_dir()` function**

Add the constant after `CONFIG_DIR_ENV` (around line 22):

```rust
/// Environment variable to override the data directory path.
///
/// When set, centralized layout worktrees and other application data are stored
/// in this directory instead of the XDG data directory (`~/.local/share/daft/`).
///
/// Only honored in dev builds (same policy as `DAFT_CONFIG_DIR`).
pub const DATA_DIR_ENV: &str = "DAFT_DATA_DIR";
```

Add the function after `daft_config_dir()` (around line 45):

```rust
/// Returns the daft data directory path.
///
/// In dev builds, when `DAFT_DATA_DIR` is set to a non-empty absolute path,
/// uses that path directly (no `daft/` suffix appended). In release builds the
/// env var is ignored. Always falls back to `dirs::data_dir()/daft`.
pub fn daft_data_dir() -> anyhow::Result<std::path::PathBuf> {
    use std::path::PathBuf;
    if cfg!(daft_dev_build) {
        if let Ok(dir) = env::var(DATA_DIR_ENV) {
            if !dir.is_empty() {
                let path = PathBuf::from(&dir);
                if path.is_relative() {
                    anyhow::bail!("DAFT_DATA_DIR must be an absolute path, got: {dir}");
                }
                return Ok(path);
            }
        }
    }
    let data_dir = dirs::data_dir()
        .ok_or_else(|| anyhow::anyhow!("Could not determine data directory"))?;
    Ok(data_dir.join("daft"))
}
```

- [ ] **Step 4: Run tests to verify they pass**

```bash
mise run test:unit
```

Expected: All pass.

- [ ] **Step 5: Commit**

```bash
git add src/lib.rs
git commit -m "feat: add daft_data_dir() for XDG data directory resolution"
```

---

### Task 2: Add `{{ daft_data_dir }}` template variable

**Files:**

- Modify: `src/core/layout/template.rs:43-59` (`resolve_expression` function and
  tests)

- [ ] **Step 1: Write failing test**

Add after `test_render_contained_template` (around line 172). Use `#[serial]`
and set `DAFT_DATA_DIR` to a deterministic value for test isolation:

```rust
#[test]
#[serial]
fn test_render_centralized_template() {
    env::set_var("DAFT_DATA_DIR", "/tmp/daft-test-data");
    let ctx = TemplateContext {
        repo_path: PathBuf::from("/home/user/myproject"),
        repo: "myproject".into(),
        branch: "feature/auth".into(),
    };
    let rendered = render(
        "{{ daft_data_dir }}/worktrees/{{ repo }}/{{ branch | sanitize }}",
        &ctx,
    )
    .unwrap();
    assert_eq!(
        rendered,
        "/tmp/daft-test-data/worktrees/myproject/feature-auth"
    );
    env::remove_var("DAFT_DATA_DIR");
}
```

Add `use serial_test::serial;` and `use std::env;` to the test module imports if
not already present.

- [ ] **Step 2: Run test to verify it fails**

```bash
mise run test:unit
```

Expected: FAIL — `Unknown template variable: daft_data_dir`

- [ ] **Step 3: Add `daft_data_dir` to `resolve_expression`**

In `resolve_expression()`, add a new match arm after `"branch"` (line 51):

```rust
"daft_data_dir" => crate::daft_data_dir()?
    .to_string_lossy()
    .to_string(),
```

The full match block should look like:

```rust
let raw_value = match var_name {
    "repo_path" => ctx.repo_path.to_string_lossy().to_string(),
    "repo" => ctx.repo.clone(),
    "branch" => ctx.branch.clone(),
    "daft_data_dir" => crate::daft_data_dir()?
        .to_string_lossy()
        .to_string(),
    _ => bail!("Unknown template variable: {var_name}"),
};
```

- [ ] **Step 4: Run tests to verify they pass**

```bash
mise run test:unit
```

Expected: All pass.

- [ ] **Step 5: Commit**

```bash
git add src/core/layout/template.rs
git commit -m "feat(layout): add daft_data_dir template variable"
```

---

### Task 3: Update centralized layout template and bare inference test

**Files:**

- Modify: `src/core/layout/mod.rs:122` (centralized template)
- Modify: `src/core/layout/bare.rs:65-70` (add test)

- [ ] **Step 1: Change the centralized template**

In `src/core/layout/mod.rs`, line 122, change:

```rust
Self::Centralized => "~/worktrees/{{ repo }}/{{ branch | sanitize }}",
```

to:

```rust
Self::Centralized => "{{ daft_data_dir }}/worktrees/{{ repo }}/{{ branch | sanitize }}",
```

- [ ] **Step 2: Add bare inference test for daft_data_dir prefix**

In `src/core/layout/bare.rs`, add after `test_home_path_not_bare` (line 70). The
existing `test_home_path_not_bare` remains as a valid generic test for
home-relative paths. This new test covers the actual centralized template:

```rust
#[test]
fn test_daft_data_dir_path_not_bare() {
    assert!(!infer_bare(
        "{{ daft_data_dir }}/worktrees/{{ repo }}/{{ branch | sanitize }}",
        None
    ));
}
```

- [ ] **Step 3: Run tests**

```bash
mise run test:unit
```

Expected: All pass.

- [ ] **Step 4: Run clippy and fmt**

```bash
mise run fmt && mise run clippy
```

- [ ] **Step 5: Commit**

```bash
git add src/core/layout/mod.rs src/core/layout/bare.rs
git commit -m "feat(layout): change centralized template to XDG data dir"
```

---

### Task 4: Add `DAFT_DATA_DIR` to test framework

**Files:**

- Modify: `xtask/src/manual_test/env.rs:15-31` (TestEnv struct)
- Modify: `xtask/src/manual_test/env.rs:40-89` (create method)
- Modify: `xtask/src/manual_test/env.rs:96-108` (new_with_vars)
- Modify: `xtask/src/manual_test/env.rs:216-252` (command_env)

- [ ] **Step 1: Add `daft_data_dir` field to TestEnv struct**

In `xtask/src/manual_test/env.rs`, add after line 29
(`pub daft_config_dir: PathBuf,`):

```rust
/// Isolated daft data directory (prevents centralized worktrees from
/// polluting the real XDG data dir).
pub daft_data_dir: PathBuf,
```

- [ ] **Step 2: Initialize `daft_data_dir` in `create()`**

After line 57 (`let daft_config_dir = base_dir.join("daft-config");`), add:

```rust
let daft_data_dir = base_dir.join("daft-data");
```

After line 64 (the `create_dir_all(&daft_config_dir)` block), add:

```rust
std::fs::create_dir_all(&daft_data_dir)
    .with_context(|| format!("creating daft data dir: {}", daft_data_dir.display()))?;
```

Add `daft_data_dir,` to the `Ok(Self { ... })` block (after `daft_config_dir,`).

- [ ] **Step 3: Add `DAFT_DATA_DIR` to scenario vars**

After line 74 (`vars.insert("BINARY_DIR".into(), ...)`), add:

```rust
vars.insert(
    "DAFT_DATA_DIR".into(),
    daft_data_dir.to_string_lossy().into_owned(),
);
```

- [ ] **Step 4: Set `DAFT_DATA_DIR` in `command_env()`**

After line 242 (the `DAFT_CONFIG_DIR` insert), add:

```rust
env.insert(
    "DAFT_DATA_DIR".into(),
    self.daft_data_dir.to_string_lossy().into_owned(),
);
```

- [ ] **Step 5: Update `new_with_vars` test helper**

Add `daft_data_dir: PathBuf::from("/tmp/test-dummy/daft-data"),` after line 105
(`daft_config_dir` line).

- [ ] **Step 6: Build and run tests**

```bash
cargo build -p xtask && mise run test:unit
```

Expected: All pass.

- [ ] **Step 7: Commit**

```bash
git add xtask/src/manual_test/env.rs
git commit -m "test: add DAFT_DATA_DIR isolation to manual test framework"
```

---

### Task 5: Update spec doc and test scenario

**Files:**

- Modify:
  `docs/superpowers/specs/2026-03-20-progressive-adoption-layout-system-design.md:67`
  (layout table)
- Modify:
  `docs/superpowers/specs/2026-03-20-progressive-adoption-layout-system-design.md:71-81`
  (template variables table and resolution note)
- Modify: `tests/manual/scenarios/layout/centralized-workflow.yml`

- [ ] **Step 1: Update built-in layout table**

In the spec, line 67, change:

```markdown
| `centralized` | `~/worktrees/{{ repo }}/{{ branch \| sanitize }}` | No |
Worktrees in a global directory |
```

to:

```markdown
| `centralized` |
`{{ daft_data_dir }}/worktrees/{{ repo }}/{{ branch \| sanitize }}` | No |
Worktrees in the XDG data directory |
```

- [ ] **Step 2: Add `{{ daft_data_dir }}` to the template variables table**

In the spec, after the `{{ branch | sanitize }}` row (line 78), add:

```markdown
| `{{ daft_data_dir }}` | XDG data directory for daft | `~/.local/share/daft` |
```

- [ ] **Step 3: Update the path resolution note**

In the spec, line 80, change:

```markdown
Templates that do not start with `~/`, `/`, or `../` are resolved relative to
`{{ repo_path }}`.
```

to:

```markdown
Templates that do not start with `~/`, `/`, `{{ daft_data_dir }}`, or `../` are
resolved relative to `{{ repo_path }}`.
```

- [ ] **Step 4: Update centralized-workflow.yml**

Replace the full file content:

```yaml
name: Centralized layout workflow
description:
  Clone with centralized layout. Worktrees should land in the daft data
  directory under worktrees/<repo>/. Verify worktree creation and layout show.

repos:
  - name: test-repo
    use_fixture: standard-remote

steps:
  - name: Clone with centralized layout
    run: git-worktree-clone --layout centralized $REMOTE_TEST_REPO 2>&1
    expect:
      exit_code: 0
      dirs_exist:
        - "$WORK_DIR/test-repo"
        - "$WORK_DIR/test-repo/.git"

  - name: Verify layout show reports centralized
    run: NO_COLOR=1 daft layout show 2>&1
    cwd: "$WORK_DIR/test-repo"
    expect:
      exit_code: 0
      output_contains:
        - "centralized"

  - name: Create second worktree
    run: |
      cd $WORK_DIR/test-repo
      git-worktree-checkout -b feature/test 2>&1
    expect:
      exit_code: 0

  - name: Verify both worktrees are valid
    run: |
      cd $WORK_DIR/test-repo
      git worktree list 2>&1
    expect:
      exit_code: 0
      output_contains:
        - "test-repo"
        - "feature/test"

  - name: Cleanup centralized worktrees
    run: rm -rf $DAFT_DATA_DIR/worktrees/test-repo
    expect:
      exit_code: 0
```

- [ ] **Step 5: Run the centralized workflow test**

```bash
mise run test:manual -- --ci layout:centralized-workflow
```

Expected: Pass.

- [ ] **Step 6: Run full test suite**

```bash
mise run fmt && mise run clippy && mise run test:unit
mise run test:manual -- --ci
```

Expected: All pass, no regressions.

- [ ] **Step 7: Commit**

```bash
git add docs/superpowers/specs/2026-03-20-progressive-adoption-layout-system-design.md \
        tests/manual/scenarios/layout/centralized-workflow.yml
git commit -m "docs(layout): update centralized layout to XDG data dir in spec and tests"
```
