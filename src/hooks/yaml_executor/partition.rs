//! DAG partitioning: split jobs into foreground and background phases.
//!
//! Background jobs that are transitively depended on by any foreground job
//! are promoted to the foreground phase. This preserves DAG validity and
//! ensures the daft command does not exit while a foreground job is still
//! waiting for a background dependency.

use crate::executor::JobSpec;
use std::collections::HashMap;

/// Partition jobs into foreground and background phases.
///
/// Background jobs that are transitively depended on by any foreground job
/// are promoted to the foreground phase. Returns `(foreground, background)`.
pub fn partition_foreground_background(jobs: &[JobSpec]) -> (Vec<JobSpec>, Vec<JobSpec>) {
    if jobs.is_empty() {
        return (Vec::new(), Vec::new());
    }

    // Build name -> index map
    let name_to_idx: HashMap<&str, usize> = jobs
        .iter()
        .enumerate()
        .map(|(i, j)| (j.name.as_str(), i))
        .collect();

    // Start with all foreground jobs as "must be foreground"
    let mut must_fg: Vec<bool> = jobs.iter().map(|j| !j.background).collect();

    // Walk backwards from foreground jobs through their dependencies,
    // promoting any background dependency to foreground
    let mut stack: Vec<usize> = must_fg
        .iter()
        .enumerate()
        .filter(|(_, &is_fg)| is_fg)
        .map(|(i, _)| i)
        .collect();

    while let Some(idx) = stack.pop() {
        for dep_name in &jobs[idx].needs {
            if let Some(&dep_idx) = name_to_idx.get(dep_name.as_str()) {
                if !must_fg[dep_idx] {
                    must_fg[dep_idx] = true;
                    stack.push(dep_idx); // Recurse into this dep's deps
                }
            }
        }
    }

    // Partition
    let mut foreground = Vec::new();
    let mut background = Vec::new();
    for (i, job) in jobs.iter().enumerate() {
        if must_fg[i] {
            foreground.push(job.clone());
        } else {
            background.push(job.clone());
        }
    }

    (foreground, background)
}

#[cfg(test)]
mod partition_tests {
    use super::*;
    use crate::executor::JobSpec;

    fn spec(name: &str, background: bool, needs: Vec<&str>) -> JobSpec {
        JobSpec {
            name: name.to_string(),
            background,
            needs: needs.into_iter().map(String::from).collect(),
            ..Default::default()
        }
    }

    #[test]
    fn test_partition_no_background_jobs() {
        let jobs = vec![spec("a", false, vec![]), spec("b", false, vec!["a"])];
        let (fg, bg) = partition_foreground_background(&jobs);
        assert_eq!(fg.len(), 2);
        assert_eq!(bg.len(), 0);
    }

    #[test]
    fn test_partition_independent_background_jobs() {
        let jobs = vec![
            spec("fg", false, vec![]),
            spec("bg1", true, vec![]),
            spec("bg2", true, vec![]),
        ];
        let (fg, bg) = partition_foreground_background(&jobs);
        assert_eq!(fg.len(), 1);
        assert_eq!(fg[0].name, "fg");
        assert_eq!(bg.len(), 2);
    }

    #[test]
    fn test_partition_background_promoted_by_foreground_dependency() {
        let jobs = vec![
            spec("bg-dep", true, vec![]),
            spec("fg-consumer", false, vec!["bg-dep"]),
        ];
        let (fg, bg) = partition_foreground_background(&jobs);
        assert_eq!(fg.len(), 2); // bg-dep promoted
        assert_eq!(bg.len(), 0);
    }

    #[test]
    fn test_partition_transitive_promotion() {
        let jobs = vec![
            spec("bg1", true, vec![]),
            spec("bg2", true, vec!["bg1"]),
            spec("fg", false, vec!["bg2"]),
        ];
        let (fg, bg) = partition_foreground_background(&jobs);
        assert_eq!(fg.len(), 3); // both bg jobs promoted
        assert_eq!(bg.len(), 0);
    }

    #[test]
    fn test_partition_mixed() {
        let jobs = vec![
            spec("install", false, vec![]),
            spec("build", true, vec!["install"]),
            spec("assets", true, vec!["install"]),
            spec("types", false, vec!["build"]),
        ];
        let (fg, bg) = partition_foreground_background(&jobs);
        // install: foreground
        // build: background, but types depends on it -> promoted
        // types: foreground
        // assets: background, no foreground dependents -> stays background
        assert_eq!(fg.len(), 3);
        assert!(fg.iter().any(|j| j.name == "install"));
        assert!(fg.iter().any(|j| j.name == "build"));
        assert!(fg.iter().any(|j| j.name == "types"));
        assert_eq!(bg.len(), 1);
        assert_eq!(bg[0].name, "assets");
    }
}
