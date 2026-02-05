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
}
