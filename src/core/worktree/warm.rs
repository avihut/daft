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

    /// Every skip reason is a phrase a user reads mid-sentence, so each one is
    /// pinned literally: a reword is a UX decision, not a refactor. Two of
    /// them are also asserted by the `copy` YAML scenarios and (once the rail
    /// lands) by the creation timeline, so drift here is drift there.
    #[test]
    fn every_skip_reason_has_a_pinned_phrase() {
        let cases = [
            (SkipReason::NoSource, "nothing to copy yet"),
            (SkipReason::DestinationExists, "already present"),
            (SkipReason::NoMatches, "matched nothing"),
            (
                SkipReason::NoReflink,
                "this filesystem cannot reflink, and fallback is 'skip'",
            ),
            (SkipReason::NotIgnored, "'**/dist' is tracked — not copied"),
            (
                SkipReason::TooLarge {
                    size_bytes: 1536,
                    limit_bytes: 1024,
                },
                "512 B over the 1.0 KB max_size",
            ),
        ];
        for (reason, expected) in cases {
            assert_eq!(skip_phrase(&reason, "**/dist"), expected, "{reason:?}");
        }
    }

    /// Only `NotIgnored` names the entry — the others are about the copy, not
    /// the path, and the rendering surfaces already put the entry in front.
    /// Pinning this stops a well-meaning "let's mention the entry everywhere"
    /// change from producing `node_modules: node_modules already present`.
    #[test]
    fn only_the_tracked_phrase_repeats_the_entry() {
        for reason in [
            SkipReason::NoSource,
            SkipReason::DestinationExists,
            SkipReason::NoMatches,
            SkipReason::NoReflink,
        ] {
            let phrase = skip_phrase(&reason, "node_modules");
            assert!(
                !phrase.contains("node_modules"),
                "{reason:?} should not repeat the entry: {phrase}"
            );
        }
        assert!(skip_phrase(&SkipReason::NotIgnored, "node_modules").contains("node_modules"));
    }

    /// A cap the size did not actually exceed cannot underflow into a
    /// nonsense number — the subtraction saturates, so the worst case reads
    /// `0 B over …` rather than panicking or printing 16 exabytes.
    #[test]
    fn an_overage_below_the_cap_saturates_instead_of_wrapping() {
        assert_eq!(
            skip_phrase(
                &SkipReason::TooLarge {
                    size_bytes: 512,
                    limit_bytes: 1024,
                },
                "target",
            ),
            "0 B over the 1.0 KB max_size"
        );
    }

    #[test]
    fn format_bytes_scales_through_the_binary_units() {
        assert_eq!(format_bytes(0), "0 B");
        assert_eq!(format_bytes(512), "512 B");
        assert_eq!(format_bytes(1024), "1.0 KB");
        assert_eq!(format_bytes(1024 * 1024), "1.0 MB");
        assert_eq!(format_bytes(3 * 1024 * 1024 * 1024 / 2), "1.5 GB");
    }

    /// The unit boundaries, from both sides. The just-under cases are the
    /// interesting ones: rounding to one decimal makes a byte count that has
    /// not reached the next unit render as `1024.0 KB` rather than `1.0 MB`.
    /// Cosmetic, deliberate, and pinned so it changes on purpose if ever.
    #[test]
    fn format_bytes_boundaries_round_predictably() {
        assert_eq!(format_bytes(1023), "1023 B");
        assert_eq!(format_bytes(1024 * 1024 - 1), "1024.0 KB");
        assert_eq!(format_bytes(1024 * 1024 * 1024 - 1), "1024.0 MB");
        assert_eq!(format_bytes(1024 * 1024 * 1024), "1.0 GB");
        // No unit above GB: a terabyte cache reads as four figures of GB
        // rather than silently losing scale.
        assert_eq!(format_bytes(1024_u64.pow(4)), "1024.0 GB");
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

    /// The gap detector answers about *declarations*, so an outcome for
    /// something nobody declared must not paper over a declaration that never
    /// got one. Counting outcomes instead of matching them would do exactly
    /// that, and the missing cache would go unmentioned.
    #[test]
    fn an_unexpected_outcome_does_not_mask_a_missing_one() {
        let declared = vec!["target".to_string(), "node_modules".to_string()];
        let result = CopyPathsResult {
            outcomes: vec![
                CopyOutcome::Skipped {
                    entry: "target".into(),
                    reason: SkipReason::NoSource,
                },
                // Never declared — a glob's expansion leaking out as its own
                // outcome is the shape this guards against.
                CopyOutcome::Skipped {
                    entry: "web/dist".into(),
                    reason: SkipReason::NoSource,
                },
            ],
        };
        assert_eq!(unreported(&declared, &result), ["node_modules"]);
    }

    /// Declaration order is reporting order: the user reads the gaps in the
    /// order they wrote them, not in whatever order the engine finished.
    #[test]
    fn unreported_preserves_declaration_order() {
        let declared = vec![
            "a".to_string(),
            "b".to_string(),
            "c".to_string(),
            "d".to_string(),
        ];
        let result = CopyPathsResult {
            outcomes: vec![CopyOutcome::Skipped {
                entry: "c".into(),
                reason: SkipReason::NoSource,
            }],
        };
        assert_eq!(unreported(&declared, &result), ["a", "b", "d"]);
    }

    /// Zero matches is a number, not a plural exception — a glob that expanded
    /// to nothing but still copied would read `0 paths`, never `0 path`.
    #[test]
    fn copied_annotation_pluralizes_everything_but_one() {
        assert_eq!(
            copied_annotation(0, 0, CopyMethod::Copied, Duration::ZERO),
            "0 paths · 0 B · copied · 0.0s"
        );
        assert_eq!(
            copied_annotation(2, 1024, CopyMethod::Reflinked, Duration::from_millis(50)),
            "2 paths · 1.0 KB · reflinked · 0.1s"
        );
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

/// Source/target resolution against a real daft container layout.
///
/// The behavior matrix `execute` implements — target defaults to here,
/// `--from` wins outright, an implicit source is "here unless here is the
/// target, then the default branch's worktree" — is entirely decided before
/// the engine is called, so it is pinned here and stays pinned when the engine
/// lands. Every case asserts the *resolved pair*, not the copy result: the
/// engine is what changes underneath, the resolution is not.
///
/// These are `#[serial]`: `execute` reads the process cwd (through
/// `git rev-parse --show-toplevel` and `git worktree list`), and the implicit
/// source path consults the repo catalog, which resolves the daft state dir.
#[cfg(test)]
mod resolution_tests {
    use super::*;
    use crate::core::NullSink;
    use serial_test::serial;
    use std::process::{Command as ShellCommand, Stdio};

    /// Restores the process cwd — a test that leaves it inside a deleted
    /// tempdir silently breaks every later test in the same process.
    struct CwdGuard {
        original: PathBuf,
    }

    impl CwdGuard {
        fn new() -> Self {
            Self {
                original: std::env::current_dir().unwrap_or_else(|_| std::env::temp_dir()),
            }
        }
    }

    impl Drop for CwdGuard {
        fn drop(&mut self) {
            if std::env::set_current_dir(&self.original).is_err() {
                let _ = std::env::set_current_dir(std::env::temp_dir());
            }
        }
    }

    fn git_at(dir: &Path, args: &[&str]) {
        let status = ShellCommand::new("git")
            .args(args)
            .current_dir(dir)
            .env_remove("GIT_DIR")
            .env_remove("GIT_WORK_TREE")
            .env("GIT_AUTHOR_NAME", "Test")
            .env("GIT_AUTHOR_EMAIL", "test@test.com")
            .env("GIT_COMMITTER_NAME", "Test")
            .env("GIT_COMMITTER_EMAIL", "test@test.com")
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .status()
            .unwrap();
        assert!(status.success(), "git {args:?} failed in {}", dir.display());
    }

    /// A daft container layout: a bare `.git` at the project root with sibling
    /// worktree directories, the shape every resolution rule is written for.
    /// `origin/HEAD` is planted so the catalog-free default-branch fallback has
    /// something to read.
    struct Layout {
        tmp: tempfile::TempDir,
        root: PathBuf,
    }

    impl Layout {
        fn new(extra_branches: &[&str]) -> Self {
            let tmp = tempfile::tempdir().unwrap();
            // Canonicalize: macOS hands out `/var/...` symlinks while git
            // reports `/private/var/...`, and every assertion here compares
            // paths git produced against paths the fixture produced.
            let base = tmp.path().canonicalize().unwrap();
            let seed = base.join("seed");
            std::fs::create_dir_all(&seed).unwrap();
            git_at(&seed, &["init", "-q", "-b", "main"]);
            git_at(&seed, &["commit", "--allow-empty", "-q", "-m", "init"]);

            let root = base.join("acme");
            std::fs::create_dir_all(&root).unwrap();
            git_at(
                &base,
                &[
                    "clone",
                    "--bare",
                    "-q",
                    &seed.display().to_string(),
                    &root.join(".git").display().to_string(),
                ],
            );
            let bare = root.join(".git");
            git_at(&bare, &["worktree", "add", "-q", "../main", "main"]);
            for branch in extra_branches {
                git_at(
                    &bare,
                    &[
                        "worktree",
                        "add",
                        "-q",
                        &format!("../{branch}"),
                        "-b",
                        branch,
                    ],
                );
            }
            Self { tmp, root }
        }

        /// Plant `origin/HEAD` so `local_default_branch` resolves without a
        /// catalog entry — the catalog-free fallback path.
        fn with_origin_head(self) -> Self {
            let remotes = self.root.join(".git/refs/remotes/origin");
            std::fs::create_dir_all(&remotes).unwrap();
            std::fs::write(remotes.join("HEAD"), "ref: refs/remotes/origin/main\n").unwrap();
            self
        }

        /// Add a worktree whose directory name deliberately differs from its
        /// branch name — the only shape that can tell the resolution tiers
        /// apart.
        fn with_worktree(self, dir: &str, branch: &str) -> Self {
            git_at(
                &self.root.join(".git"),
                &["worktree", "add", "-q", &format!("../{dir}"), "-b", branch],
            );
            self
        }

        fn wt(&self, name: &str) -> PathBuf {
            self.root.join(name)
        }

        fn base(&self) -> PathBuf {
            self.tmp.path().canonicalize().unwrap()
        }
    }

    /// Redirects `DAFT_DATA_DIR` — where the repo catalog's SQLite file lives —
    /// at a fresh tempdir for the duration of a test. `IsolatedStateDir` covers
    /// the *state* dir (per-repo coordinator DBs); the catalog is under the
    /// *data* dir, so warming's default-branch probe would otherwise read the
    /// developer's real `catalog.db` (CLAUDE.md's "never let a test touch
    /// daft's real dirs"; same class as #697).
    ///
    /// Callers MUST be `#[serial]`: the mutation is process-global.
    struct IsolatedDataDir {
        _tmp: tempfile::TempDir,
        prev: Option<std::ffi::OsString>,
    }

    impl IsolatedDataDir {
        fn new() -> Self {
            let tmp = tempfile::tempdir().unwrap();
            let prev = std::env::var_os(crate::DATA_DIR_ENV);
            // SAFETY: `set_var` is `unsafe fn` in edition 2024 (process-global,
            // not thread-safe). Every caller is `#[serial]`.
            unsafe { std::env::set_var(crate::DATA_DIR_ENV, tmp.path().canonicalize().unwrap()) };
            Self { _tmp: tmp, prev }
        }
    }

    impl Drop for IsolatedDataDir {
        fn drop(&mut self) {
            // SAFETY: as in `new` — serialized by `#[serial]`.
            unsafe {
                match &self.prev {
                    Some(v) => std::env::set_var(crate::DATA_DIR_ENV, v),
                    None => std::env::remove_var(crate::DATA_DIR_ENV),
                }
            }
        }
    }

    /// Both isolations at once — held for the whole test, not just the call,
    /// so a test may seed the sandboxed catalog before running `execute`.
    fn isolate() -> (crate::store::paths::IsolatedStateDir, IsolatedDataDir) {
        (
            crate::store::paths::IsolatedStateDir::new(),
            IsolatedDataDir::new(),
        )
    }

    /// Run `execute` from `cwd`. The daft state and data dirs must already be
    /// isolated by the caller (see [`isolate`]) — the default-branch probe
    /// reads the repo catalog.
    fn run(layout: &Layout, cwd: &Path, params: WarmParams) -> Result<WarmResult> {
        std::env::set_current_dir(cwd).unwrap();
        let git = GitCommand::new(true);
        execute(&params, &git, &layout.root, &mut NullSink)
    }

    fn params(target: Option<&str>, from: Option<&str>) -> WarmParams {
        WarmParams {
            target: target.map(str::to_string),
            from: from.map(str::to_string),
            force: false,
        }
    }

    /// `Result::expect_err` needs `Debug` on the Ok type and `WarmResult` has
    /// none; this keeps the failure message just as loud without asking the
    /// production type to grow a derive for the tests' convenience.
    fn err_of(outcome: Result<WarmResult>, why: &str) -> anyhow::Error {
        match outcome {
            Ok(ok) => panic!(
                "{why} — instead it resolved {} -> {}",
                ok.source.display(),
                ok.target.display()
            ),
            Err(e) => e,
        }
    }

    #[test]
    #[serial]
    fn a_named_target_is_warmed_from_where_you_stand() {
        let _cwd = CwdGuard::new();
        let _iso = isolate();
        let layout = Layout::new(&["develop"]).with_origin_head();

        let result = run(&layout, &layout.wt("main"), params(Some("develop"), None))
            .expect("naming a sibling target must resolve");

        assert_eq!(result.target, layout.wt("develop"));
        assert_eq!(result.source, layout.wt("main"));
        assert_eq!(result.target_name, "develop");
        assert_eq!(result.source_name, "main");
    }

    /// Path, branch name, and directory name are three spellings of one
    /// worktree; all three must land on the same pair, or `daft warm` would
    /// behave differently depending on how the user happened to type it.
    ///
    /// The fixture separates the tiers on purpose: the worktree lives in
    /// `wt-feature` but is checked out on `feature/login`, so a resolver that
    /// only ever consulted one tier would fail half the spellings.
    #[test]
    #[serial]
    fn a_target_resolves_by_path_branch_or_directory_name() {
        let _cwd = CwdGuard::new();
        let _iso = isolate();
        let layout = Layout::new(&[])
            .with_worktree("wt-feature", "feature/login")
            .with_origin_head();
        let from = layout.wt("main");
        let expected = layout.wt("wt-feature");

        let spellings = [
            ("directory name", "wt-feature".to_string()),
            ("branch name", "feature/login".to_string()),
            ("relative path", "./wt-feature".to_string()),
            ("absolute path", expected.display().to_string()),
        ];
        for (tier, spelling) in spellings {
            let result = run(&layout, &from, params(Some(&spelling), None))
                .unwrap_or_else(|e| panic!("{tier} '{spelling}' should resolve: {e:#}"));
            assert_eq!(result.target, expected, "{tier}: {spelling}");
            // The display name follows the directory, never the branch — it is
            // where the user would `cd`.
            assert_eq!(result.target_name, "wt-feature", "{tier}: {spelling}");
        }
    }

    /// `--from` goes through the same resolver as the positional, so it has to
    /// answer to the same spellings. A `--from` that only understood directory
    /// names would silently be a different vocabulary from the positional's.
    #[test]
    #[serial]
    fn from_resolves_by_the_same_spellings_as_the_target() {
        let _cwd = CwdGuard::new();
        let _iso = isolate();
        let layout = Layout::new(&["develop"])
            .with_worktree("wt-feature", "feature/login")
            .with_origin_head();
        let expected = layout.wt("wt-feature");

        for spelling in [
            "wt-feature".to_string(),
            "feature/login".to_string(),
            expected.display().to_string(),
        ] {
            let result = run(
                &layout,
                &layout.wt("main"),
                params(Some("develop"), Some(&spelling)),
            )
            .unwrap_or_else(|e| panic!("--from '{spelling}' should resolve: {e:#}"));
            assert_eq!(result.source, expected, "--from {spelling}");
            assert_eq!(result.source_name, "wt-feature", "--from {spelling}");
        }
    }

    /// Standing in the target is the "warm me from the default branch" shape:
    /// there is no other worktree the command could mean.
    #[test]
    #[serial]
    fn standing_in_the_target_falls_back_to_the_default_branch_worktree() {
        let _cwd = CwdGuard::new();
        let _iso = isolate();
        let layout = Layout::new(&["develop"]).with_origin_head();

        let result = run(&layout, &layout.wt("develop"), params(None, None))
            .expect("the default branch's worktree is the implicit source");

        assert_eq!(result.target, layout.wt("develop"));
        assert_eq!(result.source, layout.wt("main"));
    }

    /// Same fallback, reached the long way: the target is named rather than
    /// implied, but it is still the worktree we are standing in.
    #[test]
    #[serial]
    fn naming_your_own_worktree_as_the_target_still_falls_back() {
        let _cwd = CwdGuard::new();
        let _iso = isolate();
        let layout = Layout::new(&["develop"]).with_origin_head();

        let result = run(
            &layout,
            &layout.wt("develop"),
            params(Some("develop"), None),
        )
        .expect("naming the current worktree is the same case as omitting it");

        assert_eq!(result.source, layout.wt("main"));
    }

    /// `--from` beats the current worktree even when the current worktree
    /// would have been a perfectly good source.
    #[test]
    #[serial]
    fn from_wins_over_the_current_worktree() {
        let _cwd = CwdGuard::new();
        let _iso = isolate();
        let layout = Layout::new(&["develop", "release"]).with_origin_head();

        let result = run(
            &layout,
            &layout.wt("main"),
            params(Some("develop"), Some("release")),
        )
        .expect("--from names the source outright");

        assert_eq!(result.source, layout.wt("release"));
        assert_eq!(result.target, layout.wt("develop"));
    }

    /// The refusal that keeps `--force` from deleting the entries it was about
    /// to copy. Pinned on the message, not just the exit: a user who typed the
    /// wrong name needs to be told which reading to fix.
    #[test]
    #[serial]
    fn a_source_equal_to_the_target_is_refused() {
        let _cwd = CwdGuard::new();
        let _iso = isolate();
        let layout = Layout::new(&["develop"]).with_origin_head();

        let err = err_of(
            run(
                &layout,
                &layout.wt("develop"),
                params(None, Some("develop")),
            ),
            "copying a worktree onto itself must not be attempted",
        );
        let msg = format!("{err:#}");
        assert!(
            msg.contains("same worktree"),
            "the refusal must name the condition: {msg}"
        );
        assert!(msg.contains("--from"), "and offer the way out of it: {msg}");
        assert!(msg.contains("develop"), "and name the worktree: {msg}");
    }

    /// The same refusal reached without `--from`: the repo's only worktree is
    /// the default-branch one, so the implicit source is the target.
    #[test]
    #[serial]
    fn warming_the_default_branch_worktree_from_itself_is_refused() {
        let _cwd = CwdGuard::new();
        let _iso = isolate();
        let layout = Layout::new(&[]).with_origin_head();

        let err = err_of(
            run(&layout, &layout.wt("main"), params(None, None)),
            "the default branch cannot be its own source",
        );
        assert!(format!("{err:#}").contains("same worktree"));
    }

    #[test]
    #[serial]
    fn an_unknown_target_names_the_word_that_failed() {
        let _cwd = CwdGuard::new();
        let _iso = isolate();
        let layout = Layout::new(&["develop"]).with_origin_head();

        let err = err_of(
            run(
                &layout,
                &layout.wt("main"),
                params(Some("no-such-worktree"), None),
            ),
            "an unresolvable target is a hard error, never a guess",
        );
        assert!(format!("{err:#}").contains("no-such-worktree"));
    }

    #[test]
    #[serial]
    fn an_unknown_from_names_the_word_that_failed() {
        let _cwd = CwdGuard::new();
        let _iso = isolate();
        let layout = Layout::new(&["develop"]).with_origin_head();

        let err = err_of(
            run(
                &layout,
                &layout.wt("main"),
                params(Some("develop"), Some("no-such-source")),
            ),
            "an unresolvable --from is a hard error",
        );
        assert!(format!("{err:#}").contains("no-such-source"));
    }

    /// A repo with no `origin/HEAD` and no catalog entry cannot name a default
    /// branch. The user is told to pick a source rather than left with a
    /// silently wrong one.
    #[test]
    #[serial]
    fn an_undeterminable_default_branch_asks_for_from() {
        let _cwd = CwdGuard::new();
        let _iso = isolate();
        // No `with_origin_head()`: nothing on disk says which branch is
        // default, and the isolated state dir has no catalog row either.
        let layout = Layout::new(&["develop"]);

        let err = err_of(
            run(&layout, &layout.wt("develop"), params(None, None)),
            "with no default branch there is no implicit source",
        );
        let msg = format!("{err:#}");
        assert!(msg.contains("default branch"), "{msg}");
        assert!(
            msg.contains("--from"),
            "the hint must offer a way out: {msg}"
        );
    }

    /// `origin/HEAD` names a branch nobody has checked out: the answer is
    /// "there is no source", not a silent fallback to some other worktree.
    #[test]
    #[serial]
    fn a_default_branch_without_a_worktree_asks_for_from() {
        let _cwd = CwdGuard::new();
        let _iso = isolate();
        let layout = Layout::new(&["develop"]);
        let remotes = layout.root.join(".git/refs/remotes/origin");
        std::fs::create_dir_all(&remotes).unwrap();
        std::fs::write(remotes.join("HEAD"), "ref: refs/remotes/origin/trunk\n").unwrap();

        let err = err_of(
            run(&layout, &layout.wt("develop"), params(None, None)),
            "a default branch with no worktree is not a source",
        );
        let msg = format!("{err:#}");
        assert!(msg.contains("trunk"), "name the branch that failed: {msg}");
        assert!(msg.contains("--from"), "{msg}");
    }

    /// The catalog is asked first, and its answer wins over `origin/HEAD`.
    /// That ordering is the point of consulting it at all: a repo whose
    /// default branch was renamed keeps a correct catalog row long after the
    /// stale `origin/HEAD` symref still points at the old name.
    #[test]
    #[serial]
    fn the_catalog_default_branch_beats_origin_head() {
        let _cwd = CwdGuard::new();
        let _iso = isolate();
        // origin/HEAD says `main`; the catalog says `feature/login`.
        let layout = Layout::new(&["develop"])
            .with_worktree("wt-feature", "feature/login")
            .with_origin_head();

        let bare = layout.root.join(".git");
        let uuid = uuid::Uuid::new_v4().to_string();
        std::fs::write(bare.join("daft-id"), format!("{uuid}\n")).unwrap();
        let canonical_bare = bare.canonicalize().unwrap();
        crate::catalog::Catalog::open_rw()
            .expect("the sandboxed data dir must accept a catalog")
            .register(&crate::catalog::RegistrationFacts {
                uuid,
                default_name: "acme".to_string(),
                path: layout.root.display().to_string(),
                git_common_dir: canonical_bare.display().to_string(),
                remote_url: None,
                default_branch: Some("feature/login".to_string()),
            })
            .expect("seeding the catalog row must succeed");

        let result = run(&layout, &layout.wt("develop"), params(None, None))
            .expect("the catalog's default branch has a worktree to copy from");

        assert_eq!(
            result.source,
            layout.wt("wt-feature"),
            "the catalog row must beat the origin/HEAD symref"
        );
    }

    /// A worktree git still lists but whose directory was deleted out from
    /// under it: resolution succeeds and the directory check is what catches
    /// it, so the message is about the missing directory, not an unknown name.
    #[test]
    #[serial]
    fn a_resolvable_target_whose_directory_is_gone_is_reported_as_missing() {
        let _cwd = CwdGuard::new();
        let _iso = isolate();
        let layout = Layout::new(&["develop"]).with_origin_head();
        std::fs::remove_dir_all(layout.wt("develop")).unwrap();

        let err = err_of(
            run(&layout, &layout.wt("main"), params(Some("develop"), None)),
            "a vanished target directory cannot be warmed",
        );
        let msg = format!("{err:#}");
        assert!(msg.contains("does not exist"), "{msg}");
    }

    /// Run from the container root — the directory that holds the bare `.git`
    /// and every worktree, and a place users genuinely stand in. There is no
    /// current worktree there, so the implicit target is unanswerable.
    #[test]
    #[serial]
    fn running_from_the_container_root_reports_no_current_worktree() {
        let _cwd = CwdGuard::new();
        let _iso = isolate();
        let layout = Layout::new(&["develop"]).with_origin_head();

        let err = err_of(
            run(&layout, &layout.root, params(None, None)),
            "the container root is not a worktree",
        );
        assert!(format!("{err:#}").contains("current worktree"), "{err:#}");
    }

    /// The same standing point, but with both ends named explicitly: nothing
    /// about the request depends on where the user is standing.
    ///
    /// This documents current behavior — the current worktree is resolved
    /// eagerly, so a fully-specified invocation still fails outside a worktree.
    #[test]
    #[serial]
    fn a_fully_specified_run_from_the_container_root_still_needs_a_current_worktree() {
        let _cwd = CwdGuard::new();
        let _iso = isolate();
        let layout = Layout::new(&["develop"]).with_origin_head();

        let outcome = run(&layout, &layout.root, params(Some("develop"), Some("main")));
        assert!(
            outcome.is_err(),
            "current behavior: the cwd probe runs before the named pair is used"
        );
    }

    /// Outside any repository at all — the `is_git_repository` guard in the
    /// command layer covers this for real invocations, but the core must not
    /// panic or resolve something nonsensical if it is ever called directly.
    #[test]
    #[serial]
    fn running_outside_any_repository_is_an_error() {
        let _cwd = CwdGuard::new();
        let _iso = isolate();
        let layout = Layout::new(&["develop"]).with_origin_head();
        let outside = layout.base().join("outside");
        std::fs::create_dir_all(&outside).unwrap();

        let err = err_of(
            run(&layout, &outside, params(Some("develop"), None)),
            "no repository, no worktrees to resolve",
        );
        assert!(!format!("{err:#}").is_empty());
    }

    /// `--force` is a licence to replace *declared* entries, and nothing more.
    /// This module hands the flag to the engine and performs no removal of its
    /// own, so a source that declares nothing must leave the target's
    /// directories exactly where they were — even the ones a `copy:` section
    /// would have named.
    #[test]
    #[serial]
    fn force_with_nothing_declared_removes_nothing() {
        let _cwd = CwdGuard::new();
        let _iso = isolate();
        let layout = Layout::new(&["develop"]).with_origin_head();

        let victim = layout.wt("develop").join("node_modules");
        std::fs::create_dir_all(victim.join("pkg")).unwrap();
        std::fs::write(victim.join("pkg/index.js"), "built-in-develop\n").unwrap();

        let result = run(
            &layout,
            &layout.wt("main"),
            WarmParams {
                target: Some("develop".to_string()),
                from: None,
                force: true,
            },
        )
        .expect("a --force run with no declarations is a no-op, not an error");

        assert!(result.nothing_declared());
        assert_eq!(
            std::fs::read_to_string(victim.join("pkg/index.js")).unwrap(),
            "built-in-develop\n",
            "--force must not delete anything the config never named"
        );
    }

    /// A source that declares nothing is the "no work to do" case, and it must
    /// be distinguishable from "declared and everything was skipped" — the
    /// command says different things about them.
    #[test]
    #[serial]
    fn a_source_with_no_copy_section_declares_nothing() {
        let _cwd = CwdGuard::new();
        let _iso = isolate();
        let layout = Layout::new(&["develop"]).with_origin_head();

        let result = run(&layout, &layout.wt("main"), params(Some("develop"), None)).unwrap();

        assert!(result.nothing_declared());
        assert!(result.outcome.is_empty());
        assert!(!result.has_existing_skips());
    }
}
