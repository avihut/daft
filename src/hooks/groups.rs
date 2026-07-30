//! Flattening nested job groups into the flat DAG the executor runs.
//!
//! A `group:` is a job whose body is other jobs, with its own `parallel:` or
//! `piped:`. The executor has no notion of nesting — it schedules a flat list
//! with `needs:` edges — so a group is rewritten into that vocabulary before
//! it ever reaches the runner. Nothing about the group survives except its
//! name, which prefixes its children so `lint/eslint` reads as what it is.

use super::yaml_config::{GroupDef, JobDef};
use anyhow::{Result, bail};

/// The separator between a group's name and a child's.
const GROUP_SEP: char = '/';

/// Expand every `group:` job in `jobs` into its children.
///
/// Three rewrites make a group behave the way it reads:
///
/// - A `piped:` group becomes a `needs:` chain among its children, so they
///   run in order and a failure stops the rest — which is what piped means.
/// - A child inherits the group's `needs:`, so "the group runs after build"
///   holds for every job in it.
/// - Another job's `needs: <group>` is rewritten to the group's *terminal*
///   children — the ones nothing else in the group waits on. Waiting on the
///   group's name has to mean waiting for the group to finish; pointing it at
///   the first child would let a dependent start while the group was still
///   running.
///
/// Nesting is one level deep. A group inside a group is a validation error
/// rather than a recursion: the flat name would stop being readable, and no
/// scheduling need requires it.
pub fn flatten(jobs: &[JobDef]) -> Result<Vec<JobDef>> {
    let mut out: Vec<JobDef> = Vec::with_capacity(jobs.len());
    // group name → the children a dependent on the group must wait for.
    let mut terminals: Vec<(String, Vec<String>)> = Vec::new();

    for job in jobs {
        let Some(group) = job.group.as_ref() else {
            out.push(job.clone());
            continue;
        };
        let group_name = job
            .name
            .clone()
            .ok_or_else(|| anyhow::anyhow!("a group: job must have a name"))?;
        let (children, group_terminals) = expand_group(&group_name, group, job)?;
        terminals.push((group_name, group_terminals));
        out.extend(children);
    }

    if !terminals.is_empty() {
        rewrite_group_references(&mut out, &terminals);
    }
    Ok(out)
}

/// Expand one group into `(children, terminal_child_names)`.
fn expand_group(
    group_name: &str,
    group: &GroupDef,
    parent: &JobDef,
) -> Result<(Vec<JobDef>, Vec<String>)> {
    let children = group.jobs.as_deref().unwrap_or_default();
    if children.is_empty() {
        bail!("group '{group_name}' has no jobs");
    }
    if children.iter().any(|c| c.group.is_some()) {
        bail!(
            "group '{group_name}' contains a nested group; groups are one level deep \
             (flatten it, or give the inner jobs their own needs:)"
        );
    }

    let piped = group.piped == Some(true);
    let parent_needs = parent.needs.clone().unwrap_or_default();

    let mut expanded: Vec<JobDef> = Vec::with_capacity(children.len());
    let mut previous: Option<String> = None;
    for (index, child) in children.iter().enumerate() {
        let child_label = child
            .name
            .clone()
            .unwrap_or_else(|| format!("job{}", index + 1));
        let qualified = format!("{group_name}{GROUP_SEP}{child_label}");

        // A child's own `needs:` names siblings, so qualify them too.
        let mut needs: Vec<String> = child
            .needs
            .clone()
            .unwrap_or_default()
            .into_iter()
            .map(|n| format!("{group_name}{GROUP_SEP}{n}"))
            .collect();
        // The group's own dependencies apply to every child.
        needs.extend(parent_needs.iter().cloned());
        // `piped:` is a chain, expressed the only way the flat executor can
        // express ordering.
        if piped && let Some(prev) = previous.as_ref() {
            needs.push(prev.clone());
        }

        previous = Some(qualified.clone());
        expanded.push(JobDef {
            name: Some(qualified),
            needs: (!needs.is_empty()).then_some(needs),
            ..child.clone()
        });
    }

    // Terminals: the children nothing else in the group waits on. For a piped
    // group that is the last one; for a parallel group, every child that is
    // not somebody's dependency.
    let depended_on: std::collections::HashSet<&str> = expanded
        .iter()
        .flat_map(|j| j.needs.iter().flatten())
        .map(String::as_str)
        .collect();
    let group_terminals: Vec<String> = expanded
        .iter()
        .filter_map(|j| j.name.clone())
        .filter(|n| !depended_on.contains(n.as_str()))
        .collect();

    Ok((expanded, group_terminals))
}

/// Point every `needs: <group-name>` at the group's terminal children.
fn rewrite_group_references(jobs: &mut [JobDef], terminals: &[(String, Vec<String>)]) {
    for job in jobs {
        let Some(needs) = job.needs.as_mut() else {
            continue;
        };
        let mut rewritten: Vec<String> = Vec::with_capacity(needs.len());
        for need in needs.iter() {
            match terminals.iter().find(|(name, _)| name == need) {
                Some((_, group_terminals)) => rewritten.extend(group_terminals.iter().cloned()),
                None => rewritten.push(need.clone()),
            }
        }
        rewritten.dedup();
        *needs = rewritten;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::hooks::yaml_config::RunCommand;

    fn job(name: &str, needs: &[&str]) -> JobDef {
        JobDef {
            name: Some(name.into()),
            run: Some(RunCommand::Simple("true".into())),
            needs: (!needs.is_empty()).then(|| needs.iter().map(|s| s.to_string()).collect()),
            ..Default::default()
        }
    }

    fn group_job(name: &str, piped: bool, needs: &[&str], children: Vec<JobDef>) -> JobDef {
        JobDef {
            name: Some(name.into()),
            group: Some(GroupDef {
                parallel: (!piped).then_some(true),
                piped: piped.then_some(true),
                jobs: Some(children),
            }),
            needs: (!needs.is_empty()).then(|| needs.iter().map(|s| s.to_string()).collect()),
            ..Default::default()
        }
    }

    fn names(jobs: &[JobDef]) -> Vec<String> {
        jobs.iter().filter_map(|j| j.name.clone()).collect()
    }

    fn needs_of<'a>(jobs: &'a [JobDef], name: &str) -> Vec<&'a str> {
        jobs.iter()
            .find(|j| j.name.as_deref() == Some(name))
            .and_then(|j| j.needs.as_ref())
            .map(|n| n.iter().map(String::as_str).collect())
            .unwrap_or_default()
    }

    #[test]
    fn a_group_becomes_prefixed_children() {
        let jobs = vec![group_job(
            "lint",
            false,
            &[],
            vec![job("eslint", &[]), job("stylelint", &[])],
        )];
        let flat = flatten(&jobs).unwrap();
        assert_eq!(names(&flat), vec!["lint/eslint", "lint/stylelint"]);
        // A parallel group adds no ordering.
        assert!(needs_of(&flat, "lint/eslint").is_empty());
    }

    #[test]
    fn a_piped_group_becomes_a_needs_chain() {
        // `piped` means "in order, stop on failure", and a `needs:` edge is
        // the only ordering the flat executor understands.
        let jobs = vec![group_job(
            "build",
            true,
            &[],
            vec![job("deps", &[]), job("compile", &[]), job("bundle", &[])],
        )];
        let flat = flatten(&jobs).unwrap();
        assert!(needs_of(&flat, "build/deps").is_empty());
        assert_eq!(needs_of(&flat, "build/compile"), vec!["build/deps"]);
        assert_eq!(needs_of(&flat, "build/bundle"), vec!["build/compile"]);
    }

    #[test]
    fn children_inherit_the_groups_own_dependencies() {
        let jobs = vec![
            job("install", &[]),
            group_job("lint", false, &["install"], vec![job("eslint", &[])]),
        ];
        let flat = flatten(&jobs).unwrap();
        assert_eq!(needs_of(&flat, "lint/eslint"), vec!["install"]);
    }

    #[test]
    fn a_childs_own_needs_are_qualified_to_its_siblings() {
        let jobs = vec![group_job(
            "lint",
            false,
            &[],
            vec![job("gen", &[]), job("check", &["gen"])],
        )];
        let flat = flatten(&jobs).unwrap();
        assert_eq!(needs_of(&flat, "lint/check"), vec!["lint/gen"]);
    }

    #[test]
    fn depending_on_a_group_waits_for_all_of_it() {
        // Pointing the dependent at the first child would let it start while
        // the group was still running — the opposite of what it asked for.
        let jobs = vec![
            group_job(
                "lint",
                false,
                &[],
                vec![job("eslint", &[]), job("stylelint", &[])],
            ),
            job("report", &["lint"]),
        ];
        let flat = flatten(&jobs).unwrap();
        let report = needs_of(&flat, "report");
        assert!(report.contains(&"lint/eslint"), "{report:?}");
        assert!(report.contains(&"lint/stylelint"), "{report:?}");
    }

    #[test]
    fn depending_on_a_piped_group_waits_for_its_last_job_only() {
        let jobs = vec![
            group_job("build", true, &[], vec![job("a", &[]), job("b", &[])]),
            job("ship", &["build"]),
        ];
        let flat = flatten(&jobs).unwrap();
        // `build/b` already waits for `build/a`, so waiting on `build/b` is
        // waiting for the whole chain.
        assert_eq!(needs_of(&flat, "ship"), vec!["build/b"]);
    }

    #[test]
    fn ungrouped_jobs_pass_through_untouched() {
        let jobs = vec![job("a", &[]), job("b", &["a"])];
        let flat = flatten(&jobs).unwrap();
        assert_eq!(names(&flat), vec!["a", "b"]);
        assert_eq!(needs_of(&flat, "b"), vec!["a"]);
    }

    #[test]
    fn a_nested_group_is_a_configuration_error() {
        let inner = group_job("inner", false, &[], vec![job("x", &[])]);
        let jobs = vec![group_job("outer", false, &[], vec![inner])];
        let err = flatten(&jobs).unwrap_err();
        let msg = format!("{err:#}");
        assert!(msg.contains("nested group"), "{msg}");
        assert!(msg.contains("one level deep"), "{msg}");
    }

    #[test]
    fn an_empty_group_is_a_configuration_error() {
        let jobs = vec![group_job("lint", false, &[], vec![])];
        let err = flatten(&jobs).unwrap_err();
        assert!(format!("{err:#}").contains("no jobs"));
    }

    #[test]
    fn an_unnamed_child_gets_a_positional_name() {
        let unnamed = JobDef {
            run: Some(RunCommand::Simple("true".into())),
            ..Default::default()
        };
        let jobs = vec![group_job("lint", false, &[], vec![unnamed])];
        let flat = flatten(&jobs).unwrap();
        assert_eq!(names(&flat), vec!["lint/job1"]);
    }
}
