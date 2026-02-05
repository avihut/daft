//! Git hook shim generation.
//!
//! Generates small shell scripts that are placed in `.git/hooks/`
//! and delegate to `daft run <hook-name>`. This allows daft's YAML
//! configuration to drive standard git hooks (pre-commit, commit-msg, etc.).

use anyhow::{Context, Result};
use std::path::{Path, PathBuf};

/// Git hooks that can be managed via shims.
const GIT_HOOK_NAMES: &[&str] = &[
    "pre-commit",
    "commit-msg",
    "pre-push",
    "post-checkout",
    "post-merge",
    "post-rewrite",
    "prepare-commit-msg",
];

/// Generate the content of a git hook shim script.
fn shim_content(hook_name: &str) -> String {
    format!(
        r#"#!/bin/sh
# Managed by daft — do not edit manually.
# To uninstall: daft hooks uninstall
if command -v daft >/dev/null 2>&1; then
  daft run {hook_name} -- "$@"
else
  echo "daft: command not found, skipping {hook_name} hook" >&2
fi
"#
    )
}

/// Check if a hook shim was installed by daft.
fn is_daft_shim(path: &Path) -> bool {
    std::fs::read_to_string(path)
        .map(|content| content.contains("Managed by daft"))
        .unwrap_or(false)
}

/// Get the git hooks directory for the current repository.
pub fn git_hooks_dir() -> Result<PathBuf> {
    let output = std::process::Command::new("git")
        .args(["rev-parse", "--git-common-dir"])
        .output()
        .context("Failed to execute git rev-parse")?;

    if !output.status.success() {
        anyhow::bail!("Not in a git repository");
    }

    let git_dir = PathBuf::from(
        String::from_utf8(output.stdout)
            .context("Invalid UTF-8 in git output")?
            .trim(),
    );

    Ok(git_dir.join("hooks"))
}

/// Install shims for all git hooks defined in the YAML config.
///
/// Only installs shims for hooks that appear in `KNOWN_HOOK_NAMES` and
/// are standard git hooks. Skips daft lifecycle hooks (post-clone, etc.).
pub fn install_shims(
    configured_hooks: &[String],
    force: bool,
) -> Result<(Vec<String>, Vec<String>)> {
    let hooks_dir = git_hooks_dir()?;
    std::fs::create_dir_all(&hooks_dir)
        .with_context(|| format!("Failed to create hooks directory: {}", hooks_dir.display()))?;

    let mut installed = Vec::new();
    let mut skipped = Vec::new();

    for hook_name in configured_hooks {
        // Only install shims for recognized git hooks
        if !GIT_HOOK_NAMES.contains(&hook_name.as_str()) {
            continue;
        }

        let hook_path = hooks_dir.join(hook_name);

        if hook_path.exists() && !is_daft_shim(&hook_path) && !force {
            skipped.push(hook_name.clone());
            continue;
        }

        std::fs::write(&hook_path, shim_content(hook_name))
            .with_context(|| format!("Failed to write shim: {}", hook_path.display()))?;

        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(&hook_path, std::fs::Permissions::from_mode(0o755))
                .with_context(|| {
                    format!("Failed to set permissions on: {}", hook_path.display())
                })?;
        }

        installed.push(hook_name.clone());
    }

    Ok((installed, skipped))
}

/// Uninstall all daft-managed shims from `.git/hooks/`.
pub fn uninstall_shims() -> Result<Vec<String>> {
    let hooks_dir = git_hooks_dir()?;
    let mut removed = Vec::new();

    if !hooks_dir.exists() {
        return Ok(removed);
    }

    for hook_name in GIT_HOOK_NAMES {
        let hook_path = hooks_dir.join(hook_name);
        if hook_path.exists() && is_daft_shim(&hook_path) {
            std::fs::remove_file(&hook_path)
                .with_context(|| format!("Failed to remove shim: {}", hook_path.display()))?;
            removed.push(hook_name.to_string());
        }
    }

    Ok(removed)
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::tempdir;

    #[test]
    fn test_shim_content() {
        let content = shim_content("pre-commit");
        assert!(content.contains("#!/bin/sh"));
        assert!(content.contains("Managed by daft"));
        assert!(content.contains("daft run pre-commit"));
    }

    #[test]
    fn test_is_daft_shim() {
        let dir = tempdir().unwrap();
        let path = dir.path().join("pre-commit");
        std::fs::write(&path, shim_content("pre-commit")).unwrap();
        assert!(is_daft_shim(&path));

        std::fs::write(&path, "#!/bin/sh\necho hello").unwrap();
        assert!(!is_daft_shim(&path));
    }

    #[test]
    fn test_is_daft_shim_nonexistent() {
        let dir = tempdir().unwrap();
        assert!(!is_daft_shim(&dir.path().join("nope")));
    }
}
