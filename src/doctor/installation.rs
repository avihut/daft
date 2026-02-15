//! Installation checks for `daft doctor`.
//!
//! Verifies that daft and its dependencies are correctly installed:
//! binary in PATH, command symlinks, git, man pages, shell integration.

use crate::doctor::CheckResult;
use std::path::{Path, PathBuf};

/// Expected command symlinks that should point to the daft binary.
const EXPECTED_SYMLINKS: &[&str] = &[
    "git-worktree-clone",
    "git-worktree-init",
    "git-worktree-checkout",
    "git-worktree-checkout-branch",
    "git-worktree-prune",
    "git-worktree-carry",
    "git-worktree-fetch",
    "git-worktree-flow-adopt",
    "git-worktree-flow-eject",
    "git-daft",
];

/// Check that the daft binary is in PATH.
pub fn check_binary_in_path() -> CheckResult {
    match which::which("daft") {
        Ok(path) => {
            let version = crate::VERSION;
            CheckResult::pass(
                "daft binary in PATH",
                &format!("{} v{version}", path.display()),
            )
        }
        Err(_) => CheckResult::fail("daft binary in PATH", "daft not found in PATH")
            .with_suggestion("Add the directory containing 'daft' to your PATH"),
    }
}

/// Check that all expected command symlinks are present and point to daft.
pub fn check_command_symlinks() -> CheckResult {
    let install_dir = match crate::commands::shortcuts::detect_install_dir() {
        Ok(dir) => dir,
        Err(_) => {
            return CheckResult::warning(
                "Command symlinks",
                "Could not detect installation directory",
            );
        }
    };

    let mut present = Vec::new();
    let mut missing = Vec::new();

    for &name in EXPECTED_SYMLINKS {
        let path = install_dir.join(name);
        if is_valid_symlink(&path, &install_dir) {
            present.push(name);
        } else {
            missing.push(name);
        }
    }

    let total = EXPECTED_SYMLINKS.len();
    let found = present.len();

    if missing.is_empty() {
        CheckResult::pass("Command symlinks", &format!("{found}/{total} present"))
    } else {
        let details: Vec<String> = missing.iter().map(|n| format!("Missing: {n}")).collect();
        CheckResult::warning(
            "Command symlinks",
            &format!("{found}/{total} present, {} missing", missing.len()),
        )
        .with_suggestion("Run 'daft setup' to create missing symlinks")
        .with_fixable(true)
        .with_details(details)
    }
}

/// Fix missing command symlinks by creating them.
pub fn fix_command_symlinks() -> Result<(), String> {
    let install_dir = crate::commands::shortcuts::detect_install_dir()
        .map_err(|e| format!("Could not detect installation directory: {e}"))?;

    for &name in EXPECTED_SYMLINKS {
        let path = install_dir.join(name);
        if !is_valid_symlink(&path, &install_dir) {
            create_symlink(name, &install_dir)?;
        }
    }

    Ok(())
}

/// Check that git is installed and report its version.
pub fn check_git() -> CheckResult {
    match which::which("git") {
        Ok(_) => match std::process::Command::new("git").arg("--version").output() {
            Ok(output) if output.status.success() => {
                let version_str = String::from_utf8_lossy(&output.stdout);
                let version = version_str
                    .trim()
                    .strip_prefix("git version ")
                    .unwrap_or(version_str.trim());
                CheckResult::pass("git", &format!("version {version}"))
            }
            _ => CheckResult::pass("git", "installed (could not determine version)"),
        },
        Err(_) => CheckResult::fail("git", "not found in PATH").with_suggestion("Install git"),
    }
}

/// Check that man pages are installed.
pub fn check_man_pages() -> CheckResult {
    let man_dirs = get_man_search_paths();

    // Check for a representative man page
    let test_pages = ["git-worktree-clone.1", "git-worktree-checkout.1"];
    let mut found_in: Option<PathBuf> = None;

    for dir in &man_dirs {
        let man1_dir = dir.join("man1");
        if man1_dir.exists() && test_pages.iter().all(|page| man1_dir.join(page).exists()) {
            found_in = Some(man1_dir);
            break;
        }
    }

    match found_in {
        Some(dir) => CheckResult::pass("Man pages installed", &format!("{}", dir.display())),
        None => CheckResult::warning("Man pages not installed", "man pages not found")
            .with_suggestion("Run 'mise run install-man' to install man pages"),
    }
}

/// Check that shell integration is configured.
pub fn check_shell_integration() -> CheckResult {
    let shell_name = match std::env::var("SHELL") {
        Ok(shell_path) => {
            if shell_path.contains("zsh") {
                "zsh"
            } else if shell_path.contains("bash") {
                "bash"
            } else if shell_path.contains("fish") {
                "fish"
            } else {
                return CheckResult::skipped("Shell integration", "unsupported shell");
            }
        }
        Err(_) => {
            return CheckResult::skipped("Shell integration", "$SHELL not set");
        }
    };

    let home = match std::env::var("HOME") {
        Ok(h) => h,
        Err(_) => {
            return CheckResult::skipped("Shell integration", "$HOME not set");
        }
    };

    let config_file = match shell_name {
        "bash" => PathBuf::from(&home).join(".bashrc"),
        "zsh" => PathBuf::from(&home).join(".zshrc"),
        "fish" => PathBuf::from(&home)
            .join(".config")
            .join("fish")
            .join("config.fish"),
        _ => unreachable!(),
    };

    if !config_file.exists() {
        return CheckResult::warning(
            "Shell integration",
            &format!("{} not found", config_file.display()),
        )
        .with_suggestion("Run 'daft setup' to configure shell integration");
    }

    match std::fs::read_to_string(&config_file) {
        Ok(content) if content.contains("daft shell-init") => {
            CheckResult::pass("Shell integration", &format!("configured ({shell_name})"))
        }
        Ok(_) => CheckResult::warning(
            "Shell integration",
            &format!("not configured ({shell_name})"),
        )
        .with_suggestion("Run 'daft setup' to add shell integration"),
        Err(_) => CheckResult::warning(
            "Shell integration",
            &format!("could not read {}", config_file.display()),
        ),
    }
}

/// Check if a path is a symlink pointing to the daft binary.
fn is_valid_symlink(path: &Path, install_dir: &Path) -> bool {
    if path.is_symlink() {
        if let Ok(target) = std::fs::read_link(path) {
            // Check relative ("daft") or absolute path targets
            if target == Path::new("daft") || target == install_dir.join("daft") {
                return true;
            }
            // Canonicalize for other target formats
            if let Some(parent) = path.parent() {
                if let (Ok(resolved), Ok(expected)) = (
                    parent.join(&target).canonicalize(),
                    install_dir.join("daft").canonicalize(),
                ) {
                    return resolved == expected;
                }
            }
        }
        return false;
    }

    // On non-Unix platforms, could be a copy rather than a symlink
    #[cfg(not(unix))]
    if path.exists() {
        let daft_path = install_dir.join("daft");
        if let (Ok(meta_link), Ok(meta_daft)) = (path.metadata(), daft_path.metadata()) {
            return meta_link.len() == meta_daft.len();
        }
    }

    false
}

/// Create a symlink for a command.
fn create_symlink(alias: &str, install_dir: &Path) -> Result<(), String> {
    let link_path = install_dir.join(alias);

    // Remove existing file/symlink if present
    if link_path.exists() || link_path.is_symlink() {
        std::fs::remove_file(&link_path)
            .map_err(|e| format!("Failed to remove existing {alias}: {e}"))?;
    }

    #[cfg(unix)]
    {
        std::os::unix::fs::symlink("daft", &link_path)
            .map_err(|e| format!("Failed to create symlink for {alias}: {e}"))?;
    }

    #[cfg(not(unix))]
    {
        let daft_binary = install_dir.join("daft");
        std::fs::copy(&daft_binary, &link_path)
            .map_err(|e| format!("Failed to copy daft binary to {alias}: {e}"))?;
    }

    Ok(())
}

/// Get standard man page search paths.
fn get_man_search_paths() -> Vec<PathBuf> {
    let mut paths = Vec::new();

    // Check MANPATH environment variable first
    if let Ok(manpath) = std::env::var("MANPATH") {
        for path in manpath.split(':') {
            if !path.is_empty() {
                paths.push(PathBuf::from(path));
            }
        }
    }

    // User-local man directory
    if let Some(home) = dirs::home_dir() {
        paths.push(home.join(".local").join("share").join("man"));
    }

    // Homebrew paths
    paths.push(PathBuf::from("/opt/homebrew/share/man"));
    paths.push(PathBuf::from("/usr/local/share/man"));

    // System paths
    paths.push(PathBuf::from("/usr/share/man"));

    paths
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_expected_symlinks_count() {
        assert_eq!(EXPECTED_SYMLINKS.len(), 10);
    }

    #[test]
    fn test_check_git_passes() {
        // git should be installed in test environments
        let result = check_git();
        assert_eq!(result.status, crate::doctor::CheckStatus::Pass);
        assert!(result.message.contains("version"));
    }

    #[test]
    fn test_get_man_search_paths_not_empty() {
        let paths = get_man_search_paths();
        assert!(!paths.is_empty());
    }
}
