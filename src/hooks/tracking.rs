use serde::{Deserialize, Serialize};

/// Worktree attributes that a hook job can track.
/// When a tracked attribute changes (e.g., during rename or layout transform),
/// the job is re-run with teardown/setup semantics.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, Hash)]
#[serde(rename_all = "lowercase")]
pub enum TrackedAttribute {
    Path,
    Branch,
}

use std::collections::HashSet;

/// Scan a command string for template variables that imply tracking.
pub fn detect_tracked_attributes(command: &str) -> HashSet<TrackedAttribute> {
    let mut result = HashSet::new();
    if command.contains("{worktree_path}") {
        result.insert(TrackedAttribute::Path);
    }
    if command.contains("{branch}") || command.contains("{worktree_branch}") {
        result.insert(TrackedAttribute::Branch);
    }
    result
}

use super::yaml_config::{JobDef, PlatformRunCommand, RunCommand};

/// Compute the effective tracking set for a job: union of explicit `tracks`
/// field and implicitly detected template variables in `run` strings.
pub fn effective_tracks(job: &JobDef) -> HashSet<TrackedAttribute> {
    let mut result: HashSet<TrackedAttribute> = job
        .tracks
        .as_ref()
        .map(|t| t.iter().cloned().collect())
        .unwrap_or_default();

    if let Some(ref run) = job.run {
        for command_str in run_command_strings(run) {
            result.extend(detect_tracked_attributes(&command_str));
        }
    }

    result
}

/// Extract all command strings from a RunCommand (across all platform variants).
fn run_command_strings(run: &RunCommand) -> Vec<String> {
    match run {
        RunCommand::Simple(s) => vec![s.clone()],
        RunCommand::Platform(map) => map
            .values()
            .flat_map(|prc| match prc {
                PlatformRunCommand::Simple(s) => vec![s.clone()],
                PlatformRunCommand::List(list) => list.clone(),
            })
            .collect(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_detect_path_from_worktree_path_template() {
        let result = detect_tracked_attributes("mise trust {worktree_path}");
        assert!(result.contains(&TrackedAttribute::Path));
        assert!(!result.contains(&TrackedAttribute::Branch));
    }

    #[test]
    fn test_detect_branch_from_branch_template() {
        let result = detect_tracked_attributes("docker run --name {branch}");
        assert!(result.contains(&TrackedAttribute::Branch));
        assert!(!result.contains(&TrackedAttribute::Path));
    }

    #[test]
    fn test_detect_branch_from_worktree_branch_template() {
        let result = detect_tracked_attributes("echo {worktree_branch}");
        assert!(result.contains(&TrackedAttribute::Branch));
    }

    #[test]
    fn test_detect_both() {
        let result = detect_tracked_attributes("setup {worktree_path} {branch}");
        assert!(result.contains(&TrackedAttribute::Path));
        assert!(result.contains(&TrackedAttribute::Branch));
    }

    #[test]
    fn test_detect_none() {
        let result = detect_tracked_attributes("bun install");
        assert!(result.is_empty());
    }

    #[test]
    fn test_effective_tracks_unions_explicit_and_implicit() {
        let job = JobDef {
            name: Some("test".to_string()),
            run: Some(RunCommand::Simple("setup {worktree_path}".to_string())),
            tracks: Some(vec![TrackedAttribute::Branch]),
            ..Default::default()
        };
        let result = effective_tracks(&job);
        assert!(result.contains(&TrackedAttribute::Path)); // implicit
        assert!(result.contains(&TrackedAttribute::Branch)); // explicit
    }

    #[test]
    fn test_effective_tracks_platform_variants() {
        use std::collections::HashMap;

        use super::super::yaml_config::TargetOs;

        let mut platform = HashMap::new();
        platform.insert(
            TargetOs::Macos,
            PlatformRunCommand::List(vec![
                "docker stop {branch}".to_string(),
                "docker rm {branch}".to_string(),
            ]),
        );
        let job = JobDef {
            name: Some("docker".to_string()),
            run: Some(RunCommand::Platform(platform)),
            ..Default::default()
        };
        let result = effective_tracks(&job);
        assert!(result.contains(&TrackedAttribute::Branch));
        assert!(!result.contains(&TrackedAttribute::Path));
    }
}
