use anyhow::{Context, Result};
use std::path::Path;
use std::process::Command;
use which::which;

pub fn run_direnv_allow(path: &Path, quiet: bool) -> Result<()> {
    if which("direnv").is_err() {
        return Ok(());
    }

    let envrc_path = path.join(".envrc");
    if !envrc_path.exists() {
        if !quiet {
            println!(
                "--> No .envrc file found in {}. Skipping 'direnv allow'.",
                path.display()
            );
        }
        return Ok(());
    }

    if !quiet {
        println!("--> Running 'direnv allow'...");
    }

    let output = Command::new("direnv")
        .args(["allow", "."])
        .current_dir(path)
        .output()
        .context("Failed to execute direnv allow command")?;

    if output.status.success() {
        if !quiet {
            println!("--> 'direnv allow .' completed successfully.");
        }
    } else {
        let stderr = String::from_utf8_lossy(&output.stderr);
        eprintln!("Warning: 'direnv allow .' failed: {stderr}");
        eprintln!("You may need to run it manually.");
    }

    Ok(())
}

pub fn is_direnv_available() -> bool {
    which("direnv").is_ok()
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use tempfile::tempdir;

    #[test]
    fn test_is_direnv_available() {
        let _result = is_direnv_available();
        // This test just verifies the function doesn't panic
    }

    #[test]
    fn test_run_direnv_allow_no_envrc() {
        let temp_dir = tempdir().unwrap();
        let result = run_direnv_allow(temp_dir.path(), true);
        assert!(result.is_ok());
    }

    #[test]
    fn test_run_direnv_allow_with_envrc() {
        let temp_dir = tempdir().unwrap();
        let envrc_path = temp_dir.path().join(".envrc");
        fs::write(&envrc_path, "export TEST=1").unwrap();

        let result = run_direnv_allow(temp_dir.path(), true);
        assert!(result.is_ok());
    }
}
