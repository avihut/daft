//! Installation checks for `daft doctor`.
//!
//! Verifies that daft and its dependencies are correctly installed:
//! binary in PATH, command symlinks, git, man pages, shell integration,
//! shortcut symlinks, and shell wrappers.

use crate::doctor::{CheckResult, FixAction};
use crate::shortcuts::{shortcuts_for_style, ShortcutStyle};
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
    "git-worktree-branch-delete",
    "git-worktree-flow-adopt",
    "git-worktree-flow-eject",
    "git-daft",
];

/// Check that the daft binary is in PATH.
pub fn check_binary_in_path() -> CheckResult {
    match which::which("daft") {
        Ok(path) => {
            let version = crate::VERSION;
            CheckResult::pass("daft binary", &format!("{} v{version}", path.display()))
        }
        Err(_) => CheckResult::fail("daft binary", "not found in PATH")
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
                "could not detect installation directory",
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
        CheckResult::pass("Command symlinks", &format!("{found}/{total} installed"))
    } else {
        let details: Vec<String> = missing.iter().map(|n| format!("Missing: {n}")).collect();
        let missing_owned: Vec<String> = missing.iter().map(|s| s.to_string()).collect();
        let dry_dir = install_dir.clone();
        CheckResult::warning(
            "Command symlinks",
            &format!("{found}/{total} installed, {} missing", missing.len()),
        )
        .with_suggestion("Run 'daft setup' to create missing symlinks")
        .with_fix(Box::new(fix_command_symlinks))
        .with_dry_run_fix(Box::new(move || {
            dry_run_symlink_actions(&missing_owned, &dry_dir)
        }))
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
                CheckResult::pass("Git", &format!("version {version}"))
            }
            _ => CheckResult::pass("Git", "installed (could not determine version)"),
        },
        Err(_) => CheckResult::fail("Git", "not found in PATH").with_suggestion("Install git"),
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
        Some(dir) => CheckResult::pass("Man pages", &format!("installed in {}", dir.display())),
        None => CheckResult::warning("Man pages", "not found")
            .with_suggestion("Run 'mise run man:install' to install man pages"),
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
            CheckResult::pass("Shell integration", &format!("{shell_name}, configured"))
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

/// Check that shell wrapper functions are active (auto-cd enabled).
///
/// This verifies that `daft shell-init` has been sourced in the current shell,
/// meaning the `__daft_wrapper` function exists. Without this, auto-cd into
/// new worktrees won't work.
pub fn check_shell_wrappers() -> CheckResult {
    let shell = match std::env::var("SHELL") {
        Ok(s) => s,
        Err(_) => return CheckResult::skipped("Shell wrappers", "$SHELL not set"),
    };

    // Run the user's shell in login mode to check if __daft_wrapper is defined
    let output = std::process::Command::new(&shell)
        .args(["-lc", "type __daft_wrapper 2>/dev/null && echo WRAPPED"])
        .output();

    match output {
        Ok(out) => {
            let stdout = String::from_utf8_lossy(&out.stdout);
            if stdout.contains("WRAPPED") {
                CheckResult::pass("Shell wrappers", "active (auto-cd enabled)")
            } else {
                CheckResult::warning("Shell wrappers", "not active — auto-cd will not work")
                    .with_suggestion("Restart your shell or run: source ~/.zshrc")
            }
        }
        Err(_) => CheckResult::skipped("Shell wrappers", "could not check shell functions"),
    }
}

/// Check shortcut symlinks for each enabled style.
///
/// Detects which shortcut styles have any symlink installed. For each
/// partially-installed style, reports missing shortcuts. If no styles
/// are installed at all, suggests enabling them.
pub fn check_shortcut_symlinks() -> Vec<CheckResult> {
    let install_dir = match crate::commands::shortcuts::detect_install_dir() {
        Ok(dir) => dir,
        Err(_) => return vec![],
    };
    check_shortcut_symlinks_in(&install_dir)
}

/// Check shortcut symlinks in a specific directory.
///
/// This is the inner implementation used by [`check_shortcut_symlinks`].
/// It is also exposed for testing with custom directories.
pub(crate) fn check_shortcut_symlinks_in(install_dir: &Path) -> Vec<CheckResult> {
    let mut results = Vec::new();
    let mut any_style_installed = false;

    for style in ShortcutStyle::all() {
        let shortcuts = shortcuts_for_style(*style);
        let mut present = Vec::new();
        let mut missing = Vec::new();

        for shortcut in &shortcuts {
            let path = install_dir.join(shortcut.alias);
            if is_valid_symlink(&path, install_dir) {
                present.push(shortcut.alias);
            } else {
                missing.push(shortcut.alias);
            }
        }

        // Skip styles that are entirely not installed (intentionally disabled)
        if present.is_empty() {
            continue;
        }

        any_style_installed = true;
        let total = shortcuts.len();
        let found = present.len();
        let name = format!("Shortcuts: {} style", style.name());

        if missing.is_empty() {
            results.push(CheckResult::pass(
                &name,
                &format!("{found}/{total} installed"),
            ));
        } else {
            let details: Vec<String> = missing.iter().map(|n| format!("Missing: {n}")).collect();
            let style_name = style.name().to_string();

            // Capture what we need for closures
            let install_dir_owned = install_dir.to_path_buf();
            let missing_owned: Vec<String> = missing.iter().map(|s| s.to_string()).collect();

            let fix_dir = install_dir_owned.clone();
            let fix_missing = missing_owned.clone();
            let dry_dir = install_dir_owned;
            let dry_missing = missing_owned;

            results.push(
                CheckResult::warning(
                    &name,
                    &format!("{found}/{total} installed, {} missing", missing.len()),
                )
                .with_suggestion(&format!(
                    "Run 'daft setup shortcuts enable {style_name}' to install"
                ))
                .with_fix(Box::new(move || {
                    for alias in &fix_missing {
                        let path = fix_dir.join(alias);
                        if !is_valid_symlink(&path, &fix_dir) {
                            create_symlink(alias, &fix_dir)?;
                        }
                    }
                    Ok(())
                }))
                .with_dry_run_fix(Box::new(move || {
                    dry_run_symlink_actions(&dry_missing, &dry_dir)
                }))
                .with_details(details),
            );
        }
    }

    if !any_style_installed {
        results.push(
            CheckResult::warning("Shortcuts", "no shortcut aliases configured")
                .with_suggestion("Run 'daft setup shortcuts' to enable shortcuts"),
        );
    }

    results
}

/// Check if a path is a symlink pointing to the daft binary.
pub(crate) fn is_valid_symlink(path: &Path, install_dir: &Path) -> bool {
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

/// Simulate creating symlinks, checking preconditions without applying changes.
///
/// Returns a list of [`FixAction`]s describing what would be done and
/// whether each action would succeed. Reused by both shortcut and command
/// symlink dry-run checks.
pub(crate) fn dry_run_symlink_actions(missing: &[String], install_dir: &Path) -> Vec<FixAction> {
    let dir_display = install_dir.display();
    let dir_writable = is_dir_writable(install_dir);

    missing
        .iter()
        .map(|alias| {
            let description = format!("Create symlink {alias} -> daft in {dir_display}");
            let path = install_dir.join(alias);

            if !dir_writable {
                return FixAction {
                    description,
                    would_succeed: false,
                    failure_reason: Some(format!("{dir_display} is not writable")),
                };
            }

            // Check for conflicting non-daft file
            if path.exists() && !path.is_symlink() {
                return FixAction {
                    description,
                    would_succeed: false,
                    failure_reason: Some(format!("{} exists and is not a symlink", path.display())),
                };
            }

            if path.is_symlink() && !is_valid_symlink(&path, install_dir) {
                return FixAction {
                    description: format!(
                        "Replace symlink {alias} -> daft in {dir_display} (currently points elsewhere)"
                    ),
                    would_succeed: true,
                    failure_reason: None,
                };
            }

            FixAction {
                description,
                would_succeed: true,
                failure_reason: None,
            }
        })
        .collect()
}

/// Check if a directory is writable by attempting a test write.
fn is_dir_writable(dir: &Path) -> bool {
    let test_file = dir.join(".daft-write-test");
    if std::fs::write(&test_file, "test").is_ok() {
        std::fs::remove_file(&test_file).ok();
        true
    } else {
        false
    }
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
        assert_eq!(EXPECTED_SYMLINKS.len(), 11);
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

    #[test]
    #[cfg(unix)]
    fn test_check_shortcut_symlinks_with_partial_install_has_fix() {
        let temp = tempfile::tempdir().unwrap();
        let install_dir = temp.path();

        // Create a fake "daft" binary
        std::fs::write(install_dir.join("daft"), "fake").unwrap();

        // Create only one shortcut from git style to simulate partial install
        std::os::unix::fs::symlink("daft", install_dir.join("gwtclone")).unwrap();

        let results = check_shortcut_symlinks_in(install_dir);
        let git_result = results.iter().find(|r| r.name.contains("git")).unwrap();
        assert_eq!(git_result.status, crate::doctor::CheckStatus::Warning);
        assert!(
            git_result.fixable(),
            "Partially installed style should be fixable"
        );
    }

    #[test]
    #[cfg(unix)]
    fn test_check_shortcut_symlinks_with_partial_install_has_dry_run() {
        let temp = tempfile::tempdir().unwrap();
        let install_dir = temp.path();

        std::fs::write(install_dir.join("daft"), "fake").unwrap();

        std::os::unix::fs::symlink("daft", install_dir.join("gwtclone")).unwrap();

        let results = check_shortcut_symlinks_in(install_dir);
        let git_result = results.iter().find(|r| r.name.contains("git")).unwrap();
        assert!(git_result.dry_run_fix.is_some(), "Should have dry_run_fix");

        let actions = (git_result.dry_run_fix.as_ref().unwrap())();
        // Should have actions for each missing shortcut (all git shortcuts minus gwtclone)
        assert!(!actions.is_empty());
        assert!(actions.iter().all(|a| a.would_succeed));
        assert!(actions
            .iter()
            .all(|a| a.description.contains("Create symlink")));
    }

    #[test]
    fn test_dry_run_symlink_actions_basic() {
        let temp = tempfile::tempdir().unwrap();
        let install_dir = temp.path();
        std::fs::write(install_dir.join("daft"), "fake").unwrap();

        let missing = vec!["git-worktree-clone".to_string(), "git-daft".to_string()];
        let actions = dry_run_symlink_actions(&missing, install_dir);
        assert_eq!(actions.len(), 2);
        assert!(actions[0].description.contains("git-worktree-clone"));
        assert!(actions[0].would_succeed);
    }

    #[test]
    fn test_dry_run_symlink_actions_detects_conflict() {
        let temp = tempfile::tempdir().unwrap();
        let install_dir = temp.path();
        std::fs::write(install_dir.join("daft"), "fake").unwrap();
        // Create a regular file (not a symlink) that conflicts
        std::fs::write(install_dir.join("gwtco"), "not-daft").unwrap();

        let missing = vec!["gwtco".to_string()];
        let actions = dry_run_symlink_actions(&missing, install_dir);
        assert_eq!(actions.len(), 1);
        assert!(!actions[0].would_succeed);
        assert!(actions[0]
            .failure_reason
            .as_ref()
            .unwrap()
            .contains("not a symlink"));
    }
}
