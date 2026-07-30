//! Hook-level `setup:` steps and the `fail_on_changes:` check.
//!
//! Two things that bracket a hook's jobs rather than being jobs themselves.
//! Kept out of the job machinery on purpose: a setup step that could be
//! filtered out by a glob, reordered by the scheduler, or skipped by a
//! condition would not be setup, and a tree-modification check has to run
//! after everything, which no job can guarantee about itself.

use anyhow::{Context, Result};
use std::collections::BTreeSet;
use std::path::Path;

/// Why a hook stopped before or after its jobs.
#[derive(Debug, Clone)]
pub struct BracketFailure {
    /// User-facing explanation.
    pub message: String,
    /// The failing step's exit code, when there was one.
    pub exit_code: Option<i32>,
}

/// Run a hook's `setup:` commands in order, stopping at the first failure.
///
/// Each runs via `sh -c` in the hook's working directory with the hook
/// environment, so a step sees the same `DAFT_*` variables its jobs will.
/// Output is inherited rather than captured: setup is not a reported row, and
/// a step that prints progress should be seen printing it.
pub fn run_setup(
    steps: &[String],
    working_dir: &Path,
    env: &std::collections::HashMap<String, String>,
) -> Result<Option<BracketFailure>> {
    for (index, step) in steps.iter().enumerate() {
        let status = std::process::Command::new("sh")
            .args(["-c", step])
            .current_dir(working_dir)
            .envs(env)
            .stdin(std::process::Stdio::null())
            .status()
            .with_context(|| format!("failed to run setup step {}: {step}", index + 1))?;
        if !status.success() {
            return Ok(Some(BracketFailure {
                message: format!("setup step {} failed: {step}", index + 1),
                exit_code: status.code(),
            }));
        }
    }
    Ok(None)
}

/// The working tree's modified/untracked paths, as `git status --porcelain`
/// sees them.
///
/// `-z` for the same reason every other path probe uses it: without it git
/// C-quotes any path with a non-ASCII byte, and a check that compares two
/// such listings would report spurious differences on nothing but encoding.
pub fn tree_snapshot(working_dir: &Path) -> Result<BTreeSet<String>> {
    let output = crate::utils::git_command_at(working_dir)
        .args(["status", "--porcelain", "-z"])
        .output()
        .context("failed to read the working tree's status")?;
    if !output.status.success() {
        anyhow::bail!(
            "git status failed: {}",
            String::from_utf8_lossy(&output.stderr).trim()
        );
    }
    Ok(parse_porcelain(&String::from_utf8_lossy(&output.stdout)))
}

/// Compare two snapshots and describe what the hook changed.
///
/// Returns `None` when nothing changed. Entries are compared as whole
/// `XY path` records, so a file going from modified to staged counts as a
/// change — which it is, and which is exactly what a `stage_fixed:` job does.
pub fn changes_since(
    before: &BTreeSet<String>,
    after: &BTreeSet<String>,
) -> Option<BracketFailure> {
    let changed: Vec<&str> = after
        .difference(before)
        .map(String::as_str)
        .chain(before.difference(after).map(String::as_str))
        .collect();
    if changed.is_empty() {
        return None;
    }
    const SHOWN: usize = 10;
    let mut listed: Vec<String> = changed
        .iter()
        .take(SHOWN)
        .map(|entry| format!("  {entry}"))
        .collect();
    if changed.len() > SHOWN {
        listed.push(format!("  … and {} more", changed.len() - SHOWN));
    }
    Some(BracketFailure {
        message: format!(
            "the hook modified the working tree, and fail_on_changes is set:\n{}",
            listed.join("\n")
        ),
        exit_code: None,
    })
}

/// Split `git status --porcelain -z` into records.
///
/// The status format is `XY <path>` NUL-terminated, except a rename carries
/// two NUL-separated paths in one record. Renames are rare here and the check
/// only compares records for equality, so the second path becoming its own
/// entry costs nothing.
fn parse_porcelain(raw: &str) -> BTreeSet<String> {
    raw.split('\0')
        .map(str::trim_end)
        .filter(|s| !s.is_empty())
        .map(str::to_string)
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashMap;
    use tempfile::tempdir;

    fn strs(items: &[&str]) -> BTreeSet<String> {
        items.iter().map(|s| s.to_string()).collect()
    }

    #[test]
    fn setup_steps_run_in_order_and_stop_at_the_first_failure() {
        let dir = tempdir().unwrap();
        let steps = vec![
            "touch first".to_string(),
            "exit 3".to_string(),
            "touch third".to_string(),
        ];
        let failure = run_setup(&steps, dir.path(), &HashMap::new())
            .unwrap()
            .expect("the failing step must be reported");
        assert!(
            failure.message.contains("setup step 2"),
            "{}",
            failure.message
        );
        assert_eq!(failure.exit_code, Some(3));
        assert!(dir.path().join("first").exists());
        assert!(
            !dir.path().join("third").exists(),
            "steps after a failure must not run"
        );
    }

    #[test]
    fn all_steps_succeeding_reports_nothing() {
        let dir = tempdir().unwrap();
        let steps = vec!["true".to_string(), "true".to_string()];
        assert!(
            run_setup(&steps, dir.path(), &HashMap::new())
                .unwrap()
                .is_none()
        );
    }

    #[test]
    fn setup_steps_see_the_hook_environment() {
        let dir = tempdir().unwrap();
        let env: HashMap<String, String> =
            [("DAFT_GIT_STAGE".to_string(), "pre-commit".to_string())]
                .into_iter()
                .collect();
        let steps = vec!["test \"$DAFT_GIT_STAGE\" = pre-commit".to_string()];
        assert!(run_setup(&steps, dir.path(), &env).unwrap().is_none());
    }

    #[test]
    fn an_unchanged_tree_reports_no_changes() {
        let snapshot = strs(&[" M src/lib.rs"]);
        assert!(changes_since(&snapshot, &snapshot).is_none());
    }

    #[test]
    fn a_new_modification_is_reported_with_its_path() {
        // The CI-parity case: a formatter rewrote a file and the gate passed.
        let before = strs(&[]);
        let after = strs(&[" M src/lib.rs"]);
        let failure = changes_since(&before, &after).unwrap();
        assert!(
            failure.message.contains("fail_on_changes"),
            "{}",
            failure.message
        );
        assert!(
            failure.message.contains("src/lib.rs"),
            "{}",
            failure.message
        );
    }

    #[test]
    fn a_file_becoming_staged_counts_as_a_change() {
        // Exactly what `stage_fixed:` does, and the reason the comparison is
        // on whole `XY path` records rather than paths alone.
        let before = strs(&[" M src/lib.rs"]);
        let after = strs(&["M  src/lib.rs"]);
        assert!(changes_since(&before, &after).is_some());
    }

    #[test]
    fn a_long_list_is_capped_with_a_count() {
        let before = strs(&[]);
        let after: BTreeSet<String> = (0..25).map(|i| format!(" M f{i}.rs")).collect();
        let message = changes_since(&before, &after).unwrap().message;
        assert!(message.contains("… and 15 more"), "{message}");
    }

    #[test]
    fn porcelain_records_split_on_nul_not_newline() {
        // A path with a newline in it is legal and would otherwise split into
        // two bogus records.
        let parsed = parse_porcelain(" M a.rs\0?? weird\nname.rs\0");
        assert_eq!(parsed.len(), 2);
        assert!(parsed.contains("?? weird\nname.rs"));
    }
}
