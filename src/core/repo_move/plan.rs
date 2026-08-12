//! What a relocation will do — decided before anything moves.
//!
//! Every refusal lives here. The plan is computed from the repo as it stands,
//! printed verbatim by `--dry-run`, and then executed; that is what makes
//! "refuse before anything moves" a property of the code rather than an
//! intention. It is also the only record of where things came from: once the
//! root has moved the repo is unresolvable, so the executor's rollback reads
//! this plan rather than asking git.

use anyhow::{Context, Result, bail};
use std::path::{Path, PathBuf};

use crate::core::layout::Layout;
use crate::core::layout::template::normalize_path;
use crate::core::layout::transform::compute_target_worktree_path;
use crate::core::worktree::remove_repo::{RepoTarget, WorktreeEntry};
use crate::hooks::TrustDatabase;

/// Where a worktree ends up, and why.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Disposition {
    /// Inside the project root: it travels with the root and needs no rename
    /// of its own. Contained, nested and bare layouts put every worktree here.
    CarriedWithRoot,
    /// Outside the root, but exactly where this layout would place it — so it
    /// is part of the constellation and moves too. The default `sibling`
    /// layout puts every worktree here, which is why a move that ignored them
    /// would strand the repo's worktrees in the common case.
    Relocated,
    /// Outside the root and not where the layout would place it: the user put
    /// it there deliberately (`--at`), or it is detached and unpredictable. It
    /// stays. Its git linkage is still repaired, so it keeps working.
    LeftInPlace,
}

/// One worktree's place in the move.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PlannedWorktree {
    pub old_path: PathBuf,
    /// Where it ends up. Equal to `old_path` for [`Disposition::LeftInPlace`].
    pub new_path: PathBuf,
    pub branch: Option<String>,
    pub disposition: Disposition,
}

/// What happens to the catalog name.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum NamePlan {
    /// Leave it alone.
    Keep,
    /// `--name` asked for this.
    Explicit(String),
    /// The name was derived from the old directory's name, so it follows the
    /// directory to the new one.
    FollowsDirectory(String),
    /// The name would have followed the directory, but another repo holds the
    /// new name. Keeping the old one beats failing a move that has to go
    /// through anyway.
    KeptNameTaken { wanted: String, holder: String },
}

impl NamePlan {
    /// The name the catalog should end up with, when it differs from today's.
    pub fn rename_to(&self) -> Option<&str> {
        match self {
            Self::Explicit(name) | Self::FollowsDirectory(name) => Some(name),
            Self::Keep | Self::KeptNameTaken { .. } => None,
        }
    }
}

/// Everything the executor needs, and nothing it would have to re-derive.
#[derive(Debug, Clone)]
pub struct MovePlan {
    pub old_root: PathBuf,
    pub new_root: PathBuf,
    pub old_git_dir: PathBuf,
    pub new_git_dir: PathBuf,
    /// `repos.json` key for the old git dir, captured while that directory
    /// still exists. It cannot be recomputed after the move: `canonicalize`
    /// on a missing path falls back to the literal spelling, which will not
    /// match a key stored through a symlinked parent.
    pub old_trust_key: String,
    pub worktrees: Vec<PlannedWorktree>,
    pub current_name: String,
    pub name: NamePlan,
}

impl MovePlan {
    /// The directory renames, in the order they must happen: the root first,
    /// then each worktree the layout places outside it.
    ///
    /// Worktrees inside the root are absent by construction — they move when
    /// the root does, and renaming them again afterwards would be a bug.
    pub fn directory_moves(&self) -> Vec<(&Path, &Path)> {
        std::iter::once((self.old_root.as_path(), self.new_root.as_path()))
            .chain(
                self.worktrees
                    .iter()
                    .filter(|w| w.disposition == Disposition::Relocated)
                    .map(|w| (w.old_path.as_path(), w.new_path.as_path())),
            )
            .collect()
    }

    /// Every worktree at its post-move location — the argument list for
    /// `git worktree repair`.
    ///
    /// Built from the dispositions rather than from [`Self::directory_moves`]:
    /// a left-in-place worktree is not renamed but its `.git` gitlink still
    /// points into the old git dir, so dropping it here would leave it broken.
    pub fn repair_paths(&self) -> Vec<&Path> {
        self.worktrees
            .iter()
            .map(|w| w.new_path.as_path())
            .collect()
    }

    pub fn relocated(&self) -> impl Iterator<Item = &PlannedWorktree> {
        self.worktrees
            .iter()
            .filter(|w| w.disposition == Disposition::Relocated)
    }

    pub fn left_in_place(&self) -> impl Iterator<Item = &PlannedWorktree> {
        self.worktrees
            .iter()
            .filter(|w| w.disposition == Disposition::LeftInPlace)
    }

    /// Whether `cwd` sits inside something this move relocates, and where it
    /// lands if so. The shell can only be rescued by the parent, so the caller
    /// turns this into `DAFT_CD_FILE`.
    pub fn map_cwd(&self, cwd: &Path) -> Option<PathBuf> {
        // Longest match wins: a worktree inside the root would otherwise be
        // mapped by the root's own (shorter) prefix, which is the same answer
        // here but need not be for future dispositions.
        let mut best: Option<(usize, PathBuf)> = None;
        for (from, to) in self.directory_moves() {
            if let Some(mapped) = rebase(cwd, from, to) {
                let depth = from.components().count();
                if best.as_ref().is_none_or(|(d, _)| depth > *d) {
                    best = Some((depth, mapped));
                }
            }
        }
        best.map(|(_, path)| path)
    }
}

/// Inputs to [`build`], all resolved by the caller.
pub struct MoveRequest<'a> {
    pub target: &'a RepoTarget,
    /// The user's `<dest>`, before `mv` semantics are applied.
    pub dest: &'a Path,
    pub explicit_name: Option<&'a str>,
    /// The repo's current catalog name.
    pub current_name: &'a str,
    /// The layout in force, used to predict where worktrees belong.
    pub layout: &'a Layout,
    /// Live worktrees, enumerated before the move.
    pub worktrees: &'a [WorktreeEntry],
    /// Whether a live catalog entry already claims the name we would move to.
    /// `None` when nothing claims it. Supplied by the caller so planning stays
    /// free of catalog IO.
    pub name_holder: &'a dyn Fn(&str) -> Option<String>,
}

/// Decide the whole move. Refuses rather than half-doing anything.
pub fn build(req: MoveRequest<'_>) -> Result<MovePlan> {
    let old_root = canonical(&req.target.project_root);
    let old_git_dir = canonical(&req.target.bare_git_dir);
    let new_root = resolve_destination(req.dest, &old_root)?;

    if new_root.starts_with(&old_root) {
        bail!(
            "cannot move a repository inside itself\n  \
             {} is under {}",
            new_root.display(),
            old_root.display()
        );
    }
    refuse_across_filesystems(&old_root, &new_root)?;

    // `project_root` is the git dir's parent by construction, so the git dir
    // moves with the root and keeps its own name (`.git` for every layout daft
    // creates, but read it rather than assume it).
    let git_dir_name = old_git_dir
        .file_name()
        .ok_or_else(|| anyhow::anyhow!("{} has no file name", old_git_dir.display()))?;
    let new_git_dir = new_root.join(git_dir_name);

    let worktrees = classify(req.layout, &old_root, &new_root, req.worktrees);
    let name = plan_name(
        req.explicit_name,
        req.current_name,
        &old_root,
        &new_root,
        req.name_holder,
    )?;

    Ok(MovePlan {
        old_trust_key: TrustDatabase::repo_key(&old_git_dir),
        old_root,
        new_root,
        old_git_dir,
        new_git_dir,
        worktrees,
        current_name: req.current_name.to_string(),
        name,
    })
}

/// `mv` semantics: an existing directory means "move into it", anything else
/// names the new root outright.
fn resolve_destination(dest: &Path, old_root: &Path) -> Result<PathBuf> {
    let dest = canonicalize_existing_prefix(&absolutize(dest)?);

    if dest == old_root {
        bail!("{} is already there", old_root.display());
    }
    if dest.is_file() {
        bail!(
            "{} is a file\n  tip: name a directory, or a path that does not exist yet",
            dest.display()
        );
    }

    let target = if dest.is_dir() {
        let basename = old_root.file_name().ok_or_else(|| {
            anyhow::anyhow!("{} has no directory name to move", old_root.display())
        })?;
        dest.join(basename)
    } else {
        dest
    };

    if target.exists() {
        bail!(
            "{} already exists\n  tip: remove it, or name a destination that does not exist yet",
            target.display()
        );
    }
    let parent = target
        .parent()
        .ok_or_else(|| anyhow::anyhow!("{} has no parent directory", target.display()))?;
    if !parent.is_dir() {
        bail!(
            "{} does not exist\n  tip: create it first — like `mv`, this will not build the path for you",
            parent.display()
        );
    }
    Ok(target)
}

/// `fs::rename` cannot cross a filesystem boundary, and a repo half-copied
/// between two of them is worse than a repo that never moved. Refuse in
/// planning, where nothing has happened yet, rather than mid-sequence.
fn refuse_across_filesystems(old_root: &Path, new_root: &Path) -> Result<()> {
    // The destination does not exist yet; its parent does (checked above).
    let Some(dest_parent) = new_root.parent() else {
        return Ok(());
    };
    if same_filesystem(old_root, dest_parent)? {
        return Ok(());
    }
    bail!(
        "{} and {} are on different filesystems\n  \
         a move across filesystems is a copy, and daft will not leave a repository \
         half-copied\n  \
         tip: copy it yourself, then run `{}` from the new location",
        old_root.display(),
        dest_parent.display(),
        crate::daft_cmd("repo add")
    )
}

#[cfg(unix)]
fn same_filesystem(a: &Path, b: &Path) -> Result<bool> {
    use std::os::unix::fs::MetadataExt;
    let a_dev = std::fs::metadata(a)
        .with_context(|| String::from("could not stat ") + &a.display().to_string())?
        .dev();
    let b_dev = std::fs::metadata(b)
        .with_context(|| String::from("could not stat ") + &b.display().to_string())?
        .dev();
    Ok(a_dev == b_dev)
}

/// No `st_dev` off unix; the rename's own `EXDEV` is the backstop there.
#[cfg(not(unix))]
fn same_filesystem(_a: &Path, _b: &Path) -> Result<bool> {
    Ok(true)
}

/// Decide each worktree's fate. See [`Disposition`] for what each case means.
fn classify(
    layout: &Layout,
    old_root: &Path,
    new_root: &Path,
    entries: &[WorktreeEntry],
) -> Vec<PlannedWorktree> {
    entries
        .iter()
        // The bare entry is the repository, not a worktree: it names the
        // project root and has nothing to repair.
        .filter(|e| !e.is_bare)
        .map(|entry| {
            let old_path = canonical(&entry.path);
            let disposition = disposition_of(layout, old_root, &old_path, entry.branch.as_deref());
            let new_path = match disposition {
                Disposition::CarriedWithRoot => {
                    rebase(&old_path, old_root, new_root).unwrap_or_else(|| old_path.clone())
                }
                Disposition::Relocated => entry
                    .branch
                    .as_deref()
                    .and_then(|branch| compute_target_worktree_path(layout, new_root, branch).ok())
                    .unwrap_or_else(|| old_path.clone()),
                Disposition::LeftInPlace => old_path.clone(),
            };
            PlannedWorktree {
                old_path,
                new_path,
                branch: entry.branch.clone(),
                disposition,
            }
        })
        .collect()
}

fn disposition_of(
    layout: &Layout,
    old_root: &Path,
    worktree: &Path,
    branch: Option<&str>,
) -> Disposition {
    // The main worktree of a non-bare repo *is* the project root.
    if worktree == old_root || worktree.starts_with(old_root) {
        return Disposition::CarriedWithRoot;
    }
    // Outside the root: it belongs to the constellation only if this is
    // exactly where the layout would have put it. Predicting from the template
    // rather than from a "looks nearby" heuristic is what keeps a worktree the
    // user placed by hand from being dragged along.
    let Some(branch) = branch else {
        return Disposition::LeftInPlace;
    };
    match compute_target_worktree_path(layout, old_root, branch) {
        Ok(predicted) if canonical(&predicted) == worktree => Disposition::Relocated,
        _ => Disposition::LeftInPlace,
    }
}

/// Decide what the catalog name does.
///
/// `--name` always wins. Otherwise the name follows the directory when it was
/// *derived* from it — the signal being that it still equals the old
/// directory's name. `RegistrationFacts::default_name` cannot answer this:
/// it prefers the remote URL, so for any cloned repo it is stable across a
/// move and would report "derived" for a name the user typed.
fn plan_name(
    explicit: Option<&str>,
    current: &str,
    old_root: &Path,
    new_root: &Path,
    name_holder: &dyn Fn(&str) -> Option<String>,
) -> Result<NamePlan> {
    if let Some(requested) = explicit {
        crate::catalog::normalize::validate_catalog_name(requested)
            .map_err(|reason| anyhow::anyhow!("invalid --name '{requested}': {reason}"))?;
        if requested == current {
            return Ok(NamePlan::Keep);
        }
        if let Some(holder) = name_holder(requested) {
            bail!(
                "the name '{requested}' is already used by the repo at {holder}\n  \
                 tip: pick a different name, or rename that repo first with `{}` from inside it",
                crate::daft_cmd("repo add --name <name>")
            );
        }
        return Ok(NamePlan::Explicit(requested.to_string()));
    }

    let (Some(old_name), Some(new_name)) = (dir_name(old_root), dir_name(new_root)) else {
        return Ok(NamePlan::Keep);
    };
    if current != old_name || new_name == old_name {
        return Ok(NamePlan::Keep);
    }
    if crate::catalog::normalize::validate_catalog_name(&new_name).is_err() {
        return Ok(NamePlan::Keep);
    }
    // A collision must not fail the move: by the time the name is applied the
    // directories have already moved, and refusing would leave the user with a
    // relocated repo and an error.
    match name_holder(&new_name) {
        Some(holder) => Ok(NamePlan::KeptNameTaken {
            wanted: new_name,
            holder,
        }),
        None => Ok(NamePlan::FollowsDirectory(new_name)),
    }
}

fn dir_name(path: &Path) -> Option<String> {
    path.file_name()?.to_str().map(String::from)
}

/// Absolute and lexically normalized, without touching the disk — the same
/// treatment layout templates give a path, so a destination and a
/// layout-derived path are spelled the same way everywhere they surface.
fn absolutize(path: &Path) -> Result<PathBuf> {
    let absolute = if path.is_absolute() {
        path.to_path_buf()
    } else {
        std::env::current_dir()
            .context("could not determine the current directory")?
            .join(path)
    };
    Ok(normalize_path(&absolute))
}

/// Re-root `path` from `from` onto `to`, or `None` if it is not under `from`.
///
/// `Path::join("")` appends a separator, so the case that matters — a path that
/// *is* `from`, which is every non-bare repo's main worktree — would come out
/// as `/new/root/`. Two `Path`s compare equal across that, but the string does
/// not: it is what lands in `DAFT_CD_FILE` and in the recorded worktree path,
/// where nothing else spells a directory with a trailing slash.
fn rebase(path: &Path, from: &Path, to: &Path) -> Option<PathBuf> {
    let rest = path.strip_prefix(from).ok()?;
    Some(if rest.as_os_str().is_empty() {
        to.to_path_buf()
    } else {
        to.join(rest)
    })
}

/// Canonicalize as much of `path` as exists and keep the rest verbatim.
///
/// A destination does not exist yet — that is the point of it — but its parent
/// does, and every comparison here (inside-itself, cwd mapping, prefix tests
/// against the canonical paths git reports) is only sound when both sides are
/// spelled the same way. On macOS the difference is not hypothetical: a temp
/// dir is `/var/...` to the caller and `/private/var/...` once resolved.
fn canonicalize_existing_prefix(path: &Path) -> PathBuf {
    let mut trailing: Vec<std::ffi::OsString> = Vec::new();
    let mut cursor = path.to_path_buf();
    loop {
        if let Ok(resolved) = cursor.canonicalize() {
            let mut out = resolved;
            out.extend(trailing.iter().rev());
            return out;
        }
        match cursor.file_name() {
            Some(name) => trailing.push(name.to_os_string()),
            None => return path.to_path_buf(),
        }
        if !cursor.pop() {
            return path.to_path_buf();
        }
    }
}

/// Canonical spelling when the path exists, its own when it does not.
///
/// Git reports canonicalized paths (`/tmp` → `/private/tmp` on macOS) while a
/// `RepoTarget` carries whatever the caller passed, so both sides of every
/// prefix test have to go through this or an inside-the-root worktree reads as
/// outside it.
fn canonical(path: &Path) -> PathBuf {
    path.canonicalize().unwrap_or_else(|_| path.to_path_buf())
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    const SIBLING: &str = "{{ repo }}.{{ branch | sanitize }}";
    const CONTAINED: &str = "{{ repo_path }}/{{ branch }}";

    fn layout(template: &str) -> Layout {
        Layout {
            name: "test".into(),
            template: template.into(),
            bare: None,
        }
    }

    fn entry(path: &Path, branch: Option<&str>) -> WorktreeEntry {
        WorktreeEntry {
            path: path.to_path_buf(),
            branch: branch.map(String::from),
            is_bare: false,
            is_detached: branch.is_none(),
            head: None,
        }
    }

    /// `<tmp>/Projects/api` with a `.git` inside — the shape `resolve_repo`
    /// hands over, for both bare and non-bare layouts.
    struct Fixture {
        tmp: TempDir,
        root: PathBuf,
        projects: PathBuf,
        target: RepoTarget,
    }

    fn fixture() -> Fixture {
        let tmp = TempDir::new().unwrap();
        let projects = tmp.path().join("Projects");
        let root = projects.join("api");
        std::fs::create_dir_all(root.join(".git")).unwrap();
        let target = RepoTarget {
            bare_git_dir: root.join(".git"),
            project_root: root.clone(),
        };
        Fixture {
            tmp,
            root,
            projects,
            target,
        }
    }

    fn request<'a>(
        fx: &'a Fixture,
        dest: &'a Path,
        layout: &'a Layout,
        worktrees: &'a [WorktreeEntry],
    ) -> MoveRequest<'a> {
        MoveRequest {
            target: &fx.target,
            dest,
            explicit_name: None,
            current_name: "api",
            layout,
            worktrees,
            name_holder: &|_| None,
        }
    }

    // ── mv semantics ─────────────────────────────────────────────────────

    #[test]
    fn a_destination_that_does_not_exist_becomes_the_new_root() {
        let fx = fixture();
        let dest = fx.projects.join("renamed");
        let plan = build(request(&fx, &dest, &layout(SIBLING), &[])).unwrap();
        assert_eq!(plan.new_root, canonical(&fx.projects).join("renamed"));
    }

    #[test]
    fn an_existing_destination_directory_is_moved_into() {
        let fx = fixture();
        let work = fx.tmp.path().join("Work");
        std::fs::create_dir_all(&work).unwrap();
        let plan = build(request(&fx, &work, &layout(SIBLING), &[])).unwrap();
        assert_eq!(plan.new_root, canonical(&work).join("api"));
    }

    #[test]
    fn moving_into_a_directory_that_already_holds_the_name_refuses() {
        let fx = fixture();
        let work = fx.tmp.path().join("Work");
        std::fs::create_dir_all(work.join("api")).unwrap();
        let err = build(request(&fx, &work, &layout(SIBLING), &[])).unwrap_err();
        assert!(err.to_string().contains("already exists"), "{err}");
    }

    #[test]
    fn a_destination_that_is_a_file_refuses() {
        let fx = fixture();
        let file = fx.tmp.path().join("a-file");
        std::fs::write(&file, b"x").unwrap();
        let err = build(request(&fx, &file, &layout(SIBLING), &[])).unwrap_err();
        assert!(err.to_string().contains("is a file"), "{err}");
    }

    #[test]
    fn a_destination_whose_parent_is_missing_refuses_like_mv() {
        let fx = fixture();
        let dest = fx.tmp.path().join("nope/deeper/api");
        let err = build(request(&fx, &dest, &layout(SIBLING), &[])).unwrap_err();
        assert!(err.to_string().contains("does not exist"), "{err}");
    }

    #[test]
    fn moving_a_repo_inside_itself_refuses() {
        let fx = fixture();
        let dest = fx.root.join("inner");
        let err = build(request(&fx, &dest, &layout(SIBLING), &[])).unwrap_err();
        assert!(err.to_string().contains("inside itself"), "{err}");
    }

    #[test]
    fn moving_a_repo_where_it_already_is_refuses() {
        let fx = fixture();
        let root = fx.root.clone();
        let err = build(request(&fx, &root, &layout(SIBLING), &[])).unwrap_err();
        assert!(err.to_string().contains("already there"), "{err}");
    }

    /// The destination is canonicalized as far as it exists, so a path spelled
    /// through a symlinked parent still reads as "inside the repo".
    #[test]
    fn a_destination_spelled_through_a_symlink_is_still_recognised() {
        let fx = fixture();
        let link = fx.tmp.path().join("link-to-projects");
        std::os::unix::fs::symlink(&fx.projects, &link).unwrap();
        let dest = link.join("api/inner");
        let err = build(request(&fx, &dest, &layout(SIBLING), &[])).unwrap_err();
        assert!(err.to_string().contains("inside itself"), "{err}");
    }

    // ── worktree classification ──────────────────────────────────────────

    /// The default layout: worktrees are siblings of the repo directory, so a
    /// move that ignored them would strand every one of them.
    #[test]
    fn sibling_worktrees_are_relocated_under_the_new_directory_name() {
        let fx = fixture();
        let feature = fx.projects.join("api.feature-x");
        std::fs::create_dir_all(&feature).unwrap();
        let worktrees = [
            entry(&fx.root, Some("master")),
            entry(&feature, Some("feature-x")),
        ];
        let work = fx.tmp.path().join("Work");
        std::fs::create_dir_all(&work).unwrap();
        let dest = work.join("api-gateway");

        let plan = build(request(&fx, &dest, &layout(SIBLING), &worktrees)).unwrap();

        let main = &plan.worktrees[0];
        assert_eq!(main.disposition, Disposition::CarriedWithRoot);
        assert_eq!(main.new_path, plan.new_root);

        let moved = &plan.worktrees[1];
        assert_eq!(moved.disposition, Disposition::Relocated);
        assert_eq!(
            moved.new_path,
            canonical(&work).join("api-gateway.feature-x"),
            "the worktree's own name is derived from the repo directory's"
        );
        assert_eq!(
            plan.directory_moves().len(),
            2,
            "the root and the sibling both move"
        );
    }

    #[test]
    fn worktrees_inside_the_root_travel_with_it() {
        let fx = fixture();
        let inside = fx.root.join("feature-x");
        std::fs::create_dir_all(&inside).unwrap();
        let worktrees = [entry(&inside, Some("feature-x"))];
        let dest = fx.projects.join("renamed");

        let plan = build(request(&fx, &dest, &layout(CONTAINED), &worktrees)).unwrap();
        assert_eq!(plan.worktrees[0].disposition, Disposition::CarriedWithRoot);
        assert_eq!(plan.worktrees[0].new_path, plan.new_root.join("feature-x"));
        assert!(
            plan.directory_moves().len() == 1,
            "a carried worktree must not be renamed a second time"
        );
    }

    /// A worktree the user placed by hand is not part of the layout's
    /// constellation, and a repo move must not drag it along.
    #[test]
    fn a_hand_placed_worktree_stays_where_it_is() {
        let fx = fixture();
        let elsewhere = fx.tmp.path().join("scratch/somewhere");
        std::fs::create_dir_all(&elsewhere).unwrap();
        let worktrees = [entry(&elsewhere, Some("feature-x"))];
        let dest = fx.projects.join("renamed");

        let plan = build(request(&fx, &dest, &layout(SIBLING), &worktrees)).unwrap();
        assert_eq!(plan.worktrees[0].disposition, Disposition::LeftInPlace);
        assert_eq!(plan.worktrees[0].new_path, canonical(&elsewhere));
    }

    #[test]
    fn a_detached_worktree_outside_the_root_stays_where_it_is() {
        let fx = fixture();
        let sandbox = fx.projects.join("api.scratch");
        std::fs::create_dir_all(&sandbox).unwrap();
        let worktrees = [entry(&sandbox, None)];
        let dest = fx.projects.join("renamed");

        let plan = build(request(&fx, &dest, &layout(SIBLING), &worktrees)).unwrap();
        assert_eq!(plan.worktrees[0].disposition, Disposition::LeftInPlace);
    }

    #[test]
    fn the_bare_entry_is_not_a_worktree() {
        let fx = fixture();
        let mut bare = entry(&fx.root, None);
        bare.is_bare = true;
        let dest = fx.projects.join("renamed");

        let plan = build(request(&fx, &dest, &layout(SIBLING), &[bare])).unwrap();
        assert!(plan.worktrees.is_empty());
    }

    /// Left-in-place worktrees still need repairing: they did not move, but
    /// the git dir they point into did.
    #[test]
    fn the_repair_list_covers_every_worktree_including_the_ones_left_behind() {
        let fx = fixture();
        let elsewhere = fx.tmp.path().join("scratch/somewhere");
        let sibling = fx.projects.join("api.feature-x");
        std::fs::create_dir_all(&elsewhere).unwrap();
        std::fs::create_dir_all(&sibling).unwrap();
        let worktrees = [
            entry(&fx.root, Some("master")),
            entry(&sibling, Some("feature-x")),
            entry(&elsewhere, Some("odd")),
        ];
        let dest = fx.projects.join("renamed");

        let plan = build(request(&fx, &dest, &layout(SIBLING), &worktrees)).unwrap();
        assert_eq!(plan.repair_paths().len(), 3);
        assert!(
            plan.repair_paths()
                .contains(&canonical(&elsewhere).as_path()),
            "the worktree that stayed put must still be repaired"
        );
    }

    // ── the catalog name ─────────────────────────────────────────────────

    #[test]
    fn a_derived_name_follows_the_directory() {
        let fx = fixture();
        let dest = fx.projects.join("api-gateway");
        let plan = build(request(&fx, &dest, &layout(SIBLING), &[])).unwrap();
        assert_eq!(
            plan.name,
            NamePlan::FollowsDirectory("api-gateway".to_string())
        );
    }

    /// A name the user chose is theirs; the directory changing does not
    /// override it.
    #[test]
    fn a_chosen_name_does_not_follow_the_directory() {
        let fx = fixture();
        let dest = fx.projects.join("api-gateway");
        let lay = layout(SIBLING);
        let mut req = request(&fx, &dest, &lay, &[]);
        req.current_name = "backend";
        let plan = build(req).unwrap();
        assert_eq!(plan.name, NamePlan::Keep);
    }

    #[test]
    fn a_move_that_keeps_the_directory_name_keeps_the_catalog_name() {
        let fx = fixture();
        let work = fx.tmp.path().join("Work");
        std::fs::create_dir_all(&work).unwrap();
        let plan = build(request(&fx, &work, &layout(SIBLING), &[])).unwrap();
        assert_eq!(plan.name, NamePlan::Keep);
    }

    #[test]
    fn explicit_name_wins() {
        let fx = fixture();
        let dest = fx.projects.join("api-gateway");
        let lay = layout(SIBLING);
        let mut req = request(&fx, &dest, &lay, &[]);
        req.explicit_name = Some("chosen");
        let plan = build(req).unwrap();
        assert_eq!(plan.name, NamePlan::Explicit("chosen".to_string()));
    }

    #[test]
    fn an_explicit_name_already_taken_refuses_before_anything_moves() {
        let fx = fixture();
        let dest = fx.projects.join("api-gateway");
        let lay = layout(SIBLING);
        let holder = |name: &str| (name == "taken").then(|| "/elsewhere/other".to_string());
        let mut req = request(&fx, &dest, &lay, &[]);
        req.explicit_name = Some("taken");
        req.name_holder = &holder;
        let err = build(req).unwrap_err();
        assert!(err.to_string().contains("already used"), "{err}");
    }

    #[test]
    fn an_invalid_explicit_name_refuses() {
        let fx = fixture();
        let dest = fx.projects.join("api-gateway");
        let lay = layout(SIBLING);
        let mut req = request(&fx, &dest, &lay, &[]);
        req.explicit_name = Some("-bad");
        let err = build(req).unwrap_err();
        assert!(err.to_string().contains("invalid --name"), "{err}");
    }

    /// A collision on the *derived* name must not fail the move — the
    /// directories still have to go somewhere.
    #[test]
    fn a_taken_derived_name_is_kept_rather_than_refused() {
        let fx = fixture();
        let dest = fx.projects.join("api-gateway");
        let lay = layout(SIBLING);
        let holder = |name: &str| (name == "api-gateway").then(|| "/elsewhere/other".to_string());
        let mut req = request(&fx, &dest, &lay, &[]);
        req.name_holder = &holder;
        let plan = build(req).unwrap();
        assert_eq!(
            plan.name,
            NamePlan::KeptNameTaken {
                wanted: "api-gateway".to_string(),
                holder: "/elsewhere/other".to_string(),
            }
        );
        assert_eq!(plan.name.rename_to(), None);
    }

    // ── the user's shell ─────────────────────────────────────────────────

    #[test]
    fn a_cwd_inside_the_repo_maps_to_the_new_location() {
        let fx = fixture();
        let dest = fx.projects.join("renamed");
        let plan = build(request(&fx, &dest, &layout(SIBLING), &[])).unwrap();
        assert_eq!(
            plan.map_cwd(&plan.old_root.join("src/deep")),
            Some(plan.new_root.join("src/deep"))
        );
    }

    /// The cwd that matters most is the repo root itself, and its mapped
    /// spelling is what lands in `DAFT_CD_FILE`. A naive `join("")` would
    /// write `/new/root/` — equal as a `Path`, wrong as a string.
    #[test]
    fn the_mapped_root_carries_no_trailing_separator() {
        let fx = fixture();
        let dest = fx.projects.join("renamed");
        let plan = build(request(&fx, &dest, &layout(SIBLING), &[])).unwrap();
        let mapped = plan.map_cwd(&plan.old_root).expect("the root maps");
        assert_eq!(mapped, plan.new_root);
        assert!(
            !mapped.to_string_lossy().ends_with('/'),
            "mapped root must not gain a trailing separator: {}",
            mapped.display()
        );
    }

    /// Same artifact, same fix, on the path recorded for the main worktree.
    #[test]
    fn the_carried_main_worktree_path_carries_no_trailing_separator() {
        let fx = fixture();
        let worktrees = [entry(&fx.root, Some("master"))];
        let dest = fx.projects.join("renamed");
        let plan = build(request(&fx, &dest, &layout(SIBLING), &worktrees)).unwrap();
        let recorded = plan.worktrees[0].new_path.to_string_lossy().to_string();
        assert!(
            !recorded.ends_with('/'),
            "recorded worktree path must not gain a trailing separator: {recorded}"
        );
    }

    #[test]
    fn a_cwd_outside_the_move_is_left_alone() {
        let fx = fixture();
        let dest = fx.projects.join("renamed");
        let plan = build(request(&fx, &dest, &layout(SIBLING), &[])).unwrap();
        assert_eq!(plan.map_cwd(Path::new("/somewhere/else")), None);
    }
}
