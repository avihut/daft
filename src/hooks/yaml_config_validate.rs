//! Validation for YAML hooks configuration.
//!
//! Validates a parsed `YamlConfig` for semantic correctness beyond
//! what serde can enforce.

use super::yaml_config::{CopyConfig, HookDef, JobDef, YamlConfig};
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
    if let Some(ref min_ver) = config.min_version
        && !version_satisfies(VERSION, min_ver)
    {
        result.error(
            "min_version",
            format!("Config requires daft >= {min_ver}, but current version is {VERSION}"),
        );
    }

    // Validate the copy: section's shape (serde already enforced the value
    // types; these are the semantic rules it cannot express).
    if let Some(ref copy) = config.copy {
        validate_copy(copy, &mut result);
    }

    // Validate the env: section (derived per-worktree values).
    if let Some(ref env) = config.env {
        validate_env(
            env,
            config.shared.as_deref().unwrap_or_default(),
            &mut result,
        );
    }

    // Validate each hook definition
    for (hook_name, hook_def) in &config.hooks {
        validate_hook_def("hooks", hook_name, hook_def, &mut result);
    }

    // Validate each task definition. Tasks share the hook body schema and
    // validation, but add two task-specific rules: the name must be
    // CLI/completion-safe, and the legacy `commands:` form is rejected (tasks
    // are a new surface — jobs-only).
    for (task_name, task_def) in &config.tasks {
        validate_task_name(task_name, &mut result);
        if task_def.commands.is_some() {
            result.error(
                format!("tasks.{task_name}"),
                "tasks do not support the legacy 'commands:' form; use 'jobs:'",
            );
        }
        if task_def.fail_mode.is_some() {
            result.warn(
                format!("tasks.{task_name}"),
                "'fail_mode' has no effect on tasks (daft run exits on the first \
                 job failure); it applies to lifecycle hooks only",
            );
        }
        validate_hook_def("tasks", task_name, task_def, &mut result);
    }

    Ok(result)
}

/// Validate the `env:` section — derived per-worktree values (#388).
///
/// Everything here is an error, not a warning: each rule guards a value that
/// would otherwise derive *something* — plausible-looking and wrong — and a
/// wrong port is two services silently fighting instead of a message.
///
/// Name rules: `[A-Z_][A-Z0-9_]*`. Uppercase-only is load-bearing beyond
/// convention — future `daft env` sub-verbs (`pin`, `unpin`) are lowercase,
/// so the two namespaces can never collide in the positional slot. `DAFT_*`
/// is reserved: derived values ride the hook environment's `extra_env`,
/// which is applied last and would let a declared `DAFT_BRANCH_NAME`
/// silently shadow daft's own variable.
fn validate_env(
    env: &crate::hooks::yaml_config::EnvConfig,
    shared: &[String],
    result: &mut ValidationResult,
) {
    use std::collections::HashMap;

    if let Some(scheme) = env.scheme
        && scheme != 1
    {
        result.error(
            "env.scheme",
            format!("unsupported derivation scheme {scheme}; this daft supports scheme 1"),
        );
    }

    let block_size = env.block_size.unwrap_or(16);
    if block_size == 0 {
        result.error("env.block_size", "'block_size' must be at least 1");
    }

    if let Some(ref raw) = env.range {
        match crate::hooks::yaml_config::parse_port_range(raw) {
            None => {
                result.error(
                    "env.range",
                    format!(
                        "invalid range '{raw}'; expected 'START-END' with \
                         1 <= START <= END <= 65535"
                    ),
                );
            }
            Some((start, end)) => {
                // The derivation splits the range into a declared-block region
                // and an ad-hoc region (core::env_values owns the exact
                // split); the minimum viable span is one block for each half.
                let span = u32::from(end) - u32::from(start) + 1;
                if block_size > 0 && span < 2 * u32::from(block_size) {
                    result.error(
                        "env.range",
                        format!(
                            "range '{raw}' is too small: it must fit at least two \
                             blocks of {block_size} ports (declared + ad-hoc regions)"
                        ),
                    );
                }
            }
        }
    }

    let mut offsets_seen: HashMap<u16, String> = HashMap::new();
    for (name, offset) in env.resolved_ports() {
        let path = format!("env.ports.{name}");
        validate_env_name(&name, &path, result);
        if block_size > 0 && offset >= block_size {
            result.error(
                &path,
                format!(
                    "offset {offset} does not fit a block of {block_size} ports \
                     (offsets are 0..={})",
                    block_size - 1
                ),
            );
        }
        if let Some(prev_name) = offsets_seen.insert(offset, name.clone())
            && prev_name != name
        {
            result.error(
                &path,
                format!("offset {offset} is already taken by '{prev_name}'"),
            );
        }
    }

    for name in env.values.as_ref().map(|v| v.keys()).into_iter().flatten() {
        let path = format!("env.values.{name}");
        validate_env_name(name, &path, result);
    }

    // Duplicate names across the whole section (ports twice, or port+value).
    let mut all_names: HashMap<&str, u32> = HashMap::new();
    for entry in env.ports.as_deref().unwrap_or_default() {
        *all_names.entry(entry.name.as_str()).or_default() += 1;
    }
    for name in env.values.as_ref().map(|v| v.keys()).into_iter().flatten() {
        *all_names.entry(name.as_str()).or_default() += 1;
    }
    for (name, count) in all_names {
        if count > 1 {
            result.error(
                format!("env.{name}"),
                format!("'{name}' is declared {count} times; ports and values share one namespace"),
            );
        }
    }

    if let Some(ref write) = env.write {
        let trimmed = write.trim().trim_start_matches("./");
        if write.trim().starts_with('/') {
            result.error("env.write", "'write' must be a worktree-relative path");
        } else if std::path::Path::new(trimmed)
            .components()
            .any(|c| matches!(c, std::path::Component::ParentDir))
        {
            result.error("env.write", "'write' must not escape the worktree ('..')");
        }
        let shared_hit = shared
            .iter()
            .any(|s| s.trim().trim_start_matches("./").trim_end_matches('/') == trimmed);
        if shared_hit {
            result.error(
                "env.write",
                format!(
                    "'{trimmed}' is also listed in 'shared:'; a shared dotenv is one \
                     central file symlinked everywhere, so per-worktree derived values \
                     would overwrite each other. Remove it from one of the two."
                ),
            );
        }
    }
}

/// `[A-Z_][A-Z0-9_]*` — the shape of a derived env var name.
fn is_valid_env_var_name(name: &str) -> bool {
    let mut chars = name.chars();
    match chars.next() {
        Some(c) if c.is_ascii_uppercase() || c == '_' => {}
        _ => return false,
    }
    chars.all(|c| c.is_ascii_uppercase() || c.is_ascii_digit() || c == '_')
}

/// Shared name checks for ports and values entries.
fn validate_env_name(name: &str, path: &str, result: &mut ValidationResult) {
    if !is_valid_env_var_name(name) {
        result.error(
            path,
            format!("invalid name '{name}'; derived env names must match [A-Z_][A-Z0-9_]*"),
        );
        return;
    }
    if name.starts_with("DAFT_") {
        result.error(
            path,
            format!("'{name}' is reserved; the DAFT_ prefix belongs to daft's own variables"),
        );
    }
}

/// Validate the `copy:` section.
///
/// Two rules serde cannot express:
///
/// 1. The **map form must declare a non-empty `paths:`**. `paths` is
///    `#[serde(default)]` so a map that omits or misspells the key still
///    deserializes; without this check `copy: {fallback: skip}` would be a
///    silent no-op that looks configured. The bare-list form is exempt —
///    `copy: []` is an honest "nothing declared", not a mistake.
/// 2. **`max_size:` must parse.** A garbage cap (`5 gigs`, `5GBB`) would
///    otherwise be dropped at read time and silently promote every entry to
///    an uncapped byte-copy — the opposite of what was asked for.
///
/// Both are errors, not warnings: each one means the section does something
/// other than what it says.
fn validate_copy(copy: &CopyConfig, result: &mut ValidationResult) {
    if matches!(copy, CopyConfig::Full { .. }) && copy.paths().is_empty() {
        result.error(
            "copy",
            "'copy' declares no paths; give it a non-empty 'paths:' list \
             (or remove the section)",
        );
    }

    // An absolute entry is refused at copy time; catching it here means the
    // author hears about it once, at `daft hooks validate`, instead of once
    // per worktree creation. Same sentence both places.
    for entry in copy.paths() {
        if entry.trim().starts_with('/') {
            result.error(
                "copy.paths",
                format!(
                    "'{}' {}",
                    entry.trim(),
                    crate::core::copy_paths::absolute_entry_hint(entry.trim())
                ),
            );
        }
    }

    if let Some(max_size) = copy.max_size()
        && let Err(e) = crate::coordinator::clean_policy::parse_size(max_size)
    {
        result.error(
            "copy.max_size",
            format!(
                "invalid max_size '{max_size}': {e} (expected a byte count or \
                 a value with a KB/MB/GB suffix, e.g. '5GB')"
            ),
        );
    }
}

/// Validate a task name for CLI and shell-completion safety.
///
/// A task name is typed as a bare `daft run <name>` argument and completed on
/// Tab, so it must not start with `-` (clap would treat it as a flag) or
/// contain whitespace / path or shell metacharacters. Allowed: an initial
/// alphanumeric, then alphanumerics plus `.`, `_`, `-`, up to 64 chars.
fn validate_task_name(name: &str, result: &mut ValidationResult) {
    let path = format!("tasks.{name}");
    let valid = !name.is_empty()
        && name.len() <= 64
        && name
            .chars()
            .next()
            .is_some_and(|c| c.is_ascii_alphanumeric())
        && name
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, '.' | '_' | '-'));
    if !valid {
        result.error(
            &path,
            format!(
                "invalid task name '{name}': must start with a letter or digit and contain only \
                 letters, digits, '.', '_', or '-' (max 64 chars)"
            ),
        );
    }

    // A task named like a lifecycle hook is legal (the namespaces are
    // disjoint) but confusing — surface it as a warning.
    if crate::hooks::yaml_config::KNOWN_HOOK_NAMES.contains(&name) {
        result.warn(
            &path,
            format!("task '{name}' shares a name with a lifecycle hook; they are unrelated"),
        );
    }
}

/// Validate a single hook or task definition. `section` is `hooks` or `tasks`
/// and namespaces the reported paths.
fn validate_hook_def(section: &str, name: &str, hook: &HookDef, result: &mut ValidationResult) {
    let path = format!("{section}.{name}");

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
            validate_background_fields(job, hook, &job_path, result);
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

        // Detect foreground promotion of background jobs
        detect_foreground_promotions(section, name, hook, result);
    }

    // Warn if both jobs and commands are set
    if hook.jobs.is_some() && hook.commands.is_some() {
        result.warn(
            &path,
            "Both 'jobs' and 'commands' are set; 'commands' will be merged into 'jobs'",
        );
    }

    // A hook entry with neither jobs nor commands runs nothing. A
    // `fail_mode:`-only entry is the natural way to land here, and for the
    // `hooks:` section its mere presence still suppresses any legacy script
    // hook of the same name — so make the no-op visible instead of silent.
    if hook.jobs.is_none() && hook.commands.is_none() {
        let suffix = if section == "hooks" {
            "; being present, it also suppresses any legacy script hook of the same name"
        } else {
            ""
        };
        result.warn(
            &path,
            format!("'{name}' defines no jobs or commands, so it runs nothing{suffix}"),
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

/// Validate background-specific fields on a single job.
fn validate_background_fields(
    job: &JobDef,
    hook_def: &HookDef,
    path: &str,
    result: &mut ValidationResult,
) {
    let is_bg = job.background.or(hook_def.background).unwrap_or(false);

    if is_bg && job.interactive == Some(true) {
        result.warnings.push(ValidationWarning {
            path: path.to_string(),
            message: format!(
                "Job '{}' is marked as both interactive and background; \
                 interactive jobs require a terminal and will be promoted to foreground",
                job.name.as_deref().unwrap_or("<unnamed>")
            ),
        });
    }

    if !is_bg && job.background_output.is_some() {
        result.warnings.push(ValidationWarning {
            path: path.to_string(),
            message: format!(
                "Job '{}' has background_output set but is a foreground job; \
                 background_output only applies to background jobs",
                job.name.as_deref().unwrap_or("<unnamed>")
            ),
        });
    }
}

/// Detect foreground promotion: a foreground job that depends on a background job
/// forces the background job to run in the foreground.
fn detect_foreground_promotions(
    section: &str,
    hook_name: &str,
    hook_def: &HookDef,
    result: &mut ValidationResult,
) {
    let jobs = match &hook_def.jobs {
        Some(jobs) => jobs,
        None => return,
    };

    let bg_map: std::collections::HashMap<&str, bool> = jobs
        .iter()
        .filter_map(|j| {
            let name = j.name.as_deref()?;
            let is_bg = j.background.or(hook_def.background).unwrap_or(false);
            Some((name, is_bg))
        })
        .collect();

    for job in jobs {
        let is_bg = job.background.or(hook_def.background).unwrap_or(false);
        if is_bg {
            continue;
        }
        if let Some(ref needs) = job.needs {
            for dep_name in needs {
                if bg_map.get(dep_name.as_str()) == Some(&true) {
                    result.warnings.push(ValidationWarning {
                        path: format!("{section}.{hook_name}.jobs"),
                        message: format!(
                            "Background job '{}' will be promoted to foreground \
                             (required by '{}')",
                            dep_name,
                            job.name.as_deref().unwrap_or("<unnamed>")
                        ),
                    });
                }
            }
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
        if let (Some(name), Some(needs)) = (&job.name, &job.needs) {
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
        if colors.get(node) == Some(&Color::White)
            && let Some(cycle) = dfs(node, deps, &mut colors, &mut path)
        {
            return Some(cycle);
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
    use crate::hooks::yaml_config::{GroupDef, RunCommand};

    #[test]
    fn test_validate_empty_config() {
        let config = YamlConfig::default();
        let result = validate_config(&config).unwrap();
        assert!(result.is_ok());
        assert!(result.warnings.is_empty());
    }

    #[test]
    fn test_fail_mode_only_hook_entry_warns_it_runs_nothing() {
        // A `fail_mode:`-only hook entry (no jobs, no commands) runs nothing
        // and, by its mere presence, suppresses any legacy script hook of the
        // same name. Surface that as a warning rather than a silent no-op.
        let yaml = r#"
hooks:
  worktree-post-create:
    fail_mode: abort
"#;
        let config: YamlConfig = serde_yaml::from_str(yaml).unwrap();
        let result = validate_config(&config).unwrap();
        assert!(result.is_ok(), "empty entry is a warning, not an error");
        assert!(
            result.warnings.iter().any(|w| {
                w.path == "hooks.worktree-post-create"
                    && w.message.contains("runs nothing")
                    && w.message.contains("legacy")
            }),
            "expected a no-op + legacy-suppression warning, got: {:?}",
            result.warnings
        );
    }

    #[test]
    fn copy_valid_forms_pass_validation() {
        for yaml in [
            "copy:\n  - target/\n",
            "copy: []\n",
            "copy:\n  paths: [target/]\n",
            "copy:\n  paths: [target/]\n  fallback: skip\n  max_size: 5GB\n",
            "copy:\n  paths: [target/]\n  max_size: '1048576'\n",
            "copy:\n  paths: [target/]\n  max_size: 500mb\n",
        ] {
            let config: YamlConfig = serde_yaml::from_str(yaml).unwrap();
            let result = validate_config(&config).unwrap();
            assert!(
                result.is_ok(),
                "{yaml:?} should validate: {:?}",
                result.errors
            );
        }
    }

    #[test]
    fn copy_full_form_with_empty_paths_is_an_error() {
        // The map form is how a user says "I configured copying"; with no
        // paths it silently does nothing, so it must not load quietly.
        for yaml in [
            "copy:\n  paths: []\n",
            "copy:\n  fallback: skip\n",
            // A misspelled list key lands here too, with a readable message
            // instead of serde's "did not match any variant".
            "copy:\n  pahts: [target/]\n",
        ] {
            let config: YamlConfig = serde_yaml::from_str(yaml).unwrap();
            let result = validate_config(&config).unwrap();
            assert!(
                result
                    .errors
                    .iter()
                    .any(|e| e.path == "copy" && e.message.contains("no paths")),
                "{yaml:?} should error on empty paths, got: {:?}",
                result.errors
            );
        }

        // The bare-list form's empty spelling is honest, not a mistake.
        let config: YamlConfig = serde_yaml::from_str("copy: []\n").unwrap();
        assert!(validate_config(&config).unwrap().is_ok());
    }

    #[test]
    fn copy_rejects_an_absolute_entry_with_the_fix_in_the_message() {
        // An absolute entry is refused at copy time; catching it here means
        // the answer arrives once, from `daft hooks validate`, instead of once
        // per worktree creation. `/target` is what cargo writes into
        // `.gitignore`, so it is the likeliest way one appears.
        let config: YamlConfig =
            serde_yaml::from_str("copy:\n  - /target\n  - node_modules/\n").unwrap();
        let result = validate_config(&config).unwrap();

        let message = result
            .errors
            .iter()
            .find(|e| e.path == "copy.paths")
            .map(|e| e.message.clone())
            .expect("an absolute entry is an error");
        assert!(
            message.contains("/target") && message.contains("write 'target'"),
            "the error has to name the entry and the fix: {message}"
        );
        // A relative entry alongside it is not implicated.
        assert_eq!(
            result
                .errors
                .iter()
                .filter(|e| e.path == "copy.paths")
                .count(),
            1,
            "{:?}",
            result.errors
        );
    }

    #[test]
    fn copy_invalid_max_size_is_an_error_naming_the_value() {
        for bad in ["5 gigs", "abc", "10XB", "", "99999999999999GB"] {
            let config = YamlConfig {
                copy: Some(CopyConfig::Full {
                    paths: vec!["target/".to_string()],
                    fallback: None,
                    max_size: Some(bad.to_string()),
                }),
                ..Default::default()
            };
            let result = validate_config(&config).unwrap();
            assert!(
                result
                    .errors
                    .iter()
                    .any(|e| e.path == "copy.max_size" && e.message.contains(bad)),
                "max_size {bad:?} should be rejected and named, got: {:?}",
                result.errors
            );
        }
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
                            run: Some(RunCommand::Simple("cargo clippy".to_string())),
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

    fn env_yaml(body: &str) -> YamlConfig {
        serde_yaml::from_str(body).expect("test yaml parses")
    }

    fn env_errors(body: &str) -> Vec<String> {
        validate_config(&env_yaml(body))
            .unwrap()
            .errors
            .iter()
            .map(|e| format!("{e}"))
            .collect()
    }

    #[test]
    fn env_valid_section_passes() {
        let errors = env_errors(
            "env:\n  salt: myapp\n  range: 20000-32767\n  block_size: 16\n  ports:\n    - WEBAPP_PORT\n    - API_PORT: 8\n  values:\n    COMPOSE_PROJECT_NAME: \"x-{worktree_slug}\"\n  write: .env\n",
        );
        assert!(errors.is_empty(), "unexpected errors: {errors:?}");
    }

    #[test]
    fn env_name_rules_enforced() {
        let errors = env_errors("env:\n  ports:\n    - webapp_port\n");
        assert!(errors.iter().any(|e| e.contains("[A-Z_]")), "{errors:?}");

        let errors = env_errors("env:\n  ports:\n    - DAFT_BRANCH_NAME\n");
        assert!(errors.iter().any(|e| e.contains("reserved")), "{errors:?}");

        // values keys follow the same rules — one namespace.
        let errors = env_errors("env:\n  values:\n    lower: x\n");
        assert!(errors.iter().any(|e| e.contains("[A-Z_]")), "{errors:?}");
    }

    #[test]
    fn env_offset_collisions_and_overflow_rejected() {
        // B_PORT auto-follows A_PORT's explicit 3 → collides with C_PORT: 4?
        // No — construct the collision directly: explicit 0 after auto 0.
        let errors = env_errors("env:\n  ports:\n    - A_PORT\n    - B_PORT: 0\n");
        assert!(
            errors.iter().any(|e| e.contains("already taken")),
            "{errors:?}"
        );

        let errors = env_errors("env:\n  block_size: 4\n  ports:\n    - A_PORT: 4\n");
        assert!(
            errors.iter().any(|e| e.contains("does not fit a block")),
            "{errors:?}"
        );

        // Same name twice (port + value) is a namespace error.
        let errors = env_errors("env:\n  ports:\n    - A_PORT\n  values:\n    A_PORT: x\n");
        assert!(
            errors.iter().any(|e| e.contains("one namespace")),
            "{errors:?}"
        );
    }

    #[test]
    fn env_range_rules_enforced() {
        let errors = env_errors("env:\n  range: 4000-3000\n");
        assert!(
            errors.iter().any(|e| e.contains("invalid range")),
            "{errors:?}"
        );

        // Must fit declared + ad-hoc regions (two blocks minimum).
        let errors = env_errors("env:\n  range: 20000-20019\n  block_size: 16\n");
        assert!(errors.iter().any(|e| e.contains("too small")), "{errors:?}");

        let errors = env_errors("env:\n  block_size: 0\n");
        assert!(
            errors.iter().any(|e| e.contains("at least 1")),
            "{errors:?}"
        );
    }

    #[test]
    fn env_scheme_other_than_one_rejected() {
        let errors = env_errors("env:\n  scheme: 2\n");
        assert!(
            errors.iter().any(|e| e.contains("supports scheme 1")),
            "{errors:?}"
        );
    }

    #[test]
    fn env_write_shape_and_shared_collision_rejected() {
        let errors = env_errors("env:\n  write: /etc/env\n");
        assert!(
            errors.iter().any(|e| e.contains("worktree-relative")),
            "{errors:?}"
        );

        let errors = env_errors("env:\n  write: ../up/.env\n");
        assert!(errors.iter().any(|e| e.contains("escape")), "{errors:?}");

        // A shared symlinked dotenv would make worktrees overwrite each other.
        let errors = env_errors("shared:\n  - .env\nenv:\n  write: .env\n");
        assert!(errors.iter().any(|e| e.contains("shared")), "{errors:?}");
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
                            run: Some(RunCommand::Simple("echo test".to_string())),
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
                            run: Some(RunCommand::Simple("echo test".to_string())),
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
                                        run: Some(RunCommand::Simple("cargo clippy".to_string())),
                                        ..Default::default()
                                    },
                                    JobDef {
                                        name: Some("fmt".to_string()),
                                        run: Some(RunCommand::Simple(
                                            "cargo fmt --check".to_string(),
                                        )),
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
                            run: Some(RunCommand::Simple("echo test".to_string())),
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
                                run: Some(RunCommand::Simple("echo 1".to_string())),
                                ..Default::default()
                            },
                            JobDef {
                                name: Some("lint".to_string()),
                                run: Some(RunCommand::Simple("echo 2".to_string())),
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
                                run: Some(RunCommand::Simple("echo a".to_string())),
                                ..Default::default()
                            },
                            JobDef {
                                name: Some("b".to_string()),
                                run: Some(RunCommand::Simple("echo b".to_string())),
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
                                run: Some(RunCommand::Simple("echo a".to_string())),
                                needs: Some(vec!["b".to_string()]),
                                ..Default::default()
                            },
                            JobDef {
                                name: Some("b".to_string()),
                                run: Some(RunCommand::Simple("echo b".to_string())),
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
                            run: Some(RunCommand::Simple("echo a".to_string())),
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
                                run: Some(RunCommand::Simple("echo a".to_string())),
                                ..Default::default()
                            },
                            JobDef {
                                run: Some(RunCommand::Simple("echo b".to_string())),
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
                                run: Some(RunCommand::Simple("npm install".to_string())),
                                ..Default::default()
                            },
                            JobDef {
                                name: Some("install-uv".to_string()),
                                run: Some(RunCommand::Simple("pip install uv".to_string())),
                                ..Default::default()
                            },
                            JobDef {
                                name: Some("npm-build".to_string()),
                                run: Some(RunCommand::Simple("npm run build".to_string())),
                                needs: Some(vec!["install-npm".to_string()]),
                                ..Default::default()
                            },
                            JobDef {
                                name: Some("uv-sync".to_string()),
                                run: Some(RunCommand::Simple("uv sync".to_string())),
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

    #[test]
    fn test_warn_background_job_promoted_to_foreground() {
        let yaml = r#"
hooks:
  worktree-post-create:
    jobs:
      - name: bg-dep
        run: echo dep
        background: true
      - name: fg-consumer
        run: echo consume
        needs: [bg-dep]
"#;
        let config: YamlConfig = serde_yaml::from_str(yaml).unwrap();
        let result = validate_config(&config).unwrap();
        assert!(!result.warnings.is_empty());
        assert!(
            result
                .warnings
                .iter()
                .any(|w| w.message.contains("promoted to foreground"))
        );
    }

    #[test]
    fn test_warn_interactive_job_cannot_be_background() {
        let yaml = r#"
hooks:
  worktree-post-create:
    jobs:
      - name: interactive-bg
        run: vim file.txt
        background: true
        interactive: true
"#;
        let config: YamlConfig = serde_yaml::from_str(yaml).unwrap();
        let result = validate_config(&config).unwrap();
        assert!(!result.warnings.is_empty());
        assert!(
            result
                .warnings
                .iter()
                .any(|w| w.message.contains("interactive") && w.message.contains("background"))
        );
    }

    #[test]
    fn test_valid_background_output_values() {
        let yaml = r#"
hooks:
  worktree-post-create:
    jobs:
      - name: job1
        run: echo hi
        background: true
        background_output: log
      - name: job2
        run: echo hi
        background: true
        background_output: silent
"#;
        let config: YamlConfig = serde_yaml::from_str(yaml).unwrap();
        let result = validate_config(&config).unwrap();
        assert!(result.is_ok());
    }

    #[test]
    fn test_warn_background_output_on_foreground_job() {
        let yaml = r#"
hooks:
  worktree-post-create:
    jobs:
      - name: fg-job
        run: echo hi
        background_output: silent
"#;
        let config: YamlConfig = serde_yaml::from_str(yaml).unwrap();
        let result = validate_config(&config).unwrap();
        assert!(
            result.warnings.iter().any(
                |w| w.message.contains("background_output") && w.message.contains("foreground")
            )
        );
    }

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

    #[test]
    fn test_tasks_parse_and_validate_clean() {
        let yaml = r#"
tasks:
  run:
    jobs:
      - name: web
        run: pnpm dev
  seed-db:
    jobs:
      - name: seed
        run: ./scripts/seed.sh
"#;
        let config: YamlConfig = serde_yaml::from_str(yaml).unwrap();
        assert_eq!(config.tasks.len(), 2);
        let result = validate_config(&config).unwrap();
        assert!(result.is_ok(), "unexpected errors: {:?}", result.errors);
    }

    #[test]
    fn test_task_invalid_name_rejected() {
        // A leading dash would be parsed by clap as a flag; whitespace and
        // slashes are shell/path hazards.
        for bad in ["-dev", "web server", "a/b", ""] {
            let mut tasks = std::collections::HashMap::new();
            tasks.insert(
                bad.to_string(),
                HookDef {
                    jobs: Some(vec![JobDef {
                        name: Some("j".to_string()),
                        run: Some(RunCommand::Simple("true".to_string())),
                        ..Default::default()
                    }]),
                    ..Default::default()
                },
            );
            let config = YamlConfig {
                tasks,
                ..Default::default()
            };
            let result = validate_config(&config).unwrap();
            assert!(
                result
                    .errors
                    .iter()
                    .any(|e| e.path == format!("tasks.{bad}")),
                "task name {bad:?} should be rejected"
            );
        }
    }

    #[test]
    fn test_task_legacy_commands_rejected() {
        // `commands:` is the deprecated hook form; tasks are jobs-only.
        let yaml = r#"
tasks:
  run:
    commands:
      web:
        run: pnpm dev
"#;
        let config: YamlConfig = serde_yaml::from_str(yaml).unwrap();
        let result = validate_config(&config).unwrap();
        assert!(
            result
                .errors
                .iter()
                .any(|e| e.path == "tasks.run" && e.message.contains("commands")),
            "legacy commands: in a task must error, got: {:?}",
            result.errors
        );
    }

    #[test]
    fn test_task_mode_exclusivity_reported_under_tasks_path() {
        let yaml = r#"
tasks:
  run:
    parallel: true
    piped: true
    jobs:
      - name: web
        run: pnpm dev
"#;
        let config: YamlConfig = serde_yaml::from_str(yaml).unwrap();
        let result = validate_config(&config).unwrap();
        assert!(
            result.errors.iter().any(|e| e.path == "tasks.run"),
            "mode-exclusivity error must be namespaced under tasks.run: {:?}",
            result.errors
        );
    }

    #[test]
    fn test_task_shadowing_hook_name_warns_not_errors() {
        let mut tasks = std::collections::HashMap::new();
        tasks.insert(
            "post-merge".to_string(),
            HookDef {
                jobs: Some(vec![JobDef {
                    name: Some("j".to_string()),
                    run: Some(RunCommand::Simple("true".to_string())),
                    ..Default::default()
                }]),
                ..Default::default()
            },
        );
        let config = YamlConfig {
            tasks,
            ..Default::default()
        };
        let result = validate_config(&config).unwrap();
        assert!(result.is_ok(), "shadowing must not be an error");
        assert!(
            result.warnings.iter().any(|w| w.path == "tasks.post-merge"),
            "shadowing a hook name should warn: {:?}",
            result.warnings
        );
    }

    #[test]
    fn test_unknown_hook_name_validation_unaffected_by_tasks() {
        // Adding a tasks: section must not change hook validation behavior.
        let yaml = r#"
hooks:
  worktree-post-create:
    jobs:
      - name: install
        run: pnpm install
tasks:
  run:
    jobs:
      - name: web
        run: pnpm dev
"#;
        let config: YamlConfig = serde_yaml::from_str(yaml).unwrap();
        let result = validate_config(&config).unwrap();
        assert!(result.is_ok(), "unexpected errors: {:?}", result.errors);
    }
}
