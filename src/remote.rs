use anyhow::{Context, Result};
use std::fs;
use std::path::Path;
use std::process::Command;

pub fn get_default_branch_remote(repo_url: &str) -> Result<String> {
    let output = Command::new("git")
        .args(["ls-remote", "--symref", repo_url, "HEAD"])
        .output()
        .context("Failed to query remote HEAD ref")?;

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        anyhow::bail!("Could not query remote HEAD ref: {}", stderr);
    }

    let output_str =
        String::from_utf8(output.stdout).context("Failed to parse ls-remote output")?;

    for line in output_str.lines() {
        if line.starts_with("ref:") {
            let parts: Vec<&str> = line.split_whitespace().collect();
            if parts.len() >= 2 {
                let ref_path = parts[1];
                if let Some(branch) = ref_path.strip_prefix("refs/heads/") {
                    return Ok(branch.to_string());
                }
            }
        }
    }

    anyhow::bail!("Could not parse default branch from ls-remote output")
}

pub fn get_default_branch_local(git_common_dir: &Path, remote_name: &str) -> Result<String> {
    let head_ref_file = git_common_dir
        .join("refs/remotes")
        .join(remote_name)
        .join("HEAD");

    if !head_ref_file.exists() {
        anyhow::bail!(
            "Default branch reference file not found at '{}'. \
            Please ensure remote '{}' exists and its HEAD is set correctly locally. \
            Try: 'git remote set-head {} --auto' and 'git fetch {}'",
            head_ref_file.display(),
            remote_name,
            remote_name,
            remote_name
        );
    }

    let content = fs::read_to_string(&head_ref_file)
        .with_context(|| format!("Failed to read {}", head_ref_file.display()))?;

    let content = content.trim();

    if let Some(ref_path) = content.strip_prefix("ref: ") {
        let prefix = format!("refs/remotes/{remote_name}/");
        if let Some(branch) = ref_path.strip_prefix(&prefix) {
            if branch.is_empty() {
                anyhow::bail!("Empty branch name in reference file");
            }
            return Ok(branch.to_string());
        }
    }

    anyhow::bail!(
        "Unexpected content in '{}': '{}'. Expected format: 'ref: refs/remotes/{}/branch_name'",
        head_ref_file.display(),
        content,
        remote_name
    );
}

pub fn get_remote_branches(remote_name: &str) -> Result<Vec<String>> {
    let output = Command::new("git")
        .args(["ls-remote", "--heads", remote_name])
        .output()
        .context("Failed to get remote branches")?;

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        anyhow::bail!("Git ls-remote failed: {}", stderr);
    }

    let output_str =
        String::from_utf8(output.stdout).context("Failed to parse ls-remote output")?;

    let mut branches = Vec::new();
    for line in output_str.lines() {
        if let Some(tab_pos) = line.find('\t') {
            let ref_name = &line[tab_pos + 1..];
            if let Some(branch) = ref_name.strip_prefix("refs/heads/") {
                branches.push(branch.to_string());
            }
        }
    }

    Ok(branches)
}

pub fn remote_branch_exists(remote_name: &str, branch: &str) -> Result<bool> {
    let output = Command::new("git")
        .args([
            "ls-remote",
            "--heads",
            remote_name,
            &format!("refs/heads/{branch}"),
        ])
        .output()
        .context("Failed to check remote branch existence")?;

    if !output.status.success() {
        return Ok(false);
    }

    let output_str =
        String::from_utf8(output.stdout).context("Failed to parse ls-remote output")?;

    Ok(output_str.contains(&format!("refs/heads/{branch}")))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_remote_branch_exists() {
        let result = remote_branch_exists("origin", "nonexistent-branch");
        assert!(result.is_ok());
    }

    #[test]
    fn test_get_remote_branches() {
        let result = get_remote_branches("origin");
        assert!(result.is_ok());
    }
}
