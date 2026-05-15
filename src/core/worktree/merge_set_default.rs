//! Writes daft's merge.style and merge.cleanup defaults to git config --local.
//!
//! Used by the `--set-default` flag on `daft merge` to promote the current
//! invocation's preferences as the new repo defaults. Best-effort: failures
//! surface as warnings to the caller; the merge result is unaffected.

use crate::core::worktree::merge::{CleanupKind, MergeStyle};
use anyhow::{Context, Result};
use std::path::Path;
use std::process::Command;

/// Write `daft.merge.style` and `daft.merge.cleanup` to the local git config
/// of the repo containing `project_root`. Both keys are always written
/// (idempotent — even if values match the current config).
pub fn write_default_settings(
    project_root: &Path,
    style: MergeStyle,
    cleanup: CleanupKind,
) -> Result<()> {
    run_config_set(project_root, "daft.merge.style", style.as_str())
        .with_context(|| "failed to write daft.merge.style")?;
    run_config_set(project_root, "daft.merge.cleanup", cleanup.as_str())
        .with_context(|| "failed to write daft.merge.cleanup")?;
    Ok(())
}

fn run_config_set(project_root: &Path, key: &str, value: &str) -> Result<()> {
    let status = Command::new("git")
        .args(["config", "--local", key, value])
        .current_dir(project_root)
        .status()
        .with_context(|| format!("failed to invoke git config for {key}"))?;
    if !status.success() {
        anyhow::bail!("git config --local {key} {value} failed");
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::process::Stdio;

    fn init_repo(path: &Path) {
        Command::new("git")
            .args(["init", "-q", "-b", "main"])
            .current_dir(path)
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .status()
            .unwrap();
        Command::new("git")
            .args(["config", "--local", "user.name", "Test"])
            .current_dir(path)
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .status()
            .unwrap();
        Command::new("git")
            .args(["config", "--local", "user.email", "test@test.com"])
            .current_dir(path)
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .status()
            .unwrap();
    }

    fn read_local_config(path: &Path, key: &str) -> String {
        let out = Command::new("git")
            .args(["config", "--local", "--get", key])
            .current_dir(path)
            .output()
            .unwrap();
        String::from_utf8_lossy(&out.stdout).trim().to_string()
    }

    #[test]
    fn write_default_settings_persists_both_keys() {
        let tmp = tempfile::tempdir().unwrap();
        init_repo(tmp.path());

        write_default_settings(
            tmp.path(),
            MergeStyle::RebaseMerge,
            CleanupKind::RemoveBranch,
        )
        .unwrap();

        assert_eq!(
            read_local_config(tmp.path(), "daft.merge.style"),
            "rebase-merge"
        );
        assert_eq!(
            read_local_config(tmp.path(), "daft.merge.cleanup"),
            "remove-branch"
        );
    }

    #[test]
    fn write_default_settings_is_idempotent() {
        let tmp = tempfile::tempdir().unwrap();
        init_repo(tmp.path());

        // Pre-set values different from what we'll write.
        Command::new("git")
            .args(["config", "--local", "daft.merge.style", "rebase"])
            .current_dir(tmp.path())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .status()
            .unwrap();

        // Writing again with the same value should succeed (idempotent).
        write_default_settings(tmp.path(), MergeStyle::Squash, CleanupKind::Keep).unwrap();
        assert_eq!(read_local_config(tmp.path(), "daft.merge.style"), "squash");

        write_default_settings(tmp.path(), MergeStyle::Squash, CleanupKind::Keep).unwrap();
        assert_eq!(read_local_config(tmp.path(), "daft.merge.style"), "squash");
    }
}
