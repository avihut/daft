//! Isolated filesystem sandbox for one scenario run.
//!
//! [`Sandbox`] owns the directory layout, scenario-variable store, git identity
//! isolation, and `reset` / `cleanup` lifecycle for a single test scenario.
//! It is intentionally project-agnostic: it knows nothing about daft, its env
//! vars, or its binary path. Daft-specific concerns live in
//! [`super::daft_executor::DaftCommandExecutor`], which adapts the sandbox to
//! the [`super::executor::CommandExecutor`] port.

use anyhow::{Context, Result};
use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::{SystemTime, UNIX_EPOCH};

use super::schema::Scenario;

/// Within-process counter that guarantees [`alloc_default_base_dir`] produces
/// a unique path even when called concurrently from rayon workers. The
/// nanosecond+pid prefix disambiguates across overlapping xtask invocations.
static SANDBOX_COUNTER: AtomicUsize = AtomicUsize::new(0);

/// Allocate the next unique sandbox base directory under `/tmp`.
///
/// The path is reserved on the path namespace only — no filesystem state is
/// created — so workers can register the path with the SIGINT cleanup set
/// before any directory I/O begins. The nanosecond + pid + counter triple
/// guarantees uniqueness across rayon workers and overlapping xtask
/// invocations.
pub fn alloc_default_base_dir() -> Result<PathBuf> {
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .context("system clock before UNIX epoch")?
        .as_nanos();
    let pid = std::process::id();
    let counter = SANDBOX_COUNTER.fetch_add(1, Ordering::Relaxed);
    Ok(PathBuf::from(format!(
        "/tmp/daft-manual-test-{nanos}-{pid}-{counter}"
    )))
}

/// Manages the lifecycle of a single test environment (sandbox).
pub struct Sandbox {
    /// Root of the test sandbox (e.g., `/tmp/daft-manual-test-<timestamp>/`).
    pub base_dir: PathBuf,
    /// Directory containing bare remote repos.
    pub remotes_dir: PathBuf,
    /// Snapshot of remotes/ taken after initial setup, used by `reset()`.
    pub template_dir: PathBuf,
    /// Working directory where test commands execute.
    pub work_dir: PathBuf,
    /// Path to an empty gitconfig file that isolates tests from user config.
    pub git_config_path: PathBuf,
    /// Variable store for `$VAR` expansion in step commands and paths.
    vars: HashMap<String, String>,
    /// When true, `Drop` removes `base_dir` — guarantees cleanup on early
    /// returns and panics. Set to false for `--keep` and `--setup-only`.
    cleanup_on_drop: bool,
}

impl Drop for Sandbox {
    fn drop(&mut self) {
        if self.cleanup_on_drop && self.base_dir.exists() {
            let _ = std::fs::remove_dir_all(&self.base_dir);
        }
    }
}

impl Sandbox {
    /// Create a new test sandbox on disk for the given scenario.
    ///
    /// Picks a fresh sandbox path via [`alloc_default_base_dir`] and delegates
    /// to [`Self::create_at`]. Prefer [`Self::create_at`] in the parallel
    /// worker path so the cleanup registry can be populated before any
    /// directory I/O.
    #[allow(dead_code)]
    pub fn create(scenario: &Scenario, keep: bool) -> Result<Self> {
        let base_dir = alloc_default_base_dir()?;
        Self::create_at(scenario, base_dir, keep)
    }

    /// Create a sandbox rooted at a caller-supplied `base_dir`.
    ///
    /// Used by the parallel worker so it can register `base_dir` with the
    /// SIGINT cleanup set before any directories are created — that way a
    /// signal arriving mid-create still leaves a tracked path the handler
    /// can `rm -rf`.
    pub fn create_at(scenario: &Scenario, base_dir: PathBuf, keep: bool) -> Result<Self> {
        let remotes_dir = base_dir.join("remotes");
        let template_dir = base_dir.join("remotes-template");
        let work_dir = base_dir.join("work");
        let git_config_path = base_dir.join("gitconfig");

        std::fs::create_dir_all(&remotes_dir)
            .with_context(|| format!("creating remotes dir: {}", remotes_dir.display()))?;
        std::fs::create_dir_all(&work_dir)
            .with_context(|| format!("creating work dir: {}", work_dir.display()))?;
        std::fs::write(&git_config_path, "")
            .with_context(|| format!("creating gitconfig: {}", git_config_path.display()))?;

        let mut vars = HashMap::new();
        vars.insert("WORK_DIR".into(), work_dir.to_string_lossy().into_owned());
        vars.insert("BASE_DIR".into(), base_dir.to_string_lossy().into_owned());

        for (k, v) in &scenario.env {
            vars.insert(k.clone(), v.clone());
        }

        Ok(Self {
            base_dir,
            remotes_dir,
            template_dir,
            work_dir,
            git_config_path,
            vars,
            cleanup_on_drop: !keep,
        })
    }

    /// Create a `Sandbox` with only variables set (paths are dummy values).
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
            git_config_path: PathBuf::from("/tmp/test-dummy/gitconfig"),
            vars,
            cleanup_on_drop: false,
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

    /// Register an arbitrary `$NAME` variable in the sandbox's var store.
    ///
    /// Used by adapters to surface adapter-managed paths (e.g., the daft data
    /// dir) to scenario commands without leaking adapter internals into the
    /// sandbox's own constructor.
    pub fn register_var(&mut self, name: &str, value: String) {
        self.vars.insert(name.to_string(), value);
    }

    /// Read-only view of the scenario var store.
    ///
    /// Adapters call this when building the subprocess env so scenario-defined
    /// values flow into the child process under their original names. Safety
    /// vars (git identity, daemon-suppression flags) are layered on top by
    /// the adapter, which is why this is intentionally distinct from `env`
    /// construction.
    pub fn scenario_vars(&self) -> &HashMap<String, String> {
        &self.vars
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
        let sb = Sandbox::new_with_vars(vars);
        assert_eq!(sb.expand_vars("$WORK_DIR/foo"), "/tmp/work/foo");
        assert_eq!(
            sb.expand_vars("$REMOTE_MY_PROJECT"),
            "/tmp/remotes/my-project"
        );
    }

    #[test]
    fn test_expand_vars_no_match() {
        let sb = Sandbox::new_with_vars(HashMap::new());
        assert_eq!(sb.expand_vars("no vars here"), "no vars here");
    }

    #[test]
    fn test_expand_vars_multiple() {
        let mut vars = HashMap::new();
        vars.insert("A".into(), "1".into());
        vars.insert("B".into(), "2".into());
        let sb = Sandbox::new_with_vars(vars);
        assert_eq!(sb.expand_vars("$A and $B"), "1 and 2");
    }

    #[test]
    fn test_expand_vars_unknown_left_as_is() {
        let sb = Sandbox::new_with_vars(HashMap::new());
        assert_eq!(sb.expand_vars("$UNKNOWN_VAR"), "$UNKNOWN_VAR");
    }

    #[test]
    fn test_register_remote() {
        let mut sb = Sandbox::new_with_vars(HashMap::new());
        sb.remotes_dir = PathBuf::from("/tmp/remotes");
        sb.register_remote("my-project");
        assert_eq!(
            sb.expand_vars("$REMOTE_MY_PROJECT"),
            "/tmp/remotes/my-project"
        );
    }

    #[test]
    fn test_register_var() {
        let mut sb = Sandbox::new_with_vars(HashMap::new());
        sb.register_var("MY_VAR", "/some/path".into());
        assert_eq!(sb.expand_vars("$MY_VAR/foo"), "/some/path/foo");
    }

    /// Regression test for the millisecond-timestamp collision bug:
    /// two `Sandbox::create` calls in quick succession must produce distinct
    /// `base_dir`s. Pre-#510 this used `as_millis()` and would collide under
    /// rayon-parallel scheduling.
    #[test]
    fn test_create_produces_unique_base_dirs() {
        let scenario = Scenario {
            name: "unique-test".to_string(),
            description: None,
            repos: Vec::new(),
            env: HashMap::new(),
            steps: Vec::new(),
        };

        let mut paths = Vec::new();
        for _ in 0..16 {
            let sb = Sandbox::create(&scenario, false).expect("Sandbox::create should succeed");
            paths.push(sb.base_dir.clone());
            // sb drops here, removing its sandbox.
        }

        let unique: std::collections::HashSet<_> = paths.iter().collect();
        assert_eq!(
            unique.len(),
            paths.len(),
            "Sandbox::create produced colliding base_dirs: {paths:?}"
        );
    }
}
