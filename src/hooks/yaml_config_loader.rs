//! Config discovery, loading, and merging for YAML hooks configuration.
//!
//! This module handles finding the right config file, loading it, and
//! merging multiple config sources (main, extends, per-hook, local).

use super::config_merge::merge_configs;
use super::yaml_config::{HookDef, JobDef, YamlConfig};
use anyhow::{Context, Result};
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::process::Command;

/// Where the main config file was found, which determines where
/// per-hook and local config files are located.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ConfigLocation {
    /// Config at repo root (e.g., `daft.yml`, `.daft.yml`).
    Root,
    /// Config in `.config/daft/` directory (e.g., `.config/daft.yml`).
    DotConfig,
}

/// Config file candidates in priority order (first match wins). Canonical —
/// other probes for a daft config by filename (the shared-paths ref-blob
/// probe) iterate this list rather than keeping their own copy.
pub(crate) const CONFIG_CANDIDATES: &[(&str, ConfigLocation)] = &[
    ("daft.yml", ConfigLocation::Root),
    ("daft.yaml", ConfigLocation::Root),
    (".daft.yml", ConfigLocation::Root),
    (".daft.yaml", ConfigLocation::Root),
    (".config/daft.yml", ConfigLocation::DotConfig),
    (".config/daft.yaml", ConfigLocation::DotConfig),
];

/// Hook names that can have per-hook YAML files.
const PER_HOOK_NAMES: &[&str] = &[
    "post-clone",
    "worktree-pre-create",
    "worktree-post-create",
    "worktree-pre-remove",
    "worktree-post-remove",
];

/// Find the main config file in the given repository root.
///
/// Returns the path and location type if found.
pub fn find_config_file(root: &Path) -> Option<(PathBuf, ConfigLocation)> {
    for (candidate, location) in CONFIG_CANDIDATES {
        let path = root.join(candidate);
        if path.is_file() {
            return Some((path, location.clone()));
        }
    }
    None
}

/// Find the local override config file for the given main config.
///
/// Searches in priority order:
///   1. Preferred dot-infix name (e.g. `daft.local.yml`)
///   2. Deprecated dash-infix alias (e.g. `daft-local.yml`)
///
/// When the deprecated alias is used, emits a warning to stderr so users see
/// the rename suggestion at load time.
///
/// Returns the path if found.
pub fn find_local_config(main_config: &Path) -> Option<PathBuf> {
    let parent = main_config.parent()?;
    let filename = main_config.file_name()?.to_str()?;

    let (stem, ext) = if let Some(s) = filename.strip_suffix(".yaml") {
        (s, ".yaml")
    } else if let Some(s) = filename.strip_suffix(".yml") {
        (s, ".yml")
    } else {
        return None;
    };

    let preferred_name = format!("{stem}.local{ext}");
    let preferred = parent.join(&preferred_name);
    if preferred.is_file() {
        return Some(preferred);
    }

    let deprecated_name = format!("{stem}-local{ext}");
    let deprecated = parent.join(&deprecated_name);
    if deprecated.is_file() {
        crate::log_warning!(
            "deprecated local config name '{}' — rename to '{}'",
            deprecated_name,
            preferred_name
        );
        return Some(deprecated);
    }

    None
}

/// Find per-hook YAML config files based on the main config location.
///
/// Returns a map of hook name → file path.
pub fn find_per_hook_configs(root: &Path, location: &ConfigLocation) -> HashMap<String, PathBuf> {
    let mut result = HashMap::new();

    let dir = match location {
        ConfigLocation::Root => root.to_path_buf(),
        ConfigLocation::DotConfig => root.join(".config").join("daft"),
    };

    if !dir.is_dir() {
        return result;
    }

    for hook_name in PER_HOOK_NAMES {
        // Check .yml first, then .yaml
        for ext in &["yml", "yaml"] {
            let path = dir.join(format!("{hook_name}.{ext}"));
            if path.is_file() {
                result.insert(hook_name.to_string(), path);
                break; // first extension wins
            }
        }
    }

    result
}

/// Load and merge all config sources for the given repository root.
///
/// Merge order (lowest → highest precedence):
/// 1. Main config (`daft.yml`)
/// 2. Extends files
/// 3. Per-hook YAML files (`post-clone.yml`)
/// 4. Local override (`daft.local.yml`, or deprecated `daft-local.yml`)
///
/// Returns `None` if no config file is found.
pub fn load_merged_config(root: &Path) -> Result<Option<YamlConfig>> {
    let (config_path, location) = match find_config_file(root) {
        Some(found) => found,
        None => return Ok(None),
    };

    // 1. Load main config
    let mut config = load_yaml_config(&config_path)?;

    // 2. Process extends
    if let Some(extends) = config.extends.take() {
        let config_dir = config_path.parent().unwrap_or(root);
        for ext_file in &extends {
            let ext_path = config_dir.join(ext_file);
            if ext_path.is_file() {
                let ext_config = load_yaml_config(&ext_path).with_context(|| {
                    format!("Failed to load extends file: {}", ext_path.display())
                })?;
                config = merge_configs(config, ext_config);
            }
        }
    }

    // 3. Merge per-hook files
    let per_hook_configs = find_per_hook_configs(root, &location);
    for (hook_name, hook_path) in &per_hook_configs {
        let hook_def: HookDef = load_yaml_file(hook_path)
            .with_context(|| format!("Failed to load per-hook file: {}", hook_path.display()))?;
        config.hooks.insert(hook_name.clone(), hook_def);
    }

    // 4. Merge local override
    if let Some(local_path) = find_local_config(&config_path) {
        let local_config = load_yaml_config(&local_path)
            .with_context(|| format!("Failed to load local config: {}", local_path.display()))?;
        config = merge_configs(config, local_config);
    }

    // Convert any legacy `commands` to `jobs`
    normalize_commands_to_jobs(&mut config);

    Ok(Some(config))
}

/// Load a YAML file and deserialize it into the given type.
fn load_yaml_file<T: serde::de::DeserializeOwned>(path: &Path) -> Result<T> {
    let contents = std::fs::read_to_string(path)
        .with_context(|| format!("Failed to read file: {}", path.display()))?;
    serde_yaml::from_str(&contents)
        .with_context(|| format!("Failed to parse YAML file: {}", path.display()))
}

/// Load a YamlConfig from a file.
fn load_yaml_config(path: &Path) -> Result<YamlConfig> {
    load_yaml_file(path)
}

/// Convert legacy `commands` maps to `jobs` lists in all hook definitions.
fn normalize_commands_to_jobs(config: &mut YamlConfig) {
    for hook_def in config.hooks.values_mut() {
        if let Some(commands) = hook_def.commands.take() {
            let new_jobs: Vec<JobDef> = commands
                .iter()
                .map(|(name, cmd)| cmd.to_job_def(name))
                .collect();

            if let Some(ref mut jobs) = hook_def.jobs {
                jobs.extend(new_jobs);
            } else {
                hook_def.jobs = Some(new_jobs);
            }
        }
    }
}

/// Get the effective jobs for a hook definition, resolving commands to jobs.
pub fn get_effective_jobs(hook_def: &HookDef) -> Vec<JobDef> {
    let mut jobs = hook_def.jobs.clone().unwrap_or_default();

    // Also include legacy commands
    if let Some(ref commands) = hook_def.commands {
        for (name, cmd) in commands {
            jobs.push(cmd.to_job_def(name));
        }
    }

    jobs
}

/// Runtime classification of the main `daft.yml` based on git tracking status.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ConfigStatus {
    /// `daft.yml` is committed to the repo (team config).
    Tracked,
    /// `daft.yml` exists in the worktree but is not tracked (visitor config).
    Visitor,
    /// No `daft.yml` discovered at any candidate path.
    Missing,
}

/// Classify the main daft config in `worktree_root` by tracking status.
///
/// Returns `Missing` if no candidate file exists. Otherwise checks whether the
/// directory is a git worktree, then runs
/// `git ls-files --error-unmatch <relative-path>` and maps the exit status:
/// success → `Tracked`, failure → `Visitor`. If the git invocation itself
/// errors (no git binary, not inside a repo), returns `Tracked` as a
/// conservative fallback — we'd rather not implicitly treat a file as visitor
/// when we can't confirm.
pub fn classify_main_config(worktree_root: &Path) -> ConfigStatus {
    let (path, _location) = match find_config_file(worktree_root) {
        Some(found) => found,
        None => return ConfigStatus::Missing,
    };

    let relative = match path.strip_prefix(worktree_root) {
        Ok(p) => p,
        Err(_) => return ConfigStatus::Tracked,
    };

    // Conservative fallback: if we can't confirm we're inside a git repo,
    // treat the file as tracked. This handles the case where git is installed
    // but the directory is not a git repository (git exits with code 128).
    let in_repo = crate::utils::git_command_at(worktree_root)
        .args(["rev-parse", "--is-inside-work-tree"])
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .status();

    if !matches!(in_repo, Ok(s) if s.success()) {
        return ConfigStatus::Tracked;
    }

    let status = crate::utils::git_command_at(worktree_root)
        .args(["ls-files", "--error-unmatch"])
        .arg(relative)
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .status();

    match status {
        Ok(s) if s.success() => ConfigStatus::Tracked,
        Ok(_) => ConfigStatus::Visitor,
        Err(_) => ConfigStatus::Tracked,
    }
}

/// Parse a YAML string into a `YamlConfig`.
///
/// This is a standalone parser that does not process `extends`, per-hook files,
/// or local overrides — those are filesystem concepts that don't apply when
/// reading config from a git object (branch ref). Legacy `commands` maps are
/// normalized to `jobs`.
pub fn parse_yaml_config_str(yaml: &str) -> Result<YamlConfig> {
    let mut config: YamlConfig =
        serde_yaml::from_str(yaml).context("Failed to parse YAML config")?;
    normalize_commands_to_jobs(&mut config);
    Ok(config)
}

/// Read a file from a git ref using `git show <ref>:<path>`.
///
/// Returns `Some(content)` if the file exists on the given ref, `None` if it
/// does not (git exits non-zero). Runs git with `-C <git_dir>` so it works
/// without being inside a worktree.
fn git_show_file(git_dir: &Path, ref_name: &str, file_path: &str) -> Option<String> {
    let output = Command::new("git")
        .arg("-C")
        .arg(git_dir)
        .args(["show", &format!("{ref_name}:{file_path}")])
        .stderr(std::process::Stdio::null())
        .output()
        .ok()?;

    if output.status.success() {
        String::from_utf8(output.stdout).ok()
    } else {
        None
    }
}

/// Detect the default branch from the local repository's remote HEAD reference.
///
/// Reads `refs/remotes/origin/HEAD` from the git common directory. Falls back
/// to `git symbolic-ref refs/remotes/origin/HEAD` if the file is not present.
/// Returns `None` if the default branch cannot be determined.
fn detect_default_branch(git_dir: &Path) -> Option<String> {
    // Try reading the file directly (most common case)
    let head_ref_file = git_dir.join("refs/remotes/origin/HEAD");
    if let Ok(content) = std::fs::read_to_string(&head_ref_file) {
        let content = content.trim();
        if let Some(ref_path) = content.strip_prefix("ref: ")
            && let Some(branch) = ref_path.strip_prefix("refs/remotes/origin/")
            && !branch.is_empty()
        {
            return Some(branch.to_string());
        }
    }

    // Fallback: ask git to resolve the symbolic ref
    let output = Command::new("git")
        .arg("-C")
        .arg(git_dir)
        .args(["symbolic-ref", "refs/remotes/origin/HEAD"])
        .stderr(std::process::Stdio::null())
        .output()
        .ok()?;

    if output.status.success() {
        let stdout = String::from_utf8(output.stdout).ok()?;
        let trimmed = stdout.trim();
        trimmed
            .strip_prefix("refs/remotes/origin/")
            .map(|s| s.to_string())
    } else {
        None
    }
}

/// Try to load a `YamlConfig` from a single branch ref.
///
/// Searches config file candidates in priority order using `git show`.
/// Returns `None` if no config file is found on the given ref.
fn try_load_config_from_ref(git_dir: &Path, ref_name: &str) -> Result<Option<YamlConfig>> {
    for (candidate, _location) in CONFIG_CANDIDATES {
        if let Some(content) = git_show_file(git_dir, ref_name, candidate) {
            let config = parse_yaml_config_str(&content)
                .with_context(|| format!("Failed to parse {candidate} from ref {ref_name}"))?;
            return Ok(Some(config));
        }
    }
    Ok(None)
}

/// Load daft.yml from a bare repository's HEAD ref.
///
/// Used by the clone command to read the team's layout preference before
/// deciding the final layout. Searches all config file candidates
/// (daft.yml, daft.yaml, .daft.yml, etc.) in priority order.
///
/// Returns `None` if no config file is found on HEAD.
pub fn load_config_from_bare(git_dir: &Path) -> Result<Option<YamlConfig>> {
    try_load_config_from_ref(git_dir, "HEAD")
}

/// Load a `YamlConfig` from a branch ref with a fallback chain.
///
/// This is used for hooks that need config from a branch that may not have a
/// worktree checked out yet (e.g., `worktree-pre-create` where the target
/// worktree does not exist).
///
/// The fallback chain is:
/// 1. `target_branch` — the branch being checked out
/// 2. `base_branch` — the branch the new worktree is based on (if provided)
/// 3. The repository's default branch (detected from `refs/remotes/origin/HEAD`)
///
/// Extends, per-hook files, and local overrides are **not** applied — those are
/// filesystem concepts that require a worktree checkout.
///
/// # Arguments
/// * `git_dir` — path to the git common directory (`.git` or bare repo root)
/// * `target_branch` — the branch to try first
/// * `base_branch` — optional fallback branch (e.g., the source branch)
pub fn load_config_from_branch(
    git_dir: &Path,
    target_branch: &str,
    base_branch: Option<&str>,
) -> Result<Option<YamlConfig>> {
    // 1. Try the target branch
    if let Some(config) = try_load_config_from_ref(git_dir, target_branch)? {
        return Ok(Some(config));
    }

    // 2. Try the base branch if provided (and different from target)
    if let Some(base) = base_branch
        && base != target_branch
        && let Some(config) = try_load_config_from_ref(git_dir, base)?
    {
        return Ok(Some(config));
    }

    // 3. Try the default branch
    if let Some(default_branch) = detect_default_branch(git_dir) {
        // Avoid re-checking branches we already tried
        if default_branch != target_branch
            && base_branch != Some(default_branch.as_str())
            && let Some(config) = try_load_config_from_ref(git_dir, &default_branch)?
        {
            return Ok(Some(config));
        }
    }

    Ok(None)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::hooks::yaml_config::{CommandDef, RunCommand};
    use std::fs;
    use tempfile::tempdir;

    fn write_file(dir: &Path, name: &str, content: &str) {
        let path = dir.join(name);
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent).unwrap();
        }
        fs::write(path, content).unwrap();
    }

    #[test]
    fn test_find_config_file_daft_yml() {
        let dir = tempdir().unwrap();
        write_file(dir.path(), "daft.yml", "hooks: {}");

        let result = find_config_file(dir.path());
        assert!(result.is_some());
        let (path, location) = result.unwrap();
        assert_eq!(path, dir.path().join("daft.yml"));
        assert_eq!(location, ConfigLocation::Root);
    }

    #[test]
    fn test_find_config_file_priority() {
        let dir = tempdir().unwrap();
        // Both daft.yml and .daft.yml exist; daft.yml should win
        write_file(dir.path(), "daft.yml", "hooks: {}");
        write_file(dir.path(), ".daft.yml", "hooks: {}");

        let (path, _) = find_config_file(dir.path()).unwrap();
        assert_eq!(path, dir.path().join("daft.yml"));
    }

    #[test]
    fn test_find_config_file_dot_config() {
        let dir = tempdir().unwrap();
        write_file(dir.path(), ".config/daft.yml", "hooks: {}");

        let (path, location) = find_config_file(dir.path()).unwrap();
        assert_eq!(path, dir.path().join(".config/daft.yml"));
        assert_eq!(location, ConfigLocation::DotConfig);
    }

    #[test]
    fn test_find_config_file_none() {
        let dir = tempdir().unwrap();
        assert!(find_config_file(dir.path()).is_none());
    }

    #[test]
    fn test_find_local_config() {
        // NOTE: exercises the deprecated dash-infix code path and will emit a
        // "Warning: deprecated local config name..." line to stderr.
        let dir = tempdir().unwrap();
        let main_config = dir.path().join("daft.yml");
        write_file(dir.path(), "daft.yml", "hooks: {}");
        write_file(dir.path(), "daft-local.yml", "hooks: {}");

        let local = find_local_config(&main_config);
        assert!(local.is_some());
        assert_eq!(local.unwrap(), dir.path().join("daft-local.yml"));
    }

    #[test]
    fn test_find_local_config_dot_prefix() {
        // NOTE: exercises the deprecated dash-infix code path and will emit a
        // "Warning: deprecated local config name..." line to stderr.
        let dir = tempdir().unwrap();
        let main_config = dir.path().join(".daft.yml");
        write_file(dir.path(), ".daft.yml", "hooks: {}");
        write_file(dir.path(), ".daft-local.yml", "hooks: {}");

        let local = find_local_config(&main_config);
        assert!(local.is_some());
        assert_eq!(local.unwrap(), dir.path().join(".daft-local.yml"));
    }

    #[test]
    fn test_find_local_config_none() {
        let dir = tempdir().unwrap();
        let main_config = dir.path().join("daft.yml");
        write_file(dir.path(), "daft.yml", "hooks: {}");

        assert!(find_local_config(&main_config).is_none());
    }

    #[test]
    fn test_find_local_config_prefers_dot_infix() {
        let dir = tempdir().unwrap();
        let main_config = dir.path().join("daft.yml");
        write_file(dir.path(), "daft.yml", "hooks: {}");
        write_file(dir.path(), "daft.local.yml", "hooks: {}");
        write_file(dir.path(), "daft-local.yml", "hooks: {}");

        let local = find_local_config(&main_config).unwrap();
        assert_eq!(local, dir.path().join("daft.local.yml"));
    }

    #[test]
    fn test_find_local_config_falls_back_to_dash_infix() {
        // NOTE: exercises the deprecated dash-infix code path and will emit a
        // "Warning: deprecated local config name..." line to stderr.
        let dir = tempdir().unwrap();
        let main_config = dir.path().join("daft.yml");
        write_file(dir.path(), "daft.yml", "hooks: {}");
        write_file(dir.path(), "daft-local.yml", "hooks: {}");

        let local = find_local_config(&main_config).unwrap();
        assert_eq!(local, dir.path().join("daft-local.yml"));
    }

    #[test]
    fn test_find_local_config_dot_prefix_main_dot_infix_local() {
        let dir = tempdir().unwrap();
        let main_config = dir.path().join(".daft.yml");
        write_file(dir.path(), ".daft.yml", "hooks: {}");
        write_file(dir.path(), ".daft.local.yml", "hooks: {}");

        let local = find_local_config(&main_config).unwrap();
        assert_eq!(local, dir.path().join(".daft.local.yml"));
    }

    #[test]
    fn test_find_local_config_yaml_extension_dot_infix() {
        let dir = tempdir().unwrap();
        let main_config = dir.path().join("daft.yaml");
        write_file(dir.path(), "daft.yaml", "hooks: {}");
        write_file(dir.path(), "daft.local.yaml", "hooks: {}");

        let local = find_local_config(&main_config).unwrap();
        assert_eq!(local, dir.path().join("daft.local.yaml"));
    }

    #[test]
    fn test_find_per_hook_configs_root() {
        let dir = tempdir().unwrap();
        write_file(
            dir.path(),
            "post-clone.yml",
            "jobs:\n  - name: test\n    run: echo test",
        );
        write_file(
            dir.path(),
            "worktree-post-create.yml",
            "jobs:\n  - name: test\n    run: echo test",
        );

        let result = find_per_hook_configs(dir.path(), &ConfigLocation::Root);
        assert_eq!(result.len(), 2);
        assert!(result.contains_key("post-clone"));
        assert!(result.contains_key("worktree-post-create"));
    }

    #[test]
    fn test_find_per_hook_configs_dot_config() {
        let dir = tempdir().unwrap();
        let daft_dir = dir.path().join(".config").join("daft");
        fs::create_dir_all(&daft_dir).unwrap();
        write_file(
            &daft_dir,
            "post-clone.yml",
            "jobs:\n  - name: test\n    run: echo test",
        );

        let result = find_per_hook_configs(dir.path(), &ConfigLocation::DotConfig);
        assert_eq!(result.len(), 1);
        assert!(result.contains_key("post-clone"));
    }

    #[test]
    fn test_load_merged_config_basic() {
        let dir = tempdir().unwrap();
        write_file(
            dir.path(),
            "daft.yml",
            r#"
hooks:
  worktree-post-create:
    jobs:
      - name: setup
        run: echo "hello"
"#,
        );

        let config = load_merged_config(dir.path()).unwrap();
        assert!(config.is_some());
        let config = config.unwrap();
        assert!(config.hooks.contains_key("worktree-post-create"));
    }

    #[test]
    fn test_load_merged_config_with_local() {
        let dir = tempdir().unwrap();
        write_file(
            dir.path(),
            "daft.yml",
            r#"
hooks:
  worktree-post-create:
    jobs:
      - name: setup
        run: echo "base"
"#,
        );
        write_file(
            dir.path(),
            "daft-local.yml",
            r#"
hooks:
  worktree-post-create:
    jobs:
      - name: setup
        run: echo "local override"
"#,
        );

        let config = load_merged_config(dir.path()).unwrap().unwrap();
        let jobs = config.hooks["worktree-post-create"].jobs.as_ref().unwrap();
        assert_eq!(
            jobs[0]
                .run
                .as_ref()
                .and_then(|r| r.resolve_for_current_os()),
            Some("echo \"local override\"".to_string())
        );
    }

    #[test]
    fn test_load_merged_config_with_per_hook_file() {
        let dir = tempdir().unwrap();
        write_file(dir.path(), "daft.yml", "hooks: {}");
        write_file(
            dir.path(),
            "post-clone.yml",
            r#"
jobs:
  - name: init
    run: npm install
"#,
        );

        let config = load_merged_config(dir.path()).unwrap().unwrap();
        assert!(config.hooks.contains_key("post-clone"));
        let jobs = config.hooks["post-clone"].jobs.as_ref().unwrap();
        assert_eq!(
            jobs[0]
                .run
                .as_ref()
                .and_then(|r| r.resolve_for_current_os()),
            Some("npm install".to_string())
        );
    }

    #[test]
    fn test_load_merged_config_no_file() {
        let dir = tempdir().unwrap();
        let config = load_merged_config(dir.path()).unwrap();
        assert!(config.is_none());
    }

    #[test]
    fn test_load_merged_config_with_extends() {
        let dir = tempdir().unwrap();
        write_file(
            dir.path(),
            "daft.yml",
            r#"
extends:
  - shared.yml
hooks:
  worktree-post-create:
    jobs:
      - name: local-lint
        run: cargo clippy
"#,
        );
        write_file(
            dir.path(),
            "shared.yml",
            r#"
hooks:
  worktree-post-create:
    jobs:
      - name: shared-lint
        run: eslint .
"#,
        );

        let config = load_merged_config(dir.path()).unwrap().unwrap();
        let jobs = config.hooks["worktree-post-create"].jobs.as_ref().unwrap();
        // shared-lint from extends + local-lint from main
        assert_eq!(jobs.len(), 2);
    }

    #[test]
    fn test_normalize_commands_to_jobs() {
        let dir = tempdir().unwrap();
        write_file(
            dir.path(),
            "daft.yml",
            r#"
hooks:
  worktree-post-create:
    commands:
      lint:
        run: cargo clippy
      format:
        run: cargo fmt --check
"#,
        );

        let config = load_merged_config(dir.path()).unwrap().unwrap();
        let hook = &config.hooks["worktree-post-create"];
        // commands should have been converted to jobs
        assert!(hook.commands.is_none());
        let jobs = hook.jobs.as_ref().unwrap();
        assert_eq!(jobs.len(), 2);
        assert!(jobs.iter().any(|j| j.name.as_deref() == Some("lint")));
        assert!(jobs.iter().any(|j| j.name.as_deref() == Some("format")));
    }

    #[test]
    fn test_get_effective_jobs() {
        let hook = HookDef {
            jobs: Some(vec![JobDef {
                name: Some("job1".to_string()),
                run: Some(RunCommand::Simple("echo 1".to_string())),
                ..Default::default()
            }]),
            commands: Some({
                let mut map = HashMap::new();
                map.insert(
                    "cmd1".to_string(),
                    CommandDef {
                        run: Some("echo cmd1".to_string()),
                        ..Default::default()
                    },
                );
                map
            }),
            ..Default::default()
        };

        let jobs = get_effective_jobs(&hook);
        assert_eq!(jobs.len(), 2);
    }

    // ── Tests for parse_yaml_config_str ──────────────────────────────

    #[test]
    fn test_parse_yaml_config_str_basic() {
        let yaml = r#"
hooks:
  worktree-post-create:
    jobs:
      - name: setup
        run: echo "hello"
"#;
        let config = parse_yaml_config_str(yaml).unwrap();
        assert!(config.hooks.contains_key("worktree-post-create"));
        let jobs = config.hooks["worktree-post-create"].jobs.as_ref().unwrap();
        assert_eq!(jobs.len(), 1);
        assert_eq!(jobs[0].name.as_deref(), Some("setup"));
    }

    #[test]
    fn test_parse_yaml_config_str_normalizes_commands() {
        let yaml = r#"
hooks:
  worktree-post-create:
    commands:
      lint:
        run: cargo clippy
"#;
        let config = parse_yaml_config_str(yaml).unwrap();
        let hook = &config.hooks["worktree-post-create"];
        // commands should be normalized to jobs
        assert!(hook.commands.is_none());
        let jobs = hook.jobs.as_ref().unwrap();
        assert_eq!(jobs.len(), 1);
        assert_eq!(jobs[0].name.as_deref(), Some("lint"));
    }

    #[test]
    fn test_parse_yaml_config_str_empty() {
        let config = parse_yaml_config_str("").unwrap();
        assert!(config.hooks.is_empty());
    }

    #[test]
    fn test_parse_yaml_config_str_invalid() {
        let result = parse_yaml_config_str("{{invalid yaml");
        assert!(result.is_err());
    }

    // ── Tests for git_show_file ──────────────────────────────────────

    /// Helper: create a bare git repo with a committed file.
    fn create_test_repo_with_file(
        dir: &Path,
        branch: &str,
        file_name: &str,
        content: &str,
    ) -> PathBuf {
        let repo_dir = dir.join("repo.git");

        // Init a bare repo
        Command::new("git")
            .args(["init", "--bare"])
            .arg(&repo_dir)
            .output()
            .unwrap();

        // Create a temporary worktree to make commits
        let work_dir = dir.join("work");
        fs::create_dir_all(&work_dir).unwrap();

        Command::new("git")
            .arg("clone")
            .arg(&repo_dir)
            .arg(&work_dir)
            .output()
            .unwrap();

        // Configure local identity for the commit
        Command::new("git")
            .arg("-C")
            .arg(&work_dir)
            .args(["config", "user.email", "test@test.com"])
            .output()
            .unwrap();
        Command::new("git")
            .arg("-C")
            .arg(&work_dir)
            .args(["config", "user.name", "Test"])
            .output()
            .unwrap();

        // Create the file and commit
        let file_path = work_dir.join(file_name);
        if let Some(parent) = file_path.parent() {
            fs::create_dir_all(parent).unwrap();
        }
        fs::write(&file_path, content).unwrap();

        Command::new("git")
            .arg("-C")
            .arg(&work_dir)
            .args(["add", "."])
            .output()
            .unwrap();

        Command::new("git")
            .arg("-C")
            .arg(&work_dir)
            .args(["commit", "-m", "initial"])
            .output()
            .unwrap();

        // Create the requested branch if not default
        if branch != "master" && branch != "main" {
            Command::new("git")
                .arg("-C")
                .arg(&work_dir)
                .args(["checkout", "-b", branch])
                .output()
                .unwrap();

            Command::new("git")
                .arg("-C")
                .arg(&work_dir)
                .args(["push", "origin", branch])
                .output()
                .unwrap();
        } else {
            // Push the default branch
            Command::new("git")
                .arg("-C")
                .arg(&work_dir)
                .args(["push", "origin", "HEAD"])
                .output()
                .unwrap();
        }

        repo_dir
    }

    #[test]
    fn test_git_show_file_exists() {
        let dir = tempdir().unwrap();
        let repo_dir = create_test_repo_with_file(
            dir.path(),
            "master",
            "daft.yml",
            "hooks:\n  post-clone:\n    jobs:\n      - run: echo hi\n",
        );

        let content = git_show_file(&repo_dir, "master", "daft.yml");
        assert!(content.is_some());
        assert!(content.unwrap().contains("post-clone"));
    }

    #[test]
    fn test_git_show_file_not_found() {
        let dir = tempdir().unwrap();
        let repo_dir = create_test_repo_with_file(dir.path(), "master", "daft.yml", "hooks: {}");

        let content = git_show_file(&repo_dir, "master", "nonexistent.yml");
        assert!(content.is_none());
    }

    #[test]
    fn test_git_show_file_branch_not_found() {
        let dir = tempdir().unwrap();
        let repo_dir = create_test_repo_with_file(dir.path(), "master", "daft.yml", "hooks: {}");

        let content = git_show_file(&repo_dir, "no-such-branch", "daft.yml");
        assert!(content.is_none());
    }

    // ── Tests for load_config_from_branch ────────────────────────────

    #[test]
    fn test_load_config_from_branch_target_found() {
        let dir = tempdir().unwrap();
        let yaml = r#"hooks:
  worktree-pre-create:
    jobs:
      - name: check
        run: echo "from target"
"#;
        let repo_dir = create_test_repo_with_file(dir.path(), "master", "daft.yml", yaml);

        let config = load_config_from_branch(&repo_dir, "master", None)
            .unwrap()
            .unwrap();
        assert!(config.hooks.contains_key("worktree-pre-create"));
    }

    #[test]
    fn test_load_config_from_branch_falls_back_to_base() {
        let dir = tempdir().unwrap();
        let yaml = r#"hooks:
  post-clone:
    jobs:
      - run: echo "from base"
"#;
        let repo_dir = create_test_repo_with_file(dir.path(), "master", "daft.yml", yaml);

        // target branch doesn't exist, should fall back to base
        let config = load_config_from_branch(&repo_dir, "new-feature", Some("master"))
            .unwrap()
            .unwrap();
        assert!(config.hooks.contains_key("post-clone"));
    }

    #[test]
    fn test_load_config_from_branch_no_config_anywhere() {
        let dir = tempdir().unwrap();
        // Create a repo with a file that is NOT a config file
        let repo_dir = create_test_repo_with_file(dir.path(), "master", "README.md", "# Hello\n");

        let config = load_config_from_branch(&repo_dir, "master", None).unwrap();
        assert!(config.is_none());
    }

    #[test]
    fn test_load_config_from_branch_dot_daft_yml() {
        let dir = tempdir().unwrap();
        let yaml = r#"hooks:
  worktree-post-create:
    jobs:
      - run: echo "dot prefix"
"#;
        // Use .daft.yml (third priority candidate)
        let repo_dir = create_test_repo_with_file(dir.path(), "master", ".daft.yml", yaml);

        let config = load_config_from_branch(&repo_dir, "master", None)
            .unwrap()
            .unwrap();
        assert!(config.hooks.contains_key("worktree-post-create"));
    }

    #[test]
    fn test_load_config_from_branch_skips_duplicate_refs() {
        let dir = tempdir().unwrap();
        let yaml = "hooks:\n  post-clone:\n    jobs:\n      - run: echo ok\n";
        let repo_dir = create_test_repo_with_file(dir.path(), "master", "daft.yml", yaml);

        // base_branch == target_branch — should not re-check
        let config = load_config_from_branch(&repo_dir, "master", Some("master"))
            .unwrap()
            .unwrap();
        assert!(config.hooks.contains_key("post-clone"));
    }

    // ── Tests for classify_main_config ───────────────────────────────

    #[test]
    fn test_classify_main_config_missing() {
        let dir = tempdir().unwrap();
        assert_eq!(classify_main_config(dir.path()), ConfigStatus::Missing);
    }

    #[test]
    fn test_classify_main_config_visitor_untracked() {
        let dir = tempdir().unwrap();
        Command::new("git")
            .args(["init"])
            .arg(dir.path())
            .output()
            .unwrap();
        Command::new("git")
            .arg("-C")
            .arg(dir.path())
            .args(["config", "user.email", "test@test.com"])
            .output()
            .unwrap();
        Command::new("git")
            .arg("-C")
            .arg(dir.path())
            .args(["config", "user.name", "Test"])
            .output()
            .unwrap();
        write_file(dir.path(), "daft.yml", "hooks: {}");
        // daft.yml exists in worktree but is NOT tracked → visitor
        assert_eq!(classify_main_config(dir.path()), ConfigStatus::Visitor);
    }

    #[test]
    fn test_classify_main_config_tracked() {
        let dir = tempdir().unwrap();
        Command::new("git")
            .args(["init"])
            .arg(dir.path())
            .output()
            .unwrap();
        Command::new("git")
            .arg("-C")
            .arg(dir.path())
            .args(["config", "user.email", "test@test.com"])
            .output()
            .unwrap();
        Command::new("git")
            .arg("-C")
            .arg(dir.path())
            .args(["config", "user.name", "Test"])
            .output()
            .unwrap();
        write_file(dir.path(), "daft.yml", "hooks: {}");
        Command::new("git")
            .arg("-C")
            .arg(dir.path())
            .args(["add", "daft.yml"])
            .output()
            .unwrap();
        Command::new("git")
            .arg("-C")
            .arg(dir.path())
            .args(["commit", "-m", "add"])
            .output()
            .unwrap();

        assert_eq!(classify_main_config(dir.path()), ConfigStatus::Tracked);
    }

    #[test]
    fn test_classify_main_config_no_git_falls_back_to_tracked() {
        // Conservative fallback: if git can't answer, treat as tracked
        let dir = tempdir().unwrap();
        write_file(dir.path(), "daft.yml", "hooks: {}");
        // No git init here — git ls-files will fail
        assert_eq!(classify_main_config(dir.path()), ConfigStatus::Tracked);
    }

    #[test]
    fn test_classify_main_config_ignored_is_visitor() {
        let dir = tempdir().unwrap();
        Command::new("git")
            .args(["init"])
            .arg(dir.path())
            .output()
            .unwrap();
        Command::new("git")
            .arg("-C")
            .arg(dir.path())
            .args(["config", "user.email", "test@test.com"])
            .output()
            .unwrap();
        Command::new("git")
            .arg("-C")
            .arg(dir.path())
            .args(["config", "user.name", "Test"])
            .output()
            .unwrap();
        write_file(dir.path(), ".gitignore", "daft.yml\n");
        Command::new("git")
            .arg("-C")
            .arg(dir.path())
            .args(["add", ".gitignore"])
            .output()
            .unwrap();
        Command::new("git")
            .arg("-C")
            .arg(dir.path())
            .args(["commit", "-m", "add gitignore"])
            .output()
            .unwrap();
        write_file(dir.path(), "daft.yml", "hooks: {}");
        // Ignored daft.yml: ls-files --error-unmatch fails → Visitor.
        assert_eq!(classify_main_config(dir.path()), ConfigStatus::Visitor);
    }
}
