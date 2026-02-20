//! Config discovery, loading, and merging for YAML hooks configuration.
//!
//! This module handles finding the right config file, loading it, and
//! merging multiple config sources (main, extends, per-hook, local).

use super::yaml_config::{HookDef, JobDef, YamlConfig};
use anyhow::{Context, Result};
use std::collections::HashMap;
use std::path::{Path, PathBuf};

/// Where the main config file was found, which determines where
/// per-hook and local config files are located.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ConfigLocation {
    /// Config at repo root (e.g., `daft.yml`, `.daft.yml`).
    Root,
    /// Config in `.config/daft/` directory (e.g., `.config/daft.yml`).
    DotConfig,
}

/// Config file candidates in priority order (first match wins).
const CONFIG_CANDIDATES: &[(&str, ConfigLocation)] = &[
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
/// Returns the path if found.
pub fn find_local_config(main_config: &Path) -> Option<PathBuf> {
    let parent = main_config.parent()?;
    let filename = main_config.file_name()?.to_str()?;

    // Build local filename: daft.yml → daft-local.yml, .daft.yml → .daft-local.yml
    let local_filename = if let Some(stem) = filename.strip_suffix(".yaml") {
        format!("{stem}-local.yaml")
    } else if let Some(stem) = filename.strip_suffix(".yml") {
        format!("{stem}-local.yml")
    } else {
        return None;
    };

    let local_path = parent.join(&local_filename);
    if local_path.is_file() {
        Some(local_path)
    } else {
        None
    }
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
/// 4. Local override (`daft-local.yml`)
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

/// Merge two configs, with `overlay` taking precedence over `base`.
pub fn merge_configs(base: YamlConfig, overlay: YamlConfig) -> YamlConfig {
    let mut merged = base;

    // Scalar fields: overlay wins if set
    if overlay.min_version.is_some() {
        merged.min_version = overlay.min_version;
    }
    if overlay.colors.is_some() {
        merged.colors = overlay.colors;
    }
    if overlay.no_tty.is_some() {
        merged.no_tty = overlay.no_tty;
    }
    if overlay.rc.is_some() {
        merged.rc = overlay.rc;
    }
    if overlay.output.is_some() {
        merged.output = overlay.output;
    }
    if overlay.source_dir.is_some() {
        merged.source_dir = overlay.source_dir;
    }
    if overlay.source_dir_local.is_some() {
        merged.source_dir_local = overlay.source_dir_local;
    }

    // Hooks: merge each hook definition
    for (name, overlay_hook) in overlay.hooks {
        if let Some(base_hook) = merged.hooks.remove(&name) {
            merged
                .hooks
                .insert(name, merge_hook_defs(base_hook, overlay_hook));
        } else {
            merged.hooks.insert(name, overlay_hook);
        }
    }

    merged
}

/// Merge two hook definitions, with `overlay` taking precedence.
///
/// Named jobs merge by name (overlay replaces base with same name).
/// Unnamed jobs from overlay are appended.
pub fn merge_hook_defs(base: HookDef, overlay: HookDef) -> HookDef {
    let mut merged = base;

    // Scalar fields: overlay wins if set
    if overlay.parallel.is_some() {
        merged.parallel = overlay.parallel;
    }
    if overlay.piped.is_some() {
        merged.piped = overlay.piped;
    }
    if overlay.follow.is_some() {
        merged.follow = overlay.follow;
    }
    if overlay.exclude_tags.is_some() {
        merged.exclude_tags = overlay.exclude_tags;
    }
    if overlay.exclude.is_some() {
        merged.exclude = overlay.exclude;
    }
    if overlay.skip.is_some() {
        merged.skip = overlay.skip;
    }
    if overlay.only.is_some() {
        merged.only = overlay.only;
    }

    // Jobs: merge named jobs by name, append unnamed
    if let Some(overlay_jobs) = overlay.jobs {
        let mut base_jobs = merged.jobs.unwrap_or_default();
        for overlay_job in overlay_jobs {
            if let Some(ref name) = overlay_job.name {
                // Replace existing job with same name
                if let Some(pos) = base_jobs
                    .iter()
                    .position(|j| j.name.as_deref() == Some(name))
                {
                    base_jobs[pos] = overlay_job;
                } else {
                    base_jobs.push(overlay_job);
                }
            } else {
                base_jobs.push(overlay_job);
            }
        }
        merged.jobs = Some(base_jobs);
    }

    // Commands: overlay replaces entirely if set
    if overlay.commands.is_some() {
        merged.commands = overlay.commands;
    }

    merged
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::hooks::yaml_config::CommandDef;
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
    fn test_merge_configs_scalar_override() {
        let base = YamlConfig {
            min_version: Some("1.0.0".to_string()),
            colors: Some(true),
            ..Default::default()
        };
        let overlay = YamlConfig {
            min_version: Some("2.0.0".to_string()),
            ..Default::default()
        };
        let merged = merge_configs(base, overlay);
        assert_eq!(merged.min_version.as_deref(), Some("2.0.0"));
        assert_eq!(merged.colors, Some(true)); // base preserved
    }

    #[test]
    fn test_merge_hook_defs_named_jobs() {
        let base = HookDef {
            jobs: Some(vec![
                JobDef {
                    name: Some("lint".to_string()),
                    run: Some("eslint .".to_string()),
                    ..Default::default()
                },
                JobDef {
                    name: Some("format".to_string()),
                    run: Some("prettier --check .".to_string()),
                    ..Default::default()
                },
            ]),
            ..Default::default()
        };
        let overlay = HookDef {
            jobs: Some(vec![JobDef {
                name: Some("lint".to_string()),
                run: Some("cargo clippy".to_string()),
                ..Default::default()
            }]),
            ..Default::default()
        };
        let merged = merge_hook_defs(base, overlay);
        let jobs = merged.jobs.unwrap();
        assert_eq!(jobs.len(), 2);
        // lint should be overridden
        assert_eq!(jobs[0].run.as_deref(), Some("cargo clippy"));
        // format should be preserved
        assert_eq!(jobs[1].run.as_deref(), Some("prettier --check ."));
    }

    #[test]
    fn test_merge_hook_defs_unnamed_appended() {
        let base = HookDef {
            jobs: Some(vec![JobDef {
                run: Some("echo base".to_string()),
                ..Default::default()
            }]),
            ..Default::default()
        };
        let overlay = HookDef {
            jobs: Some(vec![JobDef {
                run: Some("echo overlay".to_string()),
                ..Default::default()
            }]),
            ..Default::default()
        };
        let merged = merge_hook_defs(base, overlay);
        let jobs = merged.jobs.unwrap();
        assert_eq!(jobs.len(), 2);
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
        assert_eq!(jobs[0].run.as_deref(), Some("echo \"local override\""));
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
        assert_eq!(jobs[0].run.as_deref(), Some("npm install"));
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
                run: Some("echo 1".to_string()),
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
}
