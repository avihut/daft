use anyhow::{Context, Result};
use std::env;
use std::fs;
use std::path::Path;

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

    if branch_name.contains("..") {
        anyhow::bail!("Branch name cannot contain '..'");
    }

    if branch_name.starts_with('/') || branch_name.ends_with('/') {
        anyhow::bail!("Branch name cannot start or end with '/'");
    }

    if branch_name.contains(' ') {
        anyhow::bail!("Branch name cannot contain spaces");
    }

    Ok(())
}

pub fn validate_repo_name(repo_name: &str) -> Result<()> {
    if repo_name.is_empty() {
        anyhow::bail!("Repository name cannot be empty");
    }

    if repo_name.contains('/') || repo_name.contains('\\') {
        anyhow::bail!("Repository name cannot contain path separators. Use a simple name like 'my-project', not 'path/to/my-project'");
    }

    if repo_name.contains("..") {
        anyhow::bail!("Repository name cannot contain '..'");
    }

    if repo_name.starts_with('.') {
        anyhow::bail!("Repository name cannot start with '.'");
    }

    Ok(())
}

pub fn print_success_message(repo_name: &str, worktree_path: &Path, git_dir: &str, quiet: bool) {
    if !quiet {
        println!("---");
        println!("Success!");
        println!("Repository '{repo_name}' ready.");
        println!("The main Git directory is at: '{git_dir}'");
        println!("Your worktree is ready at: '{}'", worktree_path.display());
        println!("You are now inside the worktree.");
    }
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
        let current = get_current_directory().unwrap();
        assert!(current.is_absolute());
    }
}
