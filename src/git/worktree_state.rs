//! Linked-worktree registrations, and the relocation of per-worktree git state.
//!
//! A main working tree and a linked worktree differ only in where their
//! private git state lives. The main working tree keeps it directly in the
//! common `.git/`, intermixed with the shared objects, refs and config; a
//! linked worktree keeps it in `<common>/worktrees/<name>/` — the
//! *registration* — which also holds two bookkeeping files (`gitdir`, the
//! absolute path of the worktree's `.git` pointer file, and `commondir`, the
//! relative path back to the common dir) and an optional `locked` marker.
//!
//! Changing a working tree's role therefore never needs to rebuild anything:
//! [`attach_worktree`] and [`detach_worktree`] *move* the private state between
//! the two homes. The index keeps its worktree-relative paths and, because a
//! same-volume rename leaves inodes and mtimes alone, its stat cache; staged,
//! intent-to-add and unmerged entries, `ORIG_HEAD`, the HEAD reflog and the
//! rest travel with it. Every step is journalled in a [`Relocation`] so a
//! failure part-way through can be undone exactly.
//!
//! The per-worktree set is git's own (gitrepository-layout(5); verified with
//! `git rev-parse --git-path` against git 2.55). Everything not in git's
//! common-path table is per-worktree by default, but the main→linked direction
//! still uses a positive list: the source of that direction is the common dir
//! itself, and moving the wrong entry (`objects/`, `refs/heads/`, `config`)
//! would gut the repository. The linked→main direction needs no list — a
//! registration holds nothing but per-worktree state.

use std::fs;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result};

/// Per-worktree files (relative to the private git dir).
pub const PER_WORKTREE_FILES: &[&str] = &[
    "HEAD",
    "index",
    "ORIG_HEAD",
    "FETCH_HEAD",
    "COMMIT_EDITMSG",
    "MERGE_HEAD",
    "MERGE_MSG",
    "MERGE_MODE",
    "MERGE_RR",
    "MERGE_AUTOSTASH",
    "AUTO_MERGE",
    "REBASE_HEAD",
    "CHERRY_PICK_HEAD",
    "REVERT_HEAD",
    "BISECT_LOG",
    "BISECT_START",
    "BISECT_EXPECTED_REV",
    "BISECT_ANCESTORS_OK",
    "BISECT_NAMES",
    "BISECT_RUN",
    "BISECT_TERMS",
    "BISECT_FIRST_PARENT",
    "SQUASH_MSG",
    "TAG_EDITMSG",
    "logs/HEAD",
    "info/sparse-checkout",
    "config.worktree",
];

/// Per-worktree directories, moved wholesale.
pub const PER_WORKTREE_DIRS: &[&str] = &[
    "rebase-merge",
    "rebase-apply",
    "sequencer",
    "refs/bisect",
    "refs/worktree",
    "refs/rewritten",
];

/// Directories that hold *both* per-worktree and common entries. A relocation
/// descends one level into these and moves leaves: `logs/HEAD` is per-worktree
/// while `logs/refs/` is common, so renaming `logs/` wholesale would collide.
const SPLIT_DIRS: &[&str] = &["logs", "refs", "info"];

/// Registration bookkeeping that never relocates: `gitdir` and `commondir`
/// are rewritten by [`attach_worktree`]; `locked` is a statement about a
/// registration that is about to stop existing.
const REGISTRATION_ONLY: &[&str] = &["gitdir", "commondir", "locked"];

/// Files git rewrites wholesale on the next relevant command. A copy already
/// at the destination is stale by definition and may be replaced.
const SCRATCH_FILES: &[&str] = &["FETCH_HEAD", "COMMIT_EDITMSG", "TAG_EDITMSG"];

/// Lock files whose presence means another git process is at work.
const LOCK_FILES: &[&str] = &["index.lock", "HEAD.lock"];

// ── Journal ──────────────────────────────────────────────────────────────

/// One reversible step of a relocation.
#[derive(Debug)]
enum Step {
    /// `from` was renamed to `to`. Undo renames it back.
    Renamed { from: PathBuf, to: PathBuf },
    /// `path` was written (created or overwritten). Undo restores `prior`, or
    /// removes the file when there was none.
    Wrote {
        path: PathBuf,
        prior: Option<Vec<u8>>,
    },
    /// `src`'s bytes were appended to `dest` and `src` removed. Undo truncates
    /// `dest` back to `original_len` and recreates `src`.
    Appended {
        dest: PathBuf,
        original_len: u64,
        src: PathBuf,
        bytes: Vec<u8>,
    },
    /// A directory was created. Undo removes it if it is empty by then.
    CreatedDir { path: PathBuf },
    /// A directory tree was removed; `files` are the regular files it still
    /// held, relative to `path`. Undo recreates them.
    RemovedTree {
        path: PathBuf,
        files: Vec<(PathBuf, Vec<u8>)>,
    },
}

/// Journal of one relocation, so a failure part-way through can be undone
/// exactly: every rename, write and removal, newest last.
#[must_use = "an un-undone relocation leaves the repository half-moved"]
#[derive(Debug, Default)]
pub struct Relocation {
    steps: Vec<Step>,
    /// Relative names of the state entries that moved, for progress output.
    moved: Vec<PathBuf>,
}

impl Relocation {
    /// Put everything back, newest step first. Best-effort: keeps going after
    /// a failure and returns the first error, because the caller is already
    /// on an error path and every step undone is one less to repair by hand.
    pub fn undo(self) -> Result<()> {
        let mut first_error: Option<anyhow::Error> = None;
        for step in self.steps.into_iter().rev() {
            if let Err(e) = undo_step(step)
                && first_error.is_none()
            {
                first_error = Some(e);
            }
        }
        match first_error {
            Some(e) => Err(e),
            None => Ok(()),
        }
    }

    /// The per-worktree entries this relocation carried, as names relative
    /// to the git dir (`index`, `logs/HEAD`, `rebase-merge`, …).
    pub fn moved(&self) -> impl Iterator<Item = &Path> {
        self.moved.iter().map(PathBuf::as_path)
    }

    /// Number of per-worktree entries carried.
    pub fn carried(&self) -> usize {
        self.moved.len()
    }

    fn record(&mut self, step: Step) {
        self.steps.push(step);
    }
}

fn undo_step(step: Step) -> Result<()> {
    match step {
        Step::Renamed { from, to } => {
            if let Some(parent) = from.parent() {
                fs::create_dir_all(parent)?;
            }
            fs::rename(&to, &from)
                .with_context(|| format!("undo: {} -> {}", to.display(), from.display()))
        }
        Step::Wrote { path, prior } => match prior {
            Some(bytes) => {
                fs::write(&path, bytes).with_context(|| format!("undo: restore {}", path.display()))
            }
            None => match fs::remove_file(&path) {
                Ok(()) => Ok(()),
                Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(()),
                Err(e) => Err(e).with_context(|| format!("undo: remove {}", path.display())),
            },
        },
        Step::Appended {
            dest,
            original_len,
            src,
            bytes,
        } => {
            let f = fs::OpenOptions::new()
                .write(true)
                .open(&dest)
                .with_context(|| format!("undo: open {}", dest.display()))?;
            f.set_len(original_len)
                .with_context(|| format!("undo: truncate {}", dest.display()))?;
            if let Some(parent) = src.parent() {
                fs::create_dir_all(parent)?;
            }
            fs::write(&src, bytes).with_context(|| format!("undo: recreate {}", src.display()))
        }
        Step::CreatedDir { path } => {
            // Only if empty by now — everything written inside it has already
            // been undone (newest first), so a non-empty dir means something
            // outside this journal put a file there. Leave it.
            let _ = fs::remove_dir(&path);
            Ok(())
        }
        Step::RemovedTree { path, files } => {
            fs::create_dir_all(&path)?;
            for (rel, bytes) in files {
                let p = path.join(rel);
                if let Some(parent) = p.parent() {
                    fs::create_dir_all(parent)?;
                }
                fs::write(&p, bytes).with_context(|| format!("undo: recreate {}", p.display()))?;
            }
            Ok(())
        }
    }
}

// ── Collision policy ──────────────────────────────────────────────────────

/// What to do when a per-worktree entry already exists at the destination.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Collision {
    /// Scratch file — replace it (the prior bytes are journalled).
    Replace,
    /// The HEAD reflog is append-only: concatenate, destination first.
    Append,
    /// Anything else at the destination means the repository is not in the
    /// state the plan was built from. Refuse before moving anything.
    Refuse,
}

fn collision_policy(rel: &Path) -> Collision {
    let name = rel.to_string_lossy();
    if SCRATCH_FILES.contains(&name.as_ref()) {
        Collision::Replace
    } else if name == "logs/HEAD" {
        Collision::Append
    } else {
        Collision::Refuse
    }
}

// ── Public API ───────────────────────────────────────────────────────────

/// Outcome of [`attach_worktree`].
#[derive(Debug)]
pub struct Attached {
    /// The registration directory that now holds the worktree's state.
    pub registration: PathBuf,
    /// Everything that was done, for undo.
    pub relocation: Relocation,
}

/// Outcome of [`detach_worktree`].
#[derive(Debug)]
pub struct Detached {
    /// The registration directory that was dissolved.
    pub registration: PathBuf,
    /// Everything that was done, for undo.
    pub relocation: Relocation,
}

/// Make `worktree_path` a linked worktree of the repository whose common dir
/// is `common_dir`, **carrying** the main working tree's private git state
/// into the new registration instead of rebuilding it.
///
/// Preconditions, all checked before anything is touched:
/// - `core.bare` is `true` in `common_dir/config` — the role change recorded
///   where git records it. Relocating state out of a *non-bare* common dir
///   would gut a live main working tree, so this is refused outright.
/// - No `index.lock`/`HEAD.lock` under `common_dir` (another git process).
/// - `worktree_path` is a directory with no `.git` entry of its own.
///
/// Order of operations is chosen so that a crash at any point leaves a
/// repository git can still open and `git worktree prune` cannot damage:
/// `gitdir`/`commondir` and the worktree's `.git` pointer are written *before*
/// any state moves (so the registration is never prunable while it holds the
/// user's index), `HEAD` is copied rather than renamed (both dirs hold one at
/// every instant), and `index` moves last. The common dir ends with a `HEAD`
/// of its own: `ref: refs/heads/<bare_head_branch>` when that branch exists,
/// otherwise the symref the working tree had, otherwise the first local
/// branch, otherwise the detached oid verbatim — a bare repository with a
/// detached HEAD is legal.
pub fn attach_worktree(
    common_dir: &Path,
    worktree_path: &Path,
    branch: Option<&str>,
    bare_head_branch: &str,
) -> Result<Attached> {
    // ── Preflight ──
    if !is_bare(common_dir)? {
        anyhow::bail!(
            "refusing to attach {}: {} is not a bare repository (core.bare is not true), \
             so its HEAD and index still belong to a main working tree",
            worktree_path.display(),
            common_dir.display()
        );
    }
    refuse_locks(common_dir)?;
    if !worktree_path.is_dir() {
        anyhow::bail!(
            "refusing to attach {}: not a directory",
            worktree_path.display()
        );
    }
    let pointer = worktree_path.join(".git");
    if pointer.symlink_metadata().is_ok() {
        anyhow::bail!(
            "refusing to attach {}: it already has a .git entry",
            worktree_path.display()
        );
    }
    let common_head = common_dir.join("HEAD");
    let head_bytes = fs::read(&common_head)
        .with_context(|| format!("{} has no HEAD to carry", common_dir.display()))?;

    let mut journal = Relocation::default();
    let result = (|| -> Result<PathBuf> {
        // ── Registration scaffolding, before any state moves ──
        let worktrees_root = common_dir.join("worktrees");
        let name = registration_name(&worktrees_root, worktree_path, branch)?;
        let registration = worktrees_root.join(&name);
        create_dir_journalled(&worktrees_root, &mut journal)?;
        create_dir_journalled(&registration, &mut journal)?;
        write_journalled(
            &registration.join("gitdir"),
            format!("{}\n", pointer.display()).as_bytes(),
            &mut journal,
        )?;
        write_journalled(&registration.join("commondir"), b"../..\n", &mut journal)?;
        // HEAD is copied, never renamed: both dirs hold one at every step.
        write_journalled(&registration.join("HEAD"), &head_bytes, &mut journal)?;
        write_journalled(
            &pointer,
            format!("gitdir: {}\n", registration.display()).as_bytes(),
            &mut journal,
        )?;

        // ── Carry the state ──
        let mut rels: Vec<PathBuf> = Vec::new();
        for f in PER_WORKTREE_FILES {
            if *f == "HEAD" || *f == "index" {
                continue;
            }
            rels.push(PathBuf::from(f));
        }
        for d in PER_WORKTREE_DIRS {
            rels.push(PathBuf::from(d));
        }
        rels.push(PathBuf::from("index")); // last: the crash-window guarantee
        let present: Vec<PathBuf> = rels
            .into_iter()
            .filter(|rel| common_dir.join(rel).symlink_metadata().is_ok())
            .collect();
        refuse_collisions(&present, &registration)?;
        for rel in &present {
            move_entry(rel, common_dir, &registration, &mut journal)?;
        }

        // ── Per-worktree config loses the role-bound keys ──
        strip_role_config(&registration.join("config.worktree"), &mut journal)?;
        // A `core.worktree` in the shared config would point the bare repo at
        // a working tree it no longer has.
        unset_config_key(&common_dir.join("config"), "core.worktree", &mut journal)?;

        // ── The bare repository's own HEAD ──
        let bare_head = bare_head_content(common_dir, bare_head_branch, &head_bytes);
        write_journalled(&common_head, bare_head.as_bytes(), &mut journal)?;
        Ok(registration)
    })();

    match result {
        Ok(registration) => Ok(Attached {
            registration,
            relocation: journal,
        }),
        Err(e) => {
            let undo = journal.undo();
            Err(match undo {
                Ok(()) => e,
                Err(u) => e.context(format!("and undoing the partial attach failed: {u:#}")),
            })
        }
    }
}

/// Dissolve `worktree_path`'s registration, **carrying** its private git
/// state into `common_dir`, which is about to become a main working tree's
/// `.git`.
///
/// Same `core.bare == true` precondition and lock check as
/// [`attach_worktree`] (the flag flips *after* the state has moved, so the
/// invariant holds at both relocation sites). The registration directory is
/// removed only after every entry it held has been moved out; the journal can
/// still put them back.
pub fn detach_worktree(common_dir: &Path, worktree_path: &Path) -> Result<Detached> {
    let Some(registration) = find_registration(common_dir, worktree_path) else {
        anyhow::bail!(
            "No worktree registration found for {}. The repository layout changed since \
             the plan was built — re-run `{}` to rebuild it.",
            worktree_path.display(),
            crate::daft_cmd("layout transform")
        );
    };
    if !is_bare(common_dir)? {
        anyhow::bail!(
            "refusing to detach {}: {} is not a bare repository (core.bare is not true), \
             so a main working tree already owns its HEAD and index",
            worktree_path.display(),
            common_dir.display()
        );
    }
    refuse_locks(common_dir)?;
    refuse_locks(&registration)?;
    let head_bytes = fs::read(registration.join("HEAD"))
        .with_context(|| format!("{} has no HEAD to carry", registration.display()))?;

    // Everything in the registration is per-worktree except the bookkeeping.
    let present = registration_entries(&registration)?;
    refuse_collisions(&present, common_dir)?;

    let mut journal = Relocation::default();
    let result = (|| -> Result<()> {
        for rel in &present {
            move_entry(rel, &registration, common_dir, &mut journal)?;
        }
        // HEAD: copy over the bare repository's (journalled), never rename.
        write_journalled(&common_dir.join("HEAD"), &head_bytes, &mut journal)?;
        strip_role_config(&common_dir.join("config.worktree"), &mut journal)?;

        // The worktree's pointer file — exact inverse of the attach step.
        let pointer = worktree_path.join(".git");
        if pointer.is_file() {
            let prior = fs::read(&pointer)?;
            fs::remove_file(&pointer).with_context(|| format!("removing {}", pointer.display()))?;
            journal.record(Step::Wrote {
                path: pointer,
                prior: Some(prior),
            });
        }

        // Finally the registration itself, with what little it still holds.
        let mut files = Vec::new();
        for entry in fs::read_dir(&registration)? {
            let entry = entry?;
            if entry.file_type()?.is_file() {
                files.push((PathBuf::from(entry.file_name()), fs::read(entry.path())?));
            }
        }
        fs::remove_dir_all(&registration)
            .with_context(|| format!("removing registration {}", registration.display()))?;
        journal.record(Step::RemovedTree {
            path: registration.clone(),
            files,
        });
        Ok(())
    })();

    match result {
        Ok(()) => Ok(Detached {
            registration,
            relocation: journal,
        }),
        Err(e) => {
            let undo = journal.undo();
            Err(match undo {
                Ok(()) => e,
                Err(u) => e.context(format!("and undoing the partial detach failed: {u:#}")),
            })
        }
    }
}

/// Find the `<common>/worktrees/<name>` registration that belongs to
/// `worktree_path`.
///
/// The name cannot be derived from the branch: `git worktree add` names the
/// registration after the *path basename* (`R/task/x` -> `x`) and
/// disambiguates collisions with a numeric suffix. Its `gitdir` file is the
/// authoritative back-pointer.
pub fn find_registration(common_dir: &Path, worktree_path: &Path) -> Option<PathBuf> {
    let worktrees_dir = common_dir.join("worktrees");
    let wanted_dir = canonical_or_owned(worktree_path);
    let wanted_file = canonical_or_owned(&worktree_path.join(".git"));

    for entry in fs::read_dir(&worktrees_dir).ok()? {
        let Ok(entry) = entry else { continue };
        if !entry.file_type().map(|t| t.is_dir()).unwrap_or(false) {
            continue;
        }

        let Ok(recorded) = fs::read_to_string(entry.path().join("gitdir")) else {
            continue;
        };
        let recorded = PathBuf::from(recorded.trim());
        if recorded.as_os_str().is_empty() {
            continue;
        }

        if canonical_or_owned(&recorded) == wanted_file {
            return Some(entry.path());
        }
        // The `.git` pointer file may already be gone (a collapse removes it);
        // fall back to the directory that holds it.
        if let Some(parent) = recorded.parent()
            && canonical_or_owned(parent) == wanted_dir
        {
            return Some(entry.path());
        }
    }

    None
}

/// Pick a free registration directory name for `worktree_path`.
///
/// Named the way `git worktree add` names them — after the path basename, with
/// a numeric suffix on collision. Deriving the name from the branch instead
/// (`task/x` -> `task-x`) can land on an unrelated worktree whose directory
/// happens to carry that name and silently overwrite its `gitdir`/`HEAD`.
pub fn registration_name(
    worktrees_root: &Path,
    worktree_path: &Path,
    branch: Option<&str>,
) -> Result<String> {
    let base = worktree_path
        .file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .filter(|n| !n.is_empty())
        .unwrap_or_else(|| {
            branch
                .map(|b| b.replace('/', "-"))
                .unwrap_or_else(|| "detached".to_string())
        });

    if !worktrees_root.join(&base).exists() {
        return Ok(base);
    }
    for n in 1..1000 {
        let candidate = format!("{base}{n}");
        if !worktrees_root.join(&candidate).exists() {
            return Ok(candidate);
        }
    }
    // Falling back to `base` here would hand back a name that is taken, and the
    // caller would overwrite that registration's `gitdir`/`HEAD` — the exact
    // silent clobber this function exists to prevent.
    anyhow::bail!(
        "Could not find a free worktree registration name for {} under {} \
         (tried '{base}' and '{base}1'..'{base}999').",
        worktree_path.display(),
        worktrees_root.display()
    )
}

/// `core.bare` as recorded in `<common_dir>/config`.
pub fn is_bare(common_dir: &Path) -> Result<bool> {
    let config = common_dir.join("config");
    let out = crate::utils::git_command_at(common_dir)
        .args(["config", "--file"])
        .arg(&config)
        .args(["--bool", "--get", "core.bare"])
        .output()
        .with_context(|| format!("reading core.bare from {}", config.display()))?;
    // Exit 1: the key is unset — git's default for a repository with a
    // working tree in `.git/..` is non-bare.
    Ok(out.status.success() && String::from_utf8_lossy(&out.stdout).trim() == "true")
}

// ── Internals ────────────────────────────────────────────────────────────

fn refuse_locks(dir: &Path) -> Result<()> {
    for lock in LOCK_FILES {
        let p = dir.join(lock);
        if p.exists() {
            anyhow::bail!(
                "{} exists — another git process is using this repository (or a crashed one \
                 left the lock behind); let it finish, or remove the file if nothing is running",
                p.display()
            );
        }
    }
    Ok(())
}

/// The per-worktree entries a registration holds, as relative paths: every
/// top-level entry except the bookkeeping and HEAD, descending one level into
/// the split directories.
fn registration_entries(registration: &Path) -> Result<Vec<PathBuf>> {
    let mut rels: Vec<PathBuf> = Vec::new();
    let mut index: Option<PathBuf> = None;
    for entry in
        fs::read_dir(registration).with_context(|| format!("reading {}", registration.display()))?
    {
        let entry = entry?;
        let name = entry.file_name();
        let name_str = name.to_string_lossy().into_owned();
        if name_str == "HEAD" || REGISTRATION_ONLY.contains(&name_str.as_str()) {
            continue;
        }
        if name_str == "index" {
            index = Some(PathBuf::from(name));
            continue;
        }
        if entry.file_type()?.is_dir() && SPLIT_DIRS.contains(&name_str.as_str()) {
            for child in fs::read_dir(entry.path())? {
                let child = child?;
                rels.push(PathBuf::from(&name).join(child.file_name()));
            }
        } else {
            rels.push(PathBuf::from(name));
        }
    }
    rels.sort();
    if let Some(index) = index {
        rels.push(index); // last
    }
    Ok(rels)
}

fn refuse_collisions(rels: &[PathBuf], dest_dir: &Path) -> Result<()> {
    let blocking: Vec<String> = rels
        .iter()
        .filter(|rel| collision_policy(rel) == Collision::Refuse)
        .filter(|rel| dest_dir.join(rel).symlink_metadata().is_ok())
        .map(|rel| rel.display().to_string())
        .collect();
    if !blocking.is_empty() {
        anyhow::bail!(
            "refusing to relocate worktree state: {} already exists in {} — the repository \
             is not in the state the plan was built from",
            blocking.join(", "),
            dest_dir.display()
        );
    }
    Ok(())
}

/// Move one per-worktree entry from `from_dir` to `to_dir`, honouring the
/// collision policy, journalling every step.
fn move_entry(rel: &Path, from_dir: &Path, to_dir: &Path, journal: &mut Relocation) -> Result<()> {
    let src = from_dir.join(rel);
    let dest = to_dir.join(rel);
    if let Some(parent) = dest.parent() {
        create_dir_all_journalled(parent, journal)?;
    }
    let dest_exists = dest.symlink_metadata().is_ok();
    match (dest_exists, collision_policy(rel)) {
        (false, _) => {
            fs::rename(&src, &dest)
                .with_context(|| format!("moving {} -> {}", src.display(), dest.display()))?;
            journal.record(Step::Renamed {
                from: src,
                to: dest,
            });
        }
        (true, Collision::Replace) => {
            let prior = fs::read(&dest).ok();
            journal.record(Step::Wrote {
                path: dest.clone(),
                prior,
            });
            fs::rename(&src, &dest)
                .with_context(|| format!("replacing {} with {}", dest.display(), src.display()))?;
            journal.record(Step::Renamed {
                from: src,
                to: dest,
            });
        }
        (true, Collision::Append) => {
            let bytes = fs::read(&src)?;
            let original_len = fs::metadata(&dest)?.len();
            {
                use std::io::Write;
                let mut f = fs::OpenOptions::new().append(true).open(&dest)?;
                f.write_all(&bytes)?;
            }
            fs::remove_file(&src)?;
            journal.record(Step::Appended {
                dest,
                original_len,
                src,
                bytes,
            });
        }
        (true, Collision::Refuse) => {
            // `refuse_collisions` ran first; reaching here means the tree
            // changed under us.
            anyhow::bail!(
                "{} appeared at the destination while relocating",
                dest.display()
            );
        }
    }
    journal.moved.push(rel.to_path_buf());
    Ok(())
}

fn create_dir_journalled(path: &Path, journal: &mut Relocation) -> Result<()> {
    if path.is_dir() {
        return Ok(());
    }
    fs::create_dir(path).with_context(|| format!("creating {}", path.display()))?;
    journal.record(Step::CreatedDir {
        path: path.to_path_buf(),
    });
    Ok(())
}

fn create_dir_all_journalled(path: &Path, journal: &mut Relocation) -> Result<()> {
    let mut missing = Vec::new();
    let mut cursor = Some(path);
    while let Some(dir) = cursor {
        if dir.is_dir() {
            break;
        }
        missing.push(dir.to_path_buf());
        cursor = dir.parent();
    }
    for dir in missing.into_iter().rev() {
        create_dir_journalled(&dir, journal)?;
    }
    Ok(())
}

fn write_journalled(path: &Path, bytes: &[u8], journal: &mut Relocation) -> Result<()> {
    let prior = fs::read(path).ok();
    fs::write(path, bytes).with_context(|| format!("writing {}", path.display()))?;
    journal.record(Step::Wrote {
        path: path.to_path_buf(),
        prior,
    });
    Ok(())
}

/// Drop `core.bare` and `core.worktree` from a relocated `config.worktree`:
/// both are meaningless-or-harmful in the new role (`core.bare` belongs in
/// the shared config, and `core.worktree` names a path that just changed).
/// When `extensions.worktreeConfig` is off git ignores the file entirely, so
/// this costs nothing where it does not apply.
fn strip_role_config(config_worktree: &Path, journal: &mut Relocation) -> Result<()> {
    if !config_worktree.is_file() {
        return Ok(());
    }
    unset_config_key(config_worktree, "core.bare", journal)?;
    unset_config_key(config_worktree, "core.worktree", journal)
}

/// `git config --file <file> --unset <key>`, journalled. Exit 5 (no such
/// key) is success.
fn unset_config_key(file: &Path, key: &str, journal: &mut Relocation) -> Result<()> {
    if !file.is_file() {
        return Ok(());
    }
    let prior = fs::read(file)?;
    let out = crate::utils::git_command_at(file.parent().unwrap_or(Path::new(".")))
        .args(["config", "--file"])
        .arg(file)
        .args(["--unset", key])
        .output()
        .with_context(|| format!("unsetting {key} in {}", file.display()))?;
    match out.status.code() {
        Some(0) => {
            journal.record(Step::Wrote {
                path: file.to_path_buf(),
                prior: Some(prior),
            });
            Ok(())
        }
        Some(5) => Ok(()),
        _ => anyhow::bail!(
            "could not unset {key} in {}: {}",
            file.display(),
            String::from_utf8_lossy(&out.stderr).trim()
        ),
    }
}

/// The HEAD a bare repository is left with after its working tree moved out.
fn bare_head_content(common_dir: &Path, preferred_branch: &str, moved_head: &[u8]) -> String {
    let branch_exists = |b: &str| {
        crate::utils::git_command_at(common_dir)
            .args([
                "show-ref",
                "--verify",
                "--quiet",
                &format!("refs/heads/{b}"),
            ])
            .output()
            .map(|o| o.status.success())
            .unwrap_or(false)
    };
    if !preferred_branch.is_empty() && branch_exists(preferred_branch) {
        return format!("ref: refs/heads/{preferred_branch}\n");
    }
    let moved = String::from_utf8_lossy(moved_head).trim().to_string();
    if moved.starts_with("ref: ") {
        return format!("{moved}\n");
    }
    if let Ok(out) = crate::utils::git_command_at(common_dir)
        .args([
            "for-each-ref",
            "--count=1",
            "--format=%(refname)",
            "refs/heads/",
        ])
        .output()
        && out.status.success()
    {
        let first = String::from_utf8_lossy(&out.stdout).trim().to_string();
        if !first.is_empty() {
            return format!("ref: {first}\n");
        }
    }
    // A detached main working tree with no branches at all — keep the oid;
    // a bare repository with a detached HEAD is legal.
    format!("{moved}\n")
}

/// Canonicalize when the path exists, otherwise keep it verbatim — enough to
/// see through macOS's `/tmp` -> `/private/tmp` symlink without failing on
/// paths a transform has already moved.
fn canonical_or_owned(path: &Path) -> PathBuf {
    path.canonicalize().unwrap_or_else(|_| path.to_path_buf())
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    /// Run git in `dir` with a fixed test identity — never global config.
    fn git_ok(dir: &Path, args: &[&str]) -> String {
        let out = crate::utils::git_command_at(dir)
            .args(args)
            .env("GIT_AUTHOR_NAME", "Test")
            .env("GIT_AUTHOR_EMAIL", "test@test.com")
            .env("GIT_COMMITTER_NAME", "Test")
            .env("GIT_COMMITTER_EMAIL", "test@test.com")
            .output()
            .expect("git command");
        assert!(
            out.status.success(),
            "git {args:?} failed in {}: {}",
            dir.display(),
            String::from_utf8_lossy(&out.stderr)
        );
        String::from_utf8_lossy(&out.stdout).into_owned()
    }

    fn write(path: &Path, contents: impl AsRef<[u8]>) {
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent).unwrap();
        }
        fs::write(path, contents).unwrap();
    }

    /// A fake common dir: enough of a `.git/` for the relocation to act on
    /// without a real repository — `config` with the bare flag, a HEAD, and
    /// whatever per-worktree files a test plants.
    fn fake_common(bare: bool) -> (TempDir, PathBuf) {
        let tmp = tempfile::tempdir().unwrap();
        let common = tmp.path().join(".git");
        write(&common.join("config"), format!("[core]\n\tbare = {bare}\n"));
        write(&common.join("HEAD"), "ref: refs/heads/feature/x\n");
        fs::create_dir_all(common.join("objects")).unwrap();
        fs::create_dir_all(common.join("refs/heads")).unwrap();
        write(
            &common.join("refs/heads/feature/x"),
            "0123456789abcdef0123456789abcdef01234567\n",
        );
        write(&common.join("config.worktree"), "");
        fs::remove_file(common.join("config.worktree")).unwrap();
        (tmp, common)
    }

    fn plant_state(common: &Path) {
        write(&common.join("index"), "DIRC-fake-index");
        write(&common.join("ORIG_HEAD"), "aaaa\n");
        write(
            &common.join("FETCH_HEAD"),
            "bbbb\tbranch 'main' of origin\n",
        );
        write(
            &common.join("logs/HEAD"),
            "0 1 Test <t@t> 1 +0000\tcommit: one\n",
        );
        write(
            &common.join("logs/refs/heads/feature/x"),
            "reflog for the branch\n",
        );
        write(&common.join("info/exclude"), "*.o\n");
        write(&common.join("info/sparse-checkout"), "/src/\n");
        write(&common.join("refs/bisect/bad"), "cccc\n");
        write(
            &common.join("rebase-merge/head-name"),
            "refs/heads/feature/x\n",
        );
        write(&common.join("packed-refs"), "# pack-refs\n");
    }

    fn worktree_dir(tmp: &TempDir) -> PathBuf {
        let wt = tmp.path().join("feature/x");
        fs::create_dir_all(&wt).unwrap();
        wt
    }

    // ── attach ───────────────────────────────────────────────────────────

    #[test]
    fn attach_moves_the_positive_list_and_leaves_common_paths_alone() {
        let (tmp, common) = fake_common(true);
        plant_state(&common);
        let wt = worktree_dir(&tmp);

        let attached = attach_worktree(&common, &wt, Some("feature/x"), "main").unwrap();
        let reg = &attached.registration;
        assert_eq!(reg, &common.join("worktrees/x"));

        for moved in [
            "index",
            "ORIG_HEAD",
            "FETCH_HEAD",
            "logs/HEAD",
            "info/sparse-checkout",
            "refs/bisect/bad",
            "rebase-merge/head-name",
        ] {
            assert!(
                reg.join(moved).exists(),
                "{moved} must be in the registration"
            );
            assert!(
                !common.join(moved).exists(),
                "{moved} must have left the common dir"
            );
        }
        for stayed in [
            "config",
            "objects",
            "refs/heads/feature/x",
            "logs/refs/heads/feature/x",
            "info/exclude",
            "packed-refs",
        ] {
            assert!(
                common.join(stayed).exists(),
                "{stayed} is common and must stay"
            );
        }
        assert_eq!(
            fs::read_to_string(reg.join("index")).unwrap(),
            "DIRC-fake-index"
        );
        assert_eq!(
            fs::read_to_string(reg.join("gitdir")).unwrap(),
            format!("{}\n", wt.join(".git").display())
        );
        assert_eq!(
            fs::read_to_string(reg.join("commondir")).unwrap(),
            "../..\n"
        );
        assert_eq!(
            fs::read_to_string(wt.join(".git")).unwrap(),
            format!("gitdir: {}\n", reg.display())
        );
        assert_eq!(attached.relocation.carried(), 7);
    }

    #[test]
    fn attach_copies_head_so_both_dirs_hold_one() {
        let (tmp, common) = fake_common(true);
        plant_state(&common);
        let wt = worktree_dir(&tmp);
        let attached = attach_worktree(&common, &wt, Some("feature/x"), "main").unwrap();
        assert_eq!(
            fs::read_to_string(attached.registration.join("HEAD")).unwrap(),
            "ref: refs/heads/feature/x\n"
        );
        // `main` does not exist in this fixture, so the bare HEAD keeps the
        // symref the working tree had.
        assert_eq!(
            fs::read_to_string(common.join("HEAD")).unwrap(),
            "ref: refs/heads/feature/x\n"
        );
    }

    #[test]
    fn attach_writes_the_bare_head_from_the_default_branch_when_it_exists() {
        // A real repository, so `show-ref` can answer.
        let tmp = tempfile::tempdir().unwrap();
        let repo = tmp.path().join("repo");
        fs::create_dir_all(&repo).unwrap();
        git_ok(&repo, &["init", "-q", "-b", "main"]);
        write(&repo.join("README.md"), "hi\n");
        git_ok(&repo, &["add", "."]);
        git_ok(&repo, &["commit", "-q", "-m", "init"]);
        git_ok(&repo, &["switch", "-q", "-c", "feature/x"]);
        git_ok(&repo, &["config", "core.bare", "true"]);
        let common = repo.join(".git");
        let wt = tmp.path().join("wt");
        fs::create_dir_all(&wt).unwrap();

        let attached = attach_worktree(&common, &wt, Some("feature/x"), "main").unwrap();
        assert_eq!(
            fs::read_to_string(common.join("HEAD")).unwrap(),
            "ref: refs/heads/main\n"
        );
        assert_eq!(
            fs::read_to_string(attached.registration.join("HEAD")).unwrap(),
            "ref: refs/heads/feature/x\n"
        );
        assert!(attached.registration.join("index").exists());
        assert!(!common.join("index").exists());
    }

    #[test]
    fn attach_preserves_a_detached_head_verbatim() {
        let (tmp, common) = fake_common(true);
        write(
            &common.join("HEAD"),
            "0123456789abcdef0123456789abcdef01234567\n",
        );
        plant_state(&common);
        let wt = worktree_dir(&tmp);
        let attached = attach_worktree(&common, &wt, None, "main").unwrap();
        assert_eq!(
            fs::read_to_string(attached.registration.join("HEAD")).unwrap(),
            "0123456789abcdef0123456789abcdef01234567\n"
        );
    }

    #[test]
    fn attach_moves_index_last_and_writes_the_pointer_first() {
        let (tmp, common) = fake_common(true);
        plant_state(&common);
        let wt = worktree_dir(&tmp);
        let attached = attach_worktree(&common, &wt, Some("feature/x"), "main").unwrap();
        let moved: Vec<&Path> = attached.relocation.moved().collect();
        assert_eq!(moved.last().unwrap().to_str(), Some("index"));
        // The pointer and registration bookkeeping are journalled before any
        // state entry moved.
        let steps = &attached.relocation.steps;
        let pointer_at = steps
            .iter()
            .position(|s| matches!(s, Step::Wrote { path, .. } if path == &wt.join(".git")))
            .unwrap();
        let first_move = steps
            .iter()
            .position(|s| matches!(s, Step::Renamed { .. }))
            .unwrap();
        assert!(
            pointer_at < first_move,
            "pointer must precede the first rename"
        );
    }

    #[test]
    fn attach_refuses_a_non_bare_common_dir() {
        let (tmp, common) = fake_common(false);
        plant_state(&common);
        let wt = worktree_dir(&tmp);
        let err = attach_worktree(&common, &wt, Some("feature/x"), "main").unwrap_err();
        assert!(err.to_string().contains("core.bare"), "{err}");
        assert!(common.join("index").exists(), "nothing may move");
        assert!(!wt.join(".git").exists());
    }

    #[test]
    fn attach_refuses_when_index_lock_is_present() {
        let (tmp, common) = fake_common(true);
        plant_state(&common);
        write(&common.join("index.lock"), "");
        let wt = worktree_dir(&tmp);
        let err = attach_worktree(&common, &wt, Some("feature/x"), "main").unwrap_err();
        assert!(err.to_string().contains("index.lock"), "{err}");
        assert!(common.join("index").exists());
    }

    #[test]
    fn attach_refuses_a_worktree_that_already_has_a_git_entry() {
        let (tmp, common) = fake_common(true);
        plant_state(&common);
        let wt = worktree_dir(&tmp);
        write(&wt.join(".git"), "gitdir: elsewhere\n");
        let err = attach_worktree(&common, &wt, Some("feature/x"), "main").unwrap_err();
        assert!(err.to_string().contains(".git entry"), "{err}");
        assert!(common.join("index").exists());
    }

    #[test]
    fn attach_strips_role_keys_from_a_moved_config_worktree_and_core_worktree_from_config() {
        let (tmp, common) = fake_common(true);
        plant_state(&common);
        write(
            &common.join("config.worktree"),
            "[core]\n\tbare = false\n\tworktree = /old/place\n[user]\n\tname = keep\n",
        );
        write(
            &common.join("config"),
            "[core]\n\tbare = true\n\tworktree = /old/place\n",
        );
        let wt = worktree_dir(&tmp);
        let attached = attach_worktree(&common, &wt, Some("feature/x"), "main").unwrap();
        let moved = fs::read_to_string(attached.registration.join("config.worktree")).unwrap();
        assert!(!moved.contains("bare"), "{moved}");
        assert!(!moved.contains("worktree ="), "{moved}");
        assert!(moved.contains("name = keep"), "{moved}");
        let config = fs::read_to_string(common.join("config")).unwrap();
        assert!(config.contains("bare = true"), "{config}");
        assert!(!config.contains("worktree"), "{config}");
    }

    #[test]
    fn attach_undo_restores_everything_after_a_midway_failure() {
        let (tmp, common) = fake_common(true);
        plant_state(&common);
        let wt = worktree_dir(&tmp);
        // A malformed `config.worktree` makes the role-key strip fail — after
        // every state entry, `index` included, has already been renamed.
        write(&common.join("config.worktree"), "[core\n");
        let reg = common.join("worktrees/x");
        let before = snapshot(&common);
        let err = attach_worktree(&common, &wt, Some("feature/x"), "main").unwrap_err();
        assert!(err.to_string().contains("config.worktree"), "{err}");
        assert_eq!(
            snapshot(&common),
            before,
            "the common dir must be byte-identical"
        );
        assert!(!wt.join(".git").exists(), "the pointer must be gone again");
        assert!(
            !reg.exists(),
            "the registration scaffolding must be gone again"
        );
    }

    // ── detach ───────────────────────────────────────────────────────────

    #[test]
    fn detach_round_trips_attach_byte_for_byte() {
        let (tmp, common) = fake_common(true);
        plant_state(&common);
        let wt = worktree_dir(&tmp);
        let before = snapshot(&common);
        let attached = attach_worktree(&common, &wt, Some("feature/x"), "main").unwrap();
        assert!(attached.registration.exists());

        let detached = detach_worktree(&common, &wt).unwrap();
        assert_eq!(detached.registration, attached.registration);
        assert!(!detached.registration.exists());
        assert!(!wt.join(".git").exists());
        assert_eq!(snapshot(&common), before);
    }

    #[test]
    fn detach_descends_into_split_dirs_rather_than_renaming_them() {
        let (tmp, common) = fake_common(true);
        plant_state(&common);
        let wt = worktree_dir(&tmp);
        attach_worktree(&common, &wt, Some("feature/x"), "main").unwrap();
        // The common `refs/` and `logs/` still hold branches; a wholesale
        // rename of the registration's `refs/` would collide with them.
        assert!(common.join("refs/heads/feature/x").exists());
        detach_worktree(&common, &wt).unwrap();
        assert!(common.join("refs/heads/feature/x").exists());
        assert!(common.join("refs/bisect/bad").exists());
        assert!(common.join("logs/refs/heads/feature/x").exists());
        assert!(common.join("logs/HEAD").exists());
    }

    #[test]
    fn detach_refuses_a_non_bare_common_dir() {
        let (tmp, common) = fake_common(true);
        plant_state(&common);
        let wt = worktree_dir(&tmp);
        attach_worktree(&common, &wt, Some("feature/x"), "main").unwrap();
        write(&common.join("config"), "[core]\n\tbare = false\n");
        let err = detach_worktree(&common, &wt).unwrap_err();
        assert!(err.to_string().contains("core.bare"), "{err}");
        assert!(
            common.join("worktrees/x/index").exists(),
            "nothing may move"
        );
    }

    #[test]
    fn detach_refuses_a_colliding_destination_before_moving_anything() {
        let (tmp, common) = fake_common(true);
        plant_state(&common);
        let wt = worktree_dir(&tmp);
        attach_worktree(&common, &wt, Some("feature/x"), "main").unwrap();
        // An index reappearing at the common dir is exactly the
        // not-the-state-the-plan-was-built-from condition.
        write(&common.join("index"), "stray");
        let err = detach_worktree(&common, &wt).unwrap_err();
        assert!(err.to_string().contains("index"), "{err}");
        assert!(
            common.join("worktrees/x/ORIG_HEAD").exists(),
            "nothing may move"
        );
        assert!(wt.join(".git").exists());
    }

    #[test]
    fn detach_appends_to_an_existing_head_reflog_and_replaces_scratch_files() {
        let (tmp, common) = fake_common(true);
        plant_state(&common);
        let wt = worktree_dir(&tmp);
        attach_worktree(&common, &wt, Some("feature/x"), "main").unwrap();
        write(&common.join("logs/HEAD"), "bare-side entry\n");
        write(&common.join("FETCH_HEAD"), "stale fetch\n");
        detach_worktree(&common, &wt).unwrap();
        assert_eq!(
            fs::read_to_string(common.join("logs/HEAD")).unwrap(),
            "bare-side entry\n0 1 Test <t@t> 1 +0000\tcommit: one\n"
        );
        assert_eq!(
            fs::read_to_string(common.join("FETCH_HEAD")).unwrap(),
            "bbbb\tbranch 'main' of origin\n"
        );
    }

    #[test]
    fn detach_undo_restores_the_registration_after_a_midway_failure() {
        let (tmp, common) = fake_common(true);
        plant_state(&common);
        let wt = worktree_dir(&tmp);
        attach_worktree(&common, &wt, Some("feature/x"), "main").unwrap();
        let reg = common.join("worktrees/x");
        let before = snapshot(&common);
        // A directory where `index` must land: invisible to the collision
        // check (policy refuses only *existing entries*… which this is — so
        // plant it under a name the policy replaces instead: FETCH_HEAD as a
        // directory makes the rename fail after ORIG_HEAD already moved).
        fs::create_dir_all(common.join("FETCH_HEAD")).unwrap();
        let err = detach_worktree(&common, &wt).unwrap_err();
        assert!(err.to_string().contains("FETCH_HEAD"), "{err}");
        fs::remove_dir(common.join("FETCH_HEAD")).unwrap();
        assert_eq!(snapshot(&common), before);
        assert!(reg.join("ORIG_HEAD").exists());
        assert!(wt.join(".git").exists());
    }

    #[test]
    fn detach_bails_when_no_registration_matches() {
        let (tmp, common) = fake_common(true);
        let wt = worktree_dir(&tmp);
        let err = detach_worktree(&common, &wt).unwrap_err();
        assert!(
            err.to_string().contains("No worktree registration"),
            "{err}"
        );
    }

    // ── naming / lookup ──────────────────────────────────────────────────

    /// `git worktree add` names registrations after the *path basename*, so a
    /// slashed branch's registration cannot be guessed from its name.
    #[test]
    fn find_registration_resolves_a_slashed_branch_by_path() {
        let tmp = tempfile::tempdir().unwrap();
        let repo = tmp.path().join("repo");
        fs::create_dir_all(&repo).unwrap();
        git_ok(&repo, &["init", "-q", "-b", "main"]);
        write(&repo.join("README.md"), "hi\n");
        git_ok(&repo, &["add", "."]);
        git_ok(&repo, &["commit", "-q", "-m", "init"]);
        let wt = repo.join("task/x");
        git_ok(
            &repo,
            &[
                "worktree",
                "add",
                "-q",
                wt.to_str().unwrap(),
                "-b",
                "task/x",
            ],
        );
        let found = find_registration(&repo.join(".git"), &wt).expect("registered");
        assert_eq!(
            found.file_name().unwrap().to_str(),
            Some("x"),
            "git names it after the basename, not the branch — 'task-x' would miss"
        );
    }

    #[test]
    fn registration_name_disambiguates_a_taken_basename() {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path().join("worktrees");
        fs::create_dir_all(root.join("x")).unwrap();
        fs::create_dir_all(root.join("x1")).unwrap();
        let name =
            registration_name(&root, Path::new("/elsewhere/task/x"), Some("task/x")).unwrap();
        assert_eq!(name, "x2");
    }

    #[test]
    fn registration_name_falls_back_for_a_branchless_empty_basename() {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path().join("worktrees");
        let name = registration_name(&root, Path::new("/"), None).unwrap();
        assert_eq!(name, "detached");
    }

    // ── helpers ──────────────────────────────────────────────────────────

    /// Every regular file under `dir` with its bytes, sorted — for
    /// byte-identical comparisons across a relocation and its undo.
    fn snapshot(dir: &Path) -> Vec<(PathBuf, Vec<u8>)> {
        fn walk(base: &Path, dir: &Path, out: &mut Vec<(PathBuf, Vec<u8>)>) {
            for entry in fs::read_dir(dir).unwrap() {
                let entry = entry.unwrap();
                let p = entry.path();
                if p.is_dir() {
                    walk(base, &p, out);
                } else {
                    out.push((
                        p.strip_prefix(base).unwrap().to_path_buf(),
                        fs::read(&p).unwrap(),
                    ));
                }
            }
        }
        let mut out = Vec::new();
        walk(dir, dir, &mut out);
        out.sort();
        out
    }
}
