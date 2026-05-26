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
use std::path::Path;
use walkdir::WalkDir;

/// Copy a regular file from `src` to `dst`, using reflink where supported
/// and a byte copy otherwise. Preserves mode bits.
///
/// `dst` must not already exist; its parent must.
pub fn copy_file(src: &Path, dst: &Path) -> Result<()> {
    reflink_copy::reflink_or_copy(src, dst)
        .with_context(|| format!("copying {} -> {}", src.display(), dst.display()))?;
    // Linux `FICLONE` copies content only; macOS clonefile copies metadata;
    // the byte-copy fallback copies mode. Normalize across all three.
    let src_meta = fs::symlink_metadata(src)
        .with_context(|| format!("reading metadata of {}", src.display()))?;
    fs::set_permissions(dst, src_meta.permissions())
        .with_context(|| format!("setting mode of {}", dst.display()))?;
    Ok(())
}

/// Recursively copy a directory tree from `src` to `dst`.
///
/// Regular files go through [`copy_file`] (reflink-or-byte-copy); directories
/// are recreated empty and then populated; symlinks are recreated with the
/// same link target (never dereferenced). Mode bits are preserved per entry.
///
/// `dst` must not already exist; its parent must. `src` must be a directory.
pub fn copy_dir(src: &Path, dst: &Path) -> Result<()> {
    let src_meta = fs::symlink_metadata(src)
        .with_context(|| format!("reading metadata of {}", src.display()))?;
    anyhow::ensure!(
        src_meta.is_dir(),
        "copy_dir source is not a directory: {}",
        src.display()
    );
    fs::create_dir(dst).with_context(|| format!("creating {}", dst.display()))?;
    fs::set_permissions(dst, src_meta.permissions())
        .with_context(|| format!("setting mode of {}", dst.display()))?;

    for entry in WalkDir::new(src).follow_links(false).min_depth(1) {
        let entry = entry.with_context(|| format!("walking {}", src.display()))?;
        let rel = entry
            .path()
            .strip_prefix(src)
            .expect("walkdir paths are rooted at src");
        let dst_path = dst.join(rel);
        let ftype = entry.file_type();

        if ftype.is_dir() {
            fs::create_dir(&dst_path)
                .with_context(|| format!("creating {}", dst_path.display()))?;
            let meta = entry
                .metadata()
                .with_context(|| format!("reading metadata of {}", entry.path().display()))?;
            fs::set_permissions(&dst_path, meta.permissions())
                .with_context(|| format!("setting mode of {}", dst_path.display()))?;
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
            copy_file(entry.path(), &dst_path)?;
        }
        // Block / char / fifo / socket entries are skipped. Daft's existing
        // callsites don't produce them; future consumers that need them
        // should extend this dispatch deliberately.
    }
    Ok(())
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
