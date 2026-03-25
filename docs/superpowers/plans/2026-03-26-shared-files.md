# Shared Files Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use
> superpowers:subagent-driven-development (recommended) or
> superpowers:executing-plans to implement this plan task-by-task. Steps use
> checkbox (`- [ ]`) syntax for tracking.

**Goal:** Centralize untracked config files across worktrees using symlinks,
with materialize/link for per-worktree overrides.

**Architecture:** Core logic in `src/core/shared.rs` handles storage,
symlinking, materialization tracking, and daft.yml manipulation. Command layer
in `src/commands/shared.rs` provides the `daft shared` subcommand with
add/remove/materialize/link/status/sync verbs. Worktree creation integration
calls a linking function before PostCreate hooks.

**Tech Stack:** Rust, clap (CLI), serde_yaml (daft.yml R/W),
std::os::unix::fs::symlink, std::fs for file ops.

**Spec:** `docs/superpowers/specs/2026-03-26-shared-files-design.md`

---

### Task 1: Add `shared` field to YamlConfig

**Files:**

- Modify: `src/hooks/yaml_config.rs:27-61`

- [ ] **Step 1: Add the field to YamlConfig**

In `src/hooks/yaml_config.rs`, add the `shared` field to the `YamlConfig`
struct:

```rust
// After the `layout` field (line 57):

/// Paths to share across worktrees via symlinks.
///
/// Each entry is a path relative to the worktree root (e.g., ".env",
/// ".idea", ".vscode/settings.json"). Daft centralizes these files in
/// `.git/.daft/shared/` and creates symlinks in each worktree.
pub shared: Option<Vec<String>>,
```

- [ ] **Step 2: Add unit test for shared field parsing**

Add a test at the bottom of the existing test module in
`src/hooks/yaml_config.rs`:

```rust
#[test]
fn test_shared_files_parsing() {
    let yaml = r#"
shared:
  - .env
  - .idea
  - .vscode/settings.json
"#;
    let config: YamlConfig = serde_yaml::from_str(yaml).unwrap();
    let shared = config.shared.unwrap();
    assert_eq!(shared.len(), 3);
    assert_eq!(shared[0], ".env");
    assert_eq!(shared[1], ".idea");
    assert_eq!(shared[2], ".vscode/settings.json");
}

#[test]
fn test_shared_files_empty_when_missing() {
    let yaml = r#"
hooks:
  worktree-post-create:
    jobs:
      - name: test
        run: echo hi
"#;
    let config: YamlConfig = serde_yaml::from_str(yaml).unwrap();
    assert!(config.shared.is_none());
}
```

- [ ] **Step 3: Run tests**

Run: `cargo test -p daft --lib hooks::yaml_config::tests` Expected: All tests
pass including the new ones.

- [ ] **Step 4: Commit**

```bash
git add src/hooks/yaml_config.rs
git commit -m "feat(shared): add shared field to YamlConfig"
```

---

### Task 2: Core shared files module — storage helpers and materialization tracking

**Files:**

- Create: `src/core/shared.rs`
- Modify: `src/core/mod.rs`

This task builds the foundation: path constants, shared storage helpers,
materialization JSON read/write, and daft.yml manipulation.

- [ ] **Step 1: Create the module and register it**

In `src/core/mod.rs`, add after `pub mod repo;`:

```rust
pub mod shared;
```

Create `src/core/shared.rs` with storage constants and helpers:

```rust
//! Shared file management across worktrees.
//!
//! Centralizes untracked configuration files (`.env`, `.idea/`, etc.) in
//! `.git/.daft/shared/` and creates symlinks in each worktree. Supports
//! materializing (copying out) per-worktree overrides and re-linking back.

use anyhow::{bail, Context, Result};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::fs;
use std::os::unix;
use std::path::{Path, PathBuf};

use crate::core::repo;
use crate::git::GitCommand;

/// Directory name inside git common dir for daft state.
const DAFT_DIR: &str = ".daft";

/// Subdirectory inside `.daft/` for shared file storage.
const SHARED_DIR: &str = "shared";

/// Filename for materialization tracking (inside `.daft/`).
const MATERIALIZED_FILE: &str = "materialized.json";

// ── Path helpers ──────────────────────────────────────────────────────────

/// Return the path to `.git/.daft/shared/`.
pub fn shared_storage_dir(git_common_dir: &Path) -> PathBuf {
    git_common_dir.join(DAFT_DIR).join(SHARED_DIR)
}

/// Return the path to a specific file inside shared storage.
pub fn shared_file_path(git_common_dir: &Path, rel_path: &str) -> PathBuf {
    shared_storage_dir(git_common_dir).join(rel_path)
}

/// Return the path to `.git/.daft/materialized.json`.
fn materialized_json_path(git_common_dir: &Path) -> PathBuf {
    git_common_dir.join(DAFT_DIR).join(MATERIALIZED_FILE)
}

/// Ensure the shared storage directory exists.
pub fn ensure_shared_dir(git_common_dir: &Path) -> Result<()> {
    let dir = shared_storage_dir(git_common_dir);
    if !dir.exists() {
        fs::create_dir_all(&dir)
            .with_context(|| format!("Failed to create shared storage at {}", dir.display()))?;
    }
    Ok(())
}

// ── Materialization tracking ──────────────────────────────────────────────

/// Map of shared path → list of worktree absolute paths that materialized it.
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct MaterializedState(pub HashMap<String, Vec<String>>);

impl MaterializedState {
    /// Load from disk. Returns default (empty) if file doesn't exist.
    pub fn load(git_common_dir: &Path) -> Result<Self> {
        let path = materialized_json_path(git_common_dir);
        match fs::read_to_string(&path) {
            Ok(contents) => {
                let state: Self = serde_json::from_str(&contents)
                    .with_context(|| format!("Failed to parse {}", path.display()))?;
                Ok(state)
            }
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(Self::default()),
            Err(e) => Err(e).with_context(|| format!("Failed to read {}", path.display())),
        }
    }

    /// Save to disk. Creates `.daft/` directory if needed.
    pub fn save(&self, git_common_dir: &Path) -> Result<()> {
        let path = materialized_json_path(git_common_dir);
        let dir = path.parent().unwrap();
        if !dir.exists() {
            fs::create_dir_all(dir)?;
        }
        let json = serde_json::to_string_pretty(&self.0)?;
        fs::write(&path, json)
            .with_context(|| format!("Failed to write {}", path.display()))
    }

    /// Check if a worktree has materialized a given shared path.
    pub fn is_materialized(&self, shared_path: &str, worktree_path: &Path) -> bool {
        let wt = worktree_path.to_string_lossy();
        self.0
            .get(shared_path)
            .is_some_and(|paths| paths.iter().any(|p| p == wt.as_ref()))
    }

    /// Record that a worktree materialized a shared path.
    pub fn add(&mut self, shared_path: &str, worktree_path: &Path) {
        let wt = worktree_path.to_string_lossy().to_string();
        let paths = self.0.entry(shared_path.to_string()).or_default();
        if !paths.contains(&wt) {
            paths.push(wt);
        }
    }

    /// Remove a worktree from the materialized list for a shared path.
    pub fn remove(&mut self, shared_path: &str, worktree_path: &Path) {
        let wt = worktree_path.to_string_lossy().to_string();
        if let Some(paths) = self.0.get_mut(shared_path) {
            paths.retain(|p| p != &wt);
            if paths.is_empty() {
                self.0.remove(shared_path);
            }
        }
    }

    /// Remove all entries for a shared path (used by `remove` command).
    pub fn remove_all(&mut self, shared_path: &str) {
        self.0.remove(shared_path);
    }

    /// Remove stale entries for worktrees that no longer exist.
    pub fn prune_stale(&mut self) {
        for paths in self.0.values_mut() {
            paths.retain(|p| Path::new(p).exists());
        }
        self.0.retain(|_, paths| !paths.is_empty());
    }
}

// ── Worktree enumeration ──────────────────────────────────────────────────

/// Return absolute paths of all worktrees in the repo.
pub fn list_worktree_paths() -> Result<Vec<PathBuf>> {
    let git = GitCommand::new(true);
    let porcelain = git.worktree_list_porcelain()?;
    let mut paths = Vec::new();
    for line in porcelain.lines() {
        if let Some(path_str) = line.strip_prefix("worktree ") {
            paths.push(PathBuf::from(path_str));
        }
    }
    Ok(paths)
}

// ── Symlink helpers ───────────────────────────────────────────────────────

/// Compute the relative path from `from_dir` to `target`.
///
/// Both paths must be absolute. The result is suitable for
/// `std::os::unix::fs::symlink(result, from_dir.join(name))`.
pub fn relative_symlink_target(from_dir: &Path, target: &Path) -> Result<PathBuf> {
    // Walk up from `from_dir` with `..` until we reach a common ancestor,
    // then descend into `target`.
    let from = from_dir
        .canonicalize()
        .unwrap_or_else(|_| from_dir.to_path_buf());
    let to = target
        .canonicalize()
        .unwrap_or_else(|_| target.to_path_buf());

    let from_components: Vec<_> = from.components().collect();
    let to_components: Vec<_> = to.components().collect();

    // Find common prefix length
    let common_len = from_components
        .iter()
        .zip(to_components.iter())
        .take_while(|(a, b)| a == b)
        .count();

    let ups = from_components.len() - common_len;
    let mut rel = PathBuf::new();
    for _ in 0..ups {
        rel.push("..");
    }
    for component in &to_components[common_len..] {
        rel.push(component);
    }
    Ok(rel)
}

/// Create a symlink at `link_path` pointing to `shared_target`.
///
/// - Creates intermediate directories for nested paths (e.g., `.vscode/`).
/// - Uses relative symlink targets for portability.
/// - Returns `Ok(false)` if skipped (conflict), `Ok(true)` if created.
pub fn create_shared_symlink(
    worktree_path: &Path,
    rel_path: &str,
    git_common_dir: &Path,
) -> Result<LinkResult> {
    let link_path = worktree_path.join(rel_path);
    let shared_target = shared_file_path(git_common_dir, rel_path);

    // Check if shared storage actually has this file
    if !shared_target.exists() {
        return Ok(LinkResult::NoSource);
    }

    // Check if link already exists and points to the right place
    if link_path.is_symlink() {
        let existing_target = fs::read_link(&link_path)?;
        let expected = relative_symlink_target(
            link_path.parent().unwrap_or(worktree_path),
            &shared_target,
        )?;
        if existing_target == expected {
            return Ok(LinkResult::AlreadyLinked);
        }
    }

    // Check for conflict (real file or dir at the path)
    if link_path.exists() || link_path.is_symlink() {
        return Ok(LinkResult::Conflict);
    }

    // Create intermediate directories if needed (e.g., `.vscode/` for `.vscode/settings.json`)
    if let Some(parent) = link_path.parent() {
        if parent != worktree_path && !parent.exists() {
            fs::create_dir_all(parent).with_context(|| {
                format!(
                    "Failed to create parent directory {}",
                    parent.display()
                )
            })?;
        }
    }

    // Create relative symlink
    let rel_target = relative_symlink_target(
        link_path.parent().unwrap_or(worktree_path),
        &shared_target,
    )?;
    unix::fs::symlink(&rel_target, &link_path).with_context(|| {
        format!(
            "Failed to create symlink {} → {}",
            link_path.display(),
            rel_target.display()
        )
    })?;

    Ok(LinkResult::Created)
}

/// Result of attempting to create a shared file symlink.
#[derive(Debug, PartialEq)]
pub enum LinkResult {
    /// Symlink created successfully.
    Created,
    /// Symlink already exists and points to correct target.
    AlreadyLinked,
    /// A real file/dir exists at the path (conflict).
    Conflict,
    /// No file in shared storage for this path (declared only).
    NoSource,
}

// ── daft.yml manipulation ─────────────────────────────────────────────────

/// Read the `shared:` list from daft.yml in the given worktree root.
/// Returns empty vec if no daft.yml or no shared section.
pub fn read_shared_paths(worktree_root: &Path) -> Result<Vec<String>> {
    let config = load_yaml_config(worktree_root)?;
    Ok(config.and_then(|c| c.shared).unwrap_or_default())
}

/// Add paths to the `shared:` list in daft.yml.
/// Creates daft.yml if it doesn't exist. Avoids duplicates.
pub fn add_to_daft_yml(worktree_root: &Path, paths: &[&str]) -> Result<()> {
    let config_path = find_or_create_daft_yml(worktree_root)?;
    let contents = fs::read_to_string(&config_path).unwrap_or_default();

    let mut config: serde_yaml::Value = if contents.trim().is_empty() {
        serde_yaml::Value::Mapping(serde_yaml::Mapping::new())
    } else {
        serde_yaml::from_str(&contents)
            .with_context(|| format!("Failed to parse {}", config_path.display()))?
    };

    let mapping = config
        .as_mapping_mut()
        .context("daft.yml root is not a mapping")?;

    let shared_key = serde_yaml::Value::String("shared".to_string());
    let shared_seq = mapping
        .entry(shared_key)
        .or_insert_with(|| serde_yaml::Value::Sequence(Vec::new()));

    let seq = shared_seq
        .as_sequence_mut()
        .context("shared: is not a list in daft.yml")?;

    for path in paths {
        let val = serde_yaml::Value::String(path.to_string());
        if !seq.contains(&val) {
            seq.push(val);
        }
    }

    let yaml_str = serde_yaml::to_string(&config)?;
    fs::write(&config_path, yaml_str)
        .with_context(|| format!("Failed to write {}", config_path.display()))?;

    Ok(())
}

/// Remove paths from the `shared:` list in daft.yml.
pub fn remove_from_daft_yml(worktree_root: &Path, paths: &[&str]) -> Result<()> {
    let config_path = find_daft_yml(worktree_root);
    let Some(config_path) = config_path else {
        return Ok(()); // No daft.yml, nothing to remove from
    };

    let contents = fs::read_to_string(&config_path)?;
    let mut config: serde_yaml::Value = serde_yaml::from_str(&contents)?;

    let Some(mapping) = config.as_mapping_mut() else {
        return Ok(());
    };

    let shared_key = serde_yaml::Value::String("shared".to_string());
    if let Some(shared_val) = mapping.get_mut(&shared_key) {
        if let Some(seq) = shared_val.as_sequence_mut() {
            for path in paths {
                let val = serde_yaml::Value::String(path.to_string());
                seq.retain(|v| v != &val);
            }
            // Remove the key entirely if the list is now empty
            if seq.is_empty() {
                mapping.remove(&shared_key);
            }
        }
    }

    let yaml_str = serde_yaml::to_string(&config)?;
    fs::write(&config_path, yaml_str)?;

    Ok(())
}

/// Find daft.yml in the worktree root (checks standard candidates).
fn find_daft_yml(root: &Path) -> Option<PathBuf> {
    for name in &["daft.yml", "daft.yaml", ".daft.yml", ".daft.yaml"] {
        let path = root.join(name);
        if path.is_file() {
            return Some(path);
        }
    }
    None
}

/// Find or create daft.yml (uses `daft.yml` as default name).
fn find_or_create_daft_yml(root: &Path) -> Result<PathBuf> {
    if let Some(existing) = find_daft_yml(root) {
        return Ok(existing);
    }
    let path = root.join("daft.yml");
    fs::write(&path, "").context("Failed to create daft.yml")?;
    Ok(path)
}

/// Load the YamlConfig from a worktree root, if daft.yml exists.
fn load_yaml_config(
    root: &Path,
) -> Result<Option<crate::hooks::yaml_config::YamlConfig>> {
    let Some(path) = find_daft_yml(root) else {
        return Ok(None);
    };
    let contents = fs::read_to_string(&path)?;
    let config = serde_yaml::from_str(&contents)?;
    Ok(Some(config))
}

// ── Git helpers ───────────────────────────────────────────────────────────

/// Check if a path is tracked by git (would show up in `git ls-files`).
pub fn is_git_tracked(worktree_root: &Path, rel_path: &str) -> Result<bool> {
    let output = std::process::Command::new("git")
        .args(["ls-files", "--error-unmatch", rel_path])
        .current_dir(worktree_root)
        .output()
        .context("Failed to run git ls-files")?;
    Ok(output.status.success())
}

// ── Lifecycle integration ─────────────────────────────────────────────────

/// Link all declared shared files in a worktree. Called during worktree creation,
/// before PostCreate hooks.
///
/// - Reads `shared:` from daft.yml found via `project_root`.
/// - Creates symlinks for each path that exists in shared storage.
/// - Warns (via `warn_fn`) about conflicts. Never errors fatally.
pub fn link_shared_files_on_create(
    worktree_path: &Path,
    git_common_dir: &Path,
    project_root: &Path,
    warn_fn: &mut dyn FnMut(&str),
) {
    let shared_paths = match read_shared_paths(project_root) {
        Ok(paths) => paths,
        Err(_) => return, // No daft.yml or parse error — skip silently
    };

    if shared_paths.is_empty() {
        return;
    }

    let materialized = MaterializedState::load(git_common_dir).unwrap_or_default();

    for rel_path in &shared_paths {
        if materialized.is_materialized(rel_path, worktree_path) {
            continue;
        }

        match create_shared_symlink(worktree_path, rel_path, git_common_dir) {
            Ok(LinkResult::Created) => {}
            Ok(LinkResult::AlreadyLinked) => {}
            Ok(LinkResult::Conflict) => {
                warn_fn(&format!(
                    "'{}' exists but is not shared. Run `daft shared link {}` to replace.",
                    rel_path, rel_path
                ));
            }
            Ok(LinkResult::NoSource) => {} // Declared only, skip silently
            Err(e) => {
                warn_fn(&format!("Failed to link shared file '{}': {}", rel_path, e));
            }
        }
    }
}
```

- [ ] **Step 2: Add unit tests for MaterializedState**

Append to the bottom of `src/core/shared.rs`:

```rust
#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::tempdir;

    #[test]
    fn test_materialized_state_roundtrip() {
        let dir = tempdir().unwrap();
        let daft_dir = dir.path().join(DAFT_DIR);
        fs::create_dir_all(&daft_dir).unwrap();

        let mut state = MaterializedState::default();
        let wt = PathBuf::from("/projects/repo.feat-auth");
        state.add(".env", &wt);
        state.save(dir.path()).unwrap();

        let loaded = MaterializedState::load(dir.path()).unwrap();
        assert!(loaded.is_materialized(".env", &wt));
        assert!(!loaded.is_materialized(".idea", &wt));
    }

    #[test]
    fn test_materialized_state_empty_when_no_file() {
        let dir = tempdir().unwrap();
        let state = MaterializedState::load(dir.path()).unwrap();
        assert!(state.0.is_empty());
    }

    #[test]
    fn test_materialized_add_remove() {
        let mut state = MaterializedState::default();
        let wt1 = PathBuf::from("/projects/repo.main");
        let wt2 = PathBuf::from("/projects/repo.feat");

        state.add(".env", &wt1);
        state.add(".env", &wt2);
        assert!(state.is_materialized(".env", &wt1));
        assert!(state.is_materialized(".env", &wt2));

        state.remove(".env", &wt1);
        assert!(!state.is_materialized(".env", &wt1));
        assert!(state.is_materialized(".env", &wt2));
    }

    #[test]
    fn test_materialized_no_duplicates() {
        let mut state = MaterializedState::default();
        let wt = PathBuf::from("/projects/repo.main");
        state.add(".env", &wt);
        state.add(".env", &wt);
        assert_eq!(state.0[".env"].len(), 1);
    }

    #[test]
    fn test_materialized_remove_all() {
        let mut state = MaterializedState::default();
        state.add(".env", &PathBuf::from("/a"));
        state.add(".env", &PathBuf::from("/b"));
        state.remove_all(".env");
        assert!(state.0.get(".env").is_none());
    }

    #[test]
    fn test_relative_symlink_target_sibling() {
        let from = Path::new("/projects/repo.feat/");
        let to = Path::new("/projects/.git/.daft/shared/.env");
        let rel = relative_symlink_target(from, to).unwrap();
        assert_eq!(rel, PathBuf::from("../.git/.daft/shared/.env"));
    }

    #[test]
    fn test_relative_symlink_target_nested() {
        let from = Path::new("/projects/repo.feat/.vscode/");
        let to = Path::new("/projects/.git/.daft/shared/.vscode/settings.json");
        let rel = relative_symlink_target(from, to).unwrap();
        assert_eq!(
            rel,
            PathBuf::from("../../.git/.daft/shared/.vscode/settings.json")
        );
    }

    #[test]
    fn test_create_shared_symlink_creates_link() {
        let dir = tempdir().unwrap();
        let git_common_dir = dir.path().join(".git");
        let shared_dir = git_common_dir.join(DAFT_DIR).join(SHARED_DIR);
        fs::create_dir_all(&shared_dir).unwrap();
        fs::write(shared_dir.join(".env"), "SECRET=test").unwrap();

        let worktree = dir.path().join("worktree");
        fs::create_dir_all(&worktree).unwrap();

        let result =
            create_shared_symlink(&worktree, ".env", &git_common_dir).unwrap();
        assert_eq!(result, LinkResult::Created);

        let link = worktree.join(".env");
        assert!(link.is_symlink());
        assert_eq!(fs::read_to_string(&link).unwrap(), "SECRET=test");
    }

    #[test]
    fn test_create_shared_symlink_conflict() {
        let dir = tempdir().unwrap();
        let git_common_dir = dir.path().join(".git");
        let shared_dir = git_common_dir.join(DAFT_DIR).join(SHARED_DIR);
        fs::create_dir_all(&shared_dir).unwrap();
        fs::write(shared_dir.join(".env"), "SHARED=val").unwrap();

        let worktree = dir.path().join("worktree");
        fs::create_dir_all(&worktree).unwrap();
        fs::write(worktree.join(".env"), "LOCAL=val").unwrap();

        let result =
            create_shared_symlink(&worktree, ".env", &git_common_dir).unwrap();
        assert_eq!(result, LinkResult::Conflict);
    }

    #[test]
    fn test_create_shared_symlink_no_source() {
        let dir = tempdir().unwrap();
        let git_common_dir = dir.path().join(".git");
        let shared_dir = git_common_dir.join(DAFT_DIR).join(SHARED_DIR);
        fs::create_dir_all(&shared_dir).unwrap();

        let worktree = dir.path().join("worktree");
        fs::create_dir_all(&worktree).unwrap();

        let result =
            create_shared_symlink(&worktree, ".env", &git_common_dir).unwrap();
        assert_eq!(result, LinkResult::NoSource);
    }

    #[test]
    fn test_create_shared_symlink_nested_creates_parent() {
        let dir = tempdir().unwrap();
        let git_common_dir = dir.path().join(".git");
        let shared_dir = git_common_dir.join(DAFT_DIR).join(SHARED_DIR);
        let vscode_shared = shared_dir.join(".vscode");
        fs::create_dir_all(&vscode_shared).unwrap();
        fs::write(vscode_shared.join("settings.json"), "{}").unwrap();

        let worktree = dir.path().join("worktree");
        fs::create_dir_all(&worktree).unwrap();
        // .vscode/ does NOT exist in worktree yet

        let result = create_shared_symlink(
            &worktree,
            ".vscode/settings.json",
            &git_common_dir,
        )
        .unwrap();
        assert_eq!(result, LinkResult::Created);

        // Parent dir was created
        assert!(worktree.join(".vscode").is_dir());
        // Symlink works
        assert!(worktree.join(".vscode/settings.json").is_symlink());
        assert_eq!(
            fs::read_to_string(worktree.join(".vscode/settings.json")).unwrap(),
            "{}"
        );
    }
}
```

- [ ] **Step 3: Run tests**

Run: `cargo test -p daft --lib core::shared::tests` Expected: All tests pass.
The `relative_symlink_target` tests may need adjustment since `canonicalize`
requires real paths — adjust assertions if needed to use `tempdir`-based
absolute paths.

- [ ] **Step 4: Commit**

```bash
git add src/core/shared.rs src/core/mod.rs
git commit -m "feat(shared): add core shared files module with storage, materialization, and symlink helpers"
```

---

### Task 3: Worktree creation integration — link shared files before PostCreate hooks

**Files:**

- Modify: `src/core/worktree/checkout.rs:292-313`
- Modify: `src/core/worktree/checkout_branch.rs:164-186`
- Modify: `src/commands/clone.rs` (multiple PostCreate dispatch sites)

The `link_shared_files_on_create` call must be inserted immediately **before**
every `PostCreate` hook dispatch site. There are sites in three modules:

1. `src/core/worktree/checkout.rs` — before `// Run post-create hook`
   (~line 300)
2. `src/core/worktree/checkout_branch.rs` — before `// Run post-create hook`
   (~line 172)
3. `src/commands/clone.rs` — before each PostCreate hook dispatch:
   - `run_post_create_hook()` function (~line 1274) — add linking before
     `executor.execute()`
   - Inline TUI-based hook dispatches — search for `HookType::PostCreate`
     occurrences in clone.rs (~lines 614, 856, 1017) and add linking before each

- [ ] **Step 1: Add shared file linking to checkout.rs**

In `src/core/worktree/checkout.rs`, after `change_directory(&worktree_path)?;`
(line 292) and before `// Run post-create hook` (line 300), insert:

```rust
    // Link shared files before hooks so hooks can depend on .env etc.
    crate::core::shared::link_shared_files_on_create(
        &worktree_path,
        &git_dir,
        project_root,
        &mut |msg| sink.on_warning(msg),
    );
```

- [ ] **Step 2: Add shared file linking to checkout_branch.rs**

In `src/core/worktree/checkout_branch.rs`, after
`change_directory(&worktree_path)?;` (line 164) and before
`// Run post-create hook` (line 172), insert:

```rust
    // Link shared files before hooks so hooks can depend on .env etc.
    crate::core::shared::link_shared_files_on_create(
        &worktree_path,
        &git_dir,
        project_root,
        &mut |msg| sink.on_warning(msg),
    );
```

- [ ] **Step 3: Add shared file linking to clone.rs**

In `src/commands/clone.rs`, find all `HookType::PostCreate` dispatch sites and
add shared file linking before each. The key insertion points:

**In `run_post_create_hook()`** (~line 1294, before
`let ctx = HookContext::new(...)`):

```rust
    let worktree_path = result.worktree_dir.as_ref().unwrap();
    // Link shared files before post-create hooks
    crate::core::shared::link_shared_files_on_create(
        worktree_path,
        &result.git_dir,
        &result.parent_dir,
        &mut |msg| output.warning(msg),
    );
```

For each inline TUI dispatch of `HookType::PostCreate`, add the same
`link_shared_files_on_create` call before the hook context is created. The exact
lines will shift — search for `HookType::PostCreate` and insert before each
occurrence. The `worktree_path`, `git_dir`, and `project_root`/`parent_dir`
variables should already be in scope at each site.

- [ ] **Step 4: Verify it compiles**

Run: `cargo check -p daft` Expected: No errors. The `crate::core::shared` module
is already registered.

- [ ] **Step 5: Add integration test scenario for checkout**

Create `tests/manual/scenarios/shared/link-on-create.yml`:

```yaml
name: Shared files linked on worktree create
description:
  Shared files declared in daft.yml are symlinked when creating a worktree

repos:
  - name: test-repo
    use_fixture: standard-remote

steps:
  - name: Clone the repository
    run: git-worktree-clone --layout contained $REMOTE_TEST_REPO
    expect:
      exit_code: 0

  - name: Create shared storage and daft.yml
    run: |
      mkdir -p .git/.daft/shared
      echo "SECRET=val" > .git/.daft/shared/.env
      echo "shared:" > daft.yml
      echo "  - .env" >> daft.yml
    cwd: "$WORK_DIR/test-repo"
    expect:
      exit_code: 0

  - name: Create a new worktree
    run: git-worktree-checkout develop
    cwd: "$WORK_DIR/test-repo"
    expect:
      exit_code: 0

  - name: Verify .env is symlinked in new worktree
    run: test -L develop/.env && cat develop/.env
    cwd: "$WORK_DIR/test-repo"
    expect:
      exit_code: 0
      contains_stdout:
        - "SECRET=val"
```

- [ ] **Step 6: Run the integration test**

Run: `mise run test:manual -- --ci shared` Expected: Test passes.

- [ ] **Step 7: Commit**

```bash
git add src/core/worktree/checkout.rs src/core/worktree/checkout_branch.rs src/commands/clone.rs tests/manual/scenarios/shared/
git commit -m "feat(shared): link shared files during worktree creation before hooks"
```

---

### Task 4: `daft shared` command — add and declare subcommands

**Files:**

- Create: `src/commands/shared.rs`
- Modify: `src/commands/mod.rs`
- Modify: `src/main.rs`
- Modify: `src/suggest.rs`

- [ ] **Step 1: Create the command module**

Add to `src/commands/mod.rs`:

```rust
pub mod shared;
```

Create `src/commands/shared.rs`:

```rust
//! Command: `daft shared` — manage shared files across worktrees.

use anyhow::{bail, Result};
use clap::{Parser, Subcommand};
use std::fs;
use std::path::PathBuf;

use crate::core::layout;
use crate::core::repo;
use crate::core::shared;
use crate::output::{CliOutput, Output, OutputConfig};

#[derive(Parser)]
#[command(name = "daft-shared")]
#[command(version = crate::VERSION)]
#[command(about = "Manage shared files across worktrees")]
#[command(long_about = r#"
Centralize untracked configuration files (.env, .idea/, .vscode/, etc.)
so they are shared across worktrees via symlinks.

Files are stored in .git/.daft/shared/ and symlinked into each worktree.
Use 'materialize' to make a worktree-local copy, and 'link' to rejoin
the shared version.
"#)]
pub struct Args {
    #[command(subcommand)]
    command: SharedCommand,
}

#[derive(Subcommand)]
enum SharedCommand {
    /// Collect file/dir from current worktree into shared storage
    Add(AddArgs),
    /// Stop sharing a file (materialize everywhere, then remove)
    Remove(RemoveArgs),
    /// Replace symlink with a local copy in current worktree
    Materialize(MaterializeArgs),
    /// Replace local copy with symlink to shared version
    Link(LinkArgs),
    /// Show shared files and per-worktree state
    Status(StatusArgs),
    /// Ensure all worktrees have symlinks for declared shared files
    Sync(SyncArgs),
}

#[derive(Parser)]
struct AddArgs {
    /// Paths to share (relative to worktree root)
    #[arg(required = true)]
    paths: Vec<String>,

    /// Only declare the path in daft.yml without collecting (file need not exist)
    #[arg(long)]
    declare: bool,
}

#[derive(Parser)]
struct RemoveArgs {
    /// Paths to stop sharing
    #[arg(required = true)]
    paths: Vec<String>,

    /// Delete shared file and all symlinks instead of materializing
    #[arg(long)]
    delete: bool,
}

#[derive(Parser)]
struct MaterializeArgs {
    /// Paths to materialize in current worktree
    #[arg(required = true)]
    paths: Vec<String>,

    /// Force materialization even if a non-shared file exists
    #[arg(long = "override")]
    force_override: bool,
}

#[derive(Parser)]
struct LinkArgs {
    /// Paths to link back to shared version
    #[arg(required = true)]
    paths: Vec<String>,

    /// Replace local file even if it differs from shared version
    #[arg(long = "override")]
    force_override: bool,
}

#[derive(Parser)]
struct StatusArgs;

#[derive(Parser)]
struct SyncArgs;

pub fn run() -> Result<()> {
    let args = Args::parse_from(crate::get_clap_args("daft-shared"));
    let mut output = CliOutput::default_output();

    match args.command {
        SharedCommand::Add(add_args) => run_add(add_args, &mut output),
        SharedCommand::Remove(remove_args) => run_remove(remove_args, &mut output),
        SharedCommand::Materialize(mat_args) => run_materialize(mat_args, &mut output),
        SharedCommand::Link(link_args) => run_link(link_args, &mut output),
        SharedCommand::Status(_) => run_status(&mut output),
        SharedCommand::Sync(_) => run_sync(&mut output),
    }
}

fn run_add(args: AddArgs, output: &mut dyn Output) -> Result<()> {
    let git_common_dir = repo::get_git_common_dir()?;
    let worktree_path = repo::get_current_worktree_path()?;
    let project_root = repo::get_project_root()?;

    shared::ensure_shared_dir(&git_common_dir)?;

    let existing_shared = shared::read_shared_paths(&project_root)?;
    let mut added_paths = Vec::new();

    for rel_path in &args.paths {
        // Check if already shared
        if existing_shared.contains(rel_path) {
            if args.declare {
                output.info(&format!("'{}' is already declared as shared.", rel_path));
                continue;
            }
            bail!(
                "'{}' is already shared. Use `daft shared link {}` to symlink this worktree's copy.",
                rel_path,
                rel_path
            );
        }

        if args.declare {
            // --declare: just add to daft.yml and .gitignore
            layout::ensure_gitignore_entry(&project_root, rel_path)?;
            added_paths.push(rel_path.as_str());
            output.success(&format!("Declared: {}", rel_path));
            continue;
        }

        // Normal add: file must exist
        let full_path = worktree_path.join(rel_path);
        if !full_path.exists() {
            bail!(
                "'{}' does not exist in this worktree. Use `--declare` to declare without collecting.",
                rel_path
            );
        }

        // Must not be git-tracked
        if shared::is_git_tracked(&worktree_path, rel_path)? {
            bail!(
                "'{}' is tracked by git. Untrack it first with `git rm --cached {}`",
                rel_path,
                rel_path
            );
        }

        // Ensure gitignored
        layout::ensure_gitignore_entry(&project_root, rel_path)?;

        // Move to shared storage
        let shared_target = shared::shared_file_path(&git_common_dir, rel_path);
        if let Some(parent) = shared_target.parent() {
            fs::create_dir_all(parent)?;
        }
        fs::rename(&full_path, &shared_target).or_else(|_| {
            // rename fails across filesystems — fall back to copy + delete
            if full_path.is_dir() {
                copy_dir_all(&full_path, &shared_target)?;
            } else {
                fs::copy(&full_path, &shared_target)?;
            }
            if full_path.is_dir() {
                fs::remove_dir_all(&full_path)
            } else {
                fs::remove_file(&full_path)
            }
        })?;

        // Create symlink
        shared::create_shared_symlink(&worktree_path, rel_path, &git_common_dir)?;

        added_paths.push(rel_path.as_str());
        output.success(&format!("Shared: {} → .git/.daft/shared/{}", rel_path, rel_path));
    }

    // Update daft.yml
    if !added_paths.is_empty() {
        shared::add_to_daft_yml(&project_root, &added_paths)?;
    }

    Ok(())
}

fn run_remove(args: RemoveArgs, output: &mut dyn Output) -> Result<()> {
    let git_common_dir = repo::get_git_common_dir()?;
    let project_root = repo::get_project_root()?;
    let worktree_paths = shared::list_worktree_paths()?;
    let mut materialized = shared::MaterializedState::load(&git_common_dir)?;

    for rel_path in &args.paths {
        let shared_target = shared::shared_file_path(&git_common_dir, rel_path);

        if args.delete {
            // Delete mode: remove symlinks and shared storage
            for wt in &worktree_paths {
                let link = wt.join(rel_path);
                if link.is_symlink() {
                    fs::remove_file(&link)?;
                }
            }
            if shared_target.exists() {
                if shared_target.is_dir() {
                    fs::remove_dir_all(&shared_target)?;
                } else {
                    fs::remove_file(&shared_target)?;
                }
            }
            output.success(&format!("Deleted: {} (shared storage + all symlinks)", rel_path));
        } else {
            // Default: materialize everywhere, then delete shared storage
            if shared_target.exists() {
                for wt in &worktree_paths {
                    let link = wt.join(rel_path);
                    if link.is_symlink() {
                        fs::remove_file(&link)?;
                        if shared_target.is_dir() {
                            copy_dir_all(&shared_target, &link)?;
                        } else {
                            fs::copy(&shared_target, &link)?;
                        }
                        output.info(&format!(
                            "  Materialized in {}",
                            wt.file_name().unwrap_or_default().to_string_lossy()
                        ));
                    }
                }
                if shared_target.is_dir() {
                    fs::remove_dir_all(&shared_target)?;
                } else {
                    fs::remove_file(&shared_target)?;
                }
            }
            output.success(&format!("Removed: {} (materialized in all worktrees)", rel_path));
        }

        materialized.remove_all(rel_path);
    }

    materialized.save(&git_common_dir)?;

    let path_refs: Vec<&str> = args.paths.iter().map(|s| s.as_str()).collect();
    shared::remove_from_daft_yml(&project_root, &path_refs)?;

    Ok(())
}

fn run_materialize(args: MaterializeArgs, output: &mut dyn Output) -> Result<()> {
    let git_common_dir = repo::get_git_common_dir()?;
    let worktree_path = repo::get_current_worktree_path()?;
    let mut materialized = shared::MaterializedState::load(&git_common_dir)?;

    for rel_path in &args.paths {
        let link = worktree_path.join(rel_path);
        let shared_target = shared::shared_file_path(&git_common_dir, rel_path);

        if !shared_target.exists() {
            bail!("'{}' has no shared file to materialize from.", rel_path);
        }

        if link.is_symlink() {
            // Replace symlink with copy
            fs::remove_file(&link)?;
            if shared_target.is_dir() {
                copy_dir_all(&shared_target, &link)?;
            } else {
                fs::copy(&shared_target, &link)?;
            }
            materialized.add(rel_path, &worktree_path);
            output.success(&format!("Materialized: {} (copied from shared)", rel_path));
        } else if link.exists() {
            if args.force_override {
                if link.is_dir() {
                    fs::remove_dir_all(&link)?;
                } else {
                    fs::remove_file(&link)?;
                }
                if shared_target.is_dir() {
                    copy_dir_all(&shared_target, &link)?;
                } else {
                    fs::copy(&shared_target, &link)?;
                }
                materialized.add(rel_path, &worktree_path);
                output.success(&format!("Materialized: {} (overridden)", rel_path));
            } else {
                output.info(&format!("'{}' is already a local file in this worktree.", rel_path));
            }
        } else {
            // No file at all — copy from shared
            if let Some(parent) = link.parent() {
                if !parent.exists() {
                    fs::create_dir_all(parent)?;
                }
            }
            if shared_target.is_dir() {
                copy_dir_all(&shared_target, &link)?;
            } else {
                fs::copy(&shared_target, &link)?;
            }
            materialized.add(rel_path, &worktree_path);
            output.success(&format!("Materialized: {} (copied from shared)", rel_path));
        }
    }

    materialized.save(&git_common_dir)?;

    Ok(())
}

fn run_link(args: LinkArgs, output: &mut dyn Output) -> Result<()> {
    let git_common_dir = repo::get_git_common_dir()?;
    let worktree_path = repo::get_current_worktree_path()?;
    let mut materialized = shared::MaterializedState::load(&git_common_dir)?;

    for rel_path in &args.paths {
        let link = worktree_path.join(rel_path);
        let shared_target = shared::shared_file_path(&git_common_dir, rel_path);

        if !shared_target.exists() {
            bail!("'{}' has no shared file to link to.", rel_path);
        }

        // Already a correct symlink?
        if link.is_symlink() {
            let target = fs::read_link(&link)?;
            let expected = shared::relative_symlink_target(
                link.parent().unwrap_or(&worktree_path),
                &shared_target,
            )?;
            if target == expected {
                output.info(&format!("'{}' is already linked to shared version.", rel_path));
                continue;
            }
        }

        // Real file exists — check for differences
        if link.exists() && !link.is_symlink() {
            if !args.force_override {
                // Compare contents
                let differs = if link.is_dir() {
                    true // Directory diff is complex; require --override
                } else {
                    let local = fs::read(&link)?;
                    let shared = fs::read(&shared_target)?;
                    local != shared
                };

                if differs {
                    bail!(
                        "Local '{}' differs from shared version. Use `--override` to replace.",
                        rel_path
                    );
                }
            }

            // Remove local file/dir to make way for symlink
            if link.is_dir() {
                fs::remove_dir_all(&link)?;
            } else {
                fs::remove_file(&link)?;
            }
        } else if link.is_symlink() {
            // Broken or wrong symlink — remove it
            fs::remove_file(&link)?;
        }

        shared::create_shared_symlink(&worktree_path, rel_path, &git_common_dir)?;
        materialized.remove(rel_path, &worktree_path);
        output.success(&format!("Linked: {} → shared", rel_path));
    }

    materialized.save(&git_common_dir)?;

    Ok(())
}

fn run_status(output: &mut dyn Output) -> Result<()> {
    let git_common_dir = repo::get_git_common_dir()?;
    let project_root = repo::get_project_root()?;
    let shared_paths = shared::read_shared_paths(&project_root)?;
    let worktree_paths = shared::list_worktree_paths()?;
    let materialized = shared::MaterializedState::load(&git_common_dir)?;

    if shared_paths.is_empty() {
        output.info("No shared files declared.");
        return Ok(());
    }

    println!("Shared files:\n");

    for rel_path in &shared_paths {
        let shared_target = shared::shared_file_path(&git_common_dir, rel_path);
        let has_source = shared_target.exists();

        if !has_source {
            println!("  {} (declared, not yet collected)", rel_path);
            println!();
            continue;
        }

        println!("  {}", rel_path);

        for wt in &worktree_paths {
            let wt_name = wt
                .file_name()
                .unwrap_or_default()
                .to_string_lossy();
            let link = wt.join(rel_path);

            let state = if materialized.is_materialized(rel_path, wt) {
                "materialized"
            } else if link.is_symlink() {
                let target = fs::read_link(&link).ok();
                let expected = shared::relative_symlink_target(
                    link.parent().unwrap_or(wt),
                    &shared_target,
                )
                .ok();
                if target == expected {
                    "linked"
                } else {
                    "broken"
                }
            } else if link.exists() {
                "conflict"
            } else {
                "missing"
            };

            println!("    {:<24}{}", wt_name, state);
        }

        println!();
    }

    Ok(())
}

fn run_sync(output: &mut dyn Output) -> Result<()> {
    let git_common_dir = repo::get_git_common_dir()?;
    let project_root = repo::get_project_root()?;
    let shared_paths = shared::read_shared_paths(&project_root)?;
    let worktree_paths = shared::list_worktree_paths()?;
    let materialized = shared::MaterializedState::load(&git_common_dir)?;

    if shared_paths.is_empty() {
        output.info("No shared files declared.");
        return Ok(());
    }

    for wt in &worktree_paths {
        let wt_name = wt
            .file_name()
            .unwrap_or_default()
            .to_string_lossy()
            .to_string();

        for rel_path in &shared_paths {
            if materialized.is_materialized(rel_path, wt) {
                continue;
            }

            match shared::create_shared_symlink(wt, rel_path, &git_common_dir)? {
                shared::LinkResult::Created => {
                    output.success(&format!("{}: {} → symlinked", wt_name, rel_path));
                }
                shared::LinkResult::AlreadyLinked => {}
                shared::LinkResult::Conflict => {
                    output.warning(&format!(
                        "{}: {} exists (not shared) — run `daft shared link {}` to replace",
                        wt_name, rel_path, rel_path
                    ));
                }
                shared::LinkResult::NoSource => {}
            }
        }
    }

    Ok(())
}

/// Recursively copy a directory.
fn copy_dir_all(src: &std::path::Path, dst: &std::path::Path) -> Result<()> {
    fs::create_dir_all(dst)?;
    for entry in fs::read_dir(src)? {
        let entry = entry?;
        let ty = entry.file_type()?;
        let dest = dst.join(entry.file_name());
        if ty.is_dir() {
            copy_dir_all(&entry.path(), &dest)?;
        } else {
            fs::copy(entry.path(), dest)?;
        }
    }
    Ok(())
}
```

- [ ] **Step 2: Add routing in main.rs**

In `src/main.rs`, add to the subcommand match block (after line 101
`"multi-remote" =>`):

```rust
"shared" => commands::shared::run(),
```

Also add the `worktree-shared` variant in the worktree commands section (after
`"worktree-sync"` around line 138):

```rust
"worktree-shared" => commands::shared::run(),
```

- [ ] **Step 3: Add to DAFT_SUBCOMMANDS in suggest.rs**

In `src/suggest.rs`, add `"shared"` to the `DAFT_SUBCOMMANDS` array in
alphabetical order (between `"setup"` and `"shell-init"`):

```rust
"shared",
```

Also add `"worktree-shared"` in alphabetical order (between `"worktree-prune"`
and the end of the array):

```rust
"worktree-shared",
```

- [ ] **Step 4: Verify it compiles**

Run: `cargo check -p daft` Expected: Clean compile.

- [ ] **Step 5: Commit**

```bash
git add src/commands/shared.rs src/commands/mod.rs src/main.rs src/suggest.rs
git commit -m "feat(shared): add daft shared command with add/remove/materialize/link/status/sync"
```

---

### Task 5: Register command in docs, xtask, and completions

**Files:**

- Modify: `src/commands/docs.rs`
- Modify: `xtask/src/main.rs`
- Modify: `src/commands/completions/mod.rs`

- [ ] **Step 1: Add to docs help output**

In `src/commands/docs.rs`, add a new category in `get_command_categories()`
(before the "manage daft configuration" category). First add the import at the
top (line 11):

```rust
use crate::commands::{
    carry, checkout, clone, doctor, fetch, flow_adopt, flow_eject, hooks, init, layout, list,
    multi_remote, prune, release_notes, shared, shell_init, shortcuts, sync, worktree_branch,
};
```

Then add the category:

```rust
CommandCategory {
    title: "share configuration across worktrees",
    commands: vec![CommandEntry {
        display_name: "daft shared",
        command: shared::Args::command(),
    }],
},
```

- [ ] **Step 2: Add to xtask COMMANDS**

In `xtask/src/main.rs`, add `"daft-shared"` to the `COMMANDS` array and add a
match arm in `get_command_for_name()`:

```rust
"daft-shared" => Some(daft::commands::shared::Args::command()),
```

- [ ] **Step 3: Add to completions COMMANDS**

In `src/commands/completions/mod.rs`, add `"daft-shared"` to the `COMMANDS`
array and add to `get_command_for_name()`:

```rust
"daft-shared" => Some(crate::commands::shared::Args::command()),
```

Add a verb alias group for the `shared` verb:

```rust
(&["shared"], "daft-shared"),
```

- [ ] **Step 4: Verify it compiles**

Run: `cargo check -p daft && cargo check -p xtask` Expected: Clean compile.

- [ ] **Step 5: Commit**

```bash
git add src/commands/docs.rs xtask/src/main.rs src/commands/completions/mod.rs
git commit -m "feat(shared): register shared command in docs, xtask, and completions"
```

---

### Task 6: Integration test scenarios

**Files:**

- Create: `tests/manual/scenarios/shared/add-basic.yml`
- Create: `tests/manual/scenarios/shared/materialize-and-link.yml`
- Create: `tests/manual/scenarios/shared/remove.yml`
- Create: `tests/manual/scenarios/shared/sync.yml`
- Create: `tests/manual/scenarios/shared/declare.yml`

- [ ] **Step 1: Create add-basic scenario**

Create `tests/manual/scenarios/shared/add-basic.yml`:

```yaml
name: Shared add basic
description: daft shared add collects a file and creates symlink

repos:
  - name: test-repo
    use_fixture: standard-remote

steps:
  - name: Clone the repository
    run: git-worktree-clone --layout contained $REMOTE_TEST_REPO
    expect:
      exit_code: 0

  - name: Create an untracked .env file
    run: echo "SECRET=test" > .env && echo ".env" >> .gitignore
    cwd: "$WORK_DIR/test-repo/main"
    expect:
      exit_code: 0

  - name: Share the .env file
    run: daft shared add .env
    cwd: "$WORK_DIR/test-repo/main"
    expect:
      exit_code: 0

  - name: Verify .env is now a symlink
    run: test -L .env && cat .env
    cwd: "$WORK_DIR/test-repo/main"
    expect:
      exit_code: 0
      contains_stdout:
        - "SECRET=test"

  - name: Verify shared storage has the file
    run: cat .git/.daft/shared/.env
    cwd: "$WORK_DIR/test-repo"
    expect:
      exit_code: 0
      contains_stdout:
        - "SECRET=test"

  - name: Verify daft.yml was updated
    run: cat main/daft.yml
    cwd: "$WORK_DIR/test-repo"
    expect:
      exit_code: 0
      contains_stdout:
        - ".env"
```

- [ ] **Step 2: Create materialize-and-link scenario**

Create `tests/manual/scenarios/shared/materialize-and-link.yml`:

```yaml
name: Shared materialize and link
description: Materialize breaks the symlink, link restores it

repos:
  - name: test-repo
    use_fixture: standard-remote

steps:
  - name: Clone and setup shared file
    run: |
      git-worktree-clone --layout contained $REMOTE_TEST_REPO
      cd test-repo/main
      echo "SHARED=val" > .env
      daft shared add .env
    expect:
      exit_code: 0

  - name: Materialize .env
    run: daft shared materialize .env
    cwd: "$WORK_DIR/test-repo/main"
    expect:
      exit_code: 0

  - name: Verify .env is no longer a symlink
    run: test ! -L .env && cat .env
    cwd: "$WORK_DIR/test-repo/main"
    expect:
      exit_code: 0
      contains_stdout:
        - "SHARED=val"

  - name: Link .env back to shared
    run: daft shared link .env
    cwd: "$WORK_DIR/test-repo/main"
    expect:
      exit_code: 0

  - name: Verify .env is a symlink again
    run: test -L .env
    cwd: "$WORK_DIR/test-repo/main"
    expect:
      exit_code: 0
```

- [ ] **Step 3: Create remove scenario**

Create `tests/manual/scenarios/shared/remove.yml`:

```yaml
name: Shared remove
description:
  daft shared remove materializes everywhere then deletes shared storage

repos:
  - name: test-repo
    use_fixture: standard-remote

steps:
  - name: Clone and setup
    run: |
      git-worktree-clone --layout contained $REMOTE_TEST_REPO
      cd test-repo/main
      echo "VAL=1" > .env
      daft shared add .env
    expect:
      exit_code: 0

  - name: Create second worktree
    run: git-worktree-checkout develop
    cwd: "$WORK_DIR/test-repo"
    expect:
      exit_code: 0

  - name: Remove .env from sharing
    run: daft shared remove .env
    cwd: "$WORK_DIR/test-repo/main"
    expect:
      exit_code: 0

  - name: Verify main has a real file
    run: test ! -L .env && cat .env
    cwd: "$WORK_DIR/test-repo/main"
    expect:
      exit_code: 0
      contains_stdout:
        - "VAL=1"

  - name: Verify develop has a real file
    run: test ! -L .env && cat .env
    cwd: "$WORK_DIR/test-repo/develop"
    expect:
      exit_code: 0
      contains_stdout:
        - "VAL=1"

  - name: Verify shared storage is gone
    run: test ! -e .git/.daft/shared/.env
    cwd: "$WORK_DIR/test-repo"
    expect:
      exit_code: 0
```

- [ ] **Step 4: Create sync scenario**

Create `tests/manual/scenarios/shared/sync.yml`:

```yaml
name: Shared sync
description: daft shared sync propagates symlinks to all worktrees

repos:
  - name: test-repo
    use_fixture: standard-remote

steps:
  - name: Clone and create two worktrees
    run: |
      git-worktree-clone --layout contained $REMOTE_TEST_REPO
      cd test-repo
      git-worktree-checkout develop
    expect:
      exit_code: 0

  - name: Setup shared file from main
    run: |
      echo "SHARED=yes" > .env
      daft shared add .env
    cwd: "$WORK_DIR/test-repo/main"
    expect:
      exit_code: 0

  - name: Sync to all worktrees
    run: daft shared sync
    cwd: "$WORK_DIR/test-repo/main"
    expect:
      exit_code: 0

  - name: Verify develop has symlink
    run: test -L .env && cat .env
    cwd: "$WORK_DIR/test-repo/develop"
    expect:
      exit_code: 0
      contains_stdout:
        - "SHARED=yes"
```

- [ ] **Step 5: Create declare scenario**

Create `tests/manual/scenarios/shared/declare.yml`:

```yaml
name: Shared add --declare
description: --declare adds to daft.yml without requiring file to exist

repos:
  - name: test-repo
    use_fixture: standard-remote

steps:
  - name: Clone the repository
    run: git-worktree-clone --layout contained $REMOTE_TEST_REPO
    expect:
      exit_code: 0

  - name: Declare .env as shared
    run: daft shared add --declare .env
    cwd: "$WORK_DIR/test-repo/main"
    expect:
      exit_code: 0

  - name: Verify daft.yml has .env
    run: cat daft.yml
    cwd: "$WORK_DIR/test-repo/main"
    expect:
      exit_code: 0
      contains_stdout:
        - ".env"

  - name: Verify .gitignore has .env
    run: cat .gitignore
    cwd: "$WORK_DIR/test-repo/main"
    expect:
      exit_code: 0
      contains_stdout:
        - ".env"

  - name: Verify no file was created
    run: test ! -e .env
    cwd: "$WORK_DIR/test-repo/main"
    expect:
      exit_code: 0
```

- [ ] **Step 6: Run all shared test scenarios**

Run: `mise run test:manual -- --ci shared` Expected: All scenarios pass.

- [ ] **Step 7: Commit**

```bash
git add tests/manual/scenarios/shared/
git commit -m "test(shared): add integration test scenarios for shared files"
```

---

### Task 7: Shell completion hardcoded strings and man page

**Files:**

- Modify: `src/commands/completions/bash.rs`
- Modify: `src/commands/completions/zsh.rs`
- Modify: `src/commands/completions/fish.rs`

- [ ] **Step 1: Update shell completion strings**

Add `shared` to the `daft` subcommand lists in the hardcoded completion strings
for bash, zsh, and fish. These are the `DAFT_BASH_COMPLETIONS`,
`DAFT_ZSH_COMPLETIONS`, and `DAFT_FISH_COMPLETIONS` constants. The exact edit
depends on the format of each — search for the subcommand list (where `hooks`,
`layout`, `doctor` etc. appear) and add `shared` alphabetically.

Also add `shared` subcommands (`add`, `remove`, `materialize`, `link`, `status`,
`sync`) as second-level completions for `daft shared`.

- [ ] **Step 2: Generate man page**

Run: `mise run man:gen` Expected: Man page generated for `daft-shared`.

- [ ] **Step 3: Verify man pages are up to date**

Run: `mise run man:verify` Expected: Passes.

- [ ] **Step 4: Run full test suite**

Run: `mise run fmt && mise run clippy && mise run test:unit` Expected: All pass
with zero warnings.

- [ ] **Step 5: Commit**

```bash
git add src/commands/completions/ man/
git commit -m "feat(shared): add shell completions and man page for daft shared"
```

---

### Task 8: Final verification

- [ ] **Step 1: Run full CI simulation**

Run: `mise run ci` Expected: All checks pass.

- [ ] **Step 2: Run all integration tests**

Run: `mise run test:integration` Expected: All pass, including new shared file
scenarios.

- [ ] **Step 3: Final commit if any cleanup needed**

```bash
git add -A && git commit -m "chore(shared): final cleanup"
```
