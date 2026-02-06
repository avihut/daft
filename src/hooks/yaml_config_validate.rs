//! Validation for YAML hooks configuration.
//!
//! Validates a parsed `YamlConfig` for semantic correctness beyond
//! what serde can enforce.

use super::yaml_config::{HookDef, JobDef, YamlConfig};
use crate::VERSION;
use anyhow::Result;

/// A validation warning (non-fatal).
#[derive(Debug, Clone)]
pub struct ValidationWarning {
    pub message: String,
    pub path: String,
}

impl std::fmt::Display for ValidationWarning {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}: {}", self.path, self.message)
    }
}

/// A validation error (fatal).
#[derive(Debug, Clone)]
pub struct ValidationError {
    pub message: String,
    pub path: String,
}

impl std::fmt::Display for ValidationError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}: {}", self.path, self.message)
    }
}

/// Result of validation.
#[derive(Debug, Default)]
pub struct ValidationResult {
    pub errors: Vec<ValidationError>,
    pub warnings: Vec<ValidationWarning>,
}

impl ValidationResult {
    pub fn is_ok(&self) -> bool {
        self.errors.is_empty()
    }

    fn error(&mut self, path: impl Into<String>, message: impl Into<String>) {
        self.errors.push(ValidationError {
            path: path.into(),
            message: message.into(),
        });
    }

    fn warn(&mut self, path: impl Into<String>, message: impl Into<String>) {
        self.warnings.push(ValidationWarning {
            path: path.into(),
            message: message.into(),
        });
    }
}

/// Validate a YAML config for semantic correctness.
pub fn validate_config(config: &YamlConfig) -> Result<ValidationResult> {
    let mut result = ValidationResult::default();

    // Check min_version
    if let Some(ref min_ver) = config.min_version {
        if !version_satisfies(VERSION, min_ver) {
            result.error(
                "min_version",
                format!("Config requires daft >= {min_ver}, but current version is {VERSION}"),
            );
        }
    }

    // Validate each hook definition
    for (hook_name, hook_def) in &config.hooks {
        validate_hook_def(hook_name, hook_def, &mut result);
    }

    Ok(result)
}

/// Validate a single hook definition.
fn validate_hook_def(name: &str, hook: &HookDef, result: &mut ValidationResult) {
    let path = format!("hooks.{name}");

    // Check mutually exclusive execution modes
    let mode_count = [hook.parallel, hook.piped, hook.follow]
        .iter()
        .filter(|m| m == &&Some(true))
        .count();

    if mode_count > 1 {
        result.error(&path, "Only one of parallel, piped, or follow can be true");
    }

    // Validate jobs
    if let Some(ref jobs) = hook.jobs {
        for (i, job) in jobs.iter().enumerate() {
            let job_path = if let Some(ref name) = job.name {
                format!("{path}.jobs[{name}]")
            } else {
                format!("{path}.jobs[{i}]")
            };
            validate_job(&job_path, job, result);
        }

        // Check for duplicate named jobs
        let named_jobs: Vec<&str> = jobs.iter().filter_map(|j| j.name.as_deref()).collect();
        let mut seen = std::collections::HashSet::new();
        for name in &named_jobs {
            if !seen.insert(name) {
                result.warn(&path, format!("Duplicate job name: {name}"));
            }
        }

        // Validate job dependencies (needs)
        validate_job_dependencies(&path, jobs, result);
    }

    // Warn if both jobs and commands are set
    if hook.jobs.is_some() && hook.commands.is_some() {
        result.warn(
            &path,
            "Both 'jobs' and 'commands' are set; 'commands' will be merged into 'jobs'",
        );
    }
}

/// Validate a single job definition.
fn validate_job(path: &str, job: &JobDef, result: &mut ValidationResult) {
    // Must have either run or script (but not both), unless it's a group
    let has_run = job.run.is_some();
    let has_script = job.script.is_some();
    let has_group = job.group.is_some();

    if has_run && has_script {
        result.error(path, "'run' and 'script' are mutually exclusive");
    }

    if !has_run && !has_script && !has_group {
        result.error(path, "Job must have 'run', 'script', or 'group'");
    }

    // script requires runner
    if has_script && job.runner.is_none() {
        result.warn(
            path,
            "'script' without 'runner' will use the script's shebang line",
        );
    }

    // Validate group
    if let Some(ref group) = job.group {
        let group_path = format!("{path}.group");

        // Check mutually exclusive execution modes in group
        let mode_count = [group.parallel, group.piped]
            .iter()
            .filter(|m| m == &&Some(true))
            .count();

        if mode_count > 1 {
            result.error(
                &group_path,
                "Only one of parallel or piped can be true in a group",
            );
        }

        if let Some(ref group_jobs) = group.jobs {
            for (i, group_job) in group_jobs.iter().enumerate() {
                let gjob_path = if let Some(ref name) = group_job.name {
                    format!("{group_path}.jobs[{name}]")
                } else {
                    format!("{group_path}.jobs[{i}]")
                };
                validate_job(&gjob_path, group_job, result);
            }
        } else {
            result.warn(&group_path, "Group has no jobs");
        }

        // A group job shouldn't also have run/script
        if has_run || has_script {
            result.error(path, "'group' cannot be combined with 'run' or 'script'");
        }
    }
}

/// Validate job dependency (`needs`) declarations.
///
/// Checks:
/// 1. Jobs with `needs` must have a `name`
/// 2. All `needs` references must point to existing named jobs
/// 3. No dependency cycles
fn validate_job_dependencies(path: &str, jobs: &[JobDef], result: &mut ValidationResult) {
    use std::collections::{HashMap, HashSet};

    // Build set of named jobs
    let named_jobs: HashSet<&str> = jobs.iter().filter_map(|j| j.name.as_deref()).collect();

    // Check each job's needs
    for (i, job) in jobs.iter().enumerate() {
        let needs = match job.needs.as_ref() {
            Some(n) if !n.is_empty() => n,
            _ => continue,
        };

        let job_path = if let Some(ref name) = job.name {
            format!("{path}.jobs[{name}]")
        } else {
            format!("{path}.jobs[{i}]")
        };

        // 1. Jobs with needs must have a name
        if job.name.is_none() {
            result.error(&job_path, "Job with 'needs' must have a 'name'");
            continue;
        }

        // 2. All needs references must exist
        for dep in needs {
            if !named_jobs.contains(dep.as_str()) {
                result.error(&job_path, format!("Unknown dependency in 'needs': '{dep}'"));
            }
        }
    }

    // 3. Check for cycles using DFS (white/gray/black coloring)
    // Build adjacency list: job name -> list of names it depends on
    let mut deps: HashMap<&str, Vec<&str>> = HashMap::new();
    for job in jobs {
        if let (Some(ref name), Some(ref needs)) = (&job.name, &job.needs) {
            deps.insert(name.as_str(), needs.iter().map(|s| s.as_str()).collect());
        }
    }

    if let Some(cycle) = check_dependency_cycles(&deps) {
        result.error(path, format!("Dependency cycle detected: {cycle}"));
    }
}

/// Check for cycles in the dependency graph using DFS with white/gray/black coloring.
///
/// Returns `Some(description)` if a cycle is found, `None` if the graph is acyclic.
fn check_dependency_cycles(deps: &std::collections::HashMap<&str, Vec<&str>>) -> Option<String> {
    use std::collections::HashSet;

    #[derive(Clone, Copy, PartialEq)]
    enum Color {
        White, // unvisited
        Gray,  // in current DFS path
        Black, // fully processed
    }

    let mut colors: std::collections::HashMap<&str, Color> = std::collections::HashMap::new();

    // Initialize all nodes as white
    for &node in deps.keys() {
        colors.insert(node, Color::White);
        for &dep in deps.get(node).into_iter().flatten() {
            colors.entry(dep).or_insert(Color::White);
        }
    }

    fn dfs<'a>(
        node: &'a str,
        deps: &std::collections::HashMap<&str, Vec<&'a str>>,
        colors: &mut std::collections::HashMap<&'a str, Color>,
        path: &mut Vec<&'a str>,
    ) -> Option<String> {
        colors.insert(node, Color::Gray);
        path.push(node);

        if let Some(neighbors) = deps.get(node) {
            for &neighbor in neighbors {
                match colors.get(neighbor) {
                    Some(Color::Gray) => {
                        // Found a cycle - build description
                        let cycle_start = path.iter().position(|&n| n == neighbor).unwrap();
                        let mut cycle: Vec<&str> = path[cycle_start..].to_vec();
                        cycle.push(neighbor);
                        return Some(cycle.join(" -> "));
                    }
                    Some(Color::White) | None => {
                        if let Some(cycle) = dfs(neighbor, deps, colors, path) {
                            return Some(cycle);
                        }
                    }
                    Some(Color::Black) => {} // already fully processed
                }
            }
        }

        path.pop();
        colors.insert(node, Color::Black);
        None
    }

    let nodes: HashSet<&str> = colors.keys().copied().collect();
    let mut path = Vec::new();
    for node in &nodes {
        if colors.get(node) == Some(&Color::White) {
            if let Some(cycle) = dfs(node, deps, &mut colors, &mut path) {
                return Some(cycle);
            }
        }
    }

    None
}

/// Check if the current version satisfies the minimum version requirement.
///
/// Simple semver comparison (major.minor.patch).
fn version_satisfies(current: &str, required: &str) -> bool {
    let parse_version = |s: &str| -> Option<(u32, u32, u32)> {
        let parts: Vec<&str> = s.split('.').collect();
        if parts.len() < 2 {
            return None;
        }
        let major = parts[0].parse().ok()?;
        let minor = parts[1].parse().ok()?;
        let patch = parts.get(2).and_then(|p| p.parse().ok()).unwrap_or(0);
        Some((major, minor, patch))
    };

    match (parse_version(current), parse_version(required)) {
        (Some(cur), Some(req)) => cur >= req,
        _ => true, // If we can't parse, assume it's fine
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::hooks::yaml_config::GroupDef;

    #[test]
    fn test_validate_empty_config() {
        let config = YamlConfig::default();
        let result = validate_config(&config).unwrap();
        assert!(result.is_ok());
        assert!(result.warnings.is_empty());
    }

    #[test]
    fn test_validate_valid_config() {
        let config = YamlConfig {
            hooks: {
                let mut map = std::collections::HashMap::new();
                map.insert(
                    "pre-commit".to_string(),
                    HookDef {
                        parallel: Some(true),
                        jobs: Some(vec![JobDef {
                            name: Some("lint".to_string()),
                            run: Some("cargo clippy".to_string()),
                            ..Default::default()
                        }]),
                        ..Default::default()
                    },
                );
                map
            },
            ..Default::default()
        };
        let result = validate_config(&config).unwrap();
        assert!(result.is_ok());
    }

    #[test]
    fn test_validate_mutually_exclusive_modes() {
        let config = YamlConfig {
            hooks: {
                let mut map = std::collections::HashMap::new();
                map.insert(
                    "pre-commit".to_string(),
                    HookDef {
                        parallel: Some(true),
                        piped: Some(true),
                        jobs: Some(vec![JobDef {
                            name: Some("lint".to_string()),
                            run: Some("echo test".to_string()),
                            ..Default::default()
                        }]),
                        ..Default::default()
                    },
                );
                map
            },
            ..Default::default()
        };
        let result = validate_config(&config).unwrap();
        assert!(!result.is_ok());
        assert!(result.errors[0].message.contains("parallel"));
    }

    #[test]
    fn test_validate_run_and_script_exclusive() {
        let config = YamlConfig {
            hooks: {
                let mut map = std::collections::HashMap::new();
                map.insert(
                    "pre-commit".to_string(),
                    HookDef {
                        jobs: Some(vec![JobDef {
                            name: Some("bad".to_string()),
                            run: Some("echo test".to_string()),
                            script: Some("my-script".to_string()),
                            ..Default::default()
                        }]),
                        ..Default::default()
                    },
                );
                map
            },
            ..Default::default()
        };
        let result = validate_config(&config).unwrap();
        assert!(!result.is_ok());
        assert!(result.errors[0].message.contains("mutually exclusive"));
    }

    #[test]
    fn test_validate_job_needs_action() {
        let config = YamlConfig {
            hooks: {
                let mut map = std::collections::HashMap::new();
                map.insert(
                    "pre-commit".to_string(),
                    HookDef {
                        jobs: Some(vec![JobDef {
                            name: Some("empty".to_string()),
                            ..Default::default()
                        }]),
                        ..Default::default()
                    },
                );
                map
            },
            ..Default::default()
        };
        let result = validate_config(&config).unwrap();
        assert!(!result.is_ok());
        assert!(result.errors[0].message.contains("run"));
    }

    #[test]
    fn test_validate_group_valid() {
        let config = YamlConfig {
            hooks: {
                let mut map = std::collections::HashMap::new();
                map.insert(
                    "pre-commit".to_string(),
                    HookDef {
                        jobs: Some(vec![JobDef {
                            name: Some("checks".to_string()),
                            group: Some(GroupDef {
                                parallel: Some(true),
                                jobs: Some(vec![
                                    JobDef {
                                        name: Some("lint".to_string()),
                                        run: Some("cargo clippy".to_string()),
                                        ..Default::default()
                                    },
                                    JobDef {
                                        name: Some("fmt".to_string()),
                                        run: Some("cargo fmt --check".to_string()),
                                        ..Default::default()
                                    },
                                ]),
                                ..Default::default()
                            }),
                            ..Default::default()
                        }]),
                        ..Default::default()
                    },
                );
                map
            },
            ..Default::default()
        };
        let result = validate_config(&config).unwrap();
        assert!(result.is_ok());
    }

    #[test]
    fn test_validate_group_with_run_error() {
        let config = YamlConfig {
            hooks: {
                let mut map = std::collections::HashMap::new();
                map.insert(
                    "pre-commit".to_string(),
                    HookDef {
                        jobs: Some(vec![JobDef {
                            name: Some("bad".to_string()),
                            run: Some("echo test".to_string()),
                            group: Some(GroupDef {
                                jobs: Some(vec![]),
                                ..Default::default()
                            }),
                            ..Default::default()
                        }]),
                        ..Default::default()
                    },
                );
                map
            },
            ..Default::default()
        };
        let result = validate_config(&config).unwrap();
        assert!(!result.is_ok());
    }

    #[test]
    fn test_version_satisfies() {
        assert!(version_satisfies("1.0.20", "1.0.0"));
        assert!(version_satisfies("1.0.20", "1.0.20"));
        assert!(!version_satisfies("1.0.19", "1.0.20"));
        assert!(version_satisfies("2.0.0", "1.0.20"));
        assert!(!version_satisfies("0.9.0", "1.0.0"));
    }

    #[test]
    fn test_validate_duplicate_job_names_warning() {
        let config = YamlConfig {
            hooks: {
                let mut map = std::collections::HashMap::new();
                map.insert(
                    "pre-commit".to_string(),
                    HookDef {
                        jobs: Some(vec![
                            JobDef {
                                name: Some("lint".to_string()),
                                run: Some("echo 1".to_string()),
                                ..Default::default()
                            },
                            JobDef {
                                name: Some("lint".to_string()),
                                run: Some("echo 2".to_string()),
                                ..Default::default()
                            },
                        ]),
                        ..Default::default()
                    },
                );
                map
            },
            ..Default::default()
        };
        let result = validate_config(&config).unwrap();
        assert!(result.is_ok()); // warnings don't count as errors
        assert_eq!(result.warnings.len(), 1);
        assert!(result.warnings[0].message.contains("Duplicate"));
    }

    #[test]
    fn test_validate_needs_unknown_ref() {
        let config = YamlConfig {
            hooks: {
                let mut map = std::collections::HashMap::new();
                map.insert(
                    "post-clone".to_string(),
                    HookDef {
                        jobs: Some(vec![
                            JobDef {
                                name: Some("a".to_string()),
                                run: Some("echo a".to_string()),
                                ..Default::default()
                            },
                            JobDef {
                                name: Some("b".to_string()),
                                run: Some("echo b".to_string()),
                                needs: Some(vec!["nonexistent".to_string()]),
                                ..Default::default()
                            },
                        ]),
                        ..Default::default()
                    },
                );
                map
            },
            ..Default::default()
        };
        let result = validate_config(&config).unwrap();
        assert!(!result.is_ok());
        assert!(result.errors[0].message.contains("Unknown dependency"));
        assert!(result.errors[0].message.contains("nonexistent"));
    }

    #[test]
    fn test_validate_needs_cycle() {
        let config = YamlConfig {
            hooks: {
                let mut map = std::collections::HashMap::new();
                map.insert(
                    "post-clone".to_string(),
                    HookDef {
                        jobs: Some(vec![
                            JobDef {
                                name: Some("a".to_string()),
                                run: Some("echo a".to_string()),
                                needs: Some(vec!["b".to_string()]),
                                ..Default::default()
                            },
                            JobDef {
                                name: Some("b".to_string()),
                                run: Some("echo b".to_string()),
                                needs: Some(vec!["a".to_string()]),
                                ..Default::default()
                            },
                        ]),
                        ..Default::default()
                    },
                );
                map
            },
            ..Default::default()
        };
        let result = validate_config(&config).unwrap();
        assert!(!result.is_ok());
        assert!(result.errors.iter().any(|e| e.message.contains("cycle")));
    }

    #[test]
    fn test_validate_needs_self_ref() {
        let config = YamlConfig {
            hooks: {
                let mut map = std::collections::HashMap::new();
                map.insert(
                    "post-clone".to_string(),
                    HookDef {
                        jobs: Some(vec![JobDef {
                            name: Some("a".to_string()),
                            run: Some("echo a".to_string()),
                            needs: Some(vec!["a".to_string()]),
                            ..Default::default()
                        }]),
                        ..Default::default()
                    },
                );
                map
            },
            ..Default::default()
        };
        let result = validate_config(&config).unwrap();
        assert!(!result.is_ok());
        assert!(result.errors.iter().any(|e| e.message.contains("cycle")));
    }

    #[test]
    fn test_validate_needs_without_name() {
        let config = YamlConfig {
            hooks: {
                let mut map = std::collections::HashMap::new();
                map.insert(
                    "post-clone".to_string(),
                    HookDef {
                        jobs: Some(vec![
                            JobDef {
                                name: Some("a".to_string()),
                                run: Some("echo a".to_string()),
                                ..Default::default()
                            },
                            JobDef {
                                run: Some("echo b".to_string()),
                                needs: Some(vec!["a".to_string()]),
                                ..Default::default()
                            },
                        ]),
                        ..Default::default()
                    },
                );
                map
            },
            ..Default::default()
        };
        let result = validate_config(&config).unwrap();
        assert!(!result.is_ok());
        assert!(result.errors[0].message.contains("must have a 'name'"));
    }

    #[test]
    fn test_validate_needs_valid() {
        let config = YamlConfig {
            hooks: {
                let mut map = std::collections::HashMap::new();
                map.insert(
                    "post-clone".to_string(),
                    HookDef {
                        jobs: Some(vec![
                            JobDef {
                                name: Some("install-npm".to_string()),
                                run: Some("npm install".to_string()),
                                ..Default::default()
                            },
                            JobDef {
                                name: Some("install-uv".to_string()),
                                run: Some("pip install uv".to_string()),
                                ..Default::default()
                            },
                            JobDef {
                                name: Some("npm-build".to_string()),
                                run: Some("npm run build".to_string()),
                                needs: Some(vec!["install-npm".to_string()]),
                                ..Default::default()
                            },
                            JobDef {
                                name: Some("uv-sync".to_string()),
                                run: Some("uv sync".to_string()),
                                needs: Some(vec!["install-uv".to_string()]),
                                ..Default::default()
                            },
                        ]),
                        ..Default::default()
                    },
                );
                map
            },
            ..Default::default()
        };
        let result = validate_config(&config).unwrap();
        assert!(result.is_ok());
    }
}
