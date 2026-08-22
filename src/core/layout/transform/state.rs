//! Layout state representation for transform planning.
//!
//! `LayoutState` captures where everything is (source) or should be (target):
//! git_dir location, bare flag, and all worktree positions.

use std::path::{Path, PathBuf};

use anyhow::Result;

use crate::core::layout::Layout;
use crate::core::multi_remote::path::build_template_context;

/// Snapshot of a repository's layout state.
#[derive(Debug, Clone)]
pub struct LayoutState {
    /// Absolute path to the `.git` directory.
    pub git_dir: PathBuf,
    /// Whether `core.bare` is true.
    pub is_bare: bool,
    /// The default branch name. Used for template evaluation and as the
    /// *preferred* pivot when a bare repository gains a working tree — never
    /// as the definition of which worktree holds the root role.
    pub default_branch: String,
    /// The project root / wrapper directory. For bare and wrapped non-bare
    /// layouts this is the parent of worktrees. For regular non-bare layouts
    /// this is the repo root itself.
    pub project_root: PathBuf,
    /// All worktree entries (including the default branch for bare layouts).
    pub worktrees: Vec<WorktreeEntry>,
}

/// A single worktree's position in a layout state.
#[derive(Debug, Clone)]
pub struct WorktreeEntry {
    /// Branch name (e.g., "main", "feature/auth").
    pub branch: String,
    /// Absolute path to the worktree directory.
    pub path: PathBuf,
    /// Whether this worktree holds the *root role*.
    ///
    /// In a **source** state that means the repository's main working tree —
    /// the one git refuses to `git worktree move` — whatever branch it happens
    /// to have checked out. In a **target** state it is the worktree that
    /// carries the role through the transform: the one that becomes the main
    /// working tree, or the one that was just nested out of the project root.
    ///
    /// This is deliberately *not* "the default branch": a plain clone adopted
    /// mid-task has its main working tree on a feature branch while the default
    /// branch has no worktree at all (#859).
    pub is_root: bool,
}

/// Classification of a worktree during transform planning.
#[derive(Debug, Clone, PartialEq)]
pub enum WorktreeDisposition {
    /// Worktree conforms to the target template — will be relocated if needed.
    Conforming,
    /// Worktree does not match the target template — skipped by default.
    NonConforming,
    /// Worktree holds the root role and needs special handling (collapse/nest).
    Root,
}

/// A classified worktree entry in the transform plan.
#[derive(Debug, Clone)]
pub struct ClassifiedWorktree {
    pub branch: String,
    pub current_path: PathBuf,
    pub target_path: PathBuf,
    pub disposition: WorktreeDisposition,
}

// ── State readers ──────────────────────────────────────────────────────────

/// Parse `git worktree list --porcelain` output into worktree entries.
///
/// A transform-planning view over the shared
/// [`crate::core::worktree::porcelain::parse_worktree_list_porcelain`]: bare
/// root entries and detached HEAD worktrees are skipped, as are entries without
/// a branch.
///
/// The porcelain has no marker for the main working tree — main-ness is
/// *positional*, the first non-bare stanza (the convention
/// [`crate::core::worktree::porcelain::first_main_index`] and
/// `layout::detect` already follow). That decision is therefore made **before**
/// the detached/bare filter: dropping a detached main worktree must leave the
/// state with no root at all, not promote the next worktree in the list.
///
/// `is_bare` is the repository's `core.bare` flag: a bare repository has no
/// main working tree, so none of its worktrees hold the root role.
pub fn parse_porcelain_to_entries(porcelain: &str, is_bare: bool) -> Vec<WorktreeEntry> {
    let raw = crate::core::worktree::porcelain::parse_worktree_list_porcelain(porcelain);

    let main_index = if is_bare {
        None
    } else {
        crate::core::worktree::porcelain::first_main_index(&raw)
    };

    raw.into_iter()
        .enumerate()
        .filter(|(_, e)| !e.is_bare && !e.is_detached)
        .filter_map(|(i, e)| {
            e.branch.map(|branch| WorktreeEntry {
                branch,
                path: e.path,
                is_root: main_index == Some(i),
            })
        })
        .collect()
}

/// Read current layout state from the repo.
pub fn read_source_state(
    git: &crate::git::GitCommand,
    default_branch: &str,
) -> Result<LayoutState> {
    let git_dir = crate::core::repo::get_git_common_dir()?;
    let git_dir = git_dir.canonicalize().unwrap_or(git_dir);

    let is_bare = git
        .config_get("core.bare")?
        .map(|v| v.trim() == "true")
        .unwrap_or(false);

    let project_root = git_dir
        .parent()
        .ok_or_else(|| anyhow::anyhow!("Could not determine project root from git dir"))?
        .to_path_buf();

    let porcelain = git.worktree_list_porcelain()?;
    let worktrees = parse_porcelain_to_entries(&porcelain, is_bare);

    Ok(LayoutState {
        git_dir,
        is_bare,
        default_branch: default_branch.to_string(),
        project_root,
        worktrees,
    })
}

/// Evaluate the layout template for a branch to compute its target worktree path.
pub fn compute_target_worktree_path(
    layout: &Layout,
    project_root: &Path,
    branch: &str,
) -> Result<PathBuf> {
    let ctx = build_template_context(project_root, branch);
    layout.worktree_path(&ctx)
}

/// Derive where `.git` should live for the target layout.
///
/// - Bare layouts (`layout.needs_bare()`): `project_root/.git`
/// - Wrapped non-bare (`layout.needs_wrapper()`): evaluate the template for
///   `root_branch` — the branch of the worktree that becomes the main working
///   tree — and append `/.git`
/// - Regular non-bare: `project_root/.git`
pub fn compute_target_git_dir(
    layout: &Layout,
    project_root: &Path,
    root_branch: &str,
) -> Result<PathBuf> {
    if layout.needs_bare() {
        return Ok(project_root.join(".git"));
    }

    if layout.needs_wrapper() {
        let worktree_path = compute_target_worktree_path(layout, project_root, root_branch)?;
        return Ok(worktree_path.join(".git"));
    }

    Ok(project_root.join(".git"))
}

/// Pick the worktree that carries the root role into the target layout.
///
/// Returns an index into `source.worktrees`, or `None` when the target layout
/// needs no root worktree (bare → bare) or the source has none to offer.
///
/// Every refusal here happens at *planning* time — before the caller's dirty
/// check, before `--force` stashing, and before any mutation.
fn select_pivot(layout: &Layout, source: &LayoutState) -> Result<Option<usize>> {
    if !source.is_bare {
        // The main working tree keeps the role. Nothing else can take it: git
        // refuses to move a main working tree, so a different choice would
        // plan an operation that cannot execute.
        let root = source.worktrees.iter().position(|wt| wt.is_root);
        if root.is_none() && (layout.needs_bare() || layout.needs_wrapper()) {
            anyhow::bail!(
                "The main working tree at {} has a detached HEAD, so the '{}' layout has \
                 no branch to place it under. Check out a branch there (`git switch \
                 <branch>`) and retry.",
                source.project_root.display(),
                layout.name
            );
        }
        return Ok(root);
    }

    // Bare source: a bare target places every branch by template, so no
    // worktree needs promoting.
    if layout.needs_bare() {
        return Ok(None);
    }

    // Bare → non-bare. Prefer the default branch's worktree; fall back to the
    // sole worktree. Never guess from the cwd — the same command must not
    // produce different layouts depending on where it is run.
    if let Some(i) = source
        .worktrees
        .iter()
        .position(|wt| wt.branch == source.default_branch)
    {
        return Ok(Some(i));
    }

    match source.worktrees.len() {
        0 => anyhow::bail!(
            "The '{}' layout needs a worktree at the repository root, but this bare \
             repository has none. Create one with `daft go {}` and retry.",
            layout.name,
            source.default_branch
        ),
        1 => Ok(Some(0)),
        n => anyhow::bail!(
            "The default branch '{}' has no worktree, and {n} worktrees could take the \
             repository root in the '{}' layout. Create the default branch's worktree \
             with `daft go {}` and retry.",
            source.default_branch,
            layout.name,
            source.default_branch
        ),
    }
}

/// Compute the full target state by evaluating the template for each branch.
///
/// The *pivot* — see [`select_pivot`] — is the worktree that carries the root
/// role. It is chosen structurally (which worktree is the main working tree),
/// never by branch name, and its branch is what decides where a wrapped `.git`
/// ends up.
pub fn compute_target_state(layout: &Layout, source: &LayoutState) -> Result<LayoutState> {
    let project_root = source.project_root.as_path();
    let is_bare = layout.needs_bare();
    let pivot = select_pivot(layout, source)?;

    let root_branch = pivot
        .map(|i| source.worktrees[i].branch.as_str())
        .unwrap_or(source.default_branch.as_str());
    let git_dir = compute_target_git_dir(layout, project_root, root_branch)?;

    let mut worktrees = Vec::with_capacity(source.worktrees.len());
    for (i, wt) in source.worktrees.iter().enumerate() {
        let is_root = pivot == Some(i);
        // For regular non-bare layouts (sibling, nested, centralized) the root
        // worktree IS the repo root — it's not placed by the template. For bare
        // and wrapped non-bare layouts, all branches are placed by the template.
        let target_path = if is_root && !is_bare && !layout.needs_wrapper() {
            project_root.to_path_buf()
        } else {
            compute_target_worktree_path(layout, project_root, &wt.branch)?
        };
        worktrees.push(WorktreeEntry {
            branch: wt.branch.clone(),
            path: target_path,
            is_root,
        });
    }

    Ok(LayoutState {
        git_dir,
        is_bare,
        default_branch: source.default_branch.clone(),
        project_root: project_root.to_path_buf(),
        worktrees,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::core::layout::BuiltinLayout;

    /// Build a source state directly; `worktrees` are `(branch, path, is_root)`.
    fn source_state(
        git_dir: &str,
        is_bare: bool,
        default_branch: &str,
        project_root: &str,
        worktrees: Vec<(&str, &str, bool)>,
    ) -> LayoutState {
        LayoutState {
            git_dir: PathBuf::from(git_dir),
            is_bare,
            default_branch: default_branch.to_string(),
            project_root: PathBuf::from(project_root),
            worktrees: worktrees
                .into_iter()
                .map(|(branch, path, is_root)| WorktreeEntry {
                    branch: branch.to_string(),
                    path: PathBuf::from(path),
                    is_root,
                })
                .collect(),
        }
    }

    #[test]
    fn test_parse_porcelain_basic() {
        let porcelain = "worktree /home/user/myproject\nbare\n\nworktree /home/user/myproject/main\nbranch refs/heads/main\n\nworktree /home/user/myproject/develop\nbranch refs/heads/develop\n\n";
        let entries = parse_porcelain_to_entries(porcelain, true);
        assert_eq!(entries.len(), 2);
        assert_eq!(entries[0].branch, "main");
        assert_eq!(entries[0].path, PathBuf::from("/home/user/myproject/main"));
        assert_eq!(entries[1].branch, "develop");
        // A bare repo has no main working tree.
        assert!(entries.iter().all(|e| !e.is_root));
    }

    #[test]
    fn test_parse_porcelain_skips_detached() {
        let porcelain = "worktree /repo\nbare\n\nworktree /repo/main\nbranch refs/heads/main\n\nworktree /repo/sandbox\nHEAD abc123\ndetached\n\n";
        let entries = parse_porcelain_to_entries(porcelain, true);
        assert_eq!(entries.len(), 1);
        assert_eq!(entries[0].branch, "main");
    }

    #[test]
    fn test_parse_porcelain_nonbare() {
        // Non-bare repo: first entry has branch, no "bare" line
        let porcelain = "worktree /home/user/myproject\nbranch refs/heads/main\n\nworktree /home/user/myproject.develop\nbranch refs/heads/develop\n\n";
        let entries = parse_porcelain_to_entries(porcelain, false);
        assert_eq!(entries.len(), 2);
        assert_eq!(entries[0].branch, "main");
        assert!(entries[0].is_root);
        assert_eq!(entries[1].branch, "develop");
        assert!(!entries[1].is_root);
    }

    #[test]
    fn test_parse_porcelain_root_is_positional_not_named() {
        // #859: the main working tree is on a feature branch and the default
        // branch has no worktree at all.
        let porcelain = "worktree /repo\nbranch refs/heads/task/local-docker\n\nworktree /repo.develop\nbranch refs/heads/develop\n\n";
        let entries = parse_porcelain_to_entries(porcelain, false);
        assert_eq!(entries.len(), 2);
        assert_eq!(entries[0].branch, "task/local-docker");
        assert!(
            entries[0].is_root,
            "the main working tree holds the root role"
        );
        assert!(!entries[1].is_root);
    }

    #[test]
    fn test_parse_porcelain_detached_main_yields_no_root() {
        // The detached main worktree is dropped; the *next* worktree must not
        // inherit the role.
        let porcelain = "worktree /repo\nHEAD abc123\ndetached\n\nworktree /repo.develop\nbranch refs/heads/develop\n\n";
        let entries = parse_porcelain_to_entries(porcelain, false);
        assert_eq!(entries.len(), 1);
        assert_eq!(entries[0].branch, "develop");
        assert!(!entries[0].is_root);
    }

    #[test]
    fn test_target_worktree_path_contained() {
        let layout = BuiltinLayout::Contained.to_layout();
        let path = compute_target_worktree_path(
            &layout,
            Path::new("/home/user/myproject"),
            "feature/auth",
        )
        .unwrap();
        assert_eq!(path, PathBuf::from("/home/user/myproject/feature/auth"));
    }

    #[test]
    fn test_target_worktree_path_sibling() {
        let layout = BuiltinLayout::Sibling.to_layout();
        let path = compute_target_worktree_path(
            &layout,
            Path::new("/home/user/myproject"),
            "feature/auth",
        )
        .unwrap();
        assert_eq!(path, PathBuf::from("/home/user/myproject.feature-auth"));
    }

    #[test]
    fn test_target_worktree_path_contained_classic() {
        let layout = BuiltinLayout::ContainedClassic.to_layout();
        let path = compute_target_worktree_path(
            &layout,
            Path::new("/home/user/myproject"),
            "feature/auth",
        )
        .unwrap();
        assert_eq!(path, PathBuf::from("/home/user/myproject/feature/auth"));
    }

    #[test]
    fn test_target_git_dir_bare() {
        let layout = BuiltinLayout::Contained.to_layout();
        let git_dir =
            compute_target_git_dir(&layout, Path::new("/home/user/myproject"), "main").unwrap();
        assert_eq!(git_dir, PathBuf::from("/home/user/myproject/.git"));
    }

    #[test]
    fn test_target_git_dir_wrapped_nonbare() {
        let layout = BuiltinLayout::ContainedClassic.to_layout();
        let git_dir =
            compute_target_git_dir(&layout, Path::new("/home/user/myproject"), "main").unwrap();
        assert_eq!(git_dir, PathBuf::from("/home/user/myproject/main/.git"));
    }

    #[test]
    fn test_target_git_dir_regular_nonbare() {
        let layout = BuiltinLayout::Sibling.to_layout();
        let git_dir =
            compute_target_git_dir(&layout, Path::new("/home/user/myproject"), "main").unwrap();
        assert_eq!(git_dir, PathBuf::from("/home/user/myproject/.git"));
    }

    #[test]
    fn test_compute_target_state() {
        let layout = BuiltinLayout::Sibling.to_layout();
        let source = source_state(
            "/repo/.git",
            false,
            "main",
            "/repo",
            vec![("main", "/repo", true), ("develop", "/repo/develop", false)],
        );
        let target = compute_target_state(&layout, &source).unwrap();
        assert!(!target.is_bare);
        assert_eq!(target.git_dir, PathBuf::from("/repo/.git"));
        assert_eq!(target.worktrees.len(), 2);
        // The root worktree in sibling layout lives at project root, not a
        // template path.
        assert_eq!(target.worktrees[0].path, PathBuf::from("/repo"));
        assert!(target.worktrees[0].is_root);
        assert_eq!(target.worktrees[1].path, PathBuf::from("/repo.develop"));
    }

    #[test]
    fn test_pivot_is_the_main_worktree_not_the_default_branch() {
        // #859: root on a feature branch, default branch has no worktree.
        let layout = BuiltinLayout::Contained.to_layout();
        let source = source_state(
            "/repo/.git",
            false,
            "master",
            "/repo",
            vec![("task/local-docker", "/repo", true)],
        );
        let target = compute_target_state(&layout, &source).unwrap();
        assert!(target.is_bare);
        assert!(target.worktrees[0].is_root);
        assert_eq!(
            target.worktrees[0].path,
            PathBuf::from("/repo/task/local-docker")
        );
    }

    #[test]
    fn test_wrapped_git_dir_follows_the_pivot_branch() {
        let layout = BuiltinLayout::ContainedClassic.to_layout();
        let source = source_state(
            "/repo/.git",
            false,
            "master",
            "/repo",
            vec![
                ("task/x", "/repo", true),
                ("develop", "/repo.develop", false),
            ],
        );
        let target = compute_target_state(&layout, &source).unwrap();
        // .git goes inside the *pivot's* directory, not master's.
        assert_eq!(target.git_dir, PathBuf::from("/repo/task/x/.git"));
        assert_eq!(target.worktrees[0].path, PathBuf::from("/repo/task/x"));
    }

    #[test]
    fn test_detached_main_refused_for_bare_target() {
        let source = source_state(
            "/repo/.git",
            false,
            "main",
            "/repo",
            vec![("develop", "/repo.develop", false)],
        );
        let err = compute_target_state(&BuiltinLayout::Contained.to_layout(), &source).unwrap_err();
        assert!(err.to_string().contains("detached HEAD"), "{err}");

        let err = compute_target_state(&BuiltinLayout::ContainedClassic.to_layout(), &source)
            .unwrap_err();
        assert!(err.to_string().contains("detached HEAD"), "{err}");
    }

    #[test]
    fn test_detached_main_allowed_for_regular_nonbare_target() {
        let source = source_state(
            "/repo/.git",
            false,
            "main",
            "/repo",
            vec![("develop", "/repo/develop", false)],
        );
        let target = compute_target_state(&BuiltinLayout::Sibling.to_layout(), &source).unwrap();
        assert!(target.worktrees.iter().all(|wt| !wt.is_root));
        assert_eq!(target.worktrees[0].path, PathBuf::from("/repo.develop"));
    }

    #[test]
    fn test_bare_to_nonbare_prefers_default_branch() {
        let source = source_state(
            "/repo/.git",
            true,
            "main",
            "/repo",
            vec![("dev", "/repo/dev", false), ("main", "/repo/main", false)],
        );
        let target = compute_target_state(&BuiltinLayout::Sibling.to_layout(), &source).unwrap();
        assert!(!target.worktrees[0].is_root);
        assert!(target.worktrees[1].is_root);
        assert_eq!(target.worktrees[1].path, PathBuf::from("/repo"));
    }

    #[test]
    fn test_bare_to_nonbare_falls_back_to_sole_worktree() {
        let source = source_state(
            "/repo/.git",
            true,
            "main",
            "/repo",
            vec![("task/x", "/repo/task/x", false)],
        );
        let target = compute_target_state(&BuiltinLayout::Sibling.to_layout(), &source).unwrap();
        assert!(target.worktrees[0].is_root);
        assert_eq!(target.worktrees[0].path, PathBuf::from("/repo"));
    }

    #[test]
    fn test_bare_to_nonbare_ambiguous_refuses() {
        let source = source_state(
            "/repo/.git",
            true,
            "main",
            "/repo",
            vec![("a", "/repo/a", false), ("b", "/repo/b", false)],
        );
        let err = compute_target_state(&BuiltinLayout::Sibling.to_layout(), &source).unwrap_err();
        let msg = err.to_string();
        assert!(msg.contains("'main' has no worktree"), "{msg}");
        assert!(msg.contains("daft go main"), "{msg}");
    }

    #[test]
    fn test_bare_to_nonbare_with_no_worktrees_refuses() {
        let source = source_state("/repo/.git", true, "main", "/repo", vec![]);
        let err = compute_target_state(&BuiltinLayout::Sibling.to_layout(), &source).unwrap_err();
        assert!(err.to_string().contains("has none"), "{err}");
    }

    #[test]
    fn test_bare_to_bare_has_no_pivot() {
        let source = source_state(
            "/repo/.git",
            true,
            "main",
            "/repo",
            vec![("main", "/repo/main", false), ("f/x", "/repo/f/x", false)],
        );
        let target =
            compute_target_state(&BuiltinLayout::ContainedFlat.to_layout(), &source).unwrap();
        assert!(target.worktrees.iter().all(|wt| !wt.is_root));
        assert_eq!(target.worktrees[1].path, PathBuf::from("/repo/f-x"));
    }
}
