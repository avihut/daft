//! Copy-on-write file and directory copies.
//!
//! Wraps [`reflink_copy::reflink_or_copy`] so callers get APFS clonefile on
//! macOS, `ioctl(FICLONE)` on reflink-capable Linux filesystems (btrfs, XFS
//! with `reflink=1`, OpenZFS 2.2+, bcachefs), and block-clone on Windows ReFS
//! — with a transparent byte-copy fallback everywhere else.
//!
//! [`copy_file`] is the file-level primitive; [`copy_dir`] recursively
//! reproduces a directory tree using [`copy_file`] for regular files and
//! recreating directories and symlinks. Mode bits are preserved per entry;
//! ownership, timestamps, and xattrs are not — current callsites don't
//! depend on them, and #387 will revisit if its `copy_paths:` surface needs
//! richer semantics.

use anyhow::{Context, Result};
use std::fs;
use std::path::{Path, PathBuf};
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
    // the byte-copy fallback copies mode. Normalize across all three.
    let src_meta = fs::symlink_metadata(src)
        .with_context(|| format!("reading metadata of {}", src.display()))?;
    fs::set_permissions(dst, src_meta.permissions())
        .with_context(|| format!("setting mode of {}", dst.display()))?;
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
/// Directory modes are applied in a **deferred, deepest-first pass** after the
/// tree is populated, never as each directory is created. Applying them up
/// front reproduces a read-only source directory (0555 — what a tarball restore
/// or a `chmod -R a-w` archive leaves behind) before anything can be written
/// into it, so every child creation fails. For `copy:` that failure is
/// self-perpetuating: the entry fails, the partial destination is discarded,
/// and the next run repeats it forever.
pub fn copy_dir_reporting(src: &Path, dst: &Path) -> Result<CopyStats> {
    let src_meta = fs::symlink_metadata(src)
        .with_context(|| format!("reading metadata of {}", src.display()))?;
    anyhow::ensure!(
        src_meta.is_dir(),
        "copy_dir source is not a directory: {}",
        src.display()
    );
    create_writable_dir(dst).with_context(|| format!("creating {}", dst.display()))?;

    let mut stats = CopyStats::default();
    // (path, source permissions), in the walk's top-down order — replayed in
    // reverse so a directory is sealed only after its children exist.
    let mut deferred_modes: Vec<(PathBuf, fs::Permissions)> =
        vec![(dst.to_path_buf(), src_meta.permissions())];

    for entry in WalkDir::new(src).follow_links(false).min_depth(1) {
        let entry = entry.with_context(|| format!("walking {}", src.display()))?;
        let rel = entry
            .path()
            .strip_prefix(src)
            .expect("walkdir paths are rooted at src");
        let dst_path = dst.join(rel);
        let ftype = entry.file_type();

        if ftype.is_dir() {
            create_writable_dir(&dst_path)
                .with_context(|| format!("creating {}", dst_path.display()))?;
            let meta = entry
                .metadata()
                .with_context(|| format!("reading metadata of {}", entry.path().display()))?;
            deferred_modes.push((dst_path, meta.permissions()));
        } else if ftype.is_symlink() {
            #[cfg(unix)]
            {
                let target = fs::read_link(entry.path())
                    .with_context(|| format!("reading symlink {}", entry.path().display()))?;
                std::os::unix::fs::symlink(target, &dst_path)
                    .with_context(|| format!("creating symlink {}", dst_path.display()))?;
            }
            #[cfg(not(unix))]
            {
                // Windows / non-Unix targets: symlink replication needs the
                // file-vs-dir distinction up front (`symlink_file` /
                // `symlink_dir`) and admin/dev-mode privileges. Daft's
                // current consumers don't produce symlinks in their copy
                // trees; #387's `copy_paths:` work will revisit if needed.
                anyhow::bail!(
                    "symlink replication not yet implemented on this platform: {}",
                    entry.path().display()
                );
            }
        } else if ftype.is_file() {
            stats.add(copy_file_reporting(entry.path(), &dst_path)?);
        }
        // Block / char / fifo / socket entries are skipped. Daft's existing
        // callsites don't produce them; future consumers that need them
        // should extend this dispatch deliberately.
    }

    // Deepest-first: a directory can only be made read-only once nothing more
    // needs to be written inside it.
    for (path, permissions) in deferred_modes.into_iter().rev() {
        fs::set_permissions(&path, permissions)
            .with_context(|| format!("setting mode of {}", path.display()))?;
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
