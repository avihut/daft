//! Test environment management for the manual test framework.
//!
//! `TestEnv` creates and manages an isolated filesystem sandbox for each
//! scenario run, handling directory layout, variable expansion, git identity
//! isolation, and reset/cleanup.

use anyhow::{Context, Result};
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

use super::schema::Scenario;

/// Manages the lifecycle of a single test environment (sandbox).
pub struct TestEnv {
    /// Root of the test sandbox (e.g., `/tmp/daft-manual-test-<timestamp>/`).
    pub base_dir: PathBuf,
    /// Directory containing bare remote repos.
    pub remotes_dir: PathBuf,
    /// Snapshot of remotes/ taken after initial setup, used by `reset()`.
    pub template_dir: PathBuf,
    /// Working directory where test commands execute.
    pub work_dir: PathBuf,
    /// Path to the locally-built daft binaries.
    pub binary_dir: PathBuf,
    /// Path to an empty gitconfig file that isolates tests from user config.
    pub git_config_path: PathBuf,
    /// Isolated daft config directory (prevents global config leakage).
    pub daft_config_dir: PathBuf,
    /// Isolated daft data directory (prevents centralized worktrees from
    /// polluting the real XDG data dir).
    pub daft_data_dir: PathBuf,
    /// Variable store for `$VAR` expansion in step commands and paths.
    vars: HashMap<String, String>,
}

impl TestEnv {
    /// Create a new test environment on disk for the given scenario.
    ///
    /// This creates the sandbox directory tree and initialises built-in
    /// variables (`WORK_DIR`, `BASE_DIR`, `BINARY_DIR`) plus any extra vars
    /// from `scenario.env`.
    pub fn create(scenario: &Scenario, project_root: &Path) -> Result<Self> {
        let base_dir = if let Ok(base) = std::env::var("DAFT_MANUAL_TEST_BASE") {
            // Deterministic path under a managed directory (e.g., sandbox/test/).
            let slug = scenario.name.to_lowercase().replace(' ', "-");
            PathBuf::from(base).join(slug)
        } else {
            let timestamp = SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .context("system clock before UNIX epoch")?
                .as_millis();
            PathBuf::from(format!("/tmp/daft-manual-test-{timestamp}"))
        };
        let remotes_dir = base_dir.join("remotes");
        let template_dir = base_dir.join("remotes-template");
        let work_dir = base_dir.join("work");
        let binary_dir = project_root.join("target/release");
        let git_config_path = base_dir.join("gitconfig");
        let daft_config_dir = base_dir.join("daft-config");
        let daft_data_dir = base_dir.join("daft-data");

        std::fs::create_dir_all(&remotes_dir)
            .with_context(|| format!("creating remotes dir: {}", remotes_dir.display()))?;
        std::fs::create_dir_all(&work_dir)
            .with_context(|| format!("creating work dir: {}", work_dir.display()))?;
        std::fs::create_dir_all(&daft_config_dir)
            .with_context(|| format!("creating daft config dir: {}", daft_config_dir.display()))?;
        std::fs::create_dir_all(&daft_data_dir)
            .with_context(|| format!("creating daft data dir: {}", daft_data_dir.display()))?;
        std::fs::write(&git_config_path, "")
            .with_context(|| format!("creating gitconfig: {}", git_config_path.display()))?;

        let mut vars = HashMap::new();
        vars.insert("WORK_DIR".into(), work_dir.to_string_lossy().into_owned());
        vars.insert("BASE_DIR".into(), base_dir.to_string_lossy().into_owned());
        vars.insert(
            "BINARY_DIR".into(),
            binary_dir.to_string_lossy().into_owned(),
        );
        vars.insert(
            "DAFT_DATA_DIR".into(),
            daft_data_dir.to_string_lossy().into_owned(),
        );

        for (k, v) in &scenario.env {
            vars.insert(k.clone(), v.clone());
        }

        Ok(Self {
            base_dir,
            remotes_dir,
            template_dir,
            work_dir,
            binary_dir,
            git_config_path,
            daft_config_dir,
            daft_data_dir,
            vars,
        })
    }

    /// Create a `TestEnv` with only variables set (paths are dummy values).
    ///
    /// Intended for unit-testing variable expansion without touching the
    /// filesystem.
    #[cfg(test)]
    pub fn new_with_vars(vars: HashMap<String, String>) -> Self {
        Self {
            base_dir: PathBuf::from("/tmp/test-dummy"),
            remotes_dir: PathBuf::from("/tmp/test-dummy/remotes"),
            template_dir: PathBuf::from("/tmp/test-dummy/remotes-template"),
            work_dir: PathBuf::from("/tmp/test-dummy/work"),
            binary_dir: PathBuf::from("/tmp/test-dummy/bin"),
            git_config_path: PathBuf::from("/tmp/test-dummy/gitconfig"),
            daft_config_dir: PathBuf::from("/tmp/test-dummy/daft-config"),
            daft_data_dir: PathBuf::from("/tmp/test-dummy/daft-data"),
            vars,
        }
    }

    /// Replace `$VAR_NAME` patterns in `input` with their values from the var
    /// store.
    ///
    /// Variable names consist of uppercase ASCII letters, digits, and
    /// underscores. Unknown variables are left as-is (including the `$`
    /// prefix).
    pub fn expand_vars(&self, input: &str) -> String {
        let mut result = String::with_capacity(input.len());
        let chars: Vec<char> = input.chars().collect();
        let len = chars.len();
        let mut i = 0;

        while i < len {
            if chars[i] == '$' && i + 1 < len && is_var_char(chars[i + 1]) {
                // Scan the variable name.
                let start = i + 1;
                let mut end = start;
                while end < len && is_var_char(chars[end]) {
                    end += 1;
                }
                let var_name: String = chars[start..end].iter().collect();
                if let Some(value) = self.vars.get(&var_name) {
                    result.push_str(value);
                } else {
                    // Unknown variable — leave as-is.
                    result.push('$');
                    result.push_str(&var_name);
                }
                i = end;
            } else {
                result.push(chars[i]);
                i += 1;
            }
        }

        result
    }

    /// Register a remote repository, making its path available as
    /// `$REMOTE_<NAME>` (uppercased, hyphens replaced with underscores).
    pub fn register_remote(&mut self, repo_name: &str) {
        let var_name = format!("REMOTE_{}", repo_name.to_uppercase().replace('-', "_"));
        let path = self.remotes_dir.join(repo_name);
        self.vars
            .insert(var_name, path.to_string_lossy().into_owned());
    }

    /// Snapshot `remotes/` → `remotes-template/` so that `reset()` can
    /// restore the original state. Uses `cp -a` to preserve git objects.
    pub fn create_template(&self) -> Result<()> {
        let status = std::process::Command::new("cp")
            .args(["-a"])
            .arg(&self.remotes_dir)
            .arg(&self.template_dir)
            .status()
            .context("failed to run cp -a for template creation")?;

        anyhow::ensure!(status.success(), "cp -a failed with status {status}");
        Ok(())
    }

    /// Reset the sandbox to its initial state: clear `work/`, restore
    /// `remotes/` from the template snapshot (if one exists).
    pub fn reset(&self) -> Result<()> {
        // Clear work directory contents.
        if self.work_dir.exists() {
            std::fs::remove_dir_all(&self.work_dir)
                .with_context(|| format!("removing work dir: {}", self.work_dir.display()))?;
        }
        std::fs::create_dir_all(&self.work_dir)
            .with_context(|| format!("recreating work dir: {}", self.work_dir.display()))?;

        // Restore remotes from template if available.
        if self.template_dir.exists() {
            if self.remotes_dir.exists() {
                std::fs::remove_dir_all(&self.remotes_dir).with_context(|| {
                    format!("removing remotes dir: {}", self.remotes_dir.display())
                })?;
            }

            let status = std::process::Command::new("cp")
                .args(["-a"])
                .arg(&self.template_dir)
                .arg(&self.remotes_dir)
                .status()
                .context("failed to run cp -a for reset")?;

            anyhow::ensure!(status.success(), "cp -a failed with status {status}");
        }

        Ok(())
    }

    /// Remove the entire sandbox directory tree.
    pub fn cleanup(&self) -> Result<()> {
        if self.base_dir.exists() {
            std::fs::remove_dir_all(&self.base_dir)
                .with_context(|| format!("removing base dir: {}", self.base_dir.display()))?;
        }
        Ok(())
    }

    /// Build the environment variable map for subprocess execution.
    ///
    /// Includes git identity isolation, daft feature flags, and PATH with
    /// `binary_dir` prepended so locally-built binaries take precedence.
    pub fn command_env(&self) -> HashMap<String, String> {
        let mut env = HashMap::new();

        // Scenario vars first — these can be overridden by safety vars below.
        for (k, v) in &self.vars {
            env.insert(k.clone(), v.clone());
        }

        // Safety vars LAST — cannot be overridden by scenario definitions.
        // Git identity — local to test, never touches global config.
        env.insert("GIT_AUTHOR_NAME".into(), "Manual Test".into());
        env.insert("GIT_AUTHOR_EMAIL".into(), "test@daft.test".into());
        env.insert("GIT_COMMITTER_NAME".into(), "Manual Test".into());
        env.insert("GIT_COMMITTER_EMAIL".into(), "test@daft.test".into());
        env.insert(
            "GIT_CONFIG_GLOBAL".into(),
            self.git_config_path.to_string_lossy().into_owned(),
        );

        // Daft feature flags.
        env.insert("DAFT_TESTING".into(), "1".into());
        env.insert("DAFT_NO_UPDATE_CHECK".into(), "1".into());
        env.insert("DAFT_NO_TRUST_PRUNE".into(), "1".into());
        env.insert(
            "DAFT_CONFIG_DIR".into(),
            self.daft_config_dir.to_string_lossy().into_owned(),
        );
        env.insert(
            "DAFT_DATA_DIR".into(),
            self.daft_data_dir.to_string_lossy().into_owned(),
        );

        // PATH — binary_dir first so locally-built daft wins.
        let existing_path = std::env::var("PATH").unwrap_or_default();
        env.insert(
            "PATH".into(),
            format!("{}:{existing_path}", self.binary_dir.display()),
        );

        env
    }
}

/// Returns `true` if `c` is a valid variable-name character (A-Z, 0-9, _).
fn is_var_char(c: char) -> bool {
    c.is_ascii_uppercase() || c.is_ascii_digit() || c == '_'
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_expand_vars_simple() {
        let mut vars = HashMap::new();
        vars.insert("WORK_DIR".into(), "/tmp/work".into());
        vars.insert("REMOTE_MY_PROJECT".into(), "/tmp/remotes/my-project".into());
        let env = TestEnv::new_with_vars(vars);
        assert_eq!(env.expand_vars("$WORK_DIR/foo"), "/tmp/work/foo");
        assert_eq!(
            env.expand_vars("$REMOTE_MY_PROJECT"),
            "/tmp/remotes/my-project"
        );
    }

    #[test]
    fn test_expand_vars_no_match() {
        let env = TestEnv::new_with_vars(HashMap::new());
        assert_eq!(env.expand_vars("no vars here"), "no vars here");
    }

    #[test]
    fn test_expand_vars_multiple() {
        let mut vars = HashMap::new();
        vars.insert("A".into(), "1".into());
        vars.insert("B".into(), "2".into());
        let env = TestEnv::new_with_vars(vars);
        assert_eq!(env.expand_vars("$A and $B"), "1 and 2");
    }

    #[test]
    fn test_expand_vars_unknown_left_as_is() {
        let env = TestEnv::new_with_vars(HashMap::new());
        assert_eq!(env.expand_vars("$UNKNOWN_VAR"), "$UNKNOWN_VAR");
    }

    #[test]
    fn test_register_remote() {
        let mut env = TestEnv::new_with_vars(HashMap::new());
        env.remotes_dir = PathBuf::from("/tmp/remotes");
        env.register_remote("my-project");
        assert_eq!(
            env.expand_vars("$REMOTE_MY_PROJECT"),
            "/tmp/remotes/my-project"
        );
    }

    #[test]
    fn test_command_env_has_git_identity() {
        let env = TestEnv::new_with_vars(HashMap::new());
        let cmd_env = env.command_env();
        assert_eq!(cmd_env.get("GIT_AUTHOR_NAME").unwrap(), "Manual Test");
        assert_eq!(cmd_env.get("DAFT_TESTING").unwrap(), "1");
    }
}
