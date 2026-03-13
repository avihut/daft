//! Homebrew installation detection.
//!
//! Detects whether daft is running from a Homebrew-managed installation
//! by checking if the canonicalized executable path contains a `/Cellar/`
//! path segment. This is used to adjust symlink management behavior:
//! command symlinks are managed by Homebrew, while shortcut symlinks
//! are managed by daft in the brew prefix bin directory.

use std::path::{Path, PathBuf};

/// Information about a detected Homebrew installation.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HomebrewInfo {
    /// The Homebrew prefix (e.g., `/opt/homebrew` or `/usr/local`).
    pub prefix: PathBuf,
}

impl HomebrewInfo {
    /// Returns the bin directory under the Homebrew prefix.
    pub fn bin_dir(&self) -> PathBuf {
        self.prefix.join("bin")
    }
}

/// Detect Homebrew installation from the current executable path.
///
/// Calls `current_exe()` and canonicalizes the result, then checks for
/// a `/Cellar/` path segment. Returns `Some(HomebrewInfo)` if detected.
pub fn detect() -> Option<HomebrewInfo> {
    let exe_path = std::env::current_exe().ok()?;
    let canonical = exe_path.canonicalize().ok()?;
    detect_from_path(&canonical)
}

/// Detect Homebrew installation from a given path (for testing).
///
/// Checks if the path contains a `/Cellar/` component and derives the
/// brew prefix from everything before it.
pub fn detect_from_path(path: &Path) -> Option<HomebrewInfo> {
    let components: Vec<_> = path.components().collect();
    for (i, component) in components.iter().enumerate() {
        if component.as_os_str() == "Cellar" {
            // Build prefix from all components before "Cellar"
            let prefix: PathBuf = components[..i].iter().collect();
            if prefix.as_os_str().is_empty() {
                return None;
            }
            return Some(HomebrewInfo { prefix });
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    #[test]
    fn test_detect_apple_silicon() {
        let path = PathBuf::from("/opt/homebrew/Cellar/daft/1.0.36/bin/daft");
        let info = detect_from_path(&path);
        assert!(info.is_some());
        let info = info.unwrap();
        assert_eq!(info.prefix, PathBuf::from("/opt/homebrew"));
        assert_eq!(info.bin_dir(), PathBuf::from("/opt/homebrew/bin"));
    }

    #[test]
    fn test_detect_intel_mac() {
        let path = PathBuf::from("/usr/local/Cellar/daft/1.0.36/bin/daft");
        let info = detect_from_path(&path);
        assert!(info.is_some());
        assert_eq!(info.unwrap().prefix, PathBuf::from("/usr/local"));
    }

    #[test]
    fn test_detect_linuxbrew() {
        let path = PathBuf::from("/home/linuxbrew/.linuxbrew/Cellar/daft/1.0.36/bin/daft");
        let info = detect_from_path(&path);
        assert!(info.is_some());
        assert_eq!(
            info.unwrap().prefix,
            PathBuf::from("/home/linuxbrew/.linuxbrew")
        );
    }

    #[test]
    fn test_not_homebrew_cargo() {
        let path = PathBuf::from("/Users/me/.cargo/bin/daft");
        assert!(detect_from_path(&path).is_none());
    }

    #[test]
    fn test_not_homebrew_system() {
        let path = PathBuf::from("/usr/bin/daft");
        assert!(detect_from_path(&path).is_none());
    }

    #[test]
    fn test_not_homebrew_dev() {
        let path = PathBuf::from("/Users/me/Projects/daft/target/release/daft");
        assert!(detect_from_path(&path).is_none());
    }

    #[test]
    fn test_cellar_in_filename_not_matched() {
        // "Cellar" in a filename, not as a path component
        let path = PathBuf::from("/usr/bin/Cellar-tool");
        assert!(detect_from_path(&path).is_none());
    }
}
