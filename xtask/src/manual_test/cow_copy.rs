//! Copy-on-write directory copy for the manual-test sandbox.
//!
//! Used by `sandbox.rs` to snapshot `remotes/` → `remotes-template/` and to
//! restore from the template. On APFS this is clonefile; on reflink-capable
//! Linux filesystems (btrfs, XFS with `reflink=1`, OpenZFS 2.2+, bcachefs)
//! it's `ioctl(FICLONE)`. Other filesystems silently fall back to a byte
//! copy.
//!
//! Intentionally duplicates the logic that lives in the daft library's
//! `daft::cow_copy` module (introduced by #511 for #387's `copy_paths:`
//! feature). The duplication is deliberate: the YAML runner port rule in
//! the project's CLAUDE.md forbids `runner.rs / sandbox.rs / executor.rs`
//! from importing `daft::*`, because the runner is intended to spin out as
//! its own product. Sharing the helper would either pull `daft` into the
//! spin-out's dep set or require widening that port — both larger
//! commitments than the ~30 lines below.

use anyhow::{Context, Result};
use std::fs;
use std::os::unix::fs::symlink;
use std::path::Path;
use walkdir::WalkDir;

/// Recursively copy `src` → `dst`, using reflink where supported and a byte
/// copy fallback otherwise. Mode bits are preserved per entry; symlinks are
/// recreated without dereferencing.
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
            let target = fs::read_link(entry.path())
                .with_context(|| format!("reading symlink {}", entry.path().display()))?;
            symlink(target, &dst_path)
                .with_context(|| format!("creating symlink {}", dst_path.display()))?;
        } else if ftype.is_file() {
            copy_file(entry.path(), &dst_path)?;
        }
        // Block / char / fifo / socket entries are skipped. Sandbox fixtures
        // don't produce them.
    }
    Ok(())
}

/// Copy a single regular file, reflink-or-byte-copy. Preserves mode bits.
fn copy_file(src: &Path, dst: &Path) -> Result<()> {
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

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;
    use std::os::unix::fs::PermissionsExt;
    use tempfile::TempDir;

    fn write_file(path: &Path, content: &[u8]) {
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent).unwrap();
        }
        let mut f = fs::File::create(path).unwrap();
        f.write_all(content).unwrap();
    }

    #[test]
    fn replicates_tree_and_survives_source_mutation() {
        let tmp = TempDir::new().unwrap();
        let src = tmp.path().join("src");
        let dst = tmp.path().join("dst");
        write_file(&src.join("a.txt"), b"alpha");
        write_file(&src.join("nested/b.txt"), b"beta");
        fs::set_permissions(src.join("a.txt"), fs::Permissions::from_mode(0o755)).unwrap();

        copy_dir(&src, &dst).unwrap();

        assert_eq!(fs::read(dst.join("a.txt")).unwrap(), b"alpha");
        assert_eq!(fs::read(dst.join("nested/b.txt")).unwrap(), b"beta");
        assert_eq!(
            fs::metadata(dst.join("a.txt"))
                .unwrap()
                .permissions()
                .mode()
                & 0o777,
            0o755
        );

        write_file(&src.join("a.txt"), b"MUTATED");
        assert_eq!(fs::read(dst.join("a.txt")).unwrap(), b"alpha");
    }

    #[test]
    fn recreates_symlinks_without_dereferencing() {
        let tmp = TempDir::new().unwrap();
        let src = tmp.path().join("src");
        let dst = tmp.path().join("dst");
        fs::create_dir(&src).unwrap();
        write_file(&src.join("target.txt"), b"linked");
        symlink("target.txt", src.join("link.txt")).unwrap();

        copy_dir(&src, &dst).unwrap();

        assert!(fs::symlink_metadata(dst.join("link.txt"))
            .unwrap()
            .file_type()
            .is_symlink());
        assert_eq!(
            fs::read_link(dst.join("link.txt")).unwrap(),
            Path::new("target.txt")
        );
    }

    #[test]
    fn errors_when_destination_exists() {
        let tmp = TempDir::new().unwrap();
        let src = tmp.path().join("src");
        let dst = tmp.path().join("dst");
        fs::create_dir(&src).unwrap();
        fs::create_dir(&dst).unwrap();

        assert!(copy_dir(&src, &dst).is_err());
    }

    #[test]
    fn errors_when_source_is_a_file() {
        let tmp = TempDir::new().unwrap();
        let src = tmp.path().join("not-a-dir");
        let dst = tmp.path().join("dst");
        write_file(&src, b"hi");

        assert!(copy_dir(&src, &dst).is_err());
    }
}
