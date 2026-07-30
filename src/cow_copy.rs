//! Copy-on-write file and directory copies.
//!
//! Wraps [`reflink_copy::reflink_or_copy`] so callers get APFS clonefile on
//! macOS, `ioctl(FICLONE)` on reflink-capable Linux filesystems (btrfs, XFS
//! with `reflink=1`, OpenZFS 2.2+, bcachefs), and block-clone on Windows ReFS
//! — with a transparent byte-copy fallback everywhere else.
//!
//! [`copy_file`] is the file-level primitive; [`copy_dir`] reproduces a
//! directory tree — on macOS/APFS by cloning the whole hierarchy in a single
//! `clonefile(2)` syscall plus a read-only fix-up walk ([`clone_tree`]), and
//! everywhere else (or when the filesystem declines) by walking the tree
//! using [`copy_file`] for regular files and recreating directories and
//! symlinks. Mode bits are preserved per entry; ownership and xattrs are not
//! promised (the clone path reproduces them, the walking path does not) —
//! current callsites don't depend on them.

use anyhow::{Context, Result};
use std::collections::VecDeque;
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::{Condvar, Mutex};
#[cfg(target_os = "macos")]
use walkdir::WalkDir;

/// What a copy actually did, per regular file.
///
/// `reflink_or_copy` decides file by file, and a tree that straddles a mount
/// point can have some files cloned and others byte-copied without anything
/// saying so. A caller that wants to report what a copy *cost* has to be told
/// rather than infer it from a filesystem's name, so the reporting entry points
/// hand this back.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct CopyStats {
    /// Regular files the filesystem cloned (near-free).
    pub reflinked: u64,
    /// Regular files that fell back to a real byte copy.
    pub copied: u64,
    /// Bytes written, counted once per destination file. Deliberately *not*
    /// hard-link-deduplicated the way `du` counts a source tree: every link in
    /// the source becomes an independent file here, so this is what the
    /// destination actually costs.
    pub bytes: u64,
}

impl CopyStats {
    fn add(&mut self, other: CopyStats) {
        self.reflinked += other.reflinked;
        self.copied += other.copied;
        self.bytes += other.bytes;
    }
}

/// The link text to write at `dst_link` for the symlink at `src_link`, given
/// that everything under `move_root` is moving to the destination together.
///
/// Copying relative link text verbatim only works while the link and its target
/// move together. They do not when the target sits **outside** what is moving:
/// `.venv -> ../.venvs/proj` is correct from `repo/main` and dangles from
/// `repo/feature/login`, and daft's own contained layout puts worktrees at
/// exactly those differing depths. The same shape is how a path declared in
/// both `shared:` and `copy:` loses its shared link under `warm --force` —
/// the correct link is replaced by a dangling one and reported as a clean copy.
///
/// So: a relative link resolving outside `move_root` is rewritten to reach the
/// **same resolved target** from its new position. Everything else is left
/// alone, and deliberately:
///
/// * a link resolving **inside** `move_root` keeps its text — its base moved
///   with it, and rewriting would point the copy back at the original;
/// * an **absolute** link is already position-independent;
/// * a link whose target does not exist replicates **verbatim** — the source
///   is already broken, and inventing a path would fabricate a target the user
///   never had. Preserved garbage beats invented meaning.
///
/// `move_root` is what moves, which is **not** always the tree being walked.
/// For a `copy:` entry the walked tree is `node_modules/` but the thing that
/// moves is the whole worktree, so a workspace link
/// `node_modules/@acme/api -> ../../packages/api` has to keep its text: the
/// destination worktree has its own `packages/api`, and rebasing would point a
/// new branch's dependency graph back at the branch it was seeded from — where
/// it compiles the wrong sources and dangles the moment that worktree is
/// removed. Callers that really are copying a free-standing tree pass the tree
/// itself ([`copy_dir_reporting`]); callers copying part of something larger
/// pass the larger thing ([`copy_dir_within`]).
///
/// Resolution is **lexical**: `..` is folded textually rather than by walking
/// the filesystem, because resolving for real would follow the very symlinks
/// this is reasoning about. An intermediate symlinked component can therefore
/// classify as inside when the kernel would land outside; the failure mode is a
/// link left verbatim, which is what the old code did for every link.
pub fn rebased_link_target(src_link: &Path, dst_link: &Path, move_root: &Path) -> Result<PathBuf> {
    let text = fs::read_link(src_link)
        .with_context(|| format!("reading symlink {}", src_link.display()))?;
    let src_dir = src_link.parent().unwrap_or(Path::new("/"));
    let dst_dir = dst_link.parent().unwrap_or(Path::new("/"));
    Ok(rebase_needed(&text, src_dir, dst_dir, move_root).unwrap_or(text))
}

/// The core of [`rebased_link_target`], on link text already in hand: the
/// rewritten text for a link whose position moves from `src_dir` to
/// `dst_dir`, or `None` when the existing text is already correct at the new
/// position. Split out so the post-clone fix-up walk — which has just read
/// the text from the destination — can decide without a second `readlink`.
fn rebase_needed(text: &Path, src_dir: &Path, dst_dir: &Path, move_root: &Path) -> Option<PathBuf> {
    if text.is_absolute() {
        return None;
    }
    let resolved = lexically_normalize(&src_dir.join(text));
    let root = lexically_normalize(move_root);
    if resolved.starts_with(&root) || fs::symlink_metadata(&resolved).is_err() {
        return None;
    }
    Some(relative_path_from(&lexically_normalize(dst_dir), &resolved))
}

/// Fold `.` and `..` out of a path textually, without touching the filesystem.
fn lexically_normalize(path: &Path) -> PathBuf {
    use std::path::Component;

    let mut out = PathBuf::new();
    for component in path.components() {
        match component {
            Component::CurDir => {}
            // `..` above the root is the root, matching how the kernel treats
            // it; `..` above a relative path's start has to be kept.
            Component::ParentDir => {
                if !out.pop() {
                    out.push("..");
                }
            }
            other => out.push(other),
        }
    }
    out
}

/// The relative path from `from_dir` to `to`, both already normalized.
fn relative_path_from(from_dir: &Path, to: &Path) -> PathBuf {
    let from: Vec<_> = from_dir.components().collect();
    let to_parts: Vec<_> = to.components().collect();
    let shared = from
        .iter()
        .zip(&to_parts)
        .take_while(|(a, b)| a == b)
        .count();

    let mut rel = PathBuf::new();
    for _ in shared..from.len() {
        rel.push("..");
    }
    for component in &to_parts[shared..] {
        rel.push(component);
    }
    // Same directory: a link needs a body, and `.` is the one that means here.
    if rel.as_os_str().is_empty() {
        rel.push(".");
    }
    rel
}

/// Copy a regular file from `src` to `dst`, using reflink where supported
/// and a byte copy otherwise. Preserves mode bits.
///
/// `dst` must not already exist; its parent must.
pub fn copy_file(src: &Path, dst: &Path) -> Result<()> {
    copy_file_reporting(src, dst).map(|_| ())
}

/// [`copy_file`], reporting which way the bytes actually moved.
pub fn copy_file_reporting(src: &Path, dst: &Path) -> Result<CopyStats> {
    let fallback_bytes = reflink_copy::reflink_or_copy(src, dst)
        .with_context(|| format!("copying {} -> {}", src.display(), dst.display()))?;
    // Linux `FICLONE` copies content only; macOS clonefile copies metadata;
    // the byte-copy fallback copies mode. Normalize across all three — except
    // where the normalization is provably redundant: after a macOS clonefile
    // the mode is already in place, and re-applying it costs a third of the
    // reflink path's syscalls (13% of a 45k-file copy, measured) for nothing.
    // The kernel's one deliberate divergence is stripping setuid/setgid from
    // regular files, so a source carrying special bits still takes the
    // explicit chmod rather than silently losing them.
    let src_meta = fs::symlink_metadata(src)
        .with_context(|| format!("reading metadata of {}", src.display()))?;
    let cloned_on_macos = cfg!(target_os = "macos") && fallback_bytes.is_none();
    #[cfg(unix)]
    let special_bits = {
        use std::os::unix::fs::PermissionsExt;
        src_meta.permissions().mode() & 0o7000 != 0
    };
    #[cfg(not(unix))]
    let special_bits = false;
    if !cloned_on_macos || special_bits {
        fs::set_permissions(dst, src_meta.permissions())
            .with_context(|| format!("setting mode of {}", dst.display()))?;
    }
    // `Some(n)` is the byte-copy fallback reporting what it wrote; `None` means
    // the filesystem cloned it, so the source's length is what now exists at
    // the destination.
    Ok(match fallback_bytes {
        Some(n) => CopyStats {
            reflinked: 0,
            copied: 1,
            bytes: n,
        },
        None => CopyStats {
            reflinked: 1,
            copied: 0,
            bytes: src_meta.len(),
        },
    })
}

/// Recursively copy a directory tree from `src` to `dst`.
///
/// Regular files go through [`copy_file`] (reflink-or-byte-copy); directories
/// are recreated empty and then populated; symlinks are recreated with the
/// same link target (never dereferenced). Mode bits are preserved per entry.
///
/// `dst` must not already exist; its parent must. `src` must be a directory.
pub fn copy_dir(src: &Path, dst: &Path) -> Result<()> {
    copy_dir_reporting(src, dst).map(|_| ())
}

/// [`copy_dir`], reporting how much of the tree was cloned versus copied.
///
/// On macOS the whole hierarchy is cloned in one `clonefile(2)` call when the
/// filesystem allows it ([`clone_tree`]); everything below this paragraph
/// describes the walking path that runs everywhere else.
///
/// Directory modes are applied in a **deferred, deepest-first pass** after the
/// tree is populated, never as each directory is created. Applying them up
/// front reproduces a read-only source directory (0555 — what a tarball restore
/// or a `chmod -R a-w` archive leaves behind) before anything can be written
/// into it, so every child creation fails. For `copy:` that failure is
/// self-perpetuating: the entry fails, the partial destination is discarded,
/// and the next run repeats it forever.
pub fn copy_dir_reporting(src: &Path, dst: &Path) -> Result<CopyStats> {
    copy_dir_within(src, dst, src)
}

/// [`copy_dir_reporting`] for a tree that is only **part** of what is moving.
///
/// `move_root` is the enclosing thing whose contents travel to the destination
/// together — for `copy:` that is the source worktree, not the declared entry.
/// It decides only which relative symlinks keep their text and which are
/// rebased; see [`rebased_link_target`]. Everything else is identical, and
/// `copy_dir_reporting(src, dst)` is exactly `copy_dir_within(src, dst, src)`.
pub fn copy_dir_within(src: &Path, dst: &Path, move_root: &Path) -> Result<CopyStats> {
    let src_meta = fs::symlink_metadata(src)
        .with_context(|| format!("reading metadata of {}", src.display()))?;
    anyhow::ensure!(
        src_meta.is_dir(),
        "copy_dir source is not a directory: {}",
        src.display()
    );

    // DAFT_COPY_FORCE_WALK is the bench/test escape hatch: it pins the
    // portable walking path on machines whose filesystem would always take
    // the clone, so the path Linux users actually run can be measured — and
    // regressed — anywhere. Not a user-facing setting.
    #[cfg(target_os = "macos")]
    if std::env::var_os("DAFT_COPY_FORCE_WALK").is_none()
        && let Some(stats) = clone_tree(src, dst, move_root)?
    {
        return Ok(stats);
    }

    copy_dir_walking(src, dst, move_root, src_meta.permissions())
}

/// The portable per-entry copy: a bounded pool of workers drains a shared
/// queue of directories, each worker copying one directory's immediate
/// children and enqueueing its subdirectories.
///
/// Parallel because the walk is a syscall storm with almost no user-space
/// work between calls (a sampled profile of the serial version spent 84% of
/// its time inside per-file syscalls): several in-flight operations hide
/// each other's latency. How many is worth having depends on what the walk
/// is actually doing, and the pool sizes itself from a one-file reflink
/// probe of the destination. Measured on the same 57k-entry pnpm-shaped
/// tree (930 MB):
///
/// * **byte-copy walk** (ext4): 1.32s serial → 0.51s at 4 workers, flat
///   beyond — data movement parallelizes, so the probe failing picks 4;
/// * **reflink walk** (btrfs `FICLONE`): 0.65s serial → 0.53s at 2 workers,
///   then *worse than serial* at 4+ (1.08s/1.31s) — per-file cloning is a
///   metadata storm that contends on filesystem-internal locks, so the
///   probe succeeding picks 2. APFS behaves the same way (a 10-core
///   machine's 8-worker clone walk spent 6x the kernel CPU for a 13% wall
///   win), but on macOS this path only runs when the whole-tree clone
///   already declined.
///
/// Two ordering properties the serial walk guaranteed survive the pool:
///
/// * a directory is created by its parent's worker **before** it is
///   enqueued, so no worker ever writes into a directory that does not
///   exist yet;
/// * directory modes are applied only **after the pool has joined**,
///   deepest-first — the read-only-source contract [`copy_dir_reporting`]
///   documents (a 0555 directory must be fully populated before it is
///   sealed).
///
/// The first error wins: workers observing a recorded failure stop pulling
/// work, the queue drains, and that error is returned once the pool joins.
/// Which of several racing errors gets reported is nondeterministic — the
/// serial walk's contract was "some real failure from this tree", and that
/// is preserved; its exact ordering is not.
fn copy_dir_walking(
    src: &Path,
    dst: &Path,
    move_root: &Path,
    src_perms: fs::Permissions,
) -> Result<CopyStats> {
    create_writable_dir(dst).with_context(|| format!("creating {}", dst.display()))?;

    let shared = WalkShared {
        queue: Mutex::new(WalkQueue {
            dirs: VecDeque::from([(PathBuf::new(), 0usize)]),
            active: 0,
            failed: None,
            stats: CopyStats::default(),
            deferred: vec![(dst.to_path_buf(), src_perms, 0)],
        }),
        cvar: Condvar::new(),
    };
    let shared = &shared;
    // DAFT_COPY_WALK_JOBS is the bench/test knob for the pool size (1 = the
    // old serial behavior, modulo queue overhead). Not a user-facing setting.
    let workers = std::env::var("DAFT_COPY_WALK_JOBS")
        .ok()
        .and_then(|v| v.trim().parse::<usize>().ok())
        .filter(|&n| n >= 1)
        .unwrap_or_else(|| {
            let cap = if probe_reflink(dst) { 2 } else { 4 };
            std::thread::available_parallelism()
                .map(|p| p.get())
                .unwrap_or(cap)
                .min(cap)
        });

    std::thread::scope(|scope| {
        for _ in 0..workers {
            scope.spawn(move || walk_worker(shared, src, dst, move_root));
        }
    });

    let (failed, stats, mut deferred) = {
        let mut queue = shared.queue.lock().expect("copy walk mutex poisoned");
        (
            queue.failed.take(),
            queue.stats,
            std::mem::take(&mut queue.deferred),
        )
    };
    if let Some(err) = failed {
        return Err(err);
    }

    // Deepest-first: a directory can only be made read-only once nothing more
    // needs to be written inside it.
    deferred.sort_by_key(|entry| std::cmp::Reverse(entry.2));
    for (path, permissions, _) in deferred {
        fs::set_permissions(&path, permissions)
            .with_context(|| format!("setting mode of {}", path.display()))?;
    }
    Ok(stats)
}

/// Whether the filesystem holding `dir` (which must already exist) can
/// reflink: clone one throwaway file inside it and read the verdict. The
/// walking pool's sizing signal — see [`copy_dir_walking`] for the measured
/// why. Any failure (including not being able to create the probe at all)
/// reads as "no reflink", which merely picks the byte-copy worker count.
fn probe_reflink(dir: &Path) -> bool {
    let src = dir.join(".daft-copy-probe-src");
    let dst = dir.join(".daft-copy-probe-dst");
    let verdict = fs::write(&src, b"probe").is_ok() && reflink_copy::reflink(&src, &dst).is_ok();
    let _ = fs::remove_file(&src);
    let _ = fs::remove_file(&dst);
    verdict
}

struct WalkShared {
    queue: Mutex<WalkQueue>,
    cvar: Condvar,
}

struct WalkQueue {
    /// Source-relative directory paths whose children remain to be copied,
    /// with their depth for the deferred-mode ordering.
    dirs: VecDeque<(PathBuf, usize)>,
    /// Directories currently inside a worker. Work exists while either this
    /// is non-zero (an active worker may still enqueue) or `dirs` is
    /// non-empty.
    active: usize,
    /// The first error any worker hit; the pool drains once it is set.
    failed: Option<anyhow::Error>,
    /// Merged as each worker exits.
    stats: CopyStats,
    /// (destination path, source mode, depth), applied deepest-first after
    /// the pool joins.
    deferred: Vec<(PathBuf, fs::Permissions, usize)>,
}

fn walk_worker(shared: &WalkShared, src: &Path, dst: &Path, move_root: &Path) {
    let mut local = CopyStats::default();
    loop {
        let (rel, depth) = {
            let mut queue = shared.queue.lock().expect("copy walk mutex poisoned");
            loop {
                if queue.failed.is_some() {
                    queue.stats.add(local);
                    return;
                }
                if let Some(job) = queue.dirs.pop_front() {
                    queue.active += 1;
                    break job;
                }
                if queue.active == 0 {
                    queue.stats.add(local);
                    return;
                }
                queue = shared.cvar.wait(queue).expect("copy walk mutex poisoned");
            }
        };

        let result = copy_dir_children(&rel, depth, shared, src, dst, move_root, &mut local);

        let mut queue = shared.queue.lock().expect("copy walk mutex poisoned");
        queue.active -= 1;
        if let Err(err) = result
            && queue.failed.is_none()
        {
            queue.failed = Some(err);
        }
        if queue.failed.is_some() || (queue.active == 0 && queue.dirs.is_empty()) {
            shared.cvar.notify_all();
        }
    }
}

/// Copy the immediate children of `src.join(rel)`: subdirectories are
/// created writable (real modes deferred) and enqueued for the pool, then
/// files and symlinks are copied in place — in that order, because descent
/// is what feeds the pool and must never wait behind a directory's own file
/// copies.
fn copy_dir_children(
    rel: &Path,
    depth: usize,
    shared: &WalkShared,
    src: &Path,
    dst: &Path,
    move_root: &Path,
    local: &mut CopyStats,
) -> Result<()> {
    let src_dir = src.join(rel);
    let mut child_dirs = Vec::new();
    let mut links = Vec::new();
    let mut files = Vec::new();

    for entry in fs::read_dir(&src_dir).with_context(|| format!("reading {}", src_dir.display()))? {
        let entry = entry.with_context(|| format!("reading {}", src_dir.display()))?;
        let ftype = entry
            .file_type()
            .with_context(|| format!("reading type of {}", entry.path().display()))?;
        let child_rel = rel.join(entry.file_name());
        if ftype.is_dir() {
            let meta = entry
                .metadata()
                .with_context(|| format!("reading metadata of {}", entry.path().display()))?;
            child_dirs.push((child_rel, meta.permissions()));
        } else if ftype.is_symlink() {
            links.push(child_rel);
        } else if ftype.is_file() {
            files.push(child_rel);
        }
        // Block / char / fifo / socket entries are skipped. Daft's existing
        // callsites don't produce them; future consumers that need them
        // should extend this dispatch deliberately.
    }

    if !child_dirs.is_empty() {
        let mut created = Vec::with_capacity(child_dirs.len());
        for (child_rel, perms) in child_dirs {
            let dst_path = dst.join(&child_rel);
            create_writable_dir(&dst_path)
                .with_context(|| format!("creating {}", dst_path.display()))?;
            created.push((child_rel, dst_path, perms));
        }
        {
            let mut queue = shared.queue.lock().expect("copy walk mutex poisoned");
            for (child_rel, dst_path, perms) in created {
                queue.deferred.push((dst_path, perms, depth + 1));
                queue.dirs.push_back((child_rel, depth + 1));
            }
        }
        shared.cvar.notify_all();
    }

    #[cfg(unix)]
    for child_rel in links {
        // A link pointing within what is moving keeps its text, one escaping
        // it is re-based on the new position. The boundary is `move_root`,
        // not `src`: under `copy:` the walked tree is one entry but the whole
        // worktree moves, and a workspace link that leaves `node_modules/`
        // still lands inside the worktree the destination has its own copy
        // of. See `rebased_link_target`.
        let src_path = src.join(&child_rel);
        let dst_path = dst.join(&child_rel);
        let target = rebased_link_target(&src_path, &dst_path, move_root)?;
        std::os::unix::fs::symlink(target, &dst_path)
            .with_context(|| format!("creating symlink {}", dst_path.display()))?;
    }
    #[cfg(not(unix))]
    if let Some(child_rel) = links.first() {
        // Windows / non-Unix targets: symlink replication needs the
        // file-vs-dir distinction up front (`symlink_file` / `symlink_dir`)
        // and admin/dev-mode privileges. Daft's current consumers don't
        // produce symlinks in their copy trees; #387's `copy_paths:` work
        // will revisit if needed.
        anyhow::bail!(
            "symlink replication not yet implemented on this platform: {}",
            src.join(child_rel).display()
        );
    }

    for child_rel in files {
        local.add(copy_file_reporting(
            &src.join(&child_rel),
            &dst.join(&child_rel),
        )?);
    }
    Ok(())
}

/// One-syscall whole-tree clone (macOS/APFS), or `None` when the filesystem
/// declines and the per-entry walking copy should run instead.
///
/// `clonefile(2)` clones a directory hierarchy in a single call: the kernel
/// walks the tree itself, reproducing directories (modes included — even
/// read-only 0555 ones, so the deferred-modes dance below is unnecessary on
/// this path), regular files as CoW clones, and symlinks verbatim. Measured
/// against the walking path on a 90k-entry build tree this is ~5x faster
/// end-to-end (2.9s kernel clone + a read-only fix-up walk, versus ~15s of
/// per-entry create/clone/chmod syscalls), and the gap widens on
/// symlink-dense pnpm trees, which pay the walking path's most expensive
/// per-entry case.
///
/// What the kernel cannot do is daft's symlink policy — a link that escapes
/// `move_root` must be re-based on its new position ([`rebased_link_target`])
/// — so [`fixup_cloned_tree`] runs after the clone: `readdir` + one
/// `readlink` per symlink + one `lstat` per file (for byte reporting), a
/// small fraction of the walk it replaces.
///
/// Divergences from the walking copy, all deliberate:
///
/// * **Hard links split into independent clones on both paths** — data blocks
///   stay shared (CoW), inode identity does not. Verified empirically; the
///   man page's "as if each item was cloned individually" says as much.
/// * **FIFOs, sockets, and device nodes are cloned**; the walking path skips
///   them. Cloning is the more faithful of the two, and neither shape
///   appears in the cache trees `copy:` declares.
/// * **File mtimes are preserved**; the walking path resets them to the copy
///   time. Preservation is what keeps a cloned tree's warmth ranking
///   ([`crate::core::copy_source`]) honest — a copy is exactly as stale as
///   its source, and should say so.
/// * **setuid/setgid would be stripped by the kernel** on regular files; the
///   fix-up walk restores them from the source so both paths preserve them.
///
/// The man page "strongly discourages" directory cloning in favor of
/// `copyfile(3)`; the concerns behind that are non-atomicity and poor
/// partial-failure reporting, both of which the caller contract already
/// absorbs — any error falls back to the walking path after sweeping
/// residue, exactly as if the fast path had never run. (`copyfile(3)` itself
/// is not reachable without `unsafe`, which production code forbids;
/// `reflink_copy::reflink` passes a directory straight to `clonefile(2)` and
/// documents that it does.)
#[cfg(target_os = "macos")]
fn clone_tree(src: &Path, dst: &Path, move_root: &Path) -> Result<Option<CopyStats>> {
    if fs::symlink_metadata(dst).is_ok() {
        // A pre-existing destination is the walking path's error to report,
        // in its usual shape — and it must not be mistaken for clone residue
        // below and swept away.
        return Ok(None);
    }
    if reflink_copy::reflink(src, dst).is_err() {
        // Cross-volume, non-APFS, unreadable subtree, … — the kernel unwound
        // cleanly in every probed failure (no partial destination), but the
        // man page stops short of promising that, so sweep any residue: the
        // walking path requires an absent destination.
        if fs::symlink_metadata(dst).is_ok() {
            fs::remove_dir_all(dst)
                .with_context(|| format!("removing partial clone residue at {}", dst.display()))?;
        }
        return Ok(None);
    }
    fixup_cloned_tree(src, dst, move_root).map(Some)
}

/// The read-only pass over a just-cloned tree: count regular files and bytes
/// for [`CopyStats`], re-base the (rare) symlinks whose targets escape
/// `move_root`, and restore special mode bits the kernel strips on clone.
#[cfg(target_os = "macos")]
fn fixup_cloned_tree(src: &Path, dst: &Path, move_root: &Path) -> Result<CopyStats> {
    use std::os::unix::fs::PermissionsExt;

    let mut stats = CopyStats::default();
    for entry in WalkDir::new(dst).follow_links(false).min_depth(1) {
        let entry = entry.with_context(|| format!("walking cloned tree at {}", dst.display()))?;
        let ftype = entry.file_type();
        if ftype.is_dir() {
            continue;
        }
        let rel = entry
            .path()
            .strip_prefix(dst)
            .expect("walkdir paths are rooted at dst");
        let src_path = src.join(rel);

        if ftype.is_file() {
            // lstat the SOURCE twin: same length (they are clones of one
            // another), plus the mode the kernel refused to reproduce. A file
            // deleted from the source since the clone still exists here; fall
            // back to its own metadata — the source is read live and torn
            // snapshots are accepted (see `copy_entries`).
            let meta = match fs::symlink_metadata(&src_path) {
                Ok(meta) => meta,
                Err(_) => entry
                    .metadata()
                    .with_context(|| format!("reading metadata of {}", entry.path().display()))?,
            };
            stats.reflinked += 1;
            stats.bytes += meta.len();
            if meta.permissions().mode() & 0o7000 != 0 {
                fs::set_permissions(entry.path(), meta.permissions())
                    .with_context(|| format!("setting mode of {}", entry.path().display()))?;
            }
        } else if ftype.is_symlink() {
            let text = fs::read_link(entry.path())
                .with_context(|| format!("reading symlink {}", entry.path().display()))?;
            let src_dir = src_path.parent().unwrap_or(Path::new("/"));
            let dst_dir = entry.path().parent().unwrap_or(Path::new("/"));
            if let Some(rebased) = rebase_needed(&text, src_dir, dst_dir, move_root)
                && rebased != text
            {
                fs::remove_file(entry.path())
                    .with_context(|| format!("removing {}", entry.path().display()))?;
                std::os::unix::fs::symlink(&rebased, entry.path())
                    .with_context(|| format!("re-basing symlink {}", entry.path().display()))?;
            }
        }
        // Block / char / fifo / socket entries: the kernel cloned them
        // faithfully, and they are neither counted nor touched here.
    }
    Ok(stats)
}

/// Create a directory the copy can definitely write into, whatever the source's
/// mode turns out to be. The source's mode is applied later, by
/// [`copy_dir_reporting`]'s deferred pass.
fn create_writable_dir(path: &Path) -> std::io::Result<()> {
    #[cfg(unix)]
    {
        use std::os::unix::fs::DirBuilderExt;
        fs::DirBuilder::new().mode(0o700).create(path)
    }
    #[cfg(not(unix))]
    {
        fs::create_dir(path)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;
    use tempfile::TempDir;

    #[cfg(unix)]
    use std::os::unix::fs::{PermissionsExt, symlink};

    fn write_file(path: &Path, content: &[u8]) {
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent).unwrap();
        }
        let mut f = fs::File::create(path).unwrap();
        f.write_all(content).unwrap();
    }

    #[test]
    fn copy_file_independence_after_source_mutation() {
        let tmp = TempDir::new().unwrap();
        let src = tmp.path().join("src.bin");
        let dst = tmp.path().join("dst.bin");
        write_file(&src, b"original");

        copy_file(&src, &dst).unwrap();
        write_file(&src, b"MUTATED!");

        assert_eq!(fs::read(&dst).unwrap(), b"original");
    }

    #[cfg(unix)]
    #[test]
    fn copy_file_preserves_mode_bits() {
        let tmp = TempDir::new().unwrap();
        let src = tmp.path().join("hook");
        let dst = tmp.path().join("hook.copy");
        write_file(&src, b"#!/bin/sh\necho hi\n");
        fs::set_permissions(&src, fs::Permissions::from_mode(0o755)).unwrap();

        copy_file(&src, &dst).unwrap();

        let mode = fs::metadata(&dst).unwrap().permissions().mode() & 0o777;
        assert_eq!(mode, 0o755);
    }

    /// The reflink fast path skips the mode normalization because a macOS
    /// clonefile already reproduced it — with one kernel-imposed exception:
    /// setuid/setgid are stripped from regular files on clone. Special bits
    /// must therefore keep taking the explicit chmod, or the skip silently
    /// downgrades them.
    #[cfg(unix)]
    #[test]
    fn copy_file_preserves_setuid_and_setgid_bits() {
        let tmp = TempDir::new().unwrap();
        let src = tmp.path().join("suid");
        let dst = tmp.path().join("suid.copy");
        write_file(&src, b"payload");
        fs::set_permissions(&src, fs::Permissions::from_mode(0o4755)).unwrap();

        copy_file(&src, &dst).unwrap();

        let mode = fs::metadata(&dst).unwrap().permissions().mode() & 0o7777;
        assert_eq!(mode, 0o4755);
    }

    #[test]
    fn copy_dir_replicates_tree_and_survives_source_mutation() {
        let tmp = TempDir::new().unwrap();
        let src = tmp.path().join("src");
        let dst = tmp.path().join("dst");
        write_file(&src.join("a.txt"), b"alpha");
        write_file(&src.join("nested/b.txt"), b"beta");
        write_file(&src.join("nested/deep/c.txt"), b"gamma");

        copy_dir(&src, &dst).unwrap();

        assert_eq!(fs::read(dst.join("a.txt")).unwrap(), b"alpha");
        assert_eq!(fs::read(dst.join("nested/b.txt")).unwrap(), b"beta");
        assert_eq!(fs::read(dst.join("nested/deep/c.txt")).unwrap(), b"gamma");

        write_file(&src.join("a.txt"), b"MUTATED");
        write_file(&src.join("nested/b.txt"), b"MUTATED");
        assert_eq!(fs::read(dst.join("a.txt")).unwrap(), b"alpha");
        assert_eq!(fs::read(dst.join("nested/b.txt")).unwrap(), b"beta");
    }

    #[cfg(unix)]
    #[test]
    fn copy_dir_recreates_symlinks_without_dereferencing() {
        let tmp = TempDir::new().unwrap();
        let src = tmp.path().join("src");
        let dst = tmp.path().join("dst");
        fs::create_dir(&src).unwrap();
        write_file(&src.join("target.txt"), b"linked");
        symlink("target.txt", src.join("link.txt")).unwrap();

        copy_dir(&src, &dst).unwrap();

        let dst_link_meta = fs::symlink_metadata(dst.join("link.txt")).unwrap();
        assert!(dst_link_meta.file_type().is_symlink());
        assert_eq!(
            fs::read_link(dst.join("link.txt")).unwrap(),
            Path::new("target.txt")
        );
    }

    #[cfg(unix)]
    #[test]
    fn copy_dir_rebases_a_link_that_escapes_the_copied_tree() {
        // The depth-differing case, which daft's own contained layout produces:
        // `repo/main` and `repo/feature/login` are not siblings, so verbatim
        // link text that resolves from one dangles from the other.
        let tmp = TempDir::new().unwrap();
        write_file(&tmp.path().join(".venvs/proj/bin/python"), b"#!");
        let src = tmp.path().join("main/cache");
        let dst = tmp.path().join("feature/login/cache");
        fs::create_dir_all(&src).unwrap();
        fs::create_dir_all(dst.parent().unwrap()).unwrap();
        // From `main/cache/`, `../../.venvs/proj` is `<tmp>/.venvs/proj`.
        symlink("../../.venvs/proj", src.join("venv")).unwrap();

        copy_dir(&src, &dst).unwrap();

        assert_ne!(
            fs::read_link(dst.join("venv")).unwrap(),
            Path::new("../../.venvs/proj"),
            "verbatim text cannot resolve from a different depth"
        );
        assert!(
            dst.join("venv/bin/python").exists(),
            "the copy must reach the same target: {:?}",
            fs::read_link(dst.join("venv")).unwrap()
        );
    }

    #[cfg(unix)]
    #[test]
    fn copy_dir_leaves_links_that_stay_inside_the_copied_tree_alone() {
        // The pnpm/node_modules shape. These links' base moved with them, so
        // rewriting would point the copy back at the original tree.
        let tmp = TempDir::new().unwrap();
        let src = tmp.path().join("src");
        let dst = tmp.path().join("dst");
        write_file(&src.join("pkg/bin/tool"), b"#!");
        fs::create_dir_all(src.join(".bin")).unwrap();
        symlink("../pkg/bin/tool", src.join(".bin/tool")).unwrap();

        copy_dir(&src, &dst).unwrap();

        assert_eq!(
            fs::read_link(dst.join(".bin/tool")).unwrap(),
            Path::new("../pkg/bin/tool"),
            "an inside link keeps its text"
        );
        assert!(dst.join(".bin/tool").exists());
        // And it points at the COPY, not back at the source.
        write_file(&src.join("pkg/bin/tool"), b"MUTATED");
        assert_eq!(fs::read(dst.join(".bin/tool")).unwrap(), b"#!");
    }

    /// The boundary is what MOVES, not what is walked. Under `copy:` the walked
    /// tree is one entry (`node_modules/`) but the whole worktree travels, so a
    /// link leaving the entry and landing elsewhere in the worktree — every npm
    /// and pnpm workspace — must keep its text and re-resolve in its new home.
    /// Treating the entry as the boundary rebased it back at the source, where
    /// it reads the wrong sources and dangles the moment that worktree is
    /// removed.
    #[cfg(unix)]
    #[test]
    fn copy_dir_within_keeps_a_link_that_leaves_the_entry_but_not_the_worktree() {
        let tmp = TempDir::new().unwrap();
        let src_wt = tmp.path().join("main");
        let dst_wt = tmp.path().join("feature");
        write_file(&src_wt.join("packages/api/index.js"), b"source");
        write_file(&dst_wt.join("packages/api/index.js"), b"destination");
        fs::create_dir_all(src_wt.join("node_modules/@acme")).unwrap();
        symlink("../../packages/api", src_wt.join("node_modules/@acme/api")).unwrap();

        copy_dir_within(
            &src_wt.join("node_modules"),
            &dst_wt.join("node_modules"),
            &src_wt,
        )
        .unwrap();

        let copied = dst_wt.join("node_modules/@acme/api");
        assert_eq!(
            fs::read_link(&copied).unwrap(),
            Path::new("../../packages/api"),
            "a link that stays inside what moves keeps its text"
        );
        assert_eq!(
            fs::read(copied.join("index.js")).unwrap(),
            b"destination",
            "and resolves against the destination's own copy"
        );

        // The same link, copied as a free-standing tree, still rebases — the
        // two entry points genuinely mean different things.
        let elsewhere = tmp.path().join("elsewhere/node_modules");
        fs::create_dir_all(elsewhere.parent().unwrap()).unwrap();
        copy_dir(&src_wt.join("node_modules"), &elsewhere).unwrap();
        assert_ne!(
            fs::read_link(elsewhere.join("@acme/api")).unwrap(),
            Path::new("../../packages/api")
        );
    }

    #[cfg(unix)]
    #[test]
    fn copy_dir_preserves_absolute_and_dangling_links_verbatim() {
        let tmp = TempDir::new().unwrap();
        let src = tmp.path().join("src");
        let dst = tmp.path().join("dst");
        fs::create_dir_all(&src).unwrap();
        let absolute = tmp.path().join("elsewhere");
        write_file(&absolute, b"x");
        symlink(&absolute, src.join("abs")).unwrap();
        // Already broken at the source: inventing a target would fabricate one
        // the user never had.
        symlink("../nowhere/at/all", src.join("dangling")).unwrap();

        copy_dir(&src, &dst).unwrap();

        assert_eq!(fs::read_link(dst.join("abs")).unwrap(), absolute);
        assert_eq!(
            fs::read_link(dst.join("dangling")).unwrap(),
            Path::new("../nowhere/at/all")
        );
    }

    #[test]
    fn rebasing_is_lexical_and_never_walks_the_filesystem() {
        // The path math on its own, including the same-directory case a link
        // body cannot express as the empty string.
        assert_eq!(
            lexically_normalize(Path::new("/a/b/../c/./d")),
            Path::new("/a/c/d")
        );
        assert_eq!(lexically_normalize(Path::new("/../..")), Path::new("/"));
        assert_eq!(
            relative_path_from(Path::new("/a/b/c"), Path::new("/a/x")),
            Path::new("../../x")
        );
        assert_eq!(
            relative_path_from(Path::new("/a/b"), Path::new("/a/b")),
            Path::new(".")
        );
    }

    #[cfg(unix)]
    #[test]
    fn copy_dir_preserves_directory_mode_bits() {
        let tmp = TempDir::new().unwrap();
        let src = tmp.path().join("src");
        let dst = tmp.path().join("dst");
        fs::create_dir(&src).unwrap();
        let inner = src.join("hooks");
        fs::create_dir(&inner).unwrap();
        fs::set_permissions(&inner, fs::Permissions::from_mode(0o700)).unwrap();

        copy_dir(&src, &dst).unwrap();

        let mode = fs::metadata(dst.join("hooks"))
            .unwrap()
            .permissions()
            .mode()
            & 0o777;
        assert_eq!(mode, 0o700);
    }

    #[cfg(unix)]
    #[test]
    fn copy_dir_populates_a_read_only_source_directory_before_sealing_it() {
        // A 0555 directory — what a tarball restore or `chmod -R a-w` leaves —
        // used to be reproduced at the destination *before* anything was
        // written into it, so every child creation failed. For `copy:` that is
        // self-perpetuating: the entry fails, the partial tree is discarded,
        // and the next run does it again forever.
        let tmp = TempDir::new().unwrap();
        let src = tmp.path().join("src");
        let dst = tmp.path().join("dst");
        write_file(&src.join("archive/nested/deep.txt"), b"payload");
        write_file(&src.join("archive/top.txt"), b"top");
        fs::set_permissions(
            src.join("archive/nested"),
            fs::Permissions::from_mode(0o555),
        )
        .unwrap();
        fs::set_permissions(src.join("archive"), fs::Permissions::from_mode(0o555)).unwrap();

        // A root euid ignores these bits, and some filesystems do too.
        let premise_holds = fs::File::create(src.join("archive/probe")).is_err();

        let result = copy_dir(&src, &dst);

        fs::set_permissions(src.join("archive"), fs::Permissions::from_mode(0o755)).unwrap();
        fs::set_permissions(
            src.join("archive/nested"),
            fs::Permissions::from_mode(0o755),
        )
        .unwrap();
        for dir in ["archive", "archive/nested"] {
            let _ = fs::set_permissions(dst.join(dir), fs::Permissions::from_mode(0o755));
        }
        if !premise_holds {
            return;
        }

        result.expect("a read-only source directory must still copy");
        assert_eq!(fs::read(dst.join("archive/top.txt")).unwrap(), b"top");
        assert_eq!(
            fs::read(dst.join("archive/nested/deep.txt")).unwrap(),
            b"payload"
        );
    }

    #[cfg(unix)]
    #[test]
    fn copy_dir_still_reproduces_read_only_modes_once_populated() {
        // The deferred pass must not quietly widen permissions: the mode is
        // applied, just last.
        let tmp = TempDir::new().unwrap();
        let src = tmp.path().join("src");
        let dst = tmp.path().join("dst");
        write_file(&src.join("ro/file.txt"), b"x");
        fs::set_permissions(src.join("ro"), fs::Permissions::from_mode(0o555)).unwrap();

        copy_dir(&src, &dst).unwrap();
        let mode = fs::metadata(dst.join("ro")).unwrap().permissions().mode() & 0o777;
        fs::set_permissions(src.join("ro"), fs::Permissions::from_mode(0o755)).unwrap();
        let _ = fs::set_permissions(dst.join("ro"), fs::Permissions::from_mode(0o755));

        assert_eq!(mode, 0o555);
    }

    /// Whether this filesystem can reflink at all — the premise guard for
    /// tests that assert fast-path-only behavior (the same stance as the
    /// root-euid guard in the read-only-source test above).
    #[cfg(target_os = "macos")]
    fn reflink_works_in(dir: &Path) -> bool {
        let probe_src = dir.join(".probe-src");
        let probe_dst = dir.join(".probe-dst");
        fs::write(&probe_src, b"probe").unwrap();
        let ok = reflink_copy::reflink(&probe_src, &probe_dst).is_ok();
        let _ = fs::remove_file(&probe_src);
        let _ = fs::remove_file(&probe_dst);
        ok
    }

    /// The clone fast path is *observable*: `clonefile(2)` preserves file
    /// mtimes, while the walking path re-creates files "now". If this starts
    /// failing on APFS, the fast path has silently stopped engaging — every
    /// other test in this module still passes through the walking fallback,
    /// hiding exactly the regression this one exists to catch. (Preserved
    /// mtimes are also load-bearing on their own: a cloned cache is exactly
    /// as stale as its source, and the copy-source warmth ranking should see
    /// it that way.)
    #[cfg(target_os = "macos")]
    #[test]
    fn clone_fast_path_engages_and_preserves_mtimes() {
        let tmp = TempDir::new().unwrap();
        if !reflink_works_in(tmp.path()) {
            return;
        }
        let src = tmp.path().join("src");
        let dst = tmp.path().join("dst");
        write_file(&src.join("stale.txt"), b"old");
        let a_month_ago =
            std::time::SystemTime::now() - std::time::Duration::from_secs(30 * 24 * 3600);
        fs::File::options()
            .write(true)
            .open(src.join("stale.txt"))
            .unwrap()
            .set_modified(a_month_ago)
            .unwrap();

        copy_dir(&src, &dst).unwrap();

        let copied_mtime = fs::metadata(dst.join("stale.txt"))
            .unwrap()
            .modified()
            .unwrap();
        let age = std::time::SystemTime::now()
            .duration_since(copied_mtime)
            .unwrap();
        assert!(
            age > std::time::Duration::from_secs(29 * 24 * 3600),
            "a fresh mtime means the walking path ran — the clone fast path \
             stopped engaging (age: {age:?})"
        );
    }

    /// setuid/setgid survive a tree copy on both paths. The kernel strips
    /// them on clone, so the fast path's fix-up walk must restore them; the
    /// walking path preserves them via its per-file chmod.
    #[cfg(unix)]
    #[test]
    fn copy_dir_preserves_setuid_bits_in_the_tree() {
        let tmp = TempDir::new().unwrap();
        let src = tmp.path().join("src");
        let dst = tmp.path().join("dst");
        write_file(&src.join("bin/tool"), b"#!");
        fs::set_permissions(src.join("bin/tool"), fs::Permissions::from_mode(0o4755)).unwrap();

        copy_dir(&src, &dst).unwrap();

        let mode = fs::metadata(dst.join("bin/tool"))
            .unwrap()
            .permissions()
            .mode()
            & 0o7777;
        assert_eq!(mode, 0o4755);
    }

    /// Pins a documented divergence: the clone path reproduces FIFOs (the
    /// kernel clones every entry), where the walking path skips them. If a
    /// consumer ever needs skip-semantics for special files, this is the
    /// assertion to renegotiate — deliberately, not by accident.
    #[cfg(target_os = "macos")]
    #[test]
    fn clone_fast_path_clones_fifos() {
        use std::os::unix::fs::FileTypeExt;

        let tmp = TempDir::new().unwrap();
        if !reflink_works_in(tmp.path()) {
            return;
        }
        let src = tmp.path().join("src");
        let dst = tmp.path().join("dst");
        write_file(&src.join("real.txt"), b"x");
        let status = std::process::Command::new("mkfifo")
            .arg(src.join("pipe"))
            .status()
            .unwrap();
        assert!(status.success());

        copy_dir(&src, &dst).unwrap();

        let ftype = fs::symlink_metadata(dst.join("pipe")).unwrap().file_type();
        assert!(ftype.is_fifo());
    }

    /// A link that escapes the tree from a position of EQUAL depth re-bases
    /// to text identical to what it already carries — the fix-up walk must
    /// recognize that and leave the link alone (and the result must still
    /// resolve).
    #[cfg(unix)]
    #[test]
    fn escaping_link_at_equal_depth_keeps_identical_text() {
        let tmp = TempDir::new().unwrap();
        write_file(&tmp.path().join(".venvs/proj/marker"), b"m");
        let src = tmp.path().join("a/cache");
        let dst = tmp.path().join("b/cache");
        fs::create_dir_all(&src).unwrap();
        fs::create_dir_all(dst.parent().unwrap()).unwrap();
        symlink("../../.venvs/proj", src.join("venv")).unwrap();

        copy_dir(&src, &dst).unwrap();

        assert_eq!(
            fs::read_link(dst.join("venv")).unwrap(),
            Path::new("../../.venvs/proj"),
            "equal-depth escape needs no rewrite"
        );
        assert!(dst.join("venv/marker").exists());
    }

    #[test]
    fn copy_reporting_counts_what_actually_happened() {
        // The annotation `copy:` shows is built from these numbers, so they
        // have to describe the destination, not the source: `bytes` counts
        // every file written, and the method split comes from the copier
        // itself rather than a guess about the filesystem.
        let tmp = TempDir::new().unwrap();
        let src = tmp.path().join("src");
        let dst = tmp.path().join("dst");
        write_file(&src.join("a.txt"), b"12345");
        write_file(&src.join("sub/b.txt"), b"678");

        let stats = copy_dir_reporting(&src, &dst).unwrap();
        assert_eq!(stats.bytes, 8);
        assert_eq!(
            stats.reflinked + stats.copied,
            2,
            "every regular file is accounted for exactly once, whichever way it moved"
        );

        let file_stats = copy_file_reporting(&src.join("a.txt"), &tmp.path().join("solo")).unwrap();
        assert_eq!(file_stats.bytes, 5);
        assert_eq!(file_stats.reflinked + file_stats.copied, 1);
    }

    #[cfg(unix)]
    #[test]
    fn copy_reporting_does_not_deduplicate_hard_links() {
        // `du` counts a hard-linked source once; the copy writes independent
        // files, so the destination really does cost the full sum. Gating
        // `max_size` on the deduplicated figure is how a 500 MB measurement
        // lands 5 GB.
        let tmp = TempDir::new().unwrap();
        let src = tmp.path().join("src");
        let dst = tmp.path().join("dst");
        write_file(&src.join("real"), &vec![7u8; 1000]);
        fs::hard_link(src.join("real"), src.join("link")).unwrap();

        assert_eq!(copy_dir_reporting(&src, &dst).unwrap().bytes, 2000);
    }

    fn walking(src: &Path, dst: &Path, move_root: &Path) -> Result<CopyStats> {
        let perms = fs::symlink_metadata(src).unwrap().permissions();
        copy_dir_walking(src, dst, move_root, perms)
    }

    /// Twin of `copy_dir_populates_a_read_only_source_directory_before_sealing_it`,
    /// pinned to the WALKING path: on APFS every `copy_dir` call takes the
    /// clone fast path, which would leave the deferred-modes machinery — the
    /// thing that test exists to cover — exercised only on Linux CI.
    #[cfg(unix)]
    #[test]
    fn walking_path_populates_a_read_only_source_directory_before_sealing_it() {
        let tmp = TempDir::new().unwrap();
        let src = tmp.path().join("src");
        let dst = tmp.path().join("dst");
        write_file(&src.join("archive/nested/deep.txt"), b"payload");
        write_file(&src.join("archive/top.txt"), b"top");
        fs::set_permissions(
            src.join("archive/nested"),
            fs::Permissions::from_mode(0o555),
        )
        .unwrap();
        fs::set_permissions(src.join("archive"), fs::Permissions::from_mode(0o555)).unwrap();

        let premise_holds = fs::File::create(src.join("archive/probe")).is_err();

        let result = walking(&src, &dst, &src);

        fs::set_permissions(src.join("archive"), fs::Permissions::from_mode(0o755)).unwrap();
        fs::set_permissions(
            src.join("archive/nested"),
            fs::Permissions::from_mode(0o755),
        )
        .unwrap();
        let mut modes = Vec::new();
        for dir in ["archive", "archive/nested"] {
            if let Ok(meta) = fs::metadata(dst.join(dir)) {
                modes.push(meta.permissions().mode() & 0o777);
            }
            let _ = fs::set_permissions(dst.join(dir), fs::Permissions::from_mode(0o755));
        }
        if !premise_holds {
            return;
        }

        result.expect("a read-only source directory must still copy");
        assert_eq!(fs::read(dst.join("archive/top.txt")).unwrap(), b"top");
        assert_eq!(
            fs::read(dst.join("archive/nested/deep.txt")).unwrap(),
            b"payload"
        );
        assert_eq!(modes, vec![0o555, 0o555], "modes still applied, just last");
    }

    /// Twin of `copy_dir_rebases_a_link_that_escapes_the_copied_tree` for the
    /// walking path, same rationale as above.
    #[cfg(unix)]
    #[test]
    fn walking_path_rebases_a_link_that_escapes_the_copied_tree() {
        let tmp = TempDir::new().unwrap();
        write_file(&tmp.path().join(".venvs/proj/bin/python"), b"#!");
        let src = tmp.path().join("main/cache");
        let dst = tmp.path().join("feature/login/cache");
        fs::create_dir_all(&src).unwrap();
        fs::create_dir_all(dst.parent().unwrap()).unwrap();
        symlink("../../.venvs/proj", src.join("venv")).unwrap();

        walking(&src, &dst, &src).unwrap();

        assert!(dst.join("venv/bin/python").exists());
    }

    /// Twin of `copy_reporting_does_not_deduplicate_hard_links` for the
    /// walking path.
    #[cfg(unix)]
    #[test]
    fn walking_path_counts_hard_links_every_time() {
        let tmp = TempDir::new().unwrap();
        let src = tmp.path().join("src");
        let dst = tmp.path().join("dst");
        write_file(&src.join("real"), &vec![7u8; 1000]);
        fs::hard_link(src.join("real"), src.join("link")).unwrap();

        assert_eq!(walking(&src, &dst, &src).unwrap().bytes, 2000);
    }

    /// The pool must copy a wide, deep tree completely and account for every
    /// file exactly once — width is what actually exercises concurrent
    /// workers, and the recount catches both lost subtrees and double-copies.
    #[test]
    fn walking_path_copies_a_wide_deep_tree_completely() {
        let tmp = TempDir::new().unwrap();
        let src = tmp.path().join("src");
        let dst = tmp.path().join("dst");
        let mut expected_files = 0u64;
        let mut expected_bytes = 0u64;
        for a in 0..4 {
            for b in 0..4 {
                for c in 0..4 {
                    for f in 0..6 {
                        let content = format!("{a}/{b}/{c}/{f}");
                        write_file(
                            &src.join(format!("d{a}/d{b}/d{c}/f{f}.txt")),
                            content.as_bytes(),
                        );
                        expected_files += 1;
                        expected_bytes += content.len() as u64;
                    }
                }
            }
        }

        let stats = walking(&src, &dst, &src).unwrap();

        assert_eq!(stats.reflinked + stats.copied, expected_files);
        assert_eq!(stats.bytes, expected_bytes);
        let mut seen = 0;
        for entry in walkdir::WalkDir::new(&dst).min_depth(1) {
            let entry = entry.unwrap();
            if entry.file_type().is_file() {
                seen += 1;
            }
        }
        assert_eq!(seen, expected_files);
        assert_eq!(
            fs::read(dst.join("d3/d2/d1/f5.txt")).unwrap(),
            b"3/2/1/5",
            "content lands at the mirrored path"
        );
    }

    /// An unreadable directory anywhere in the tree fails the copy with a
    /// real error — the pool must surface it, not deadlock or swallow it.
    #[cfg(unix)]
    #[test]
    fn walking_path_surfaces_an_unreadable_subtree_as_an_error() {
        let tmp = TempDir::new().unwrap();
        let src = tmp.path().join("src");
        let dst = tmp.path().join("dst");
        write_file(&src.join("ok/a.txt"), b"a");
        write_file(&src.join("sealed/hidden.txt"), b"h");
        fs::set_permissions(src.join("sealed"), fs::Permissions::from_mode(0o000)).unwrap();

        let premise_holds = fs::read_dir(src.join("sealed")).is_err();
        let result = walking(&src, &dst, &src);
        fs::set_permissions(src.join("sealed"), fs::Permissions::from_mode(0o755)).unwrap();
        if !premise_holds {
            return;
        }

        let err = result.expect_err("an unreadable subtree must fail the copy");
        assert!(
            format!("{err:#}").contains("sealed"),
            "the error names the unreadable directory: {err:#}"
        );
    }

    #[test]
    fn copy_dir_errors_when_destination_exists() {
        let tmp = TempDir::new().unwrap();
        let src = tmp.path().join("src");
        let dst = tmp.path().join("dst");
        fs::create_dir(&src).unwrap();
        fs::create_dir(&dst).unwrap();

        assert!(copy_dir(&src, &dst).is_err());
    }

    #[test]
    fn copy_dir_errors_when_source_is_a_file() {
        let tmp = TempDir::new().unwrap();
        let src = tmp.path().join("not-a-dir");
        let dst = tmp.path().join("dst");
        write_file(&src, b"hi");

        assert!(copy_dir(&src, &dst).is_err());
    }
}
