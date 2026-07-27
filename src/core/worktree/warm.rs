//! Core logic for the `git-worktree-warm` command (#387).
//!
//! `daft warm` is the manual re-run of the creation-time `copy:` stage: it
//! replicates the declared build caches from one worktree into another,
//! outside any worktree creation. Two journeys motivate it — a worktree that
//! was created before `copy:` was declared, and a cache that has since been
//! rebuilt in the source and is worth re-seeding elsewhere.
//!
//! All the actual work lives in [`crate::core::copy_paths`]; this module only
//! answers "which two worktrees?" and hands them over. Keeping resolution here
//! rather than in the engine is what lets the creation journeys (which already
//! know both paths) share the engine without inheriting `warm`'s defaults.
//!
//! ## Contract
//!
//! **Warn, never abort** — inherited from the engine. A per-entry failure is a
//! yellow line and a zero exit; only a question daft cannot answer (an unknown
//! worktree name, no source to copy from) is an error. That asymmetry is the
//! point: `warm` is an optimization, and an optimization that fails a script is
//! worse than one that quietly did four caches out of five.
//!
//! **`--force` is the engine's, not ours.** It is passed straight through to
//! [`copy_entries`](crate::core::copy_paths::copy_entries), which owns removing
//! an existing destination entry before re-copying. Deleting a user's directory
//! is exactly the kind of thing that must have one implementation, and the
//! engine's is the one the creation journeys already audit.

use std::path::{Path, PathBuf};

use anyhow::{Context, Result};

use crate::core::ProgressSink;
use crate::core::copy_paths::{
    CopyMethod, CopyOutcome, CopyPathsResult, SkipReason, copy_entries, read_copy_config,
};
use crate::git::GitCommand;

/// Input parameters for the warm operation.
pub struct WarmParams {
    /// Worktree to warm, by path under the project root, branch name, or
    /// directory name. `None` = the worktree the command was run from.
    pub target: Option<String>,
    /// Worktree to copy from, resolved the same way as `target`. `None` = the
    /// current worktree when it differs from the target, otherwise the
    /// default branch's worktree.
    pub from: Option<String>,
    /// Replace destination entries that already exist instead of skipping
    /// them.
    pub force: bool,
}

/// Result of a warm operation.
pub struct WarmResult {
    /// Absolute path of the worktree that was copied from.
    pub source: PathBuf,
    /// Absolute path of the worktree that was copied into.
    pub target: PathBuf,
    /// Display name of the source, relative to the project root.
    pub source_name: String,
    /// Display name of the target, relative to the project root.
    pub target_name: String,
    /// Entries declared by the source's `copy:` section, in config order.
    /// Empty means nothing was declared — the "no work to do" case, kept
    /// distinct from "declared and all skipped" so the command can say so.
    pub declared: Vec<String>,
    /// One outcome per declared entry.
    pub outcome: CopyPathsResult,
}

impl WarmResult {
    /// True when the source declares no `copy:` entries at all.
    pub fn nothing_declared(&self) -> bool {
        self.declared.is_empty()
    }

    /// True when at least one entry was skipped only because the destination
    /// already had it — the case `--force` exists for.
    pub fn has_existing_skips(&self) -> bool {
        self.outcome.outcomes.iter().any(|o| {
            matches!(
                o,
                CopyOutcome::Skipped {
                    reason: SkipReason::DestinationExists,
                    ..
                }
            )
        })
    }
}

/// Execute the warm operation.
///
/// Resolves the source and target worktrees, reads the source's `copy:`
/// section, and runs the copy engine over it. Performs no output of its own
/// beyond `progress` narration; the command layer renders
/// [`WarmResult`].
///
/// Returns `Err` only for questions daft cannot answer — an unresolvable
/// worktree name, or a default source that does not exist. Per-entry copy
/// failures are outcomes, never errors.
pub fn execute<S: ProgressSink>(
    params: &WarmParams,
    git: &GitCommand,
    project_root: &Path,
    progress: &mut S,
) -> Result<WarmResult> {
    let current = git
        .get_current_worktree_path()
        .context("Could not determine the current worktree")?;

    let target = match &params.target {
        Some(name) => git
            .resolve_worktree_path(name, project_root)
            .with_context(|| format!("Could not resolve worktree '{name}'"))?,
        None => current.clone(),
    };

    let source = resolve_source(params, git, project_root, &current, &target)?;

    // Copying a worktree onto itself is never what was meant, and with
    // `--force` it would delete the very entries it was about to copy. The
    // hint offers both readings — wrong source, or forgotten target — because
    // which one the user meant is not knowable from here.
    if source == target {
        anyhow::bail!(
            "source and target are the same worktree ('{}'); pass --from naming a different \
             worktree, or name the target to warm",
            display_name(&target, project_root)
        );
    }
    if !source.is_dir() {
        anyhow::bail!("source worktree does not exist: {}", source.display());
    }
    if !target.is_dir() {
        anyhow::bail!("target worktree does not exist: {}", target.display());
    }

    let source_name = display_name(&source, project_root);
    let target_name = display_name(&target, project_root);

    progress.on_step(&format!(
        "Copying declared paths from '{source_name}' into '{target_name}'"
    ));

    let Some(config) = read_copy_config(&source) else {
        return Ok(WarmResult {
            source,
            target,
            source_name,
            target_name,
            declared: Vec::new(),
            outcome: CopyPathsResult::default(),
        });
    };

    let declared = config.paths.clone();
    let outcome = copy_entries(&source, &target, &config, params.force, progress);

    Ok(WarmResult {
        source,
        target,
        source_name,
        target_name,
        declared,
        outcome,
    })
}

/// Resolve the worktree to copy *from*.
///
/// `--from` wins outright. Otherwise the current worktree is the source
/// whenever it is not itself the target — warming a sibling from where you
/// stand is the common shape. When the target *is* the current worktree there
/// is no such answer, so the default branch's worktree stands in: it is the
/// one worktree every repo has, and the one whose caches are most likely to be
/// both present and generic.
fn resolve_source(
    params: &WarmParams,
    git: &GitCommand,
    project_root: &Path,
    current: &Path,
    target: &Path,
) -> Result<PathBuf> {
    if let Some(name) = &params.from {
        return git
            .resolve_worktree_path(name, project_root)
            .with_context(|| format!("Could not resolve worktree '{name}'"));
    }

    if current != target {
        return Ok(current.to_path_buf());
    }

    let branch = default_branch(project_root).ok_or_else(|| {
        anyhow::anyhow!(
            "could not determine this repository's default branch; run `{}` to choose a source",
            crate::daft_cmd("warm --from <worktree>")
        )
    })?;

    git.find_worktree_for_branch(&branch)
        .with_context(|| format!("Could not look up the worktree for '{branch}'"))?
        .ok_or_else(|| {
            anyhow::anyhow!(
                "the default branch '{branch}' has no worktree to copy from; run `{}` to choose a source",
                crate::daft_cmd("warm --from <worktree>")
            )
        })
}

/// This repository's default branch: the catalog's record first (it survives a
/// missing `origin/HEAD`), then the local symref. Both probes are best-effort —
/// a machine with no catalog, or a repo cloned before daft knew it, still gets
/// an answer from git alone.
fn default_branch(project_root: &Path) -> Option<String> {
    crate::core::repo::git_common_dir_at(project_root)
        .and_then(|dir| crate::catalog::live_catalog_row_for(&dir))
        .and_then(|row| crate::catalog::effective_default_branch(&row))
        .or_else(|| crate::core::remote::local_default_branch(project_root, "origin"))
}

/// A worktree's name as the user thinks of it: its path relative to the
/// project root, falling back to the final component for anything outside.
fn display_name(path: &Path, project_root: &Path) -> String {
    path.strip_prefix(project_root)
        .ok()
        .and_then(|p| p.to_str())
        .filter(|s| !s.is_empty())
        .map(str::to_string)
        .unwrap_or_else(|| {
            path.file_name()
                .map(|n| n.to_string_lossy().to_string())
                .unwrap_or_else(|| path.display().to_string())
        })
}

/// The one-line reason a `copy:` entry was left out, phrased as a complete
/// standalone clause.
///
/// Deliberately prefix-free (`already present`, not `skipped — already
/// present`): the rail's timeline exempts `StageId::CopyPath` from the
/// `skipped — ` prefix, and `warm`'s plain lines carry the entry name in front
/// instead. One phrasing serves both surfaces so a user who sees
/// `node_modules is tracked — not copied` on create reads the identical
/// sentence from `daft warm`.
pub fn skip_phrase(reason: &SkipReason, entry: &str) -> String {
    match reason {
        SkipReason::NoSource => "nothing to copy yet".to_string(),
        SkipReason::DestinationExists => "already present".to_string(),
        SkipReason::NotIgnored => format!("'{entry}' is tracked — not copied"),
        SkipReason::NoReflink => {
            "this filesystem cannot reflink, and fallback is 'skip'".to_string()
        }
        SkipReason::TooLarge {
            size_bytes,
            limit_bytes,
        } => format!(
            "{} over the {} max_size",
            format_bytes(size_bytes.saturating_sub(*limit_bytes)),
            format_bytes(*limit_bytes)
        ),
        SkipReason::NoMatches => "matched nothing".to_string(),
    }
}

/// The success annotation for one copied entry: how many paths it expanded to,
/// how much they weighed, how they were replicated, and how long it took —
/// `3 dirs · 1.2 GB · reflinked · 0.3s`.
pub fn copied_annotation(
    matches: usize,
    bytes: u64,
    method: CopyMethod,
    elapsed: std::time::Duration,
) -> String {
    let unit = if matches == 1 { "path" } else { "paths" };
    let how = match method {
        CopyMethod::Reflinked => "reflinked",
        CopyMethod::Copied => "copied",
    };
    format!(
        "{matches} {unit} · {} · {how} · {:.1}s",
        format_bytes(bytes),
        elapsed.as_secs_f64()
    )
}

/// Human-readable byte count, binary multiples with decimal-style unit names —
/// the spelling daft already uses for job-log sizes.
pub fn format_bytes(n: u64) -> String {
    const KB: f64 = 1024.0;
    const MB: f64 = KB * 1024.0;
    const GB: f64 = MB * 1024.0;
    let n_f = n as f64;
    if n_f >= GB {
        format!("{:.1} GB", n_f / GB)
    } else if n_f >= MB {
        format!("{:.1} MB", n_f / MB)
    } else if n_f >= KB {
        format!("{:.1} KB", n_f / KB)
    } else {
        format!("{n} B")
    }
}

/// The declared entries that never produced an outcome.
///
/// The engine promises one outcome per entry, so this is normally empty; it
/// exists because a silent gap between "declared" and "reported" is exactly the
/// failure a cache feature must not have — the user would read a clean summary
/// while a cache they asked for was never even attempted.
pub fn unreported<'a>(declared: &'a [String], result: &CopyPathsResult) -> Vec<&'a str> {
    declared
        .iter()
        .filter(|entry| !result.outcomes.iter().any(|o| o.entry() == entry.as_str()))
        .map(String::as_str)
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Duration;

    #[test]
    fn display_name_is_relative_to_the_project_root() {
        let root = Path::new("/repos/acme");
        assert_eq!(display_name(Path::new("/repos/acme/main"), root), "main");
        assert_eq!(
            display_name(Path::new("/repos/acme/feature/login"), root),
            "feature/login"
        );
        // Outside the project root there is no relative name; the final
        // component still identifies the worktree for a human.
        assert_eq!(display_name(Path::new("/elsewhere/wt"), root), "wt");
        // The project root itself has no relative name — fall back rather
        // than render an empty string.
        assert_eq!(display_name(root, root), "acme");
    }

    #[test]
    fn skip_phrases_read_as_complete_clauses() {
        // No `skipped — ` prefix anywhere: the rail exempts CopyPath rows and
        // warm puts the entry name in front, so each phrase must stand alone.
        for (reason, expected) in [
            (SkipReason::NoSource, "nothing to copy yet"),
            (SkipReason::DestinationExists, "already present"),
            (SkipReason::NoMatches, "matched nothing"),
        ] {
            assert_eq!(skip_phrase(&reason, "target"), expected);
        }
        assert_eq!(
            skip_phrase(&SkipReason::NotIgnored, "target"),
            "'target' is tracked — not copied"
        );
        assert!(!skip_phrase(&SkipReason::NoReflink, "target").starts_with("skipped"));
    }

    #[test]
    fn too_large_names_the_overage_and_the_cap() {
        let phrase = skip_phrase(
            &SkipReason::TooLarge {
                size_bytes: 3 * 1024 * 1024 * 1024,
                limit_bytes: 1024 * 1024 * 1024,
            },
            "target",
        );
        assert_eq!(phrase, "2.0 GB over the 1.0 GB max_size");
    }

    #[test]
    fn format_bytes_scales_through_the_binary_units() {
        assert_eq!(format_bytes(0), "0 B");
        assert_eq!(format_bytes(512), "512 B");
        assert_eq!(format_bytes(1024), "1.0 KB");
        assert_eq!(format_bytes(1024 * 1024), "1.0 MB");
        assert_eq!(format_bytes(3 * 1024 * 1024 * 1024 / 2), "1.5 GB");
    }

    #[test]
    fn copied_annotation_singularizes_one_path() {
        assert_eq!(
            copied_annotation(1, 1024, CopyMethod::Reflinked, Duration::from_millis(300)),
            "1 path · 1.0 KB · reflinked · 0.3s"
        );
        assert_eq!(
            copied_annotation(3, 0, CopyMethod::Copied, Duration::from_secs(2)),
            "3 paths · 0 B · copied · 2.0s"
        );
    }

    #[test]
    fn unreported_finds_declared_entries_with_no_outcome() {
        let declared = vec!["target".to_string(), "node_modules".to_string()];
        let result = CopyPathsResult {
            outcomes: vec![CopyOutcome::Skipped {
                entry: "target".into(),
                reason: SkipReason::NoSource,
            }],
        };
        assert_eq!(unreported(&declared, &result), ["node_modules"]);
        assert!(unreported(&declared, &CopyPathsResult::default()).len() == 2);
    }

    #[test]
    fn warm_result_flags_the_force_case() {
        let base = |outcomes| WarmResult {
            source: PathBuf::from("/repos/acme/main"),
            target: PathBuf::from("/repos/acme/dev"),
            source_name: "main".into(),
            target_name: "dev".into(),
            declared: vec!["target".to_string()],
            outcome: CopyPathsResult { outcomes },
        };

        let existing = base(vec![CopyOutcome::Skipped {
            entry: "target".into(),
            reason: SkipReason::DestinationExists,
        }]);
        assert!(existing.has_existing_skips());
        assert!(!existing.nothing_declared());

        let missing = base(vec![CopyOutcome::Skipped {
            entry: "target".into(),
            reason: SkipReason::NoSource,
        }]);
        assert!(!missing.has_existing_skips());
    }
}
