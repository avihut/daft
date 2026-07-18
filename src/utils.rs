use anyhow::{Context, Result};
use std::env;
use std::fs;
use std::path::Path;
use std::process::Command;

/// Build a `git` `Command` rooted at `dir` with inherited `GIT_*` env vars
/// stripped.
///
/// When daft runs inside a git hook (pre-push, post-checkout, etc.) the
/// hook process exports `GIT_DIR` / `GIT_WORK_TREE` / friends. Child
/// processes inherit them, and they override `-C <dir>` for repository
/// discovery — so a query like "is this file in *this* directory's repo?"
/// silently retargets the hook's repo. Clearing the inherited vars makes
/// `-C` authoritative again.
pub fn git_command_at(dir: &Path) -> Command {
    let mut cmd = Command::new("git");
    cmd.arg("-C").arg(dir);
    for var in [
        "GIT_DIR",
        "GIT_WORK_TREE",
        "GIT_INDEX_FILE",
        "GIT_OBJECT_DIRECTORY",
        "GIT_ALTERNATE_OBJECT_DIRECTORIES",
        "GIT_COMMON_DIR",
        "GIT_CEILING_DIRECTORIES",
        "GIT_NAMESPACE",
    ] {
        cmd.env_remove(var);
    }
    cmd
}

pub fn change_directory(path: &Path) -> Result<()> {
    env::set_current_dir(path)
        .with_context(|| format!("Failed to change directory to {}", path.display()))?;
    Ok(())
}

pub fn get_current_directory() -> Result<std::path::PathBuf> {
    env::current_dir().context("Failed to get current directory")
}

pub fn path_exists(path: &Path) -> bool {
    path.exists()
}

pub fn create_directory(path: &Path) -> Result<()> {
    fs::create_dir_all(path)
        .with_context(|| format!("Failed to create directory: {}", path.display()))?;
    Ok(())
}

pub fn remove_directory(path: &Path) -> Result<()> {
    if path.exists() {
        fs::remove_dir_all(path)
            .with_context(|| format!("Failed to remove directory: {}", path.display()))?;
    }
    Ok(())
}

pub fn validate_branch_name(branch_name: &str) -> Result<()> {
    if branch_name.is_empty() {
        anyhow::bail!("Branch name cannot be empty");
    }

    // Security: Check for path traversal attempts
    if branch_name.contains("..") {
        anyhow::bail!("Branch name cannot contain '..'");
    }

    // Security: Check for absolute paths
    if branch_name.starts_with('/') || branch_name.ends_with('/') {
        anyhow::bail!("Branch name cannot start or end with '/'");
    }

    // Security: Check for command injection attempts
    if branch_name.contains(';')
        || branch_name.contains('&')
        || branch_name.contains('|')
        || branch_name.contains('$')
        || branch_name.contains('`')
        || branch_name.contains('<')
        || branch_name.contains('>')
    {
        anyhow::bail!("Branch name contains unsafe characters");
    }

    // Security: Check for null bytes, control characters, and problematic Unicode
    if branch_name.contains('\0')
        || branch_name.chars().any(|c| {
            c.is_control() ||
        // Zero-width characters and format characters
        matches!(c, '\u{200B}' | '\u{200C}' | '\u{200D}' | '\u{FEFF}' | '\u{2028}' | '\u{2029}')
        })
    {
        anyhow::bail!("Branch name contains control or problematic Unicode characters");
    }

    // Security: Check for whitespace (not just spaces)
    if branch_name.chars().any(|c| c.is_whitespace()) {
        anyhow::bail!("Branch name cannot contain whitespace");
    }

    // Security: Check for Git-specific dangerous patterns
    if branch_name.starts_with(".git")
        || branch_name.contains("/.git")
        || branch_name.starts_with("refs/")
        || branch_name.contains("HEAD")
    {
        anyhow::bail!("Branch name contains Git-specific patterns");
    }

    // Security: Check for hidden files/directories
    if branch_name.starts_with('.') {
        anyhow::bail!("Branch name cannot start with '.'");
    }

    // Security: Length limit to prevent buffer overflow attacks and encourage reasonable naming
    if branch_name.len() >= 100 {
        anyhow::bail!("Branch name too long (max 99 characters)");
    }

    Ok(())
}

pub fn validate_repo_name(repo_name: &str) -> Result<()> {
    if repo_name.is_empty() {
        anyhow::bail!("Repository name cannot be empty");
    }

    // Security: Check for path traversal attempts
    if repo_name.contains("..") {
        anyhow::bail!("Repository name cannot contain '..'");
    }

    // Security: Check for path separators and absolute paths
    if repo_name.contains('/') || repo_name.contains('\\') || repo_name.contains(':') {
        anyhow::bail!(
            "Repository name cannot contain path separators. Use a simple name like 'my-project', not 'path/to/my-project'"
        );
    }

    // Security: Check for command injection attempts
    if repo_name.contains(';')
        || repo_name.contains('&')
        || repo_name.contains('|')
        || repo_name.contains('$')
        || repo_name.contains('`')
        || repo_name.contains('<')
        || repo_name.contains('>')
    {
        anyhow::bail!("Repository name contains unsafe characters");
    }

    // Security: Check for null bytes and control characters
    if repo_name.contains('\0') || repo_name.chars().any(|c| c.is_control()) {
        anyhow::bail!("Repository name contains control characters");
    }

    // Security: Check for whitespace
    if repo_name.chars().any(|c| c.is_whitespace()) {
        anyhow::bail!("Repository name cannot contain whitespace");
    }

    // Security: Check for hidden files/directories and system files
    if repo_name.starts_with('.') || repo_name == "." || repo_name == ".." {
        anyhow::bail!("Repository name cannot start with '.' or be '.' or '..'");
    }

    // Security: Check for Windows reserved names
    let windows_reserved = [
        "CON", "PRN", "AUX", "NUL", "COM1", "COM2", "COM3", "COM4", "COM5", "COM6", "COM7", "COM8",
        "COM9", "LPT1", "LPT2", "LPT3", "LPT4", "LPT5", "LPT6", "LPT7", "LPT8", "LPT9",
    ];
    if windows_reserved.contains(&repo_name.to_uppercase().as_str()) {
        anyhow::bail!("Repository name is a Windows reserved name");
    }

    // Security: Length limit to prevent buffer overflow attacks
    if repo_name.len() > 255 {
        anyhow::bail!("Repository name too long (max 255 characters)");
    }

    // Security: Minimum length to prevent empty-like names
    if repo_name.is_empty() {
        anyhow::bail!("Repository name too short");
    }

    Ok(())
}

/// Shell-quote each argv element (POSIX) and join with spaces so the
/// result can be handed to `sh -c` while preserving the original
/// argument boundaries.
pub fn quote_argv(parts: &[String]) -> String {
    parts
        .iter()
        .map(|p| {
            shlex::try_quote(p)
                .map(|c| c.into_owned())
                .unwrap_or_else(|_| p.clone())
        })
        .collect::<Vec<_>>()
        .join(" ")
}

pub fn print_error_cleanup(error_msg: &str, cleanup_path: Option<&Path>) {
    eprintln!("Error: {error_msg}");
    if let Some(path) = cleanup_path {
        eprintln!("Cleaning up created directory...");
        if let Err(e) = remove_directory(path) {
            eprintln!("Warning: Failed to cleanup {}: {}", path.display(), e);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::tempdir;

    #[test]
    fn test_validate_branch_name() {
        assert!(validate_branch_name("feature/test").is_ok());
        assert!(validate_branch_name("main").is_ok());
        assert!(validate_branch_name("").is_err());
        assert!(validate_branch_name("feature..bad").is_err());
        assert!(validate_branch_name("/feature").is_err());
        assert!(validate_branch_name("feature/").is_err());
        assert!(validate_branch_name("feature test").is_err());
    }

    #[test]
    fn test_validate_repo_name() {
        assert!(validate_repo_name("my-project").is_ok());
        assert!(validate_repo_name("").is_err());
        assert!(validate_repo_name("path/to/project").is_err());
        assert!(validate_repo_name("path\\to\\project").is_err());
    }

    #[test]
    fn test_path_exists() {
        let temp_dir = tempdir().unwrap();
        assert!(path_exists(temp_dir.path()));
        assert!(!path_exists(&temp_dir.path().join("nonexistent")));
    }

    #[test]
    fn test_create_remove_directory() {
        let temp_dir = tempdir().unwrap();
        let test_path = temp_dir.path().join("test_dir");

        create_directory(&test_path).unwrap();
        assert!(test_path.exists());

        remove_directory(&test_path).unwrap();
        assert!(!test_path.exists());
    }

    #[test]
    fn test_current_directory() {
        // In CI environments with parallel tests, the current directory might
        // become invalid if another test deletes its temp directory. Skip gracefully.
        if let Ok(current) = get_current_directory() {
            assert!(current.is_absolute());
        }
    }
}
