//! Hook checks for `daft doctor`.
//!
//! Verifies hook configuration: executability, deprecated names, trust level.

use crate::doctor::CheckResult;
use crate::hooks::{HookType, TrustDatabase, TrustLevel, PROJECT_HOOKS_DIR};
use std::path::{Path, PathBuf};

/// Find the hooks directory by scanning worktrees under the project root.
///
/// Returns the first `.daft/hooks/` directory found in any worktree.
fn find_hooks_dir(project_root: &Path) -> Option<PathBuf> {
    let entries = std::fs::read_dir(project_root).ok()?;
    for entry in entries.flatten() {
        let path = entry.path();
        if path.is_dir() && path.file_name().map(|n| n != ".git").unwrap_or(false) {
            let hooks_dir = path.join(PROJECT_HOOKS_DIR);
            if hooks_dir.exists() && hooks_dir.is_dir() {
                return Some(hooks_dir);
            }
        }
    }
    None
}

/// List hook files in a hooks directory.
fn list_hook_files(hooks_dir: &Path) -> Vec<PathBuf> {
    let mut files = Vec::new();
    if let Ok(entries) = std::fs::read_dir(hooks_dir) {
        for entry in entries.flatten() {
            let path = entry.path();
            if path.is_file() {
                files.push(path);
            }
        }
    }
    files.sort();
    files
}

/// Check if hooks directory exists. Returns None if no hooks to check.
pub fn has_hooks(project_root: &Path) -> bool {
    find_hooks_dir(project_root).is_some()
}

/// Check that all hook files are executable.
pub fn check_hooks_executable(project_root: &Path) -> CheckResult {
    let hooks_dir = match find_hooks_dir(project_root) {
        Some(dir) => dir,
        None => return CheckResult::skipped("Hooks executable", "no hooks found"),
    };

    let files = list_hook_files(&hooks_dir);
    if files.is_empty() {
        return CheckResult::skipped("Hooks executable", "no hook files");
    }

    let mut non_executable = Vec::new();

    for file in &files {
        if !is_executable(file) {
            if let Some(name) = file.file_name() {
                non_executable.push(name.to_string_lossy().to_string());
            }
        }
    }

    if non_executable.is_empty() {
        CheckResult::pass(
            "Hooks executable",
            &format!("all {} hooks executable", files.len()),
        )
    } else {
        let details: Vec<String> = non_executable
            .iter()
            .map(|n| format!("Not executable: {n}"))
            .collect();
        CheckResult::warning(
            "Hooks executable",
            &format!("{} hook(s) not executable", non_executable.len()),
        )
        .with_suggestion("Run 'chmod +x' on the listed hooks")
        .with_fixable(true)
        .with_details(details)
    }
}

/// Fix non-executable hooks by adding execute permission.
pub fn fix_hooks_executable(project_root: &Path) -> Result<(), String> {
    let hooks_dir =
        find_hooks_dir(project_root).ok_or_else(|| "No hooks directory found".to_string())?;

    let files = list_hook_files(&hooks_dir);
    for file in &files {
        if !is_executable(file) {
            set_executable(file)?;
        }
    }

    Ok(())
}

/// Check for deprecated hook filenames.
pub fn check_deprecated_names(project_root: &Path) -> CheckResult {
    let hooks_dir = match find_hooks_dir(project_root) {
        Some(dir) => dir,
        None => return CheckResult::skipped("Hook names", "no hooks found"),
    };

    let files = list_hook_files(&hooks_dir);
    let mut deprecated = Vec::new();

    for file in &files {
        if let Some(name) = file.file_name().and_then(|n| n.to_str()) {
            if let Some(hook_type) = HookType::from_filename(name) {
                if let Some(old_name) = hook_type.deprecated_filename() {
                    if name == old_name {
                        deprecated.push((old_name, hook_type.filename()));
                    }
                }
            }
        }
    }

    if deprecated.is_empty() {
        CheckResult::pass("Hook names", "no deprecated names")
    } else {
        let details: Vec<String> = deprecated
            .iter()
            .map(|(old, new)| format!("{old} -> {new}"))
            .collect();
        CheckResult::warning(
            "Hook names",
            &format!("{} deprecated name(s)", deprecated.len()),
        )
        .with_suggestion("Run 'git daft hooks migrate'")
        .with_fixable(true)
        .with_details(details)
    }
}

/// Fix deprecated hook names by renaming them.
pub fn fix_deprecated_names(project_root: &Path) -> Result<(), String> {
    let hooks_dir =
        find_hooks_dir(project_root).ok_or_else(|| "No hooks directory found".to_string())?;

    let files = list_hook_files(&hooks_dir);
    for file in &files {
        if let Some(name) = file.file_name().and_then(|n| n.to_str()) {
            if let Some(hook_type) = HookType::from_filename(name) {
                if let Some(old_name) = hook_type.deprecated_filename() {
                    if name == old_name {
                        let new_path = hooks_dir.join(hook_type.filename());
                        if !new_path.exists() {
                            std::fs::rename(file, &new_path)
                                .map_err(|e| format!("Failed to rename {old_name}: {e}"))?;
                        }
                    }
                }
            }
        }
    }

    Ok(())
}

/// Check the trust level for the current repository.
pub fn check_trust_level(git_common_dir: &Path) -> CheckResult {
    let db = match TrustDatabase::load() {
        Ok(db) => db,
        Err(e) => {
            return CheckResult::warning(
                "Trust level",
                &format!("could not load trust database: {e}"),
            );
        }
    };

    let level = db.get_trust_level(git_common_dir);

    match level {
        TrustLevel::Allow => CheckResult::pass("Trust level", "allow"),
        TrustLevel::Prompt => CheckResult::pass("Trust level", "prompt"),
        TrustLevel::Deny => CheckResult::warning("Trust level", "deny (hooks will not run)")
            .with_suggestion("Run 'git daft hooks trust' to allow hook execution"),
    }
}

/// Check if a file is executable (Unix).
#[cfg(unix)]
fn is_executable(path: &Path) -> bool {
    use std::os::unix::fs::PermissionsExt;
    path.metadata()
        .map(|m| m.permissions().mode() & 0o111 != 0)
        .unwrap_or(false)
}

#[cfg(not(unix))]
fn is_executable(_path: &Path) -> bool {
    true
}

/// Set executable permission on a file.
#[cfg(unix)]
fn set_executable(path: &Path) -> Result<(), String> {
    use std::os::unix::fs::PermissionsExt;
    let metadata = path
        .metadata()
        .map_err(|e| format!("Could not read metadata for {}: {e}", path.display()))?;
    let mut perms = metadata.permissions();
    let mode = perms.mode();
    perms.set_mode(mode | 0o111);
    std::fs::set_permissions(path, perms)
        .map_err(|e| format!("Could not set permissions for {}: {e}", path.display()))?;
    Ok(())
}

#[cfg(not(unix))]
fn set_executable(_path: &Path) -> Result<(), String> {
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::doctor::CheckStatus;
    use tempfile::tempdir;

    #[test]
    fn test_has_hooks_no_hooks_dir() {
        let temp = tempdir().unwrap();
        assert!(!has_hooks(temp.path()));
    }

    #[test]
    fn test_has_hooks_with_hooks_dir() {
        let temp = tempdir().unwrap();
        let worktree = temp.path().join("main");
        let hooks_dir = worktree.join(PROJECT_HOOKS_DIR);
        std::fs::create_dir_all(&hooks_dir).unwrap();
        assert!(has_hooks(temp.path()));
    }

    #[test]
    fn test_check_hooks_executable_no_hooks() {
        let temp = tempdir().unwrap();
        let result = check_hooks_executable(temp.path());
        assert_eq!(result.status, CheckStatus::Skipped);
    }

    #[test]
    #[cfg(unix)]
    fn test_check_hooks_executable_all_ok() {
        use std::os::unix::fs::PermissionsExt;

        let temp = tempdir().unwrap();
        let worktree = temp.path().join("main");
        let hooks_dir = worktree.join(PROJECT_HOOKS_DIR);
        std::fs::create_dir_all(&hooks_dir).unwrap();

        let hook = hooks_dir.join("post-clone");
        std::fs::write(&hook, "#!/bin/bash").unwrap();
        let mut perms = hook.metadata().unwrap().permissions();
        perms.set_mode(0o755);
        std::fs::set_permissions(&hook, perms).unwrap();

        let result = check_hooks_executable(temp.path());
        assert_eq!(result.status, CheckStatus::Pass);
    }

    #[test]
    #[cfg(unix)]
    fn test_check_hooks_executable_missing_permission() {
        use std::os::unix::fs::PermissionsExt;

        let temp = tempdir().unwrap();
        let worktree = temp.path().join("main");
        let hooks_dir = worktree.join(PROJECT_HOOKS_DIR);
        std::fs::create_dir_all(&hooks_dir).unwrap();

        let hook = hooks_dir.join("post-clone");
        std::fs::write(&hook, "#!/bin/bash").unwrap();
        let mut perms = hook.metadata().unwrap().permissions();
        perms.set_mode(0o644);
        std::fs::set_permissions(&hook, perms).unwrap();

        let result = check_hooks_executable(temp.path());
        assert_eq!(result.status, CheckStatus::Warning);
        assert!(result.fixable);
    }

    #[test]
    fn test_check_deprecated_names_no_deprecated() {
        let temp = tempdir().unwrap();
        let worktree = temp.path().join("main");
        let hooks_dir = worktree.join(PROJECT_HOOKS_DIR);
        std::fs::create_dir_all(&hooks_dir).unwrap();

        // Use canonical name
        std::fs::write(hooks_dir.join("worktree-post-create"), "#!/bin/bash").unwrap();

        let result = check_deprecated_names(temp.path());
        assert_eq!(result.status, CheckStatus::Pass);
    }

    #[test]
    fn test_check_deprecated_names_found() {
        let temp = tempdir().unwrap();
        let worktree = temp.path().join("main");
        let hooks_dir = worktree.join(PROJECT_HOOKS_DIR);
        std::fs::create_dir_all(&hooks_dir).unwrap();

        // Use deprecated name
        std::fs::write(hooks_dir.join("post-create"), "#!/bin/bash").unwrap();

        let result = check_deprecated_names(temp.path());
        assert_eq!(result.status, CheckStatus::Warning);
        assert!(result.fixable);
    }
}
