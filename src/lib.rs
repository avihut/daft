use anyhow::{Context, Result};
use git_version::git_version;
use std::env;
use std::fs;
use std::path::{Path, PathBuf};
use which::which;

/// Version string derived from git tags at build time.
/// Falls back to Cargo.toml version if not in a git repository.
pub const VERSION: &str = git_version!(
    args = ["--tags", "--always", "--dirty=-modified"],
    fallback = env!("CARGO_PKG_VERSION")
);

/// Marker prefix for shell wrapper cd path extraction.
/// Shell wrappers look for this marker to determine which directory to cd into.
pub const CD_PATH_MARKER: &str = "__DAFT_CD__:";

/// Environment variable that shell wrappers set to signal they want cd path output.
pub const SHELL_WRAPPER_ENV: &str = "DAFT_SHELL_WRAPPER";

/// Outputs the final worktree path for shell wrappers to consume.
///
/// Only outputs if DAFT_SHELL_WRAPPER env var is set. This keeps output clean
/// for users who don't use wrappers - they won't see the marker line.
///
/// Shell wrappers set DAFT_SHELL_WRAPPER=1 before calling the binary, then
/// parse the output for lines starting with `__DAFT_CD__:` to extract the
/// path they should cd into.
pub fn output_cd_path(path: &Path) {
    if env::var(SHELL_WRAPPER_ENV).is_ok() {
        println!("{}{}", CD_PATH_MARKER, path.display());
    }
}

pub mod config;
pub mod direnv;
pub mod git;
pub mod logging;
pub mod output;
pub mod remote;
pub mod utils;

#[derive(Debug, Clone)]
pub struct WorktreeConfig {
    pub remote_name: String,
    pub quiet: bool,
}

impl Default for WorktreeConfig {
    fn default() -> Self {
        Self {
            remote_name: "origin".to_string(),
            quiet: false,
        }
    }
}

pub fn is_git_repository() -> Result<bool> {
    let git = git::GitCommand::new(true); // Use quiet mode for this check
    git.rev_parse_is_inside_work_tree()
}

pub fn get_git_common_dir() -> Result<PathBuf> {
    let git = git::GitCommand::new(false);
    let path_str = git
        .rev_parse_git_common_dir()
        .context("Failed to get git common directory")?;
    Ok(PathBuf::from(path_str))
}

pub fn get_project_root() -> Result<PathBuf> {
    let git_common_dir = get_git_common_dir()?;
    let project_root = git_common_dir
        .parent()
        .context("Failed to determine project root directory")?;
    Ok(project_root.to_path_buf())
}

pub fn get_current_branch() -> Result<String> {
    let git = git::GitCommand::new(false);
    let branch = git
        .symbolic_ref_short_head()
        .context("Could not determine current branch (maybe detached HEAD?)")?;

    if branch.is_empty() {
        anyhow::bail!("Empty branch name returned");
    }

    Ok(branch)
}

pub fn extract_repo_name(repo_url: &str) -> Result<String> {
    let repo_name = if repo_url.contains(':') {
        let parts: Vec<&str> = repo_url.split(':').collect();
        if parts.len() >= 2 {
            Path::new(parts[1])
                .file_stem()
                .and_then(|s| s.to_str())
                .context("Failed to extract repository name from shorthand URL")?
                .to_string()
        } else {
            anyhow::bail!("Invalid repository URL format");
        }
    } else {
        Path::new(repo_url)
            .file_stem()
            .and_then(|s| s.to_str())
            .context("Failed to extract repository name from URL")?
            .to_string()
    };

    if repo_name.is_empty() {
        anyhow::bail!("Could not extract repository name from URL: '{}'", repo_url);
    }

    // Security: Sanitize the extracted repository name
    let sanitized_name = sanitize_extracted_name(&repo_name)?;

    Ok(sanitized_name)
}

/// Sanitizes an extracted repository name for security
///
/// This function applies security measures to repository names extracted from URLs
/// to prevent injection attacks, path traversal, and other security vulnerabilities.
fn sanitize_extracted_name(name: &str) -> Result<String> {
    // Remove null bytes and control characters
    let cleaned: String = name
        .chars()
        .filter(|c| !c.is_control() && *c != '\0')
        .collect();

    // Remove dangerous characters that could be used for injection
    let safe_chars: String = cleaned
        .chars()
        .filter(|c| match c {
            // Allow alphanumeric, hyphens, underscores, and dots
            'a'..='z' | 'A'..='Z' | '0'..='9' | '-' | '_' | '.' => true,
            _ => false,
        })
        .collect();

    // Remove leading/trailing dots and ensure it's not empty
    let trimmed = safe_chars.trim_matches('.');

    if trimmed.is_empty() {
        anyhow::bail!("Repository name contains only unsafe characters");
    }

    // Prevent path traversal patterns
    if trimmed.contains("..") {
        anyhow::bail!("Repository name contains path traversal patterns");
    }

    // Length limit
    if trimmed.len() > 255 {
        anyhow::bail!("Repository name too long after sanitization");
    }

    Ok(trimmed.to_string())
}

pub fn check_dependencies() -> Result<()> {
    let required_tools = vec!["git", "basename", "awk"];
    let mut missing = Vec::new();

    for tool in required_tools {
        if which(tool).is_err() {
            missing.push(tool);
        }
    }

    if !missing.is_empty() {
        anyhow::bail!("Missing required dependencies: {}", missing.join(", "));
    }

    Ok(())
}

pub fn ensure_directory_exists(path: &Path) -> Result<()> {
    if !path.exists() {
        fs::create_dir_all(path)
            .with_context(|| format!("Failed to create directory: {}", path.display()))?;
    }
    Ok(())
}

pub fn cleanup_on_error<P: AsRef<Path>>(path: P) -> Result<()> {
    let path = path.as_ref();
    if path.exists() {
        fs::remove_dir_all(path)
            .with_context(|| format!("Failed to cleanup directory: {}", path.display()))?;
    }
    Ok(())
}

pub fn change_to_original_dir(original_dir: &Path) -> Result<()> {
    env::set_current_dir(original_dir).with_context(|| {
        format!(
            "Failed to change back to original directory: {}",
            original_dir.display()
        )
    })?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::tempdir;

    #[test]
    fn test_extract_repo_name_ssh() {
        let url = "git@github.com:user/repo.git";
        let name = extract_repo_name(url).unwrap();
        assert_eq!(name, "repo");
    }

    #[test]
    fn test_extract_repo_name_https() {
        let url = "https://github.com/user/repo.git";
        let name = extract_repo_name(url).unwrap();
        assert_eq!(name, "repo");
    }

    #[test]
    fn test_extract_repo_name_shorthand() {
        let url = "user:repo.git";
        let name = extract_repo_name(url).unwrap();
        assert_eq!(name, "repo");
    }

    #[test]
    fn test_ensure_directory_exists() {
        let temp_dir = tempdir().unwrap();
        let test_path = temp_dir.path().join("test_dir");

        ensure_directory_exists(&test_path).unwrap();
        assert!(test_path.exists());
        assert!(test_path.is_dir());
    }
}
