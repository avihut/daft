//! `git status --porcelain` snapshots — the evidence `ValidateIntegrity`
//! checks a transform against, and the counts the plan line reports.
//!
//! A layout transform promises to carry a working tree unchanged. The check
//! that it did is the obvious one: capture each worktree's porcelain status
//! before anything moves, and require the same lines at the worktree's new
//! path afterwards. Layout artifacts are the one legitimate difference —
//! `nested` ignores its `.worktrees/` directory through a `.gitignore` line
//! daft itself maintains — so both sides pass through the same normalizer.

use std::path::{Path, PathBuf};

use anyhow::{Context, Result};

use super::state::{ClassifiedWorktree, LayoutState, WorktreeDisposition};
use crate::core::worktree::list::{ChangedFiles, classify_porcelain_status};

/// A worktree's porcelain status as of plan time.
#[derive(Debug, Clone)]
pub struct StatusSnapshot {
    /// Where the worktree was when this was captured (canonicalized) — the
    /// key. Never the branch: a detached pivot has none, and two worktrees can
    /// transiently report the same one mid-rebase.
    pub source_path: PathBuf,
    /// Where to re-probe after execution.
    pub target_path: PathBuf,
    pub branch: Option<String>,
    /// Normalized, sorted porcelain lines.
    pub lines: Vec<String>,
    /// Per-state counts over the normalized lines, for the plan line.
    pub counts: ChangedFiles,
}

/// Paths a status comparison must ignore: the layout's own artifacts.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Artifacts {
    /// First path components (relative to the main working tree) that hold
    /// linked worktrees in either state, e.g. `.worktrees`.
    prefixes: Vec<String>,
}

impl Artifacts {
    /// Derive the artifact set from **both** states' worktree positions: a
    /// `nested` → `sibling` transform must ignore `.worktrees/` even though
    /// only the source has it, and the reverse must ignore it even though
    /// only the target will.
    ///
    /// Relative to the **main working tree**, not the project root, and only
    /// for worktrees that actually sit inside it. Only a `nested`-style layout
    /// puts linked worktrees where the main working tree's `git status` can
    /// see them; `contained` and friends put them beside it, inside a wrapper
    /// that is not a working tree at all. Measuring from the project root
    /// turned every branch name's first segment into a blanket mask — a repo
    /// on branch `main` with an untracked `./main` binary, or one with a
    /// top-level `docs/` and a `docs` branch, had those entries stripped from
    /// the before snapshot, the after snapshot and the carried count alike, so
    /// losing them would have read as "Integrity check passed".
    pub fn for_transform(source: &LayoutState, target: &LayoutState) -> Self {
        let mut prefixes: Vec<String> = Vec::new();
        for state in [source, target] {
            let Some(root) = state.worktrees.iter().find(|wt| wt.is_root) else {
                continue;
            };
            for wt in &state.worktrees {
                if wt.is_root {
                    continue;
                }
                if let Ok(rel) = wt.path.strip_prefix(&root.path)
                    && let Some(first) = rel.components().next()
                {
                    let first = first.as_os_str().to_string_lossy().into_owned();
                    if !first.is_empty() && !prefixes.contains(&first) {
                        prefixes.push(first);
                    }
                }
            }
        }
        prefixes.sort();
        Self { prefixes }
    }

    /// Explicit artifact prefixes, for tests and callers that know better.
    pub fn with_prefixes(prefixes: &[&str]) -> Self {
        let mut prefixes: Vec<String> = prefixes.iter().map(|p| (*p).to_string()).collect();
        prefixes.sort();
        Self { prefixes }
    }

    /// Whether `path` (as printed by porcelain v1, relative to the worktree)
    /// is a layout artifact.
    pub fn ignores(&self, path: &str) -> bool {
        let path = path.trim_end_matches('/');
        // `.gitignore` only where a layout maintains one: the nested layouts
        // keep a `.worktrees/` line in it, which is exactly the case that
        // produces a prefix. Everywhere else it is an ordinary file, and
        // exempting it hid a real change for no benefit.
        if path == ".gitignore" && !self.prefixes.is_empty() {
            return true;
        }
        self.prefixes
            .iter()
            .any(|p| path == p || path.starts_with(&format!("{p}/")))
    }
}

/// `git status --porcelain` (v1) reduced to the lines a layout transform must
/// not change, sorted so the comparison is order-independent.
pub fn normalize(stdout: &str, artifacts: &Artifacts) -> Vec<String> {
    let mut lines: Vec<String> = stdout
        .lines()
        .filter(|l| l.len() >= 3)
        .filter(|l| !artifacts.ignores(porcelain_path(l)))
        .map(str::to_string)
        .collect();
    lines.sort();
    lines
}

/// The path part of a porcelain v1 line (`XY path`, or `XY old -> new` for a
/// rename — the *destination* is what exists in the tree).
fn porcelain_path(line: &str) -> &str {
    let rest = line.get(3..).unwrap_or("");
    match rest.rsplit_once(" -> ") {
        Some((_, new)) => new,
        None => rest,
    }
}

/// Snapshot every classified worktree that exists and is part of the
/// transform — root, conforming, moving or not. Non-conforming worktrees are
/// left alone by the plan, so they are not snapshotted.
pub fn capture(
    classified: &[ClassifiedWorktree],
    artifacts: &Artifacts,
) -> Result<Vec<StatusSnapshot>> {
    let mut out = Vec::new();
    for cw in classified {
        if cw.disposition == WorktreeDisposition::NonConforming || !cw.current_path.is_dir() {
            continue;
        }
        let raw = porcelain_status(&cw.current_path)?;
        let lines = normalize(&raw, artifacts);
        let counts = classify_porcelain_status(&lines.join("\n"));
        out.push(StatusSnapshot {
            source_path: cw
                .current_path
                .canonicalize()
                .unwrap_or_else(|_| cw.current_path.clone()),
            target_path: cw.target_path.clone(),
            branch: cw.branch.clone(),
            lines,
            counts,
        });
    }
    Ok(out)
}

/// `git status --porcelain` at `worktree`, raw.
pub fn porcelain_status(worktree: &Path) -> Result<String> {
    let out = crate::utils::git_command_at(worktree)
        .args(["status", "--porcelain"])
        .output()
        .with_context(|| format!("running git status in {}", worktree.display()))?;
    if !out.status.success() {
        anyhow::bail!(
            "git status failed in {}: {}",
            worktree.display(),
            String::from_utf8_lossy(&out.stderr).trim()
        );
    }
    Ok(String::from_utf8_lossy(&out.stdout).into_owned())
}

/// Lines present before a transform but missing after it, and the converse.
#[derive(Debug, Default, PartialEq, Eq)]
pub struct Drift {
    /// Status entries that vanished: something the transform was supposed to
    /// carry did not arrive.
    pub missing: Vec<String>,
    /// Status entries that appeared: something (a move hook, typically) wrote
    /// into the tree. Not a loss.
    pub extra: Vec<String>,
}

impl Drift {
    pub fn is_empty(&self) -> bool {
        self.missing.is_empty() && self.extra.is_empty()
    }
}

/// Compare two normalized line sets.
pub fn drift(before: &[String], after: &[String]) -> Drift {
    Drift {
        missing: before
            .iter()
            .filter(|l| !after.contains(l))
            .cloned()
            .collect(),
        extra: after
            .iter()
            .filter(|l| !before.contains(l))
            .cloned()
            .collect(),
    }
}

#[cfg(test)]
mod tests {
    use super::super::state::WorktreeEntry;
    use super::*;

    fn state(root: &str, worktrees: &[(&str, bool)]) -> LayoutState {
        LayoutState {
            git_dir: PathBuf::from(root).join(".git"),
            is_bare: false,
            default_branch: "main".into(),
            project_root: PathBuf::from(root),
            worktrees: worktrees
                .iter()
                .map(|(path, is_root)| WorktreeEntry {
                    branch: Some("b".into()),
                    head: None,
                    path: PathBuf::from(path),
                    is_root: *is_root,
                })
                .collect(),
        }
    }

    #[test]
    fn normalize_drops_the_gitignore_line_and_the_worktree_parent() {
        let artifacts = Artifacts::with_prefixes(&[".worktrees"]);
        let raw = " M src/main.rs\n?? .worktrees/\n M .gitignore\n?? NOTES.md\n?? .worktrees/feat/x.txt\n";
        assert_eq!(
            normalize(raw, &artifacts),
            vec![" M src/main.rs".to_string(), "?? NOTES.md".to_string()]
        );
    }

    #[test]
    fn normalize_derives_the_artifact_prefix_from_both_states() {
        // nested source → sibling target: only the source has `.worktrees/`.
        let source = state(
            "/repo",
            &[("/repo", true), ("/repo/.worktrees/feat", false)],
        );
        let target = state("/repo", &[("/repo", true), ("/repo.feat", false)]);
        let a = Artifacts::for_transform(&source, &target);
        assert!(a.ignores(".worktrees/"));
        assert!(a.ignores(".worktrees/feat/x"));
        assert!(!a.ignores("src/.worktrees"));
        // And the reverse direction finds the same prefix in the target.
        let b = Artifacts::for_transform(&target, &source);
        assert_eq!(a, b);
    }

    /// #875 review: prefixes were measured from the *project root*, so for
    /// any bare or wrapped target — where no worktree is the root — every
    /// worktree contributed the first segment of its branch name as a blanket
    /// mask. A repo on branch `main` with an untracked `./main` binary had that
    /// entry stripped from the before snapshot, the after snapshot and the
    /// carried count alike, so losing it read as "Integrity check passed".
    #[test]
    fn a_branch_name_is_not_an_artifact_prefix() {
        // contained: the wrapper holds every worktree; none is the main
        // working tree, so nothing a worktree's own `git status` can see.
        let source = state("/repo", &[("/repo/main", false), ("/repo/docs", false)]);
        let target = state("/repo", &[("/repo/main", false), ("/repo/docs", false)]);
        let a = Artifacts::for_transform(&source, &target);
        assert!(
            !a.ignores("main"),
            "an untracked ./main binary is not an artifact"
        );
        assert!(
            !a.ignores("docs/index.md"),
            "a top-level docs/ is not an artifact"
        );
        assert!(
            !a.ignores(".gitignore"),
            "no nested layout maintains one here"
        );
    }

    /// Only a layout that puts linked worktrees *inside* the main working tree
    /// produces an artifact prefix — and only then is `.gitignore` daft's to
    /// maintain.
    #[test]
    fn only_a_nested_layout_exempts_the_gitignore() {
        let nested = state(
            "/repo",
            &[("/repo", true), ("/repo/.worktrees/feat", false)],
        );
        let sibling = state("/repo", &[("/repo", true), ("/repo.feat", false)]);
        assert!(Artifacts::for_transform(&nested, &sibling).ignores(".gitignore"));
        assert!(!Artifacts::for_transform(&sibling, &sibling).ignores(".gitignore"));
        // A sibling worktree is beside the main working tree, never inside it.
        assert!(!Artifacts::for_transform(&sibling, &sibling).ignores("repo.feat"));
    }

    #[test]
    fn normalize_is_order_independent() {
        let artifacts = Artifacts::default();
        let a = normalize("?? b\n M a\n", &artifacts);
        let b = normalize(" M a\n?? b\n", &artifacts);
        assert_eq!(a, b);
    }

    #[test]
    fn renames_compare_by_destination() {
        let artifacts = Artifacts::with_prefixes(&[".worktrees"]);
        assert_eq!(porcelain_path("R  old.txt -> new.txt"), "new.txt");
        assert!(normalize("R  a -> .worktrees/b\n", &artifacts).is_empty());
    }

    #[test]
    fn drift_separates_losses_from_additions() {
        let before = vec![" M a".to_string(), "?? b".to_string()];
        let after = vec![" M a".to_string(), "?? hook-made".to_string()];
        let d = drift(&before, &after);
        assert_eq!(d.missing, vec!["?? b".to_string()]);
        assert_eq!(d.extra, vec!["?? hook-made".to_string()]);
        assert!(drift(&before, &before).is_empty());
    }
}
