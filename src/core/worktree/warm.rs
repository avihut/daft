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
use crate::core::copy_paths;
use crate::core::copy_paths::{
    CopyOutcome, CopyPathsResult, SkipReason, copy_entries, read_copy_config,
};
use crate::core::copy_source::same_path;
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
    /// The remote whose `HEAD` names the default branch when the catalog has
    /// no record — `daft.remote`, not a hardcoded `origin`. A fork whose
    /// `daft.remote` is `upstream` would otherwise fall back to the wrong
    /// remote's idea of "default" and warm from a worktree the user never
    /// pointed at.
    pub remote: String,
}

/// Result of a warm operation.
#[derive(Debug)]
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
    /// Why the source's config could not be read, when it could not be.
    ///
    /// [`read_copy_config`] answers `None` for "no `copy:` key" and for "the
    /// config would not load" alike, and the two demand opposite advice:
    /// *declare some paths* versus *fix your YAML*. `daft warm` is the command
    /// a user reaches for to debug `copy:`, so it is the last place that may
    /// tell them their perfectly-present config is absent. Populated by a
    /// separate load attempt whose only job is to tell the two apart.
    pub config_error: Option<String>,
    /// True when the source has a `copy:` key that declares nothing usable —
    /// a misspelled `paths:` inside the map form, or entries that all
    /// normalized away.
    ///
    /// The third answer beside "no key at all" and "would not load", and it
    /// needs its own advice too: telling someone to *declare some paths* when
    /// the block is right in front of them sends them to write a key they have
    /// already written.
    pub declared_empty: bool,
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
    // Resolved on demand, never eagerly: `daft warm <target> --from <source>`
    // names both ends, and must work from the container root of a contained
    // layout — a directory that is inside the repository but is not a worktree
    // at all. Probing "where am I?" up front failed that run on a value it
    // then never read.
    let mut current: Option<PathBuf> = None;

    let target = match &params.target {
        Some(name) => resolve_named(git, name, project_root)?,
        None => current_worktree(&mut current)?,
    };

    let source = resolve_source(params, git, project_root, &mut current, &target)?;

    // Checked before the same-worktree refusal: when both ends land on the
    // container root, "these are the same worktree" is the less useful of the
    // two true statements.
    reject_bare(&target, Role::Target, project_root)?;
    reject_bare(&source, Role::Source, project_root)?;

    // Copying a worktree onto itself is never what was meant, and with
    // `--force` it would delete the very entries it was about to copy. The
    // hint offers both readings — wrong source, or forgotten target — because
    // which one the user meant is not knowable from here.
    //
    // Compared canonically: the two operands can arrive spelled differently
    // (git's porcelain listing vs. a resolved cwd), and a refusal that misses
    // because of a `/var` → `/private/var` symlink would, under `--force`,
    // delete a worktree's caches and re-copy them from themselves.
    if same_path(&source, &target) {
        anyhow::bail!(
            "source and target are the same worktree ('{}'); pass --from naming a different \
             worktree, or name the target to warm",
            display_name(&target, project_root)
        );
    }
    require_directory(&source, "source")?;
    require_directory(&target, "target")?;

    let source_name = display_name(&source, project_root);
    let target_name = display_name(&target, project_root);

    progress.on_step(&format!("Warming '{target_name}' from '{source_name}'"));

    let config_error = config_load_error(&source);
    let config = if config_error.is_none() {
        read_copy_config(&source)
    } else {
        None
    };

    // An empty declaration stops here rather than in `copy_entries`: the engine
    // warns about it for the creation journeys, which have no other voice, but
    // `warm` renders the same fact as a whole message of its own and saying it
    // twice on one command reads as two different problems.
    let declared_empty = config.as_ref().is_some_and(|c| c.paths.is_empty());
    let Some(config) = config.filter(|c| !c.paths.is_empty()) else {
        return Ok(WarmResult {
            source,
            target,
            source_name,
            target_name,
            declared: Vec::new(),
            outcome: CopyPathsResult::default(),
            config_error,
            declared_empty,
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
        config_error: None,
        declared_empty: false,
    })
}

/// Resolve a worktree the user named, in any of its three spellings.
fn resolve_named(git: &GitCommand, name: &str, project_root: &Path) -> Result<PathBuf> {
    git.resolve_worktree_path(name, project_root)
        .with_context(|| format!("Could not resolve worktree '{name}'"))
}

/// Which end of the copy a rejected path was standing in for.
#[derive(Debug, Clone, Copy)]
enum Role {
    Target,
    Source,
}

/// Refuse a path git reports as a **bare** checkout.
///
/// In a contained layout the container root is a genuine row in
/// `git worktree list --porcelain` (`worktree <root>` followed by `bare`), so
/// `resolve_worktree_path` hands it back for `.`, for its directory name, and
/// for its path — and `require_directory` is perfectly happy with it, because
/// it is a real directory. It just has no working tree.
///
/// Every other daft verb that can resolve it either reads or refuses. `warm` is
/// the first that **writes and deletes** at the resolved path: with the engine
/// in place, `--force` removes a declared entry before re-copying it. A
/// mistyped target would scatter `node_modules/` beside the bare repository and
/// then delete it again on the next run — so the guard sits in resolution,
/// before anything is read, and covers both ends.
fn reject_bare(path: &Path, role: Role, project_root: &Path) -> Result<()> {
    if !is_bare_checkout(path) {
        return Ok(());
    }
    let (end, fix) = match role {
        Role::Target => ("copy into", "name a worktree to warm"),
        Role::Source => ("copy from", "pass --from naming a worktree"),
    };
    anyhow::bail!(
        "'{}' is this repository's bare container, not a worktree — it has no working tree to \
         {end}; {fix}",
        display_name(path, project_root)
    )
}

/// Whether `path` is a bare checkout, asked of git through the same env-cleared
/// authority as every other operand (see [`current_worktree`]).
///
/// A probe rather than a `path == project_root` test: in a flat
/// (progressive-adoption) layout the project root *is* the main worktree, and
/// refusing it there would break the most ordinary run there is.
fn is_bare_checkout(path: &Path) -> bool {
    crate::utils::git_command_at(path)
        .args(["rev-parse", "--is-bare-repository"])
        .stderr(std::process::Stdio::null())
        .output()
        .is_ok_and(|out| {
            out.status.success() && String::from_utf8_lossy(&out.stdout).trim() == "true"
        })
}

/// Assert `path` is a directory, naming *why* it is not when the answer is
/// something other than absence.
///
/// `Path::is_dir` folds EACCES, ENOTDIR and ELOOP into `false`, so an
/// unreadable-but-present worktree used to be reported as "does not exist" —
/// advice that sends the user looking for the wrong problem.
fn require_directory(path: &Path, role: &str) -> Result<()> {
    match std::fs::metadata(path) {
        Ok(meta) if meta.is_dir() => Ok(()),
        Ok(_) => anyhow::bail!("{role} worktree is not a directory: {}", path.display()),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
            anyhow::bail!("{role} worktree does not exist: {}", path.display())
        }
        Err(e) => anyhow::bail!("{role} worktree is unreadable ({e}): {}", path.display()),
    }
}

/// The worktree the command was run from, memoized in `cache`.
///
/// Deliberately a fresh env-cleared `git` invocation rather than
/// `GitCommand::get_current_worktree_path`: that helper shells out to a bare
/// `git rev-parse`, which honors an inherited `GIT_DIR`, while
/// `resolve_worktree_path` enumerates through [`crate::utils::git_command_at`],
/// which clears it. Inside a git-driven hook (pre-push, post-checkout) the two
/// therefore describe *different repositories*, and the self-copy refusal below
/// would compare a path from one against a path from the other — a comparison
/// that cannot fail, letting `warm` copy across repos. Every operand here comes
/// from the same, env-cleared authority.
fn current_worktree(cache: &mut Option<PathBuf>) -> Result<PathBuf> {
    if let Some(path) = cache {
        return Ok(path.clone());
    }
    let cwd = std::env::current_dir().context("Could not determine the current directory")?;
    let path = toplevel_at(&cwd).context("Could not determine the current worktree")?;
    cache.replace(path.clone());
    Ok(path)
}

/// `git -C <dir> rev-parse --show-toplevel`, env-cleared and canonicalized.
fn toplevel_at(dir: &Path) -> Result<PathBuf> {
    let out = crate::utils::git_command_at(dir)
        .args(["rev-parse", "--show-toplevel"])
        .stderr(std::process::Stdio::null())
        .output()
        .context("Failed to execute git rev-parse")?;
    if !out.status.success() {
        anyhow::bail!("not inside a worktree: {}", dir.display());
    }
    let top = String::from_utf8_lossy(&out.stdout).trim().to_string();
    if top.is_empty() {
        anyhow::bail!("not inside a worktree: {}", dir.display());
    }
    let path = PathBuf::from(top);
    Ok(path.canonicalize().unwrap_or(path))
}

/// The project root — the parent of the git common dir — resolved through the
/// same env-cleared authority as every other operand (see
/// [`current_worktree`]). Used by the command layer in place of
/// `core::repo::get_project_root`, whose bare `git rev-parse` an inherited
/// `GIT_DIR` retargets.
pub fn locate_project_root() -> Result<PathBuf> {
    let cwd = std::env::current_dir().context("Could not determine the current directory")?;
    let out = crate::utils::git_command_at(&cwd)
        .args(["rev-parse", "--git-common-dir"])
        .stderr(std::process::Stdio::null())
        .output()
        .context("Failed to execute git rev-parse")?;
    if !out.status.success() {
        anyhow::bail!("not inside a Git repository: {}", cwd.display());
    }
    let raw = String::from_utf8_lossy(&out.stdout).trim().to_string();
    // `--git-common-dir` answers relatively (`.git`) from a worktree root and
    // absolutely from elsewhere; join before canonicalizing so both land in the
    // same place.
    let common = cwd.join(raw);
    let common = common.canonicalize().unwrap_or(common);
    common
        .parent()
        .map(Path::to_path_buf)
        .context("Could not determine the project root")
}

/// Resolve the worktree to copy *from*.
///
/// `--from` wins outright — an explicit source is never second-guessed.
///
/// Otherwise the same specificity ladder the creation journeys use
/// ([`crate::core::copy_source`]), anchored on the commit the **target**
/// already sits at: a worktree holding that exact commit holds exactly the
/// tree being warmed, which beats both "wherever you happen to be standing"
/// and "the default branch". The branch rung is vacuous here — the target's
/// own worktree is the target, and the destination is never its own source.
///
/// Below the ladder, the previous behaviour is the floor: the current worktree
/// whenever it is not itself the target — warming a sibling from where you
/// stand is the common shape — and, when the target *is* the current worktree,
/// the default branch's, the one worktree every repo has and the one whose
/// caches are most likely to be both present and generic.
fn resolve_source(
    params: &WarmParams,
    git: &GitCommand,
    project_root: &Path,
    current: &mut Option<PathBuf>,
    target: &Path,
) -> Result<PathBuf> {
    if let Some(name) = &params.from {
        return resolve_named(git, name, project_root);
    }

    // Best-effort: from a container root there is no "here" at all, and the
    // ladder must still be able to run.
    let here = current_worktree(current).ok();

    if let Some(commit) = crate::core::copy_source::resolve_oid(target, "HEAD") {
        // Warmth is scored over the *target's* declared paths. The source's
        // own `copy:` section is what will actually be copied, but it cannot
        // be read before the source is chosen — and the target's declaration
        // is the best available statement of which paths matter. When there is
        // none, warmth is uniformly zero and the tie falls to path order,
        // which is deterministic and claims no tiebreak that did not happen.
        let declared = crate::core::copy_paths::read_copy_config(target)
            .map(|config| config.paths)
            .unwrap_or_default();
        let resolved = crate::core::copy_source::resolve(
            git,
            &crate::core::copy_source::CopyAnchor {
                branch: None,
                commit: Some(commit),
            },
            here.as_deref().unwrap_or(target),
            target,
            &declared,
        );
        if resolved.reason != crate::core::copy_source::CopySourceReason::Standing {
            return Ok(resolved.path);
        }
    }

    // No second probe. `here` was made best-effort a few lines up precisely so
    // the ladder could run from a container root, and re-asking the question
    // that already failed there — with a `?` this time — put the hard error
    // back and made the default-branch rung below unreachable from exactly the
    // place it was written for. `daft warm develop` typed beside the bare
    // `.git` aborted with "Could not determine the current worktree", naming a
    // problem the user did not have: they had named the target outright.
    if let Some(here) = here.filter(|here| here != target) {
        return Ok(here);
    }

    let branch = default_branch(project_root, &params.remote)?.ok_or_else(|| {
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

/// This repository's default branch: the catalog's recorded branch first (it
/// survives a missing `<remote>/HEAD`), then the local symref for `remote`.
///
/// The catalog rung reads `default_branch` **directly** rather than through
/// `catalog::effective_default_branch`, whose own fallback hardcodes `origin`.
/// Routing through it would honour `remote` on the git rung and ignore it on
/// the catalog one, so a fork configured with `daft.remote = upstream` and a
/// catalog row predating default-branch capture would silently warm from
/// `origin/HEAD`'s branch — the one thing the parameter exists to prevent.
///
/// **The catalog probe fails closed.** `Ok(None)` — no catalog file, no row, a
/// tombstoned row — is genuine absence and falls through to git, exactly as for
/// a repo daft never cataloged. A store *outage* is not: a sibling worktree
/// running a newer daft build migrates the shared catalog, and this binary then
/// reads `SchemaTooNew`. Collapsing that to `None` would silently retarget the
/// source to whatever stale `<remote>/HEAD` happens to say, and the user would
/// never learn the catalog was unreadable — the repo-aware grammar's rule that
/// a probe gating an action must report the outage, never reinterpret it.
fn default_branch(project_root: &Path, remote: &str) -> Result<Option<String>> {
    let from_catalog = match crate::core::repo::git_common_dir_at(project_root) {
        Some(dir) => crate::catalog::try_live_catalog_row_for(&dir)
            .with_context(|| {
                format!(
                    "could not read the repo catalog to find {}'s default branch",
                    project_root.display()
                )
            })?
            .and_then(|row| row.default_branch),
        None => None,
    };

    Ok(from_catalog.or_else(|| crate::core::remote::local_default_branch(project_root, remote)))
}

/// A worktree's name as the user thinks of it: its path relative to the project
/// root.
///
/// Two fallbacks, for two different situations:
///
/// - **The path *is* the project root** — a flat (progressive-adoption) layout,
///   where the root and the main worktree are the same directory. The relative
///   suffix is empty, so the directory's own name is the answer; rendering the
///   absolute path would make the most ordinary worktree in that layout the
///   ugliest line in the output.
/// - **The project root does not contain the path at all** — the full path.
///   A bare final component would read exactly like a sibling worktree's name,
///   so a resolution that escaped the repository, the one case worth noticing,
///   would be indistinguishable from an ordinary local one.
fn display_name(path: &Path, project_root: &Path) -> String {
    let full = || path.display().to_string();
    let Ok(relative) = path.strip_prefix(project_root) else {
        return full();
    };
    match relative.to_str() {
        Some("") => path
            .file_name()
            .map(|n| n.to_string_lossy().to_string())
            .unwrap_or_else(full),
        Some(rel) => rel.to_string(),
        // Non-UTF-8 suffix: the lossy full path at least round-trips to
        // something a user can paste back.
        None => full(),
    }
}

/// Why the source's config would not load, if it would not.
///
/// [`read_copy_config`]'s `None` is deliberately silent about the difference
/// between "no `copy:` key" and "this YAML is broken"; the loader is the only
/// thing that knows, so ask it directly. Absence stays `None` here — a repo
/// with no config file at all is not an error.
fn config_load_error(source: &Path) -> Option<String> {
    match crate::hooks::yaml_config_loader::load_merged_config(source) {
        Ok(_) => None,
        Err(e) => Some(format!("{e:#}")),
    }
}

/// One entry's flat line, for `daft warm`'s per-entry output.
///
/// Composed from the engine's two halves rather than phrased here:
/// [`copy_paths::skip_phrase`] is the bare clause the rail prints beside a row
/// whose label already names the entry, and [`copy_paths::qualified_phrase`]
/// puts the entry back for a surface with only one column. `warm` is that
/// surface, so it is the composition — never a second vocabulary — that lives
/// in this module.
///
/// This replaces an interim leading-quote heuristic that guessed whether a
/// phrase had already named its entry. The engine now settles it: no clause
/// ever names the entry, and the offending path inside a glob expansion is the
/// only path a phrase quotes.
pub fn skip_line(entry: &str, reason: &SkipReason, unreadable: usize) -> String {
    copy_paths::qualified_phrase(
        entry,
        &copy_paths::with_shortfall(copy_paths::skip_phrase(entry, reason), unreadable),
    )
}

/// One failed entry's flat line, on the same rule as [`skip_line`].
pub fn failure_line(entry: &str, detail: &str) -> String {
    copy_paths::qualified_phrase(entry, &copy_paths::failure_phrase(detail))
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

    #[test]
    fn display_name_is_relative_to_the_project_root() {
        let root = Path::new("/repos/acme");
        assert_eq!(display_name(Path::new("/repos/acme/main"), root), "main");
        assert_eq!(
            display_name(Path::new("/repos/acme/feature/login"), root),
            "feature/login"
        );
        // Outside the project root, the FULL path — the bare final component
        // (`wt`) would read exactly like a sibling worktree's name, so the one
        // resolution worth noticing in the announce line would be the one
        // rendered indistinguishably from an ordinary local one.
        assert_eq!(
            display_name(Path::new("/elsewhere/wt"), root),
            "/elsewhere/wt"
        );
        // A flat (progressive-adoption) layout: the project root IS the main
        // worktree, so the relative suffix is empty. Its own directory name is
        // what the user calls it — the absolute path would make the most
        // ordinary worktree in that layout the ugliest line in the output.
        assert_eq!(display_name(root, root), "acme");
    }

    /// `Path::is_dir` folds every failure into `false`, so a worktree that is
    /// present but unreadable used to be reported as missing — advice that
    /// sends the user hunting for a directory that is right there. Each
    /// non-directory answer names itself instead.
    #[test]
    fn a_non_directory_is_reported_by_what_is_actually_wrong() {
        let tmp = tempfile::tempdir().unwrap();

        let missing = tmp.path().join("nope");
        let err = require_directory(&missing, "target").unwrap_err();
        assert!(format!("{err:#}").contains("does not exist"), "{err:#}");

        let file = tmp.path().join("a-file");
        std::fs::write(&file, "not a worktree").unwrap();
        let err = require_directory(&file, "source").unwrap_err();
        let msg = format!("{err:#}");
        assert!(msg.contains("not a directory"), "{msg}");
        assert!(
            !msg.contains("does not exist"),
            "a present file is not an absent directory: {msg}"
        );
        assert!(msg.contains("source"), "the role must be named: {msg}");

        assert!(require_directory(tmp.path(), "target").is_ok());
    }

    /// Every skip reason the engine can produce, rendered as `warm` renders
    /// it. The *wording* is the engine's and is pinned by its own tests; what
    /// belongs here is the composition — that a flat line is always
    /// `entry: <clause>`, for every variant, with the entry appearing once.
    ///
    /// Exhaustive by construction: a new `SkipReason` fails to compile here
    /// until it is added, which is what stops a fourteenth variant reaching
    /// `daft warm` unrendered.
    #[test]
    fn every_skip_reason_renders_as_the_entry_then_its_clause() {
        let named = |s: &str| s.to_string();
        let reasons = [
            SkipReason::NoSource,
            SkipReason::SourceUnreadable {
                path: named("**/dist/a"),
                detail: named("permission denied"),
            },
            SkipReason::DestinationExists,
            SkipReason::DestinationUnreadable {
                path: named("**/dist"),
                detail: named("permission denied"),
            },
            SkipReason::DestinationUnclassifiable {
                offender: named("**/dist"),
                detail: named("not a repository"),
            },
            SkipReason::DestinationConflict {
                path: named("**/dist"),
                detail: named("a symlink"),
            },
            SkipReason::NotIgnored {
                offender: named("**/dist"),
            },
            SkipReason::TargetTracked {
                offender: named("**/dist"),
            },
            SkipReason::Unclassifiable {
                offender: named("**/dist"),
                detail: named("probe failed"),
            },
            SkipReason::Uncontained {
                offender: named("../../etc"),
                detail: named("escapes the worktree"),
            },
            SkipReason::SameWorktree,
            SkipReason::NoReflink,
            SkipReason::TooLarge {
                size_bytes: 1536,
                limit_bytes: 1024,
            },
            SkipReason::NoMatches,
        ];

        for reason in reasons {
            let line = skip_line("**/dist", &reason, 0);
            let clause = copy_paths::skip_phrase("**/dist", &reason);

            assert_eq!(
                line,
                format!("**/dist: {clause}"),
                "the flat line is the engine's clause with the entry restored: {reason:?}"
            );
            // The stutter condition, precisely: the clause quoting the ENTRY.
            // A clause may quote a distinct offending path — and that path can
            // legitimately have the entry as a prefix (`**/dist/a` under
            // `**/dist`), which a naive `contains` would flag.
            assert!(
                !clause.contains("'**/dist'"),
                "the clause must not quote the entry — warm's prefix already names it, and \
                 the pair would stutter: {reason:?} -> {clause}"
            );
            assert!(
                !clause.starts_with("skipped"),
                "CopyPath is exempt from the `skipped — ` prefix: {clause}"
            );
        }
    }

    /// The interim rule this replaced: `warm` used to sniff a leading quote to
    /// guess whether a clause had already named its entry, and prefixed only
    /// the ones that had not. The engine settled it — no clause ever names the
    /// entry, and a quoted path inside one is a *glob match*, not the entry —
    /// so the prefix is now unconditional and the offender still shows.
    #[test]
    fn a_quoted_offender_inside_a_glob_is_still_prefixed_with_the_entry() {
        let line = skip_line(
            "**/dist",
            &SkipReason::NotIgnored {
                offender: "web/dist".to_string(),
            },
            0,
        );
        assert!(
            line.starts_with("**/dist: "),
            "the declaration is what the user wrote and must lead: {line}"
        );
        assert!(
            line.contains("'web/dist'"),
            "the offending match is the only reason one row can explain a \
             thirty-way expansion: {line}"
        );
    }

    /// Failures compose the same way, through the engine's own clause.
    #[test]
    fn a_failure_renders_as_the_entry_then_its_clause() {
        assert_eq!(
            failure_line("node_modules", "permission denied"),
            format!(
                "node_modules: {}",
                copy_paths::failure_phrase("permission denied")
            )
        );
    }

    #[test]
    fn unreported_finds_declared_entries_with_no_outcome() {
        let declared = vec!["target".to_string(), "node_modules".to_string()];
        let result = CopyPathsResult {
            outcomes: vec![CopyOutcome::Skipped {
                entry: "target".into(),
                reason: SkipReason::NoSource,
                unreadable: 0,
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
                    unreadable: 0,
                },
                // Never declared — a glob's expansion leaking out as its own
                // outcome is the shape this guards against.
                CopyOutcome::Skipped {
                    entry: "web/dist".into(),
                    reason: SkipReason::NoSource,
                    unreadable: 0,
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
                unreadable: 0,
            }],
        };
        assert_eq!(unreported(&declared, &result), ["a", "b", "d"]);
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
            config_error: None,
            declared_empty: false,
        };

        let existing = base(vec![CopyOutcome::Skipped {
            entry: "target".into(),
            reason: SkipReason::DestinationExists,
            unreadable: 0,
        }]);
        assert!(existing.has_existing_skips());
        assert!(!existing.nothing_declared());

        let missing = base(vec![CopyOutcome::Skipped {
            entry: "target".into(),
            reason: SkipReason::NoSource,
            unreadable: 0,
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

        /// A **flat** (progressive-adoption) layout: an ordinary non-bare repo
        /// whose root *is* the main worktree, with linked worktrees nested
        /// under it. `project_root` and a real worktree are the same directory
        /// here, which is what separates the bare-checkout guard from a
        /// `path == project_root` test.
        fn flat(worktrees: &[&str]) -> Self {
            let tmp = tempfile::tempdir().unwrap();
            let base = tmp.path().canonicalize().unwrap();
            let root = base.join("flat");
            std::fs::create_dir_all(&root).unwrap();
            git_at(&root, &["init", "-q", "-b", "main"]);
            git_at(&root, &["commit", "--allow-empty", "-q", "-m", "init"]);
            for wt in worktrees {
                git_at(&root, &["worktree", "add", "-q", wt, "-b", wt]);
            }
            Self { tmp, root }
        }

        /// The project root's own directory name — how a user names it.
        fn root_name(&self) -> String {
            self.root.file_name().unwrap().to_string_lossy().to_string()
        }

        /// Plant `origin/HEAD` so `local_default_branch` resolves without a
        /// catalog entry — the catalog-free fallback path.
        fn with_origin_head(self) -> Self {
            self.with_remote_head("origin", "main")
        }

        /// Plant `<remote>/HEAD`. The remote is a parameter because the
        /// fallback follows `daft.remote`, and a fork configured against
        /// `upstream` has no `origin` refs at all.
        fn with_remote_head(self, remote: &str, branch: &str) -> Self {
            let remotes = self.root.join(".git/refs/remotes").join(remote);
            std::fs::create_dir_all(&remotes).unwrap();
            std::fs::write(
                remotes.join("HEAD"),
                format!("ref: refs/remotes/{remote}/{branch}\n"),
            )
            .unwrap();
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

        /// Move one worktree off the commit every other worktree shares.
        ///
        /// The fixture cuts every branch from a single commit, so without this
        /// the identical-commit rung of the source ladder always answers first
        /// and the default-branch fallback beneath it is unreachable. Real
        /// branches diverge; a test about the *fallback* needs a fixture that
        /// does too.
        fn diverge(&self, name: &str) {
            git_at(
                &self.wt(name),
                &["commit", "--allow-empty", "-q", "-m", "diverge"],
            );
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

    /// Run one test body with a fresh cwd guard and fresh isolation.
    ///
    /// Callers must be `#[serial]` — the cwd and the daft dirs are
    /// process-global.
    fn with_isolation(body: impl FnOnce()) {
        let _cwd = CwdGuard::new();
        let _iso = isolate();
        body();
    }

    /// Run `execute` from `cwd`. The daft state and data dirs must already be
    /// isolated by the caller — the default-branch probe reads the repo
    /// catalog.
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
            remote: "origin".to_string(),
        }
    }

    /// Unwraps the error half with a message that names what the run resolved
    /// instead — `expect_err`'s `Debug` dump of the whole `WarmResult` buries
    /// the two paths that actually explain the failure.
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
        with_isolation(|| {
            let layout = Layout::new(&["develop"]).with_origin_head();

            let result = run(&layout, &layout.wt("main"), params(Some("develop"), None))
                .expect("naming a sibling target must resolve");

            assert_eq!(result.target, layout.wt("develop"));
            assert_eq!(result.source, layout.wt("main"));
            assert_eq!(result.target_name, "develop");
            assert_eq!(result.source_name, "main");
        });
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
        with_isolation(|| {
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
        });
    }

    /// `--from` goes through the same resolver as the positional, so it has to
    /// answer to the same spellings. A `--from` that only understood directory
    /// names would silently be a different vocabulary from the positional's.
    #[test]
    #[serial]
    fn from_resolves_by_the_same_spellings_as_the_target() {
        with_isolation(|| {
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
        });
    }

    /// Standing in the target is the "warm me from the default branch" shape:
    /// there is no other worktree the command could mean.
    #[test]
    #[serial]
    fn standing_in_the_target_falls_back_to_the_default_branch_worktree() {
        with_isolation(|| {
            let layout = Layout::new(&["develop"]).with_origin_head();

            let result = run(&layout, &layout.wt("develop"), params(None, None))
                .expect("the default branch's worktree is the implicit source");

            assert_eq!(result.target, layout.wt("develop"));
            assert_eq!(result.source, layout.wt("main"));
        });
    }

    /// Same fallback, reached the long way: the target is named rather than
    /// implied, but it is still the worktree we are standing in.
    #[test]
    #[serial]
    fn naming_your_own_worktree_as_the_target_still_falls_back() {
        with_isolation(|| {
            let layout = Layout::new(&["develop"]).with_origin_head();

            let result = run(
                &layout,
                &layout.wt("develop"),
                params(Some("develop"), None),
            )
            .expect("naming the current worktree is the same case as omitting it");

            assert_eq!(result.source, layout.wt("main"));
        });
    }

    /// `--from` beats the current worktree even when the current worktree
    /// would have been a perfectly good source.
    #[test]
    #[serial]
    fn from_wins_over_the_current_worktree() {
        with_isolation(|| {
            let layout = Layout::new(&["develop", "release"]).with_origin_head();

            let result = run(
                &layout,
                &layout.wt("main"),
                params(Some("develop"), Some("release")),
            )
            .expect("--from names the source outright");

            assert_eq!(result.source, layout.wt("release"));
            assert_eq!(result.target, layout.wt("develop"));
        });
    }

    /// The refusal that keeps `--force` from deleting the entries it was about
    /// to copy. Pinned on the message, not just the exit: a user who typed the
    /// wrong name needs to be told which reading to fix.
    #[test]
    #[serial]
    fn a_source_equal_to_the_target_is_refused() {
        with_isolation(|| {
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
        });
    }

    /// The same refusal reached without `--from`: the repo's only worktree is
    /// the default-branch one, so the implicit source is the target.
    #[test]
    #[serial]
    fn warming_the_default_branch_worktree_from_itself_is_refused() {
        with_isolation(|| {
            let layout = Layout::new(&[]).with_origin_head();

            let err = err_of(
                run(&layout, &layout.wt("main"), params(None, None)),
                "the default branch cannot be its own source",
            );
            assert!(format!("{err:#}").contains("same worktree"));
        });
    }

    #[test]
    #[serial]
    fn an_unknown_target_names_the_word_that_failed() {
        with_isolation(|| {
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
        });
    }

    #[test]
    #[serial]
    fn an_unknown_from_names_the_word_that_failed() {
        with_isolation(|| {
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
        });
    }

    /// A worktree holding the target's exact commit holds exactly the tree
    /// being warmed, so it outranks the default branch — which is a guess
    /// about which caches are *generic*, not a claim that they match.
    #[test]
    #[serial]
    fn an_identical_tip_outranks_the_default_branch_fallback() {
        with_isolation(|| {
            let layout = Layout::new(&["develop", "twin"]).with_origin_head();
            // `main` is the default branch and moves ahead; `twin` stays where
            // `develop` is, so the two rungs disagree and the test can tell
            // which one answered.
            layout.diverge("main");

            let result = run(&layout, &layout.wt("develop"), params(None, None))
                .expect("warming the current worktree resolves a source");
            assert_eq!(
                result.source,
                layout.wt("twin"),
                "the sibling at the identical commit wins over the default branch"
            );

            // An explicit source is never second-guessed by the ladder.
            let forced = run(&layout, &layout.wt("develop"), params(None, Some("main")))
                .expect("--from resolves");
            assert_eq!(forced.source, layout.wt("main"));
        });
    }

    /// Among worktrees holding the identical commit, the one the command was
    /// run from wins — the tree you are in is the one you most recently built.
    #[test]
    #[serial]
    fn an_identical_tip_tie_prefers_the_worktree_you_are_standing_in() {
        with_isolation(|| {
            let layout = Layout::new(&["develop", "twin", "triplet"]).with_origin_head();
            layout.diverge("main");

            // `twin` and `triplet` both sit at `develop`'s commit; standing in
            // `triplet` must decide it, even though `twin` sorts first.
            let result = run(
                &layout,
                &layout.wt("triplet"),
                params(Some("develop"), None),
            )
            .expect("warming a sibling resolves a source");
            assert_eq!(result.source, layout.wt("triplet"));
        });
    }

    /// A repo with no `origin/HEAD` and no catalog entry cannot name a default
    /// branch. The user is told to pick a source rather than left with a
    /// silently wrong one.
    #[test]
    #[serial]
    fn an_undeterminable_default_branch_asks_for_from() {
        with_isolation(|| {
            // No `with_origin_head()`: nothing on disk says which branch is
            // default, and the isolated state dir has no catalog row either.
            let layout = Layout::new(&["develop"]);
            layout.diverge("develop");

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
        });
    }

    /// `origin/HEAD` names a branch nobody has checked out: the answer is
    /// "there is no source", not a silent fallback to some other worktree.
    #[test]
    #[serial]
    fn a_default_branch_without_a_worktree_asks_for_from() {
        with_isolation(|| {
            let layout = Layout::new(&["develop"]);
            layout.diverge("develop");
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
        });
    }

    /// The catalog is asked first, and its answer wins over `origin/HEAD`.
    /// That ordering is the point of consulting it at all: a repo whose
    /// default branch was renamed keeps a correct catalog row long after the
    /// stale `origin/HEAD` symref still points at the old name.
    #[test]
    #[serial]
    fn the_catalog_default_branch_beats_origin_head() {
        with_isolation(|| {
            // origin/HEAD says `main`; the catalog says `feature/login`.
            let layout = Layout::new(&["develop"])
                .with_worktree("wt-feature", "feature/login")
                .with_origin_head();
            layout.diverge("develop");

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
        });
    }

    /// The container root of a contained layout is a real row in
    /// `git worktree list --porcelain` — `worktree <root>` followed by `bare` —
    /// so every resolution tier finds it: its directory name, its path, and
    /// `.` from inside it. `require_directory` waves it through too, because it
    /// genuinely is a directory.
    ///
    /// `warm` is the first daft verb that *writes and deletes* at the resolved
    /// path: with the engine in place, `--force` removes each declared entry
    /// before re-copying it. Warming the container would scatter
    /// `node_modules/` beside the bare repository and delete it again on the
    /// next run, so it is refused during resolution — before anything is read.
    #[test]
    #[serial]
    fn the_bare_container_is_refused_as_a_target() {
        with_isolation(|| {
            let layout = Layout::new(&["develop"]).with_origin_head();
            let container = layout
                .root
                .file_name()
                .unwrap()
                .to_string_lossy()
                .to_string();

            for spelling in [container.as_str(), &layout.root.display().to_string()] {
                let err = err_of(
                    run(
                        &layout,
                        &layout.wt("main"),
                        params(Some(spelling), Some("main")),
                    ),
                    "the bare container is not a warmable target",
                );
                let msg = format!("{err:#}");
                assert!(
                    msg.contains("bare container"),
                    "spelled '{spelling}': {msg}"
                );
                assert!(msg.contains("copy into"), "{msg}");
            }
        });
    }

    /// The same refusal for the other end. A bare checkout has no working tree
    /// to read declared caches out of, so it can no more be a source than a
    /// target — and the message has to say which end went wrong.
    #[test]
    #[serial]
    fn the_bare_container_is_refused_as_a_source() {
        with_isolation(|| {
            let layout = Layout::new(&["develop"]).with_origin_head();
            let container = layout
                .root
                .file_name()
                .unwrap()
                .to_string_lossy()
                .to_string();

            let err = err_of(
                run(
                    &layout,
                    &layout.wt("main"),
                    params(Some("develop"), Some(&container)),
                ),
                "the bare container is not a warmable source",
            );
            let msg = format!("{err:#}");
            assert!(msg.contains("bare container"), "{msg}");
            assert!(msg.contains("copy from"), "{msg}");
            assert!(msg.contains("--from"), "name the flag to fix: {msg}");
        });
    }

    /// The guard is a bare-checkout probe, not a `path == project_root` test.
    /// In a flat (progressive-adoption) layout the project root *is* the main
    /// worktree, and refusing it there would break the most ordinary run in
    /// that layout.
    #[test]
    #[serial]
    fn a_flat_layout_root_is_a_worktree_and_is_not_refused() {
        with_isolation(|| {
            let flat = Layout::flat(&["feature"]);

            let result = run(
                &flat,
                &flat.wt("feature"),
                params(None, Some(&flat.root_name())),
            )
            .expect("the root of a flat layout is an ordinary worktree");

            assert_eq!(result.source, flat.root);
            assert_eq!(
                result.source_name,
                flat.root_name(),
                "a flat layout's root renders as its directory name, not an absolute path"
            );
            assert_eq!(result.target_name, "feature");
        });
    }

    /// A catalog that cannot be *asked* is not a catalog that says "no".
    ///
    /// The reachable shape: a sibling worktree running a newer daft build
    /// migrates the shared catalog, and this binary then finds a `user_version`
    /// it does not understand. Collapsing that to `None` would fall through to
    /// whatever `origin/HEAD` says and silently warm from a worktree the user
    /// never chose — an outage quietly reinterpreted as an answer, which the
    /// repo-aware command grammar forbids for a probe that gates an action.
    #[test]
    #[serial]
    fn an_unreadable_catalog_aborts_instead_of_falling_back() {
        with_isolation(|| {
            // origin/HEAD resolves perfectly well — the fallback the broken
            // catalog must NOT be allowed to silently reach.
            let layout = Layout::new(&["develop"]).with_origin_head();
            layout.diverge("main");

            let db = crate::store::paths::catalog_db().expect("sandboxed data dir");
            crate::catalog::Catalog::open_rw().expect("create the catalog file");
            rusqlite::Connection::open(&db)
                .unwrap()
                .pragma_update(None, "user_version", 9_999_i64)
                .unwrap();

            let err = err_of(
                run(&layout, &layout.wt("main"), params(Some("main"), None)),
                "an unreadable catalog must abort the run",
            );
            let msg = format!("{err:#}");
            assert!(
                msg.contains("catalog"),
                "the message must name the outage: {msg}"
            );
        });
    }

    /// A tombstoned row is *absence*, not an outage: `daft repo remove`
    /// leaves the worktrees exactly where they are, and warming them must keep
    /// working the way it does for a repo daft never cataloged.
    #[test]
    #[serial]
    fn a_removed_catalog_row_falls_back_to_the_remote_head() {
        with_isolation(|| {
            let layout = Layout::new(&["develop"]).with_origin_head();

            let bare = layout.root.join(".git");
            let uuid = uuid::Uuid::new_v4().to_string();
            std::fs::write(bare.join("daft-id"), format!("{uuid}\n")).unwrap();
            let catalog = crate::catalog::Catalog::open_rw().expect("sandboxed data dir");
            catalog
                .register(&crate::catalog::RegistrationFacts {
                    uuid: uuid.clone(),
                    default_name: "acme".to_string(),
                    path: layout.root.display().to_string(),
                    git_common_dir: bare.canonicalize().unwrap().display().to_string(),
                    remote_url: None,
                    default_branch: Some("develop".to_string()),
                })
                .expect("seeding the catalog row must succeed");
            catalog.mark_removed(&uuid).expect("tombstone the row");

            let result = run(&layout, &layout.wt("develop"), params(None, None))
                .expect("a tombstoned row must not break warming");

            assert_eq!(
                result.source,
                layout.wt("main"),
                "the tombstoned row's 'develop' must not win; origin/HEAD says main"
            );
        });
    }

    /// The default-branch fallback reads `<daft.remote>/HEAD`, not a hardcoded
    /// `origin/HEAD`. A fork configured against `upstream` has no `origin` at
    /// all, and hardcoding it left the source unresolvable in exactly the
    /// layout the setting exists for.
    #[test]
    #[serial]
    fn the_default_branch_fallback_follows_the_configured_remote() {
        with_isolation(|| {
            let layout = Layout::new(&["develop"]).with_remote_head("upstream", "main");
            layout.diverge("develop");

            let result = run(
                &layout,
                &layout.wt("develop"),
                WarmParams {
                    remote: "upstream".to_string(),
                    ..params(None, None)
                },
            )
            .expect("upstream/HEAD names the default branch");
            assert_eq!(result.source, layout.wt("main"));

            // The same layout under the default remote has nothing to read.
            let err = err_of(
                run(&layout, &layout.wt("develop"), params(None, None)),
                "there is no origin/HEAD in this layout",
            );
            assert!(format!("{err:#}").contains("--from"), "{err:#}");
        });
    }

    /// An inherited `GIT_DIR` — every command daft runs from inside a git hook
    /// has one — must not be able to make the two operands describe different
    /// repositories.
    ///
    /// `rev-parse` honors `GIT_DIR`; the worktree enumeration behind
    /// `resolve_worktree_path` clears it. While "where am I?" came from the
    /// former and "which worktrees exist?" from the latter, the self-copy
    /// refusal compared a path from one repo against paths from another — a
    /// comparison that can never fire, so `daft warm` would happily copy across
    /// repository boundaries.
    #[test]
    #[serial]
    fn an_inherited_git_dir_does_not_retarget_resolution() {
        with_isolation(|| {
            let layout = Layout::new(&["develop"]).with_origin_head();
            // A second, unrelated repo for GIT_DIR to point at.
            let other = Layout::new(&[]).with_origin_head();

            struct GitDirGuard;
            impl Drop for GitDirGuard {
                fn drop(&mut self) {
                    // SAFETY: `remove_var` is `unsafe fn` in edition 2024;
                    // every caller of this fixture is `#[serial]`.
                    unsafe { std::env::remove_var("GIT_DIR") };
                }
            }
            // SAFETY: as above — serialized by `#[serial]`.
            unsafe { std::env::set_var("GIT_DIR", other.root.join(".git")) };
            let _restore = GitDirGuard;

            // Standing in this repo's `main`, warming its `develop`.
            let result = run(&layout, &layout.wt("main"), params(Some("develop"), None))
                .expect("resolution must stay inside the repo the user is standing in");

            assert_eq!(result.target, layout.wt("develop"));
            assert_eq!(
                result.source,
                layout.wt("main"),
                "the source must come from this repo, not the one GIT_DIR names"
            );

            // And the self-copy refusal still fires, which it cannot do if the
            // two operands come from different repositories.
            let err = err_of(
                run(
                    &layout,
                    &layout.wt("main"),
                    params(Some("main"), Some("main")),
                ),
                "the same worktree named twice is still a refusal under GIT_DIR",
            );
            assert!(format!("{err:#}").contains("same worktree"), "{err:#}");
        });
    }

    /// `daft warm` is the command a user runs to work out why `copy:` is not
    /// copying, so it is the last place that may report a mistyped config as an
    /// absent one. `read_copy_config` cannot tell them apart — it answers
    /// `None` for both — so the load is attempted separately.
    #[test]
    #[serial]
    fn a_source_whose_config_will_not_load_reports_the_error() {
        with_isolation(|| {
            let layout = Layout::new(&["develop"]).with_origin_head();
            std::fs::write(
                layout.wt("main").join("daft.yml"),
                "copy:\n  paths: [target]\n  fallback: symlink\n",
            )
            .unwrap();

            let result = run(&layout, &layout.wt("main"), params(Some("develop"), None))
                .expect("a broken config is reported, not fatal");

            let detail = result
                .config_error
                .as_deref()
                .expect("a config that will not load must be reported as such");
            assert!(
                detail.to_lowercase().contains("copy") || detail.contains("daft.yml"),
                "the message must point at what failed: {detail}"
            );
            assert!(
                result.declared.is_empty() && result.outcome.is_empty(),
                "nothing may be attempted from a config that would not load"
            );
        });
    }

    /// The other half of the same distinction: a genuinely absent `copy:` key
    /// is not an error, and must not be reported as one.
    #[test]
    #[serial]
    fn a_source_with_a_valid_config_and_no_copy_key_reports_no_error() {
        with_isolation(|| {
            let layout = Layout::new(&["develop"]).with_origin_head();
            std::fs::write(layout.wt("main").join("daft.yml"), "shared:\n  - .env\n").unwrap();

            let result = run(&layout, &layout.wt("main"), params(Some("develop"), None))
                .expect("a config without copy: is ordinary");

            assert!(result.config_error.is_none(), "{:?}", result.config_error);
            assert!(result.nothing_declared());
        });
    }

    /// A worktree git still lists but whose directory was deleted out from
    /// under it: resolution succeeds and the directory check is what catches
    /// it, so the message is about the missing directory, not an unknown name.
    #[test]
    #[serial]
    fn a_resolvable_target_whose_directory_is_gone_is_reported_as_missing() {
        with_isolation(|| {
            let layout = Layout::new(&["develop"]).with_origin_head();
            std::fs::remove_dir_all(layout.wt("develop")).unwrap();

            let err = err_of(
                run(&layout, &layout.wt("main"), params(Some("develop"), None)),
                "a vanished target directory cannot be warmed",
            );
            let msg = format!("{err:#}");
            assert!(msg.contains("does not exist"), "{msg}");
        });
    }

    /// Run from the container root — the directory that holds the bare `.git`
    /// and every worktree, and a place users genuinely stand in. There is no
    /// current worktree there, so the implicit target is unanswerable.
    #[test]
    #[serial]
    fn running_from_the_container_root_reports_no_current_worktree() {
        with_isolation(|| {
            let layout = Layout::new(&["develop"]).with_origin_head();

            let err = err_of(
                run(&layout, &layout.root, params(None, None)),
                "the container root is not a worktree",
            );
            assert!(format!("{err:#}").contains("current worktree"), "{err:#}");
        });
    }

    /// The same standing point, but with both ends named explicitly: nothing
    /// about the request depends on where the user is standing, so nothing
    /// about where the user is standing may make it fail.
    ///
    /// The container root is a real place to stand — it holds the shared
    /// `.git` and every worktree — and `daft warm develop --from main` is
    /// completely specified there. An eager "where am I?" probe used to fail
    /// this run on a value it then never read.
    #[test]
    #[serial]
    fn a_fully_specified_run_from_the_container_root_needs_no_current_worktree() {
        with_isolation(|| {
            let layout = Layout::new(&["develop"]).with_origin_head();

            let result = run(&layout, &layout.root, params(Some("develop"), Some("main")))
                .expect("both ends are named; the cwd is irrelevant");

            assert_eq!(result.target, layout.wt("develop"));
            assert_eq!(result.source, layout.wt("main"));
        });
    }

    /// Only the TARGET named, from the container root. The ladder is
    /// best-effort about "here" precisely so this can work — and then asked the
    /// same failing question again, with a `?`, which put the hard error back
    /// and made the default-branch rung below it unreachable from the one place
    /// it was written for. The error even named the wrong problem: the user
    /// supplied the target.
    #[test]
    #[serial]
    fn a_target_only_run_from_the_container_root_falls_back_to_the_default_branch() {
        with_isolation(|| {
            let layout = Layout::new(&["develop"]).with_origin_head();
            // Diverge, so no other worktree sits at develop's commit and the
            // identical-commit rung genuinely misses — a fixture where two
            // rungs coincide tests neither.
            layout.diverge("develop");

            let result = run(&layout, &layout.root, params(Some("develop"), None))
                .expect("the target is named; there is nothing to ask the cwd");

            assert_eq!(result.target, layout.wt("develop"));
            assert_eq!(
                result.source,
                layout.wt("main"),
                "the default branch's worktree is the documented floor"
            );
        });
    }

    /// Outside any repository at all — the `is_git_repository` guard in the
    /// command layer covers this for real invocations, but the core must not
    /// panic or resolve something nonsensical if it is ever called directly.
    #[test]
    #[serial]
    fn running_outside_any_repository_is_an_error() {
        with_isolation(|| {
            let layout = Layout::new(&["develop"]).with_origin_head();
            let outside = layout.base().join("outside");
            std::fs::create_dir_all(&outside).unwrap();

            let err = err_of(
                run(&layout, &outside, params(Some("develop"), None)),
                "no repository, no worktrees to resolve",
            );
            assert!(!format!("{err:#}").is_empty());
        });
    }

    /// `--force` is a licence to replace *declared* entries, and nothing more.
    /// This module hands the flag to the engine and performs no removal of its
    /// own, so a source that declares nothing must leave the target's
    /// directories exactly where they were — even the ones a `copy:` section
    /// would have named.
    #[test]
    #[serial]
    fn force_with_nothing_declared_removes_nothing() {
        with_isolation(|| {
            let layout = Layout::new(&["develop"]).with_origin_head();

            let victim = layout.wt("develop").join("node_modules");
            std::fs::create_dir_all(victim.join("pkg")).unwrap();
            std::fs::write(victim.join("pkg/index.js"), "built-in-develop\n").unwrap();

            let result = run(
                &layout,
                &layout.wt("main"),
                WarmParams {
                    force: true,
                    ..params(Some("develop"), None)
                },
            )
            .expect("a --force run with no declarations is a no-op, not an error");

            assert!(result.nothing_declared());
            assert_eq!(
                std::fs::read_to_string(victim.join("pkg/index.js")).unwrap(),
                "built-in-develop\n",
                "--force must not delete anything the config never named"
            );
        });
    }

    /// A source that declares nothing is the "no work to do" case, and it must
    /// be distinguishable from "declared and everything was skipped" — the
    /// command says different things about them.
    #[test]
    #[serial]
    fn a_source_with_no_copy_section_declares_nothing() {
        with_isolation(|| {
            let layout = Layout::new(&["develop"]).with_origin_head();

            let result = run(&layout, &layout.wt("main"), params(Some("develop"), None)).unwrap();

            assert!(result.nothing_declared());
            assert!(result.outcome.is_empty());
            assert!(!result.has_existing_skips());
        });
    }
}
