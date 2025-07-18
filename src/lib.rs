use anyhow::{Context, Result};
use std::env;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use which::which;

pub mod direnv;
pub mod git;
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
    let output = Command::new("git")
        .args(["rev-parse", "--is-inside-work-tree"])
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .context("Failed to check if inside Git repository")?;

    Ok(output.success())
}

pub fn get_git_common_dir() -> Result<PathBuf> {
    let output = Command::new("git")
        .args(["rev-parse", "--git-common-dir"])
        .output()
        .context("Failed to get git common directory")?;

    if !output.status.success() {
        anyhow::bail!("Not inside a Git repository");
    }

    let path_str = String::from_utf8(output.stdout)
        .context("Failed to parse git common directory output")?
        .trim()
        .to_string();

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
    let output = Command::new("git")
        .args(["symbolic-ref", "--short", "HEAD"])
        .output()
        .context("Failed to get current branch")?;

    if !output.status.success() {
        anyhow::bail!("Could not determine current branch (maybe detached HEAD?)");
    }

    let branch = String::from_utf8(output.stdout)
        .context("Failed to parse current branch output")?
        .trim()
        .to_string();

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

    Ok(repo_name)
}

pub fn quiet_echo(message: &str, quiet: bool) {
    if !quiet {
        println!("{message}");
    }
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

    #[test]
    fn test_quiet_echo() {
        quiet_echo("test message", true);
        quiet_echo("test message", false);
    }
}
