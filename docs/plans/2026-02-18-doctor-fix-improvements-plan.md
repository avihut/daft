# Doctor --fix and --dry-run Improvements Plan

> **For Claude:** REQUIRED SUB-SKILL: Use superpowers:executing-plans to
> implement this plan task-by-task.

**Goal:** Make `daft doctor --fix` actually fix missing shortcuts/symlinks, and
make `--dry-run` show concrete planned actions with pre-flight validation.

**Architecture:** Add a `FixAction` type and `dry_run_fix` callback to
`CheckResult`. Each fixable check provides both a fix closure and a dry-run
closure that validates preconditions and describes planned actions. The existing
`preview_fixes()` function calls dry-run closures when available.

**Tech Stack:** Rust, clap, std::fs, std::os::unix::fs

---

### Task 1: Add FixAction type and dry_run_fix field to CheckResult

**Files:**

- Modify: `src/doctor/mod.rs`

**Step 1: Write tests for FixAction and the new field**

Add to the `#[cfg(test)] mod tests` block in `src/doctor/mod.rs`:

```rust
#[test]
fn test_fix_action_success() {
    let action = FixAction {
        description: "Create symlink foo -> daft".to_string(),
        would_succeed: true,
        failure_reason: None,
    };
    assert!(action.would_succeed);
    assert!(action.failure_reason.is_none());
}

#[test]
fn test_fix_action_failure() {
    let action = FixAction {
        description: "Create symlink foo -> daft".to_string(),
        would_succeed: false,
        failure_reason: Some("Directory not writable".to_string()),
    };
    assert!(!action.would_succeed);
    assert_eq!(action.failure_reason.as_deref(), Some("Directory not writable"));
}

#[test]
fn test_check_result_with_dry_run_fix() {
    let result = CheckResult::warning("test", "something off")
        .with_fix(Box::new(|| Ok(())))
        .with_dry_run_fix(Box::new(|| vec![
            FixAction {
                description: "Would do thing".to_string(),
                would_succeed: true,
                failure_reason: None,
            }
        ]));
    assert!(result.fixable());
    assert!(result.dry_run_fix.is_some());
    let actions = (result.dry_run_fix.unwrap())();
    assert_eq!(actions.len(), 1);
    assert_eq!(actions[0].description, "Would do thing");
}
```

**Step 2: Run tests to verify they fail**

Run: `mise run test-unit` Expected: Compilation errors — `FixAction` doesn't
exist, `dry_run_fix` doesn't exist, `with_dry_run_fix` doesn't exist.

**Step 3: Implement FixAction and extend CheckResult**

In `src/doctor/mod.rs`, add after the `FixFn` type alias:

```rust
/// A single planned action from a dry-run simulation.
pub struct FixAction {
    /// What would be done, e.g. "Create symlink gwtco -> daft in /usr/local/bin"
    pub description: String,
    /// Whether preconditions are met for this action to succeed.
    pub would_succeed: bool,
    /// Why it would fail, if would_succeed is false.
    pub failure_reason: Option<String>,
}

/// A closure that simulates a fix, checking preconditions without applying changes.
type DryRunFn = Box<dyn Fn() -> Vec<FixAction>>;
```

Add `dry_run_fix: Option<DryRunFn>` field to `CheckResult` struct (after `fix`).

Update the `Debug` impl to include
`.field("dry_run_fix", &self.dry_run_fix.is_some())`.

Add `dry_run_fix: None` to all four constructors (`pass`, `warning`, `fail`,
`skipped`).

Add builder method:

```rust
pub fn with_dry_run_fix(mut self, dry_run_fix: DryRunFn) -> Self {
    self.dry_run_fix = Some(dry_run_fix);
    self
}
```

**Step 4: Run tests to verify they pass**

Run: `mise run test-unit` Expected: All pass, including the 3 new tests.

**Step 5: Commit**

```
test(doctor): add FixAction type and dry_run_fix field to CheckResult
```

---

### Task 2: Add fix + dry-run for shortcut symlinks

**Files:**

- Modify: `src/doctor/installation.rs`

**Step 1: Write tests for shortcut fix behavior**

Add to the `#[cfg(test)] mod tests` block in `src/doctor/installation.rs`:

```rust
#[test]
fn test_check_shortcut_symlinks_with_partial_install_has_fix() {
    // When a style is partially installed (some shortcuts present, some missing),
    // the check result should have a fix closure
    let temp = tempfile::tempdir().unwrap();
    let install_dir = temp.path();

    // Create a fake "daft" binary
    std::fs::write(install_dir.join("daft"), "fake").unwrap();

    // Create only one shortcut from git style to simulate partial install
    #[cfg(unix)]
    std::os::unix::fs::symlink("daft", install_dir.join("gwtclone")).unwrap();

    let results = check_shortcut_symlinks_in(install_dir);
    let git_result = results.iter().find(|r| r.name.contains("git")).unwrap();
    assert_eq!(git_result.status, crate::doctor::CheckStatus::Warning);
    assert!(git_result.fixable(), "Partially installed style should be fixable");
}

#[test]
fn test_check_shortcut_symlinks_with_partial_install_has_dry_run() {
    let temp = tempfile::tempdir().unwrap();
    let install_dir = temp.path();

    std::fs::write(install_dir.join("daft"), "fake").unwrap();

    #[cfg(unix)]
    std::os::unix::fs::symlink("daft", install_dir.join("gwtclone")).unwrap();

    let results = check_shortcut_symlinks_in(install_dir);
    let git_result = results.iter().find(|r| r.name.contains("git")).unwrap();
    assert!(git_result.dry_run_fix.is_some(), "Should have dry_run_fix");

    let actions = (git_result.dry_run_fix.as_ref().unwrap())();
    // Should have actions for each missing shortcut (all git shortcuts minus gwtclone)
    assert!(!actions.is_empty());
    assert!(actions.iter().all(|a| a.would_succeed));
    assert!(actions.iter().all(|a| a.description.contains("Create symlink")));
}
```

**Step 2: Run tests to verify they fail**

Run: `mise run test-unit` Expected: `check_shortcut_symlinks_in` doesn't exist
yet.

**Step 3: Refactor check_shortcut_symlinks to accept install_dir parameter**

Extract the body of `check_shortcut_symlinks()` into a new
`check_shortcut_symlinks_in(install_dir: &Path) -> Vec<CheckResult>` function,
and make `check_shortcut_symlinks()` call it:

```rust
pub fn check_shortcut_symlinks() -> Vec<CheckResult> {
    let install_dir = match crate::commands::shortcuts::detect_install_dir() {
        Ok(dir) => dir,
        Err(_) => return vec![],
    };
    check_shortcut_symlinks_in(&install_dir)
}

pub fn check_shortcut_symlinks_in(install_dir: &Path) -> Vec<CheckResult> {
    // ... existing body, but now uses install_dir param ...
}
```

In the `check_shortcut_symlinks_in` function, when there are missing shortcuts
for a partially-installed style (the `else` branch at old line 268), add fix and
dry-run closures:

```rust
// Capture what we need for closures
let install_dir_owned = install_dir.to_path_buf();
let missing_owned: Vec<String> = missing.iter().map(|s| s.to_string()).collect();

let fix_dir = install_dir_owned.clone();
let fix_missing = missing_owned.clone();

let dry_dir = install_dir_owned;
let dry_missing = missing_owned;

results.push(
    CheckResult::warning(
        &name,
        &format!("{found}/{total} installed, {} missing", missing.len()),
    )
    .with_suggestion(&format!(
        "Run 'daft setup shortcuts enable {style_name}' to install"
    ))
    .with_fix(Box::new(move || {
        for alias in &fix_missing {
            let path = fix_dir.join(alias);
            if !is_valid_symlink(&path, &fix_dir) {
                create_symlink(alias, &fix_dir)?;
            }
        }
        Ok(())
    }))
    .with_dry_run_fix(Box::new(move || {
        dry_run_symlink_actions(&dry_missing, &dry_dir)
    }))
    .with_details(details),
);
```

Add the `dry_run_symlink_actions` helper (reused by both command and shortcut
symlinks):

```rust
fn dry_run_symlink_actions(missing: &[String], install_dir: &Path) -> Vec<FixAction> {
    let dir_display = install_dir.display();
    let dir_writable = is_dir_writable(install_dir);

    missing.iter().map(|alias| {
        let description = format!("Create symlink {alias} -> daft in {dir_display}");
        let path = install_dir.join(alias);

        if !dir_writable {
            return FixAction {
                description,
                would_succeed: false,
                failure_reason: Some(format!("{dir_display} is not writable")),
            };
        }

        // Check for conflicting non-daft file
        if path.exists() && !path.is_symlink() {
            return FixAction {
                description,
                would_succeed: false,
                failure_reason: Some(format!(
                    "{} exists and is not a symlink",
                    path.display()
                )),
            };
        }

        if path.is_symlink() && !is_valid_symlink(&path, install_dir) {
            return FixAction {
                description,
                would_succeed: true,
                failure_reason: None,
            };
        }

        FixAction {
            description,
            would_succeed: true,
            failure_reason: None,
        }
    }).collect()
}

fn is_dir_writable(dir: &Path) -> bool {
    let test_file = dir.join(".daft-write-test");
    if std::fs::write(&test_file, "test").is_ok() {
        std::fs::remove_file(&test_file).ok();
        true
    } else {
        false
    }
}
```

Also add `use crate::doctor::FixAction;` at the top of the file.

**Step 4: Run tests to verify they pass**

Run: `mise run test-unit` Expected: All pass.

**Step 5: Commit**

```
feat(doctor): add fix and dry-run for missing shortcut symlinks
```

---

### Task 3: Add dry-run for command symlinks

**Files:**

- Modify: `src/doctor/installation.rs`

**Step 1: Write test**

Add to tests in `src/doctor/installation.rs`:

```rust
#[test]
fn test_check_command_symlinks_dry_run() {
    // We can't easily unit-test this without controlling detect_install_dir,
    // but we can test dry_run_symlink_actions directly
    let temp = tempfile::tempdir().unwrap();
    let install_dir = temp.path();
    std::fs::write(install_dir.join("daft"), "fake").unwrap();

    let missing = vec!["git-worktree-clone".to_string(), "git-daft".to_string()];
    let actions = dry_run_symlink_actions(&missing, install_dir);
    assert_eq!(actions.len(), 2);
    assert!(actions[0].description.contains("git-worktree-clone"));
    assert!(actions[0].would_succeed);
}

#[test]
fn test_dry_run_symlink_actions_detects_conflict() {
    let temp = tempfile::tempdir().unwrap();
    let install_dir = temp.path();
    std::fs::write(install_dir.join("daft"), "fake").unwrap();
    // Create a regular file (not a symlink) that conflicts
    std::fs::write(install_dir.join("gwtco"), "not-daft").unwrap();

    let missing = vec!["gwtco".to_string()];
    let actions = dry_run_symlink_actions(&missing, install_dir);
    assert_eq!(actions.len(), 1);
    assert!(!actions[0].would_succeed);
    assert!(actions[0].failure_reason.as_ref().unwrap().contains("not a symlink"));
}
```

**Step 2: Run tests to verify they pass**

These test `dry_run_symlink_actions` directly. Since we built it in Task 2, they
should pass already. Run: `mise run test-unit`

**Step 3: Add dry-run to check_command_symlinks**

In `check_command_symlinks()`, update the warning branch to add
`.with_dry_run_fix()`:

```rust
if missing.is_empty() {
    CheckResult::pass("Command symlinks", &format!("{found}/{total} installed"))
} else {
    let details: Vec<String> = missing.iter().map(|n| format!("Missing: {n}")).collect();
    let missing_owned: Vec<String> = missing.iter().map(|s| s.to_string()).collect();
    let dry_dir = install_dir.clone();
    CheckResult::warning(
        "Command symlinks",
        &format!("{found}/{total} installed, {} missing", missing.len()),
    )
    .with_suggestion("Run 'daft setup' to create missing symlinks")
    .with_fix(Box::new(fix_command_symlinks))
    .with_dry_run_fix(Box::new(move || {
        dry_run_symlink_actions(&missing_owned, &dry_dir)
    }))
    .with_details(details)
}
```

**Step 4: Run tests**

Run: `mise run test-unit` Expected: All pass.

**Step 5: Commit**

```
feat(doctor): add dry-run simulation for command symlinks
```

---

### Task 4: Add dry-run for repository fixes

**Files:**

- Modify: `src/doctor/repository.rs`

**Step 1: Write tests**

Add to tests in `src/doctor/repository.rs`:

```rust
#[test]
fn test_dry_run_worktree_consistency() {
    let actions = dry_run_worktree_consistency();
    assert_eq!(actions.len(), 1);
    assert!(actions[0].description.contains("git worktree prune"));
    // In test env, git should be available
    assert!(actions[0].would_succeed);
}

#[test]
fn test_dry_run_fetch_refspec() {
    let actions = dry_run_fetch_refspec();
    assert_eq!(actions.len(), 1);
    assert!(actions[0].description.contains("fetch refspec"));
}

#[test]
fn test_dry_run_remote_head() {
    let actions = dry_run_remote_head();
    assert_eq!(actions.len(), 1);
    assert!(actions[0].description.contains("git remote set-head"));
}
```

**Step 2: Run tests to verify they fail**

Run: `mise run test-unit` Expected: Functions don't exist yet.

**Step 3: Implement dry-run functions**

Add `use crate::doctor::FixAction;` at the top of `src/doctor/repository.rs`.

```rust
/// Dry-run simulation for worktree consistency fix.
pub fn dry_run_worktree_consistency() -> Vec<FixAction> {
    let git_available = which::which("git").is_ok();
    vec![FixAction {
        description: "Run git worktree prune to clean up orphaned entries".to_string(),
        would_succeed: git_available,
        failure_reason: if git_available {
            None
        } else {
            Some("git is not available".to_string())
        },
    }]
}

/// Dry-run simulation for fetch refspec fix.
pub fn dry_run_fetch_refspec() -> Vec<FixAction> {
    let expected = "+refs/heads/*:refs/remotes/origin/*";
    vec![FixAction {
        description: format!("Set fetch refspec to {expected}"),
        would_succeed: true,
        failure_reason: None,
    }]
}

/// Dry-run simulation for remote HEAD fix.
pub fn dry_run_remote_head() -> Vec<FixAction> {
    vec![FixAction {
        description: "Run git remote set-head origin --auto".to_string(),
        would_succeed: true,
        failure_reason: None,
    }]
}
```

Wire them into the checks. In `check_worktree_consistency`, after
`.with_fix(...)`:

```rust
.with_dry_run_fix(Box::new(dry_run_worktree_consistency))
```

In `check_fetch_refspec`, after `.with_fix(...)` (both branches):

```rust
.with_dry_run_fix(Box::new(dry_run_fetch_refspec))
```

In `check_remote_head`, after `.with_fix(...)`:

```rust
.with_dry_run_fix(Box::new(dry_run_remote_head))
```

**Step 4: Run tests**

Run: `mise run test-unit` Expected: All pass.

**Step 5: Commit**

```
feat(doctor): add dry-run simulation for repository fixes
```

---

### Task 5: Add dry-run for hooks fixes

**Files:**

- Modify: `src/doctor/hooks_checks.rs`

**Step 1: Write tests**

Add to tests in `src/doctor/hooks_checks.rs`:

```rust
#[test]
#[cfg(unix)]
fn test_dry_run_hooks_executable() {
    use std::os::unix::fs::PermissionsExt;

    let temp = tempdir().unwrap();
    let worktree = temp.path().join("main");
    let hooks_dir = worktree.join(PROJECT_HOOKS_DIR);
    std::fs::create_dir_all(&hooks_dir).unwrap();

    let hook = hooks_dir.join("post-clone");
    std::fs::write(&hook, "#!/bin/bash").unwrap();
    let mut perms = hook.metadata().unwrap().permissions();
    perms.set_mode(0o644);
    std::fs::set_permissions(&hook, perms).unwrap();

    let actions = dry_run_hooks_executable(temp.path());
    assert_eq!(actions.len(), 1);
    assert!(actions[0].description.contains("post-clone"));
    assert!(actions[0].description.contains("executable"));
    assert!(actions[0].would_succeed);
}

#[test]
fn test_dry_run_deprecated_names_no_conflict() {
    let temp = tempdir().unwrap();
    let worktree = temp.path().join("main");
    let hooks_dir = worktree.join(PROJECT_HOOKS_DIR);
    std::fs::create_dir_all(&hooks_dir).unwrap();

    std::fs::write(hooks_dir.join("post-create"), "#!/bin/bash").unwrap();

    let actions = dry_run_deprecated_names(temp.path());
    assert_eq!(actions.len(), 1);
    assert!(actions[0].description.contains("post-create"));
    assert!(actions[0].description.contains("worktree-post-create"));
    assert!(actions[0].would_succeed);
}

#[test]
fn test_dry_run_deprecated_names_with_conflict() {
    let temp = tempdir().unwrap();
    let worktree = temp.path().join("main");
    let hooks_dir = worktree.join(PROJECT_HOOKS_DIR);
    std::fs::create_dir_all(&hooks_dir).unwrap();

    // Both old and new names exist
    std::fs::write(hooks_dir.join("post-create"), "#!/bin/bash").unwrap();
    std::fs::write(hooks_dir.join("worktree-post-create"), "#!/bin/bash").unwrap();

    let actions = dry_run_deprecated_names(temp.path());
    assert_eq!(actions.len(), 1);
    assert!(!actions[0].would_succeed);
    assert!(actions[0].failure_reason.as_ref().unwrap().contains("already exists"));
}
```

**Step 2: Run tests to verify they fail**

Run: `mise run test-unit` Expected: Functions don't exist yet.

**Step 3: Implement dry-run functions**

Add `use crate::doctor::FixAction;` at the top of `src/doctor/hooks_checks.rs`.

```rust
/// Dry-run simulation for hooks executable fix.
pub fn dry_run_hooks_executable(project_root: &Path) -> Vec<FixAction> {
    let hooks_dir = match find_hooks_dir(project_root) {
        Some(dir) => dir,
        None => return vec![],
    };

    let files = list_hook_files(&hooks_dir);
    files
        .iter()
        .filter(|f| !is_executable(f))
        .map(|file| {
            let name = file.file_name().unwrap_or_default().to_string_lossy();
            FixAction {
                description: format!("Set {name} as executable (chmod +x)"),
                would_succeed: true,
                failure_reason: None,
            }
        })
        .collect()
}

/// Dry-run simulation for deprecated hook name fix.
pub fn dry_run_deprecated_names(project_root: &Path) -> Vec<FixAction> {
    let hooks_dir = match find_hooks_dir(project_root) {
        Some(dir) => dir,
        None => return vec![],
    };

    let files = list_hook_files(&hooks_dir);
    let mut actions = Vec::new();

    for file in &files {
        if let Some(name) = file.file_name().and_then(|n| n.to_str()) {
            if let Some(hook_type) = HookType::from_filename(name) {
                if let Some(old_name) = hook_type.deprecated_filename() {
                    if name == old_name {
                        let new_name = hook_type.filename();
                        let new_path = hooks_dir.join(new_name);
                        let would_succeed = !new_path.exists();
                        actions.push(FixAction {
                            description: format!("Rename {old_name} -> {new_name}"),
                            would_succeed,
                            failure_reason: if would_succeed {
                                None
                            } else {
                                Some(format!("{new_name} already exists"))
                            },
                        });
                    }
                }
            }
        }
    }

    actions
}
```

Wire into checks. In `check_hooks_executable`, after `.with_fix(...)`:

```rust
let dry_project_root = project_root.to_path_buf();
// ... add to the builder chain:
.with_dry_run_fix(Box::new(move || dry_run_hooks_executable(&dry_project_root)))
```

In `check_deprecated_names`, after `.with_fix(...)`:

```rust
let dry_project_root = project_root.to_path_buf();
// ... add to the builder chain:
.with_dry_run_fix(Box::new(move || dry_run_deprecated_names(&dry_project_root)))
```

Note: You'll need to clone `project_root` separately for the fix and dry-run
closures since each closure takes ownership.

**Step 4: Run tests**

Run: `mise run test-unit` Expected: All pass.

**Step 5: Commit**

```
feat(doctor): add dry-run simulation for hooks fixes
```

---

### Task 6: Update preview_fixes to use dry-run functions

**Files:**

- Modify: `src/commands/doctor.rs`

**Step 1: Write the implementation**

No separate unit tests needed here — this is display-only code. We'll verify
manually and via integration test later.

Replace the `preview_fixes` function in `src/commands/doctor.rs`:

```rust
fn preview_fixes(categories: &[CheckCategory]) {
    let fixable: Vec<_> = categories
        .iter()
        .flat_map(|c| &c.results)
        .filter(|r| r.fixable() && matches!(r.status, CheckStatus::Warning | CheckStatus::Fail))
        .collect();

    if fixable.is_empty() {
        println!("{}", dim("No fixable issues found."));
        return;
    }

    println!(
        "{}",
        bold(&format!("Would fix {} issue(s):", fixable.len()))
    );
    println!();

    let mut any_would_fail = false;

    for result in &fixable {
        let symbol = status_symbol(result.status);
        println!(
            "  {symbol} {} {} {}",
            result.name,
            dim("\u{2014}"),
            result.message
        );

        if let Some(ref dry_run) = result.dry_run_fix {
            let actions = dry_run();
            for action in &actions {
                if action.would_succeed {
                    println!("      {} {}", green("+"), action.description);
                } else {
                    any_would_fail = true;
                    println!("      {} {}", red("x"), action.description);
                    if let Some(ref reason) = action.failure_reason {
                        println!("        {}", dim(reason));
                    }
                }
            }
        } else if let Some(ref suggestion) = result.suggestion {
            println!("      {}", dim(&format!("Action: {suggestion}")));
        }
    }

    println!();
    if any_would_fail {
        println!(
            "{}",
            yellow("Some fixes would fail. Resolve the issues above first.")
        );
        println!(
            "{}",
            dim("Run 'daft doctor --fix' to apply fixes that can succeed.")
        );
    } else {
        println!("{}", dim("Run 'daft doctor --fix' to apply these fixes."));
    }
}
```

Add `use crate::styles::yellow;` if not already imported (check — it's already
in the imports on line 13 of doctor.rs).

**Step 2: Run all tests and clippy**

Run: `mise run test-unit && mise run clippy` Expected: All pass, zero warnings.

**Step 3: Commit**

```
feat(doctor): update --dry-run to show concrete actions with pre-flight checks
```

---

### Task 7: Format, lint, and full test pass

**Step 1: Run formatter**

Run: `mise run fmt`

**Step 2: Run clippy**

Run: `mise run clippy` Expected: Zero warnings.

**Step 3: Run all unit tests**

Run: `mise run test-unit` Expected: All pass.

**Step 4: Commit any formatting fixes**

```
style: format doctor changes
```

(Skip commit if nothing changed.)
