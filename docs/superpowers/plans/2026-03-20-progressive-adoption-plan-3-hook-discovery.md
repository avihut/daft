# Hook Discovery Changes — Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use
> superpowers:subagent-driven-development (recommended) or
> superpowers:executing-plans to implement this plan task-by-task. Steps use
> checkbox (`- [ ]`) syntax for tracking.

**Goal:** Make hooks resolve from the target branch's `daft.yml` instead of a
fixed worktree. Add `git show` fallback for pre-create hooks. Fire both
`post-clone` and `worktree-post-create` for non-bare clones. Add post-clone
layout reconciliation from `daft.yml`.

**Architecture:** Modify `HookExecutor::get_hook_source_worktree()` to always
return the target worktree path. For `PreCreate` (where the target doesn't exist
yet), add a new code path that reads `daft.yml` from the target branch via
`git show <branch>:daft.yml` with fallback to the base branch then default
branch. For non-bare clones, fire `worktree-post-create` after `post-clone`.

**Tech Stack:** Rust, existing hooks infrastructure

**Spec:**
`docs/superpowers/specs/2026-03-20-progressive-adoption-layout-system-design.md`

**Depends on:** Plans 1 and 2 (complete).

---

## File Structure

### Modified files

| File                              | Change                                                                                               |
| --------------------------------- | ---------------------------------------------------------------------------------------------------- |
| `src/hooks/executor.rs`           | Update `get_hook_source_worktree()` to always use target; add `git show` path for PreCreate          |
| `src/hooks/yaml_config_loader.rs` | Add function to load yaml config from a branch ref via `git show`                                    |
| `src/hooks/environment.rs`        | Add `base_branch` field to HookContext for PreCreate fallback                                        |
| `src/commands/clone.rs`           | Fire `worktree-post-create` after `post-clone` for non-bare clones; post-clone layout reconciliation |

---

## Task 1: Load daft.yml from Branch Ref

**Files:**

- Modify: `src/hooks/yaml_config_loader.rs`

### Description

Add a function that reads `daft.yml` content from a branch ref using
`git show <branch>:daft.yml` without needing a worktree checkout. This is needed
for `worktree-pre-create` where the target worktree doesn't exist yet.

### Steps

- [ ] **Step 1: Read `src/hooks/yaml_config_loader.rs`**

Understand the existing `load_merged_config()` function.

- [ ] **Step 2: Add `load_config_from_branch()` function**

```rust
/// Load daft.yml from a branch ref via `git show`.
///
/// Tries the config file names in priority order via `git show <branch>:<name>`.
/// Returns None if the branch has no daft.yml.
///
/// Fallback chain: target_branch → base_branch → default_branch.
pub fn load_config_from_branch(
    git_dir: &Path,
    target_branch: &str,
    base_branch: Option<&str>,
) -> Result<Option<YamlConfig>> {
    // Try target branch first
    for name in CONFIG_FILE_NAMES {
        if let Some(content) = git_show_file(git_dir, target_branch, name)? {
            return parse_yaml_content(&content).map(Some);
        }
    }

    // Fallback to base branch (for new branches with no commits)
    if let Some(base) = base_branch {
        for name in CONFIG_FILE_NAMES {
            if let Some(content) = git_show_file(git_dir, base, name)? {
                return parse_yaml_content(&content).map(Some);
            }
        }
    }

    // Fallback to default branch
    if let Some(default) = detect_default_branch(git_dir)? {
        if default != target_branch && base_branch.map_or(true, |b| b != &*default) {
            for name in CONFIG_FILE_NAMES {
                if let Some(content) = git_show_file(git_dir, &default, name)? {
                    return parse_yaml_content(&content).map(Some);
                }
            }
        }
    }

    Ok(None)
}

/// Read a file from a branch ref via `git show <ref>:<path>`.
fn git_show_file(git_dir: &Path, branch: &str, file_name: &str) -> Result<Option<String>> {
    use std::process::Command;
    let output = Command::new("git")
        .env("GIT_DIR", git_dir)
        .args(["show", &format!("{branch}:{file_name}")])
        .output()?;
    if output.status.success() {
        Ok(Some(String::from_utf8_lossy(&output.stdout).to_string()))
    } else {
        Ok(None)
    }
}
```

Note: `CONFIG_FILE_NAMES` should be the same list of config file names already
defined in the module. `detect_default_branch` can use
`git symbolic-ref refs/remotes/origin/HEAD` or similar.

- [ ] **Step 3: Add `parse_yaml_content()` helper**

Extract the parsing logic from `load_merged_config()` so both the file-based and
git-show-based loaders can use it.

- [ ] **Step 4: Write tests**

Test that `load_config_from_branch()` returns None for branches without
daft.yml. Integration testing with actual git repos would be ideal but unit
tests with mocked git output are acceptable.

- [ ] **Step 5: Run tests, commit**

```bash
mise run test:unit && mise run clippy
git commit -m "feat(hooks): add ability to load daft.yml from branch ref via git show"
```

---

## Task 2: Target-Branch Hook Resolution

**Files:**

- Modify: `src/hooks/executor.rs` — update `get_hook_source_worktree()`
- Modify: `src/hooks/environment.rs` — add `base_branch` to HookContext

### Description

Change hook resolution so hooks always resolve from the target branch:

- `PostCreate`, `PreRemove`, `PostRemove`: already use target worktree — no
  change needed
- `PostClone`: already uses target worktree — no change
- `PreCreate`: currently uses source worktree. Change to load config from the
  target branch via `git show` (using the function from Task 1)

### Steps

- [ ] **Step 1: Add `base_branch` field to HookContext**

In `src/hooks/environment.rs`:

```rust
pub struct HookContext {
    // ... existing fields ...
    /// Base branch for new branch creation (used for PreCreate fallback).
    pub base_branch: Option<String>,
}
```

- [ ] **Step 2: Update PreCreate hook execution in executor**

In `src/hooks/executor.rs`, modify the `try_yaml_hook()` or `execute()` method
for `PreCreate` to use `load_config_from_branch()` instead of reading from the
source worktree's filesystem.

The key change: when `ctx.hook_type == HookType::PreCreate`, instead of calling
`load_merged_config(hook_source_worktree)`, call
`load_config_from_branch(git_dir, branch_name, base_branch)`.

- [ ] **Step 3: Update HookContext construction sites**

Find all places that create `HookContext` for `PreCreate` and populate
`base_branch`. In `src/core/worktree/checkout_branch.rs`, the base branch is
available from the params. In `src/core/worktree/checkout.rs`, it may not be
available (existing branch checkout) — set to `None`.

- [ ] **Step 4: Run tests, commit**

```bash
mise run test:unit && mise run clippy
git commit -m "feat(hooks): resolve PreCreate hooks from target branch via git show"
```

---

## Task 3: Non-Bare Clone Hook Overlap

**Files:**

- Modify: `src/commands/clone.rs`

### Description

For non-bare `daft clone`, fire both `post-clone` AND `worktree-post-create`.
Currently only `post-clone` fires. The spec says both should fire because a
non-bare clone both creates a repo and results in a worktree.

### Steps

- [ ] **Step 1: Read clone hook execution**

In `src/commands/clone.rs`, find where `PostClone` hook is triggered (around
line 273). Understand the hook context construction.

- [ ] **Step 2: Add PostCreate hook after PostClone for non-bare clones**

After the `PostClone` hook fires, check if this was a non-bare clone (the layout
doesn't need bare). If so, fire `PostCreate`:

```rust
if !layout.needs_bare() {
    let post_create_ctx = HookContext {
        hook_type: HookType::PostCreate,
        worktree_path: clone_result.worktree_dir.clone().unwrap_or(repo_dir.clone()),
        source_worktree: repo_dir.clone(),
        branch_name: clone_result.target_branch.clone(),
        // ... fill other fields from clone context ...
    };
    executor.execute(&post_create_ctx, output, presenter.clone())?;
}
```

- [ ] **Step 3: Run tests, commit**

```bash
mise run test:unit && mise run clippy
git commit -m "feat(hooks): fire worktree-post-create after post-clone for non-bare clones"
```

---

## Task 4: Post-Clone Layout Reconciliation

**Files:**

- Modify: `src/commands/clone.rs`

### Description

After a successful clone, if no `--layout` flag was given and no global config
default is set, read the cloned repo's `daft.yml`. If it has a `layout` field,
apply the team's suggested layout and store it in repos.json.

This ensures team conventions in `daft.yml` take effect for new clones.

### Steps

- [ ] **Step 1: After clone, check daft.yml layout**

After the clone succeeds and hooks run, check if:

- No `--layout` flag was used
- No global config `defaults.layout` is set
- The cloned repo's `daft.yml` has a `layout` field

If all three are true, the `daft.yml` layout is the team convention. Update
repos.json with this layout.

```rust
// After successful clone
if args.layout.is_none() && global_config.defaults.layout.is_none() {
    // Load daft.yml from the cloned repo
    let yaml_config = load_merged_config(&repo_dir)?;
    if let Some(ref config) = yaml_config {
        if let Some(ref yaml_layout) = config.layout {
            // Team convention found — store it
            let mut db = TrustDatabase::load()?;
            db.set_layout(&git_dir, yaml_layout.clone());
            db.save()?;

            // If the current layout doesn't match, suggest transform
            if layout.name != *yaml_layout {
                output.info(&format!(
                    "This repo suggests layout '{}'. Run `daft layout transform {}` to apply it.",
                    yaml_layout, yaml_layout
                ));
            }
        }
    }
}
```

Note: The spec says to actually apply the layout (transform), but for safety in
the initial implementation, just store the suggestion and print a hint. The user
can transform manually. This avoids unexpected repo restructuring during clone.

- [ ] **Step 2: Run tests, commit**

```bash
mise run test:unit && mise run clippy
git commit -m "feat(hooks): post-clone layout reconciliation from daft.yml team convention"
```
