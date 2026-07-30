//! Filtering a job's file list by what the paths *are*.
//!
//! `glob:` says where files live. It cannot say that `assets/logo.png` is a
//! PNG and must not be handed to a formatter, or that `scripts/deploy` is the
//! executable this shellcheck job means. `file_types:` answers that, and is
//! applied after glob selection so the two compose in the obvious order.

use anyhow::{Result, bail};
use std::path::Path;

/// One predicate over a path's nature.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FileType {
    Text,
    Binary,
    Executable,
    NotExecutable,
    Symlink,
    NotSymlink,
}

impl FileType {
    /// Parse a spelling from `file_types:`.
    ///
    /// The negative forms are spelled `not executable` rather than
    /// `non-executable` because that is how they read aloud, and whitespace
    /// is normalised so `not  executable` is the same value.
    pub fn parse(raw: &str) -> Result<Self> {
        let normalised = raw.split_whitespace().collect::<Vec<_>>().join(" ");
        Ok(match normalised.as_str() {
            "text" => FileType::Text,
            "binary" => FileType::Binary,
            "executable" => FileType::Executable,
            "not executable" => FileType::NotExecutable,
            "symlink" => FileType::Symlink,
            "not symlink" => FileType::NotSymlink,
            other => bail!(
                "unknown file type '{other}': expected one of text, binary, executable, \
                 not executable, symlink, not symlink"
            ),
        })
    }

    /// Whether `path` (relative to `root`) satisfies this predicate.
    ///
    /// A path that cannot be examined — deleted between the diff and the
    /// check, or a broken symlink — fails every positive predicate and
    /// satisfies the negative ones. That keeps a vanished file out of a
    /// formatter's argv without turning a race into a hook failure.
    pub fn matches(self, root: &Path, path: &str) -> bool {
        let full = root.join(path);
        match self {
            FileType::Symlink => is_symlink(&full),
            FileType::NotSymlink => !is_symlink(&full),
            FileType::Executable => is_executable(&full),
            FileType::NotExecutable => !is_executable(&full),
            FileType::Text => is_text(&full).unwrap_or(false),
            FileType::Binary => !is_text(&full).unwrap_or(true),
        }
    }
}

/// Keep the paths satisfying every predicate. An empty predicate list keeps
/// everything.
pub fn filter(root: &Path, files: &[String], types: &[FileType]) -> Vec<String> {
    if types.is_empty() {
        return files.to_vec();
    }
    files
        .iter()
        .filter(|f| types.iter().all(|t| t.matches(root, f)))
        .cloned()
        .collect()
}

fn is_symlink(path: &Path) -> bool {
    std::fs::symlink_metadata(path).is_ok_and(|m| m.file_type().is_symlink())
}

#[cfg(unix)]
fn is_executable(path: &Path) -> bool {
    use std::os::unix::fs::PermissionsExt;
    // Follows symlinks deliberately: "is this executable" is a question about
    // what running it would do.
    std::fs::metadata(path).is_ok_and(|m| m.permissions().mode() & 0o111 != 0)
}

#[cfg(not(unix))]
fn is_executable(path: &Path) -> bool {
    // No permission bits to read; treat everything as non-executable rather
    // than claim a capability the platform does not express.
    let _ = path;
    false
}

/// Whether a file looks like text.
///
/// git's own heuristic, and for the same reason: a NUL byte in the first
/// block is what every tool in this space treats as the binary signal, and
/// agreeing with git means `file_types: text` selects the same files
/// `git diff` will show as text.
fn is_text(path: &Path) -> Result<bool> {
    use std::io::Read;
    const PROBE: usize = 8000;

    let mut file = std::fs::File::open(path)?;
    let mut buf = [0u8; PROBE];
    let read = file.read(&mut buf)?;
    Ok(!buf[..read].contains(&0))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use tempfile::tempdir;

    #[test]
    fn spellings_parse_including_whitespace_variants() {
        assert_eq!(FileType::parse("text").unwrap(), FileType::Text);
        assert_eq!(FileType::parse("binary").unwrap(), FileType::Binary);
        assert_eq!(
            FileType::parse("not executable").unwrap(),
            FileType::NotExecutable
        );
        assert_eq!(
            FileType::parse("not   executable").unwrap(),
            FileType::NotExecutable
        );
        assert_eq!(FileType::parse("symlink").unwrap(), FileType::Symlink);
        assert_eq!(
            FileType::parse("not symlink").unwrap(),
            FileType::NotSymlink
        );
    }

    #[test]
    fn an_unknown_spelling_names_the_alternatives() {
        let err = FileType::parse("nonexecutable").unwrap_err();
        let msg = format!("{err:#}");
        assert!(msg.contains("nonexecutable"), "{msg}");
        assert!(msg.contains("not executable"), "{msg}");
    }

    #[test]
    fn text_and_binary_split_on_a_nul_byte() {
        let dir = tempdir().unwrap();
        fs::write(dir.path().join("a.txt"), "hello\nworld\n").unwrap();
        fs::write(dir.path().join("b.bin"), [0x89, 0x50, 0x00, 0x01]).unwrap();

        assert!(FileType::Text.matches(dir.path(), "a.txt"));
        assert!(!FileType::Binary.matches(dir.path(), "a.txt"));
        assert!(FileType::Binary.matches(dir.path(), "b.bin"));
        assert!(!FileType::Text.matches(dir.path(), "b.bin"));
    }

    #[test]
    #[cfg(unix)]
    fn executable_reads_the_permission_bits() {
        use std::os::unix::fs::PermissionsExt;
        let dir = tempdir().unwrap();
        let script = dir.path().join("deploy");
        fs::write(&script, "#!/bin/sh\n").unwrap();
        fs::set_permissions(&script, fs::Permissions::from_mode(0o755)).unwrap();
        let plain = dir.path().join("notes.md");
        fs::write(&plain, "hi").unwrap();

        assert!(FileType::Executable.matches(dir.path(), "deploy"));
        assert!(!FileType::NotExecutable.matches(dir.path(), "deploy"));
        assert!(FileType::NotExecutable.matches(dir.path(), "notes.md"));
    }

    #[test]
    #[cfg(unix)]
    fn symlinks_are_detected_without_following() {
        let dir = tempdir().unwrap();
        fs::write(dir.path().join("real.txt"), "x").unwrap();
        std::os::unix::fs::symlink("real.txt", dir.path().join("link.txt")).unwrap();

        assert!(FileType::Symlink.matches(dir.path(), "link.txt"));
        assert!(!FileType::Symlink.matches(dir.path(), "real.txt"));
        assert!(FileType::NotSymlink.matches(dir.path(), "real.txt"));
    }

    #[test]
    fn predicates_are_anded() {
        let dir = tempdir().unwrap();
        fs::write(dir.path().join("a.txt"), "text").unwrap();
        fs::write(dir.path().join("b.bin"), [0u8, 1]).unwrap();

        let files = vec!["a.txt".to_string(), "b.bin".to_string()];
        let kept = filter(
            dir.path(),
            &files,
            &[FileType::Text, FileType::NotExecutable],
        );
        assert_eq!(kept, vec!["a.txt".to_string()]);
    }

    #[test]
    fn no_predicates_keeps_everything() {
        let files = vec!["a".to_string(), "b".to_string()];
        assert_eq!(filter(Path::new("/nope"), &files, &[]), files);
    }

    #[test]
    fn a_vanished_path_fails_positives_and_passes_negatives() {
        // A file deleted between the diff and the check is a race, not a
        // config error: keep it out of a formatter's argv without failing
        // the hook.
        let dir = tempdir().unwrap();
        assert!(!FileType::Text.matches(dir.path(), "gone.txt"));
        assert!(!FileType::Executable.matches(dir.path(), "gone.txt"));
        assert!(!FileType::Symlink.matches(dir.path(), "gone.txt"));
        assert!(FileType::NotExecutable.matches(dir.path(), "gone.txt"));
        assert!(FileType::NotSymlink.matches(dir.path(), "gone.txt"));
    }
}
