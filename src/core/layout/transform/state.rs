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
    /// The checked-out branch (e.g., "main", "feature/auth"), or `None` for a
    /// detached HEAD.
    ///
    /// Only the *main* working tree may be detached here — detached linked
    /// worktrees are dropped by [`parse_porcelain_to_entries`], exactly as
    /// they always were. A detached main working tree has no branch to name
    /// its directory by, so nesting it needs a name from the caller
    /// ([`PivotOverride::dirname`]).
    pub branch: Option<String>,
    /// The commit HEAD resolves to. For a detached entry this is its only
    /// identity; the relocation carries it verbatim.
    pub head: Option<String>,
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

impl WorktreeEntry {
    /// The branch, or a stable placeholder for a detached entry.
    pub fn label(&self) -> &str {
        self.branch.as_deref().unwrap_or("(detached)")
    }
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
    pub branch: Option<String>,
    pub current_path: PathBuf,
    pub target_path: PathBuf,
    pub disposition: WorktreeDisposition,
}

impl ClassifiedWorktree {
    /// The branch, or a stable placeholder for a detached entry.
    pub fn label(&self) -> &str {
        self.branch.as_deref().unwrap_or("(detached)")
    }
}

/// The two answers a transform cannot work out for itself, supplied by the
/// caller (flags, a prompt, or a default).
#[derive(Debug, Clone, Default)]
pub struct PivotOverride {
    /// `--pivot <branch>`: which worktree takes the root role when a bare
    /// source has no default-branch worktree and more than one candidate.
    pub branch: Option<String>,
    /// `--as <dir>`: the directory name for a pivot with no branch — a
    /// detached main working tree being nested under the project root.
    pub dirname: Option<String>,
}

/// The root-role question a transform may have to ask before it can plan.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RootSituation {
    /// The engine can decide the root role by itself.
    Settled,
    /// The main working tree is detached and the target layout nests it: it
    /// needs a directory name. `head` is the commit it is detached at.
    DetachedMain { head: Option<String> },
    /// Bare → non-bare with no default-branch worktree and several
    /// candidates: indices into `source.worktrees`.
    AmbiguousPivot { candidates: Vec<usize> },
}

/// Classify what `compute_target_state` would refuse for want of an answer,
/// so the caller can ask (or name the flag) *before* planning.
pub fn root_situation(layout: &Layout, source: &LayoutState) -> RootSituation {
    if !source.is_bare {
        if let Some(root) = source.worktrees.iter().find(|wt| wt.is_root)
            && root.branch.is_none()
            && (layout.needs_bare() || layout.needs_wrapper())
        {
            return RootSituation::DetachedMain {
                head: root.head.clone(),
            };
        }
        return RootSituation::Settled;
    }
    if layout.needs_bare() {
        return RootSituation::Settled;
    }
    let has_default = source
        .worktrees
        .iter()
        .any(|wt| wt.branch.as_deref() == Some(source.default_branch.as_str()));
    if has_default || source.worktrees.len() <= 1 {
        return RootSituation::Settled;
    }
    RootSituation::AmbiguousPivot {
        candidates: (0..source.worktrees.len()).collect(),
    }
}

// ── State readers ──────────────────────────────────────────────────────────

/// Parse `git worktree list --porcelain` output into worktree entries.
///
/// A transform-planning view over the shared
/// [`crate::core::worktree::porcelain::parse_worktree_list_porcelain`]: bare
/// root entries and detached *linked* worktrees are skipped, as are entries
/// without a branch — except the main working tree, which is kept even when
/// detached, because it holds the root role whatever HEAD says.
///
/// The porcelain has no marker for the main working tree — main-ness is
/// *positional*, the first non-bare stanza (the convention
/// [`crate::core::worktree::porcelain::first_main_index`] and
/// `layout::detect` already follow). That decision is therefore made **before**
/// the detached/bare filter.
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
        .filter(|(i, e)| !e.is_bare && (!e.is_detached || main_index == Some(*i)))
        .filter_map(|(i, e)| {
            let is_root = main_index == Some(i);
            // A linked worktree without a branch is a detached sandbox — not
            // placed by any layout. A main one is kept, branchless.
            if e.branch.is_none() && !is_root {
                return None;
            }
            Some(WorktreeEntry {
                branch: e.branch,
                head: e.head,
                path: e.path,
                is_root,
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
    let mut worktrees = parse_porcelain_to_entries(&porcelain, is_bare);

    // Mid-rebase git detaches HEAD to replay commits, so the porcelain reports
    // the main working tree as branchless even though `rebase-merge/head-name`
    // still names its branch for the whole operation (#736). Recover it: the
    // transform refuses the paused rebase anyway, and it should say so rather
    // than also ask what to call a tree that has a perfectly good name.
    for wt in worktrees.iter_mut().filter(|wt| wt.branch.is_none()) {
        wt.branch = crate::git::op_state::recovered_branch(&wt.path);
    }

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
///   `root_name` — the branch (or supplied directory name) of the worktree
///   that becomes the main working tree — and append `/.git`
/// - Regular non-bare: `project_root/.git`
pub fn compute_target_git_dir(
    layout: &Layout,
    project_root: &Path,
    root_name: &str,
) -> Result<PathBuf> {
    if layout.needs_bare() {
        return Ok(project_root.join(".git"));
    }

    if layout.needs_wrapper() {
        let worktree_path = compute_target_worktree_path(layout, project_root, root_name)?;
        return Ok(worktree_path.join(".git"));
    }

    Ok(project_root.join(".git"))
}

/// Pick the worktree that carries the root role into the target layout.
///
/// Returns an index into `source.worktrees`, or `None` when the target layout
/// needs no root worktree (bare → bare) or the source has none to offer.
///
/// Every refusal here happens at *planning* time — before any mutation. The
/// two that used to be dead ends now name the flag that answers them:
/// `--as <dir>` for a detached main working tree, `--pivot <branch>` for an
/// ambiguous bare → non-bare collapse.
fn select_pivot(
    layout: &Layout,
    source: &LayoutState,
    over: &PivotOverride,
) -> Result<Option<usize>> {
    if !source.is_bare {
        // The main working tree keeps the role. Nothing else can take it: git
        // refuses to move a main working tree, so a different choice would
        // plan an operation that cannot execute.
        let root = source.worktrees.iter().position(|wt| wt.is_root);
        if let Some(i) = root
            && source.worktrees[i].branch.is_none()
            && (layout.needs_bare() || layout.needs_wrapper())
            && over.dirname.is_none()
        {
            anyhow::bail!(
                "The main working tree at {} has a detached HEAD, so the '{}' layout has \
                 no branch to name its directory. Pass `--as <dir>` to name it (or `-y` \
                 to accept a name derived from the commit) and retry.",
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

    // Bare → non-bare. An explicit `--pivot` wins; then prefer the default
    // branch's worktree; then fall back to the sole worktree. Never guess
    // from the cwd — the same command must not produce different layouts
    // depending on where it is run.
    if let Some(wanted) = over.branch.as_deref() {
        return match source
            .worktrees
            .iter()
            .position(|wt| wt.branch.as_deref() == Some(wanted))
        {
            Some(i) => Ok(Some(i)),
            None => anyhow::bail!(
                "`--pivot {wanted}` names a branch with no worktree in this repository. \
                 The worktrees that could take the root are: {}.",
                candidate_list(source)
            ),
        };
    }

    if let Some(i) = source
        .worktrees
        .iter()
        .position(|wt| wt.branch.as_deref() == Some(source.default_branch.as_str()))
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
             repository root in the '{}' layout: {}. Name one with `--pivot <branch>` \
             and retry.",
            source.default_branch,
            layout.name,
            candidate_list(source)
        ),
    }
}

fn candidate_list(source: &LayoutState) -> String {
    source
        .worktrees
        .iter()
        .map(|wt| format!("'{}'", wt.label()))
        .collect::<Vec<_>>()
        .join(", ")
}

/// The name the layout template is evaluated against for `wt`.
///
/// A branch names itself. A detached pivot has no name of its own, so it
/// takes the caller-supplied directory name (`--as`).
fn placement_name<'a>(wt: &'a WorktreeEntry, over: &'a PivotOverride) -> Result<&'a str> {
    wt.branch
        .as_deref()
        .or(over.dirname.as_deref())
        .ok_or_else(|| {
            anyhow::anyhow!(
                "the detached worktree at {} has no branch to place it by; pass `--as <dir>`",
                wt.path.display()
            )
        })
}

/// Compute the full target state by evaluating the template for each branch.
///
/// The *pivot* — see [`select_pivot`] — is the worktree that carries the root
/// role. It is chosen structurally (which worktree is the main working tree),
/// never by branch name, and its branch (or `--as` name) is what decides
/// where a wrapped `.git` ends up.
pub fn compute_target_state(
    layout: &Layout,
    source: &LayoutState,
    over: &PivotOverride,
) -> Result<LayoutState> {
    let project_root = source.project_root.as_path();
    let is_bare = layout.needs_bare();
    let pivot = select_pivot(layout, source, over)?;

    let root_name = match pivot {
        Some(i) if !is_bare && !layout.needs_wrapper() => source.worktrees[i]
            .branch
            .as_deref()
            .unwrap_or(source.default_branch.as_str()),
        Some(i) => placement_name(&source.worktrees[i], over)?,
        None => source.default_branch.as_str(),
    };
    let git_dir = compute_target_git_dir(layout, project_root, root_name)?;

    let mut worktrees = Vec::with_capacity(source.worktrees.len());
    for (i, wt) in source.worktrees.iter().enumerate() {
        let is_root = pivot == Some(i);
        // For regular non-bare layouts (sibling, nested, centralized) the root
        // worktree IS the repo root — it's not placed by the template. For bare
        // and wrapped non-bare layouts, all branches are placed by the template.
        let target_path = if is_root && !is_bare && !layout.needs_wrapper() {
            project_root.to_path_buf()
        } else {
            compute_target_worktree_path(layout, project_root, placement_name(wt, over)?)?
        };
        worktrees.push(WorktreeEntry {
            branch: wt.branch.clone(),
            head: wt.head.clone(),
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
                    branch: Some(branch.to_string()),
                    head: None,
                    path: PathBuf::from(path),
                    is_root,
                })
                .collect(),
        }
    }

    /// A non-bare source whose main working tree is detached at `head`.
    fn detached_source_state(
        project_root: &str,
        head: &str,
        linked: Vec<(&str, &str)>,
    ) -> LayoutState {
        let mut worktrees = vec![WorktreeEntry {
            branch: None,
            head: Some(head.to_string()),
            path: PathBuf::from(project_root),
            is_root: true,
        }];
        worktrees.extend(linked.into_iter().map(|(branch, path)| WorktreeEntry {
            branch: Some(branch.to_string()),
            head: None,
            path: PathBuf::from(path),
            is_root: false,
        }));
        LayoutState {
            git_dir: PathBuf::from(project_root).join(".git"),
            is_bare: false,
            default_branch: "main".to_string(),
            project_root: PathBuf::from(project_root),
            worktrees,
        }
    }

    fn target(layout: BuiltinLayout, source: &LayoutState) -> Result<LayoutState> {
        compute_target_state(&layout.to_layout(), source, &PivotOverride::default())
    }

    #[test]
    fn test_parse_porcelain_basic() {
        let porcelain = "worktree /home/user/myproject\nbare\n\nworktree /home/user/myproject/main\nbranch refs/heads/main\n\nworktree /home/user/myproject/develop\nbranch refs/heads/develop\n\n";
        let entries = parse_porcelain_to_entries(porcelain, true);
        assert_eq!(entries.len(), 2);
        assert_eq!(entries[0].branch.as_deref(), Some("main"));
        assert_eq!(entries[0].path, PathBuf::from("/home/user/myproject/main"));
        assert_eq!(entries[1].branch.as_deref(), Some("develop"));
        // A bare repo has no main working tree.
        assert!(entries.iter().all(|e| !e.is_root));
    }

    #[test]
    fn test_parse_porcelain_skips_detached() {
        // A detached *linked* worktree (a sandbox) is not placed by any
        // layout and stays out of the state.
        let porcelain = "worktree /repo\nbare\n\nworktree /repo/main\nbranch refs/heads/main\n\nworktree /repo/sandbox\nHEAD abc123\ndetached\n\n";
        let entries = parse_porcelain_to_entries(porcelain, true);
        assert_eq!(entries.len(), 1);
        assert_eq!(entries[0].branch.as_deref(), Some("main"));
    }

    #[test]
    fn test_parse_porcelain_nonbare() {
        // Non-bare repo: first entry has branch, no "bare" line
        let porcelain = "worktree /home/user/myproject\nbranch refs/heads/main\n\nworktree /home/user/myproject.develop\nbranch refs/heads/develop\n\n";
        let entries = parse_porcelain_to_entries(porcelain, false);
        assert_eq!(entries.len(), 2);
        assert_eq!(entries[0].branch.as_deref(), Some("main"));
        assert!(entries[0].is_root);
        assert_eq!(entries[1].branch.as_deref(), Some("develop"));
        assert!(!entries[1].is_root);
    }

    #[test]
    fn test_parse_porcelain_root_is_positional_not_named() {
        // #859: the main working tree is on a feature branch and the default
        // branch has no worktree at all.
        let porcelain = "worktree /repo\nbranch refs/heads/task/local-docker\n\nworktree /repo.develop\nbranch refs/heads/develop\n\n";
        let entries = parse_porcelain_to_entries(porcelain, false);
        assert_eq!(entries.len(), 2);
        assert_eq!(entries[0].branch.as_deref(), Some("task/local-docker"));
        assert!(
            entries[0].is_root,
            "the main working tree holds the root role"
        );
        assert!(!entries[1].is_root);
    }

    #[test]
    fn test_parse_porcelain_keeps_a_detached_main_as_the_root() {
        // The detached main working tree stays, branchless, and holds the
        // root role; the *next* worktree must not inherit it.
        let porcelain = "worktree /repo\nHEAD abc123\ndetached\n\nworktree /repo.develop\nbranch refs/heads/develop\n\n";
        let entries = parse_porcelain_to_entries(porcelain, false);
        assert_eq!(entries.len(), 2);
        assert_eq!(entries[0].branch, None);
        assert_eq!(entries[0].head.as_deref(), Some("abc123"));
        assert!(entries[0].is_root);
        assert_eq!(entries[1].branch.as_deref(), Some("develop"));
        assert!(!entries[1].is_root);
    }

    #[test]
    fn test_detached_linked_worktree_is_still_dropped_in_a_nonbare_repo() {
        let porcelain = "worktree /repo\nbranch refs/heads/main\n\nworktree /repo.sandbox\nHEAD abc123\ndetached\n\n";
        let entries = parse_porcelain_to_entries(porcelain, false);
        assert_eq!(entries.len(), 1);
        assert!(entries[0].is_root);
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
        let source = source_state(
            "/repo/.git",
            false,
            "main",
            "/repo",
            vec![("main", "/repo", true), ("develop", "/repo/develop", false)],
        );
        let target = target(BuiltinLayout::Sibling, &source).unwrap();
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
        let source = source_state(
            "/repo/.git",
            false,
            "master",
            "/repo",
            vec![("task/local-docker", "/repo", true)],
        );
        let target = target(BuiltinLayout::Contained, &source).unwrap();
        assert!(target.is_bare);
        assert!(target.worktrees[0].is_root);
        assert_eq!(
            target.worktrees[0].path,
            PathBuf::from("/repo/task/local-docker")
        );
    }

    #[test]
    fn test_wrapped_git_dir_follows_the_pivot_branch() {
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
        let target = target(BuiltinLayout::ContainedClassic, &source).unwrap();
        // .git goes inside the *pivot's* directory, not master's.
        assert_eq!(target.git_dir, PathBuf::from("/repo/task/x/.git"));
        assert_eq!(target.worktrees[0].path, PathBuf::from("/repo/task/x"));
    }

    #[test]
    fn test_detached_main_refused_for_bare_target_without_a_name() {
        let source = detached_source_state("/repo", "abc123", vec![("develop", "/repo.develop")]);
        let err = target(BuiltinLayout::Contained, &source).unwrap_err();
        assert!(err.to_string().contains("detached HEAD"), "{err}");
        assert!(err.to_string().contains("--as <dir>"), "{err}");

        let err = target(BuiltinLayout::ContainedClassic, &source).unwrap_err();
        assert!(err.to_string().contains("--as <dir>"), "{err}");
    }

    #[test]
    fn test_detached_main_nests_under_the_supplied_dirname() {
        let source = detached_source_state("/repo", "abc123", vec![("develop", "/repo.develop")]);
        let over = PivotOverride {
            branch: None,
            dirname: Some("sandbox".to_string()),
        };
        let t =
            compute_target_state(&BuiltinLayout::Contained.to_layout(), &source, &over).unwrap();
        assert!(t.is_bare);
        assert!(t.worktrees[0].is_root);
        assert_eq!(t.worktrees[0].branch, None);
        assert_eq!(t.worktrees[0].head.as_deref(), Some("abc123"));
        assert_eq!(t.worktrees[0].path, PathBuf::from("/repo/sandbox"));
        assert_eq!(t.worktrees[1].path, PathBuf::from("/repo/develop"));

        // The wrapper layout puts `.git` inside the named directory.
        let t = compute_target_state(&BuiltinLayout::ContainedClassic.to_layout(), &source, &over)
            .unwrap();
        assert_eq!(t.git_dir, PathBuf::from("/repo/sandbox/.git"));
    }

    #[test]
    fn test_detached_main_stays_the_root_for_regular_nonbare_target() {
        // No name needed: the main working tree stays where it is.
        let source = detached_source_state("/repo", "abc123", vec![("develop", "/repo/develop")]);
        let t = target(BuiltinLayout::Sibling, &source).unwrap();
        assert!(t.worktrees[0].is_root);
        assert_eq!(t.worktrees[0].path, PathBuf::from("/repo"));
        assert_eq!(t.worktrees[1].path, PathBuf::from("/repo.develop"));
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
        let target = target(BuiltinLayout::Sibling, &source).unwrap();
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
        let target = target(BuiltinLayout::Sibling, &source).unwrap();
        assert!(target.worktrees[0].is_root);
        assert_eq!(target.worktrees[0].path, PathBuf::from("/repo"));
    }

    #[test]
    fn test_bare_to_nonbare_ambiguous_refuses_naming_pivot() {
        let source = source_state(
            "/repo/.git",
            true,
            "main",
            "/repo",
            vec![("a", "/repo/a", false), ("b", "/repo/b", false)],
        );
        let err = target(BuiltinLayout::Sibling, &source).unwrap_err();
        let msg = err.to_string();
        assert!(msg.contains("'main' has no worktree"), "{msg}");
        assert!(msg.contains("--pivot <branch>"), "{msg}");
        assert!(msg.contains("'a', 'b'"), "{msg}");
    }

    #[test]
    fn test_pivot_override_breaks_the_bare_ambiguity() {
        let source = source_state(
            "/repo/.git",
            true,
            "main",
            "/repo",
            vec![("a", "/repo/a", false), ("b", "/repo/b", false)],
        );
        let over = PivotOverride {
            branch: Some("b".to_string()),
            dirname: None,
        };
        let t = compute_target_state(&BuiltinLayout::Sibling.to_layout(), &source, &over).unwrap();
        assert!(!t.worktrees[0].is_root);
        assert!(t.worktrees[1].is_root);
        assert_eq!(t.worktrees[1].path, PathBuf::from("/repo"));
        assert_eq!(t.worktrees[0].path, PathBuf::from("/repo.a"));
    }

    #[test]
    fn test_pivot_override_beats_the_default_branch() {
        let source = source_state(
            "/repo/.git",
            true,
            "main",
            "/repo",
            vec![("main", "/repo/main", false), ("b", "/repo/b", false)],
        );
        let over = PivotOverride {
            branch: Some("b".to_string()),
            dirname: None,
        };
        let t = compute_target_state(&BuiltinLayout::Sibling.to_layout(), &source, &over).unwrap();
        assert!(t.worktrees[1].is_root, "an explicit --pivot is honoured");
    }

    #[test]
    fn test_pivot_override_naming_no_worktree_refuses() {
        let source = source_state(
            "/repo/.git",
            true,
            "main",
            "/repo",
            vec![("a", "/repo/a", false), ("b", "/repo/b", false)],
        );
        let over = PivotOverride {
            branch: Some("nope".to_string()),
            dirname: None,
        };
        let err =
            compute_target_state(&BuiltinLayout::Sibling.to_layout(), &source, &over).unwrap_err();
        assert!(err.to_string().contains("--pivot nope"), "{err}");
    }

    #[test]
    fn test_bare_to_nonbare_with_no_worktrees_refuses() {
        let source = source_state("/repo/.git", true, "main", "/repo", vec![]);
        let err = target(BuiltinLayout::Sibling, &source).unwrap_err();
        assert!(err.to_string().contains("has none"), "{err}");
        assert!(
            !err.to_string().contains("--pivot"),
            "a flag that cannot help must not be suggested: {err}"
        );
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
        let target = target(BuiltinLayout::ContainedFlat, &source).unwrap();
        assert!(target.worktrees.iter().all(|wt| !wt.is_root));
        assert_eq!(target.worktrees[1].path, PathBuf::from("/repo/f-x"));
    }
}
