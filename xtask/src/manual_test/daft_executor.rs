//! Daft-specific adapter for the [`CommandExecutor`] port.
//!
//! Owns every assumption the runner makes about daft itself:
//!   - `target/release/` is on `PATH` (locally-built `daft` wins over any
//!     system install).
//!   - `DAFT_CONFIG_DIR` and `DAFT_DATA_DIR` are per-sandbox so suites running
//!     in parallel never read each other's trust / repo state.
//!   - The daemon-suppression flags (`DAFT_TESTING`, `DAFT_NO_UPDATE_CHECK`,
//!     `DAFT_NO_TRUST_PRUNE`, `DAFT_NO_LOG_CLEAN`) prevent orphaned background
//!     processes from accumulating across a parallel suite — load average
//!     used to climb into the hundreds without them.
//!
//! Keeping all of this in the adapter is what lets the runner core compile
//! and run against a non-daft executor (see [`super::runner`]'s `FakeExecutor`
//! tests). Future #509 sub-tasks (e.g. `DAFT_BINARY_DIR=` for #514) extend the
//! constructor here, not the runner.

use anyhow::{Context, Result};
use std::collections::HashMap;
use std::os::unix::process::CommandExt;
use std::path::{Path, PathBuf};
use std::process::Command;

use super::executor::{CommandExecutor, CommandOutput};
use super::sandbox::Sandbox;

/// Adapter that runs scenario commands against a locally-built `daft`.
pub struct DaftCommandExecutor {
    /// Directory containing the daft binary (and any symlinked multicalls
    /// like `git-worktree-clone`). Prepended to `PATH` so locally-built
    /// binaries win over a system install.
    binary_dir: PathBuf,
    /// Per-sandbox config dir surfaced as `DAFT_CONFIG_DIR`.
    daft_config_dir: PathBuf,
    /// Per-sandbox data dir surfaced as `DAFT_DATA_DIR` and `$DAFT_DATA_DIR`
    /// (the var-expansion form is registered on the sandbox at construction
    /// time so scenario commands can reference it directly).
    daft_data_dir: PathBuf,
}

impl DaftCommandExecutor {
    /// Construct an adapter for `sandbox` and register the daft-specific
    /// variables (`$BINARY_DIR`, `$DAFT_DATA_DIR`) on the sandbox so scenario
    /// commands can refer to them.
    pub fn new_for_sandbox(sandbox: &mut Sandbox, project_root: &Path) -> Result<Self> {
        let binary_dir = project_root.join("target/release");
        let daft_config_dir = sandbox.base_dir.join("daft-config");
        let daft_data_dir = sandbox.base_dir.join("daft-data");

        std::fs::create_dir_all(&daft_config_dir)
            .with_context(|| format!("creating daft config dir: {}", daft_config_dir.display()))?;
        std::fs::create_dir_all(&daft_data_dir)
            .with_context(|| format!("creating daft data dir: {}", daft_data_dir.display()))?;

        // Surface the adapter-managed paths to scenario commands. These were
        // historically baked into the sandbox's own var store; keeping them
        // here is what lets the sandbox stay daft-agnostic.
        sandbox.register_var("BINARY_DIR", binary_dir.to_string_lossy().into_owned());
        sandbox.register_var(
            "DAFT_DATA_DIR",
            daft_data_dir.to_string_lossy().into_owned(),
        );

        Ok(Self {
            binary_dir,
            daft_config_dir,
            daft_data_dir,
        })
    }

    /// Build the environment passed to `bash -c` for a step.
    ///
    /// Layered so safety-critical entries (git identity, daemon suppression,
    /// config-dir isolation) cannot be overridden by scenario-defined env —
    /// scenario vars come first, safety vars last.
    fn build_env(&self, sandbox: &Sandbox) -> HashMap<String, String> {
        let mut env = HashMap::new();

        // Scenario vars first — these can be overridden by safety vars below.
        for (k, v) in sandbox.scenario_vars() {
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
            sandbox.git_config_path.to_string_lossy().into_owned(),
        );

        // Daft feature flags. Disable every daemon-style background spawn:
        // the test harness invokes `daft` many times back-to-back, and any
        // detached child that survives its parent (e.g. `daft __clean-logs`)
        // accumulates as init-reparented orphans and steals CPU — visible as
        // load-average climbing into the hundreds during parallel runs.
        env.insert("DAFT_TESTING".into(), "1".into());
        env.insert("DAFT_NO_UPDATE_CHECK".into(), "1".into());
        env.insert("DAFT_NO_TRUST_PRUNE".into(), "1".into());
        env.insert("DAFT_NO_LOG_CLEAN".into(), "1".into());
        env.insert(
            "DAFT_CONFIG_DIR".into(),
            self.daft_config_dir.to_string_lossy().into_owned(),
        );
        env.insert(
            "DAFT_DATA_DIR".into(),
            self.daft_data_dir.to_string_lossy().into_owned(),
        );

        // PATH — binary_dir first so locally-built daft wins. `to_string_lossy`
        // (not `display`) is the right idiom here: this is a string going into
        // the subprocess env, not human-readable terminal output.
        let existing_path = std::env::var("PATH").unwrap_or_default();
        env.insert(
            "PATH".into(),
            format!("{}:{existing_path}", self.binary_dir.to_string_lossy()),
        );

        env
    }
}

impl CommandExecutor for DaftCommandExecutor {
    fn execute(&self, command: &str, cwd: &Path, sandbox: &Sandbox) -> Result<CommandOutput> {
        let expanded = sandbox.expand_vars(command);
        // process_group(0) puts the child in its own process group so the
        // terminal's SIGINT (sent to the foreground process group) doesn't
        // hit it. Without this, Ctrl+C delivered to the runner is also
        // delivered to every in-flight bash subprocess and they exit with
        // signal-killed status — the runner then sees a "step failed"
        // (non-zero exit) and marks the scenario as Fail instead of
        // Cancelled. The runner's own ctrlc handler is the sole intended
        // observer of SIGINT; subprocesses must be insulated from it.
        let output = Command::new("bash")
            .process_group(0)
            .args(["-c", &expanded])
            .current_dir(cwd)
            .envs(self.build_env(sandbox))
            .output()
            .with_context(|| format!("Failed to execute: {expanded}"))?;

        Ok(CommandOutput {
            exit_code: output.status.code().unwrap_or(-1),
            stdout: output.stdout,
            stderr: output.stderr,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    /// Dummy `project_root` value. The adapter only uses it to compute
    /// `project_root.join("target/release")` for PATH construction — no I/O
    /// against this path, so a non-existent dummy is fine.
    fn project_root() -> PathBuf {
        PathBuf::from("/nonexistent/dummy-project-root")
    }

    /// Build a `Sandbox` whose `base_dir` points at a fresh temp directory.
    /// The returned `TempDir` must outlive the sandbox: dropping it removes
    /// the directory tree.
    fn sandbox_with_tempdir() -> (Sandbox, TempDir) {
        let tmp = tempfile::tempdir().expect("create temp sandbox base dir");
        let mut sandbox = Sandbox::new_with_vars(HashMap::new());
        sandbox.base_dir = tmp.path().to_path_buf();
        (sandbox, tmp)
    }

    #[test]
    fn build_env_has_git_identity_and_daft_flags() {
        let (mut sandbox, _tmp) = sandbox_with_tempdir();

        let exec = DaftCommandExecutor::new_for_sandbox(&mut sandbox, &project_root()).unwrap();
        let env = exec.build_env(&sandbox);

        assert_eq!(env.get("GIT_AUTHOR_NAME").unwrap(), "Manual Test");
        assert_eq!(env.get("DAFT_TESTING").unwrap(), "1");
        assert_eq!(env.get("DAFT_NO_UPDATE_CHECK").unwrap(), "1");
        assert!(env.get("PATH").unwrap().contains("target/release"));
        assert!(env.get("DAFT_CONFIG_DIR").unwrap().contains("daft-config"));
        assert!(env.get("DAFT_DATA_DIR").unwrap().contains("daft-data"));
    }

    #[test]
    fn registers_binary_dir_and_data_dir_vars_on_sandbox() {
        let (mut sandbox, _tmp) = sandbox_with_tempdir();

        let _exec = DaftCommandExecutor::new_for_sandbox(&mut sandbox, &project_root()).unwrap();

        // After construction, $BINARY_DIR and $DAFT_DATA_DIR are expandable
        // through the sandbox's normal variable expansion.
        let expanded = sandbox.expand_vars("$BINARY_DIR/daft and data=$DAFT_DATA_DIR");
        assert!(expanded.contains("target/release"));
        assert!(expanded.contains("daft-data"));
    }
}
