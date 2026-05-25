//! DAG partitioning: split jobs into foreground and background phases.
//!
//! Background jobs that are transitively depended on by any foreground job
//! are promoted to the foreground phase. This preserves DAG validity and
//! ensures the daft command does not exit while a foreground job is still
//! waiting for a background dependency.
//!
//! Cross-partition dependencies in the other direction — a background job
//! declaring `needs:` on a foreground job — are stripped from the BG
//! `JobSpec.needs` returned here. The coordinator only sees the background
//! slice, so a `needs:` entry naming a foreground job would be a dangling
//! reference and `DagGraph::new` would reject it with `MissingDependency`.
//! FG-before-BG sequencing in the caller already guarantees the
//! "happens-after" intent of such a dep, so dropping the name in the BG
//! view is semantically a no-op for ordering. Failure cascade (skip the BG
//! job when its FG dep failed) is handled by the caller, which has the FG
//! outcomes and threads the affected BG names into
//! `CoordinatorState::prefailed_jobs`.

use crate::executor::JobSpec;
use std::collections::{HashMap, HashSet};

/// Partition jobs into foreground and background phases.
///
/// Background jobs that are transitively depended on by any foreground job
/// are promoted to the foreground phase. Background jobs that survive the
/// partition have their `needs:` filtered to BG-only names (cross-partition
/// references to foreground jobs are removed — see module docs).
///
/// Returns `(foreground, background)`.
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
        .filter(|&(_, &is_fg)| is_fg)
        .map(|(i, _)| i)
        .collect();

    while let Some(idx) = stack.pop() {
        for dep_name in &jobs[idx].needs {
            if let Some(&dep_idx) = name_to_idx.get(dep_name.as_str())
                && !must_fg[dep_idx]
            {
                must_fg[dep_idx] = true;
                stack.push(dep_idx); // Recurse into this dep's deps
            }
        }
    }

    // Names that landed in the background partition — used to strip
    // cross-partition needs from BG `JobSpec`s below.
    let bg_names: HashSet<&str> = jobs
        .iter()
        .enumerate()
        .filter(|&(i, _)| !must_fg[i])
        .map(|(_, j)| j.name.as_str())
        .collect();

    // Partition
    let mut foreground = Vec::new();
    let mut background = Vec::new();
    for (i, job) in jobs.iter().enumerate() {
        if must_fg[i] {
            foreground.push(job.clone());
        } else {
            let mut bg_job = job.clone();
            bg_job.needs.retain(|n| bg_names.contains(n.as_str()));
            background.push(bg_job);
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
        // assets's needs: ["install"] crosses into the FG partition and
        // must be stripped — the coordinator's DAG won't have an "install"
        // node to satisfy the dep. FG-before-BG sequencing in the caller
        // already guarantees the "happens-after" intent (see module docs).
        assert!(
            bg[0].needs.is_empty(),
            "expected assets.needs to be empty after stripping FG names, got {:?}",
            bg[0].needs
        );
    }

    #[test]
    fn test_partition_bg_needs_fg_is_stripped() {
        // Regression for #556: a BG job whose only `needs:` entries are FG
        // names must end up with empty `needs` in the BG slice, otherwise
        // the coordinator's `DagGraph::new` rejects it with
        // `MissingDependency` and the job silently never runs.
        let jobs = vec![
            spec("install", false, vec![]),
            spec("warm", true, vec!["install"]),
        ];
        let (fg, bg) = partition_foreground_background(&jobs);
        assert_eq!(fg.len(), 1);
        assert_eq!(fg[0].name, "install");
        assert_eq!(bg.len(), 1);
        assert_eq!(bg[0].name, "warm");
        assert!(
            bg[0].needs.is_empty(),
            "FG-only needs must be stripped from BG slice, got {:?}",
            bg[0].needs
        );
    }

    #[test]
    fn test_partition_bg_needs_mix_keeps_bg_drops_fg() {
        // A BG job that depends on one FG name AND one BG name should keep
        // the BG name (so the coordinator's wave loop honors bg→bg ordering
        // — see #454/#463) and drop the FG name (FG already ran).
        let jobs = vec![
            spec("install", false, vec![]),
            spec("bg-a", true, vec![]),
            spec("bg-b", true, vec!["install", "bg-a"]),
        ];
        let (fg, bg) = partition_foreground_background(&jobs);
        assert_eq!(fg.len(), 1);
        assert_eq!(bg.len(), 2);
        let bg_b = bg.iter().find(|j| j.name == "bg-b").unwrap();
        assert_eq!(bg_b.needs, vec!["bg-a".to_string()]);
    }
}
