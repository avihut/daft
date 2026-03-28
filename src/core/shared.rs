//! Shared file management across worktrees.
//!
//! Centralizes untracked configuration files (`.env`, `.idea/`, etc.) in
//! `.git/.daft/shared/` and creates symlinks in each worktree. Supports
//! materializing (copying out) per-worktree overrides and re-linking back.

use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::fs;
#[cfg(unix)]
use std::os::unix;
use std::path::{Path, PathBuf};

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
        fs::write(&path, json).with_context(|| format!("Failed to write {}", path.display()))
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
///
/// Excludes the bare repo entry (which appears in `git worktree list`
/// output but is not an actual worktree with a working tree).
pub fn list_worktree_paths() -> Result<Vec<PathBuf>> {
    let git = GitCommand::new(true);
    let porcelain = git.worktree_list_porcelain()?;
    let mut paths = Vec::new();
    let mut current_path: Option<String> = None;
    let mut is_bare = false;

    for line in porcelain.lines() {
        if let Some(path_str) = line.strip_prefix("worktree ") {
            // Flush previous entry if it wasn't bare
            if let Some(prev) = current_path.take() {
                if !is_bare {
                    paths.push(PathBuf::from(prev));
                }
            }
            current_path = Some(path_str.to_string());
            is_bare = false;
        } else if line == "bare" {
            is_bare = true;
        }
    }

    // Flush last entry
    if let Some(prev) = current_path {
        if !is_bare {
            paths.push(PathBuf::from(prev));
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
        let expected =
            relative_symlink_target(link_path.parent().unwrap_or(worktree_path), &shared_target)?;
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
                format!("Failed to create parent directory {}", parent.display())
            })?;
        }
    }

    // Create relative symlink
    let rel_target =
        relative_symlink_target(link_path.parent().unwrap_or(worktree_path), &shared_target)?;
    #[cfg(unix)]
    unix::fs::symlink(&rel_target, &link_path).with_context(|| {
        format!(
            "Failed to create symlink {} → {}",
            link_path.display(),
            rel_target.display()
        )
    })?;
    #[cfg(not(unix))]
    anyhow::bail!(
        "Shared file symlinks are not supported on this platform ({})",
        rel_path
    );

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

/// Resolve the directory where `daft.yml` and `.gitignore` live.
///
/// Checks the worktree root first (sibling layout), then falls back to
/// the project root / git common dir parent (contained layout).
/// When no `daft.yml` exists anywhere, returns the project root (where
/// new config files should be created).
pub fn resolve_config_root(worktree_root: &Path) -> PathBuf {
    if find_daft_yml(worktree_root).is_some() {
        return worktree_root.to_path_buf();
    }
    if let Ok(git_common_dir) = crate::core::repo::get_git_common_dir() {
        if let Some(project_root) = git_common_dir.parent() {
            // In contained layout, project_root is the container dir
            // In sibling layout, project_root is the common parent
            return project_root.to_path_buf();
        }
    }
    worktree_root.to_path_buf()
}

/// Read the `shared:` list from daft.yml.
///
/// Searches for daft.yml in `worktree_root` first (sibling layout), then
/// falls back to the project root (contained layout where daft.yml lives
/// at the repo container level, not inside individual worktrees).
pub fn read_shared_paths(worktree_root: &Path) -> Result<Vec<String>> {
    let config = load_yaml_config_with_fallback(worktree_root)?;
    Ok(config.and_then(|c| c.shared).unwrap_or_default())
}

/// Add paths to the `shared:` list in daft.yml.
/// Creates daft.yml if it doesn't exist. Avoids duplicates.
///
/// The `root` parameter should be the resolved config root (from
/// `resolve_config_root`), not a raw worktree path.
pub fn add_to_daft_yml(root: &Path, paths: &[&str]) -> Result<()> {
    let config_path = find_or_create_daft_yml(root)?;
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
///
/// The `root` parameter should be the resolved config root (from
/// `resolve_config_root`), not a raw worktree path.
pub fn remove_from_daft_yml(root: &Path, paths: &[&str]) -> Result<()> {
    let config_path = find_daft_yml(root);
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
fn load_yaml_config(root: &Path) -> Result<Option<crate::hooks::yaml_config::YamlConfig>> {
    let Some(path) = find_daft_yml(root) else {
        return Ok(None);
    };
    let contents = fs::read_to_string(&path)?;
    let config = serde_yaml::from_str(&contents)?;
    Ok(Some(config))
}

/// Load YamlConfig, checking `worktree_root` first, then the project root
/// (git_common_dir parent) as fallback for contained layouts.
fn load_yaml_config_with_fallback(
    worktree_root: &Path,
) -> Result<Option<crate::hooks::yaml_config::YamlConfig>> {
    // Try worktree root first (works for sibling layout where daft.yml is tracked)
    if let Some(config) = load_yaml_config(worktree_root)? {
        return Ok(Some(config));
    }

    // Fall back to project root (contained layout: daft.yml at repo container level)
    if let Ok(git_common_dir) = crate::core::repo::get_git_common_dir() {
        if let Some(project_root) = git_common_dir.parent() {
            if project_root != worktree_root {
                return load_yaml_config(project_root);
            }
        }
    }

    Ok(None)
}

// ── Uncollected file detection ───────────────────────────────────────────

/// A worktree that has a real (non-symlink) copy of a declared shared file.
#[derive(Debug, Clone)]
pub struct WorktreeCopy {
    /// Absolute path of the worktree directory.
    pub worktree_path: PathBuf,
    /// Worktree display name (directory basename).
    pub worktree_name: String,
}

/// A declared shared file that has not yet been collected into shared storage.
#[derive(Debug, Clone)]
pub struct UncollectedFile {
    /// Path relative to the worktree root (e.g., ".env").
    pub rel_path: String,
    /// Worktrees that have a real copy of this file.
    /// Empty if no worktree has it (will be stubbed).
    pub copies: Vec<WorktreeCopy>,
}

/// Scan worktrees for declared shared paths that are not yet in shared storage.
///
/// Returns one `UncollectedFile` per declared path that has no file in
/// `.git/.daft/shared/`. Each entry lists the worktrees that have a real
/// (non-symlink) copy of that file.
pub fn detect_uncollected(
    declared_paths: &[String],
    worktree_paths: &[PathBuf],
    git_common_dir: &Path,
) -> Vec<UncollectedFile> {
    let mut uncollected = Vec::new();

    for rel_path in declared_paths {
        let shared_target = shared_file_path(git_common_dir, rel_path);

        // Already collected — skip
        if shared_target.exists() {
            continue;
        }

        let mut copies = Vec::new();
        for wt in worktree_paths {
            let file_path = wt.join(rel_path);
            // Only count real files/dirs, not symlinks
            if file_path.exists() && !file_path.is_symlink() {
                let name = wt
                    .file_name()
                    .unwrap_or_default()
                    .to_string_lossy()
                    .to_string();
                copies.push(WorktreeCopy {
                    worktree_path: wt.clone(),
                    worktree_name: name,
                });
            }
        }

        uncollected.push(UncollectedFile {
            rel_path: rel_path.clone(),
            copies,
        });
    }

    uncollected
}

// ── Collection execution ─────────────────────────────────────────────────

/// Where to source the shared file from.
#[derive(Debug, Clone, PartialEq)]
pub enum CollectSource {
    /// Use the copy from this worktree.
    FromWorktree(PathBuf),
    /// No copy exists; create an empty stub.
    Stub,
}

/// A decision about how to collect a declared-but-uncollected shared file.
#[derive(Debug, Clone)]
pub struct CollectDecision {
    /// Path relative to the worktree root (e.g., ".env").
    pub rel_path: String,
    /// Where to get the file content from.
    pub source: CollectSource,
}

/// Execute a single collection decision.
///
/// 1. Moves the file from the chosen worktree to `.git/.daft/shared/`, or
///    creates an empty stub if `source` is `Stub`.
/// 2. Creates symlinks in the source worktree and any worktrees missing the file.
/// 3. Marks all other worktrees that have a real copy as materialized.
/// 4. Ensures the `.gitignore` entry exists.
pub fn execute_collect(
    decision: &CollectDecision,
    worktree_paths: &[PathBuf],
    git_common_dir: &Path,
    project_root: &Path,
    materialized: &mut MaterializedState,
) -> Result<()> {
    let rel_path = &decision.rel_path;
    let shared_target = shared_file_path(git_common_dir, rel_path);

    // Ensure parent dirs in shared storage
    if let Some(parent) = shared_target.parent() {
        fs::create_dir_all(parent)?;
    }

    match &decision.source {
        CollectSource::FromWorktree(source_wt) => {
            let source_file = source_wt.join(rel_path);

            // Move to shared storage (rename, fallback to copy+delete)
            if fs::rename(&source_file, &shared_target).is_err() {
                if source_file.is_dir() {
                    copy_dir_all(&source_file, &shared_target)?;
                    fs::remove_dir_all(&source_file)?;
                } else {
                    fs::copy(&source_file, &shared_target)?;
                    fs::remove_file(&source_file)?;
                }
            }

            // Create symlink in source worktree (file was moved out)
            create_shared_symlink(source_wt, rel_path, git_common_dir)?;
        }
        CollectSource::Stub => {
            // Create empty file
            fs::write(&shared_target, "")
                .with_context(|| format!("Failed to create stub for {rel_path}"))?;
        }
    }

    // Process remaining worktrees
    for wt in worktree_paths {
        if let CollectSource::FromWorktree(source_wt) = &decision.source {
            if wt == source_wt {
                continue;
            }
        }

        let file_path = wt.join(rel_path);
        if file_path.exists() && !file_path.is_symlink() {
            // Real copy exists — mark as materialized
            materialized.add(rel_path, wt);
        } else if !file_path.exists() {
            // No file — create symlink
            create_shared_symlink(wt, rel_path, git_common_dir)?;
        }
    }

    // Ensure .gitignore entry
    crate::core::layout::ensure_gitignore_entry(project_root, rel_path)?;

    Ok(())
}

/// Recursively copy a directory tree.
pub fn copy_dir_all(src: &Path, dst: &Path) -> Result<()> {
    fs::create_dir_all(dst)?;
    for entry in fs::read_dir(src)? {
        let entry = entry?;
        let ty = entry.file_type()?;
        let dest = dst.join(entry.file_name());
        if ty.is_dir() {
            copy_dir_all(&entry.path(), &dest)?;
        } else {
            fs::copy(entry.path(), &dest)?;
        }
    }
    Ok(())
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

/// Outcome of linking a single shared file.
pub enum LinkFileOutcome {
    /// Symlink created successfully.
    Linked(String),
    /// File already correctly linked (no action needed).
    AlreadyLinked(String),
    /// A real file exists at the path (conflict).
    Conflict(String),
    /// Failed to create symlink.
    Error(String, String),
}

/// Result of linking shared files during worktree creation.
#[derive(Default)]
pub struct LinkSharedResult {
    pub outcomes: Vec<LinkFileOutcome>,
}

impl LinkSharedResult {
    pub fn is_empty(&self) -> bool {
        self.outcomes.is_empty()
    }

    pub fn linked_count(&self) -> usize {
        self.outcomes
            .iter()
            .filter(|o| {
                matches!(
                    o,
                    LinkFileOutcome::Linked(_) | LinkFileOutcome::AlreadyLinked(_)
                )
            })
            .count()
    }
}

/// Link all declared shared files in a worktree. Called during worktree creation,
/// before PostCreate hooks.
///
/// - Reads `shared:` from daft.yml found via `project_root`.
/// - Creates symlinks for each path that exists in shared storage.
/// - Renders results immediately to stderr (before hooks take over).
/// - Never errors fatally.
pub fn link_shared_files_on_create(
    worktree_path: &Path,
    git_common_dir: &Path,
    _project_root: &Path,
) -> LinkSharedResult {
    let shared_paths = match read_shared_paths(worktree_path) {
        Ok(paths) => paths,
        Err(_) => return LinkSharedResult::default(),
    };

    if shared_paths.is_empty() {
        return LinkSharedResult::default();
    }

    let materialized = MaterializedState::load(git_common_dir).unwrap_or_default();
    let mut outcomes = Vec::new();

    for rel_path in &shared_paths {
        if materialized.is_materialized(rel_path, worktree_path) {
            continue;
        }

        match create_shared_symlink(worktree_path, rel_path, git_common_dir) {
            Ok(LinkResult::Created) => {
                outcomes.push(LinkFileOutcome::Linked(rel_path.clone()));
            }
            Ok(LinkResult::AlreadyLinked) => {
                outcomes.push(LinkFileOutcome::AlreadyLinked(rel_path.clone()));
            }
            Ok(LinkResult::Conflict) => {
                outcomes.push(LinkFileOutcome::Conflict(rel_path.clone()));
            }
            Ok(LinkResult::NoSource) => {} // Declared only, skip silently
            Err(e) => {
                outcomes.push(LinkFileOutcome::Error(rel_path.clone(), e.to_string()));
            }
        }
    }

    let result = LinkSharedResult { outcomes };
    render_link_results(&result);
    result
}

/// Render shared file linking results to stderr with colors.
///
/// Clears the current line first to avoid leaving spinner artifacts,
/// since this may be called while a spinner is active.
pub fn render_link_results(result: &LinkSharedResult) {
    use crate::styles;

    if result.is_empty() {
        return;
    }

    // Clear the current line (wipe spinner ghost) and move cursor to start
    if std::io::IsTerminal::is_terminal(&std::io::stderr()) {
        eprint!("\r\x1b[2K");
    }

    let use_color = styles::colors_enabled_stderr();

    for outcome in &result.outcomes {
        match outcome {
            LinkFileOutcome::Linked(path) => {
                if use_color {
                    eprintln!("{}Linked{} {}", styles::GREEN, styles::RESET, path,);
                } else {
                    eprintln!("Linked {}", path);
                }
            }
            LinkFileOutcome::AlreadyLinked(_) => {} // Silent
            LinkFileOutcome::Conflict(path) => {
                if use_color {
                    eprintln!(
                        "{}warning:{} '{}' exists but is not shared. Run `daft shared link {}` to replace.",
                        styles::YELLOW,
                        styles::RESET,
                        path,
                        path,
                    );
                } else {
                    eprintln!(
                        "warning: '{}' exists but is not shared. Run `daft shared link {}` to replace.",
                        path, path,
                    );
                }
            }
            LinkFileOutcome::Error(path, err) => {
                if use_color {
                    eprintln!(
                        "{}warning:{} Failed to link shared file '{}': {}",
                        styles::YELLOW,
                        styles::RESET,
                        path,
                        err,
                    );
                } else {
                    eprintln!("warning: Failed to link shared file '{}': {}", path, err);
                }
            }
        }
    }
}

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
        assert!(!state.0.contains_key(".env"));
    }

    #[test]
    fn test_relative_symlink_target_sibling() {
        let dir = tempdir().unwrap();
        // Create real directories so canonicalize works
        let from = dir.path().join("repo.feat");
        let git_dir = dir.path().join(".git").join(".daft").join("shared");
        fs::create_dir_all(&from).unwrap();
        fs::create_dir_all(&git_dir).unwrap();
        let to = git_dir.join(".env");
        fs::write(&to, "").unwrap();

        let rel = relative_symlink_target(&from, &to).unwrap();
        // from: <tmpdir>/repo.feat → to: <tmpdir>/.git/.daft/shared/.env
        // expected: ../.git/.daft/shared/.env
        assert_eq!(rel, PathBuf::from("../.git/.daft/shared/.env"));
    }

    #[test]
    fn test_relative_symlink_target_nested() {
        let dir = tempdir().unwrap();
        // Create real directories so canonicalize works
        let from = dir.path().join("repo.feat").join(".vscode");
        let git_dir = dir
            .path()
            .join(".git")
            .join(".daft")
            .join("shared")
            .join(".vscode");
        fs::create_dir_all(&from).unwrap();
        fs::create_dir_all(&git_dir).unwrap();
        let to = git_dir.join("settings.json");
        fs::write(&to, "{}").unwrap();

        let rel = relative_symlink_target(&from, &to).unwrap();
        // from: <tmpdir>/repo.feat/.vscode → to: <tmpdir>/.git/.daft/shared/.vscode/settings.json
        // expected: ../../.git/.daft/shared/.vscode/settings.json
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

        let result = create_shared_symlink(&worktree, ".env", &git_common_dir).unwrap();
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

        let result = create_shared_symlink(&worktree, ".env", &git_common_dir).unwrap();
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

        let result = create_shared_symlink(&worktree, ".env", &git_common_dir).unwrap();
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

        let result =
            create_shared_symlink(&worktree, ".vscode/settings.json", &git_common_dir).unwrap();
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

    // ── detect_uncollected tests ─────────────────────────────────────────

    /// Helper: create a minimal directory structure for testing.
    fn setup_test_repo(
        worktree_names: &[&str],
    ) -> (tempfile::TempDir, PathBuf, PathBuf, Vec<PathBuf>) {
        let tmp = tempfile::TempDir::new().unwrap();
        let root = tmp.path().to_path_buf();
        let git_dir = root.join(".git");
        fs::create_dir_all(git_dir.join(".daft/shared")).unwrap();

        let mut wt_paths = Vec::new();
        for name in worktree_names {
            let wt = root.join(name);
            fs::create_dir_all(&wt).unwrap();
            wt_paths.push(wt);
        }

        (tmp, git_dir, root, wt_paths)
    }

    #[test]
    fn detect_uncollected_finds_copies_across_worktrees() {
        let (_tmp, git_dir, _root, wt_paths) = setup_test_repo(&["main", "develop"]);
        fs::write(wt_paths[0].join(".env"), "FROM_MAIN=1").unwrap();
        fs::write(wt_paths[1].join(".env"), "FROM_DEVELOP=1").unwrap();

        let declared = vec![".env".to_string()];
        let result = detect_uncollected(&declared, &wt_paths, &git_dir);

        assert_eq!(result.len(), 1);
        assert_eq!(result[0].rel_path, ".env");
        assert_eq!(result[0].copies.len(), 2);
        assert_eq!(result[0].copies[0].worktree_path, wt_paths[0]);
        assert_eq!(result[0].copies[1].worktree_path, wt_paths[1]);
    }

    #[test]
    fn detect_uncollected_skips_already_collected() {
        let (_tmp, git_dir, _root, wt_paths) = setup_test_repo(&["main"]);
        fs::write(shared_file_path(&git_dir, ".env"), "SHARED=1").unwrap();
        fs::write(wt_paths[0].join(".env"), "LOCAL=1").unwrap();

        let declared = vec![".env".to_string()];
        let result = detect_uncollected(&declared, &wt_paths, &git_dir);
        assert!(result.is_empty());
    }

    #[test]
    fn detect_uncollected_file_in_no_worktree() {
        let (_tmp, git_dir, _root, wt_paths) = setup_test_repo(&["main"]);

        let declared = vec![".secrets".to_string()];
        let result = detect_uncollected(&declared, &wt_paths, &git_dir);

        assert_eq!(result.len(), 1);
        assert_eq!(result[0].rel_path, ".secrets");
        assert!(result[0].copies.is_empty());
    }

    // ── execute_collect tests ────────────────────────────────────────────

    #[test]
    fn execute_collect_moves_to_shared_and_links() {
        let (_tmp, git_dir, root, wt_paths) = setup_test_repo(&["main", "develop", "feature"]);

        fs::write(wt_paths[0].join(".env"), "FROM_MAIN=1").unwrap();
        fs::write(wt_paths[1].join(".env"), "FROM_DEVELOP=1").unwrap();
        // Create .gitignore so ensure_gitignore_entry doesn't fail
        fs::write(root.join(".gitignore"), "").unwrap();

        let decision = CollectDecision {
            rel_path: ".env".to_string(),
            source: CollectSource::FromWorktree(wt_paths[0].clone()),
        };

        let mut materialized = MaterializedState::default();
        execute_collect(&decision, &wt_paths, &git_dir, &root, &mut materialized).unwrap();

        // Shared storage should have the file from main
        let shared = shared_file_path(&git_dir, ".env");
        assert!(shared.exists());
        assert_eq!(fs::read_to_string(&shared).unwrap(), "FROM_MAIN=1");

        // main: should now be a symlink
        let main_env = wt_paths[0].join(".env");
        assert!(main_env.is_symlink());

        // develop: should still be a real file, marked materialized
        let dev_env = wt_paths[1].join(".env");
        assert!(!dev_env.is_symlink());
        assert!(materialized.is_materialized(".env", &wt_paths[1]));

        // feature: should be a symlink (had no file)
        let feat_env = wt_paths[2].join(".env");
        assert!(feat_env.is_symlink());
    }

    #[test]
    fn execute_collect_stubs_when_no_source() {
        let (_tmp, git_dir, root, wt_paths) = setup_test_repo(&["main"]);
        fs::write(root.join(".gitignore"), "").unwrap();

        let decision = CollectDecision {
            rel_path: ".secrets".to_string(),
            source: CollectSource::Stub,
        };

        let mut materialized = MaterializedState::default();
        execute_collect(&decision, &wt_paths, &git_dir, &root, &mut materialized).unwrap();

        let shared = shared_file_path(&git_dir, ".secrets");
        assert!(shared.exists());
        assert_eq!(fs::read_to_string(&shared).unwrap(), "");

        let main_secrets = wt_paths[0].join(".secrets");
        assert!(main_secrets.is_symlink());
    }

    #[test]
    fn execute_collect_ensures_gitignore_entry() {
        let (_tmp, git_dir, root, wt_paths) = setup_test_repo(&["main"]);

        fs::write(wt_paths[0].join(".env"), "VAL=1").unwrap();
        fs::write(root.join(".gitignore"), "*.log\n").unwrap();

        let decision = CollectDecision {
            rel_path: ".env".to_string(),
            source: CollectSource::FromWorktree(wt_paths[0].clone()),
        };

        let mut materialized = MaterializedState::default();
        execute_collect(&decision, &wt_paths, &git_dir, &root, &mut materialized).unwrap();

        let gitignore = fs::read_to_string(root.join(".gitignore")).unwrap();
        assert!(gitignore.contains(".env"));
    }

    #[test]
    fn detect_uncollected_ignores_symlinked_worktrees() {
        let (_tmp, git_dir, _root, wt_paths) = setup_test_repo(&["main"]);
        let shared_target = shared_file_path(&git_dir, ".env");
        fs::write(&shared_target, "SHARED=1").unwrap();

        #[cfg(unix)]
        std::os::unix::fs::symlink(&shared_target, wt_paths[0].join(".env")).unwrap();

        let declared = vec![".env".to_string()];
        let result = detect_uncollected(&declared, &wt_paths, &git_dir);
        // .env IS in shared storage, so detect_uncollected skips it entirely
        assert!(result.is_empty());
    }
}
