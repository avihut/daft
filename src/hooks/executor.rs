//! Hook execution logic.
//!
//! This module provides the `HookExecutor` which handles discovering,
//! validating, and executing hooks with proper security checks.

use super::yaml_config_loader;
use super::yaml_executor;
use super::{
    find_hooks, list_hooks, FailMode, HookConfig, HookContext, HookEnvironment, HookType,
    HooksConfig, TrustDatabase, TrustLevel, DEPRECATED_HOOK_REMOVAL_VERSION,
};
use crate::output::Output;
use anyhow::{Context, Result};
use std::io::{BufRead, BufReader};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::time::Duration;

/// Result of a hook execution.
#[derive(Debug, Clone)]
pub struct HookResult {
    /// Whether the hook succeeded (exit code 0).
    pub success: bool,
    /// Exit code from the hook.
    pub exit_code: Option<i32>,
    /// Standard output from the hook.
    pub stdout: String,
    /// Standard error from the hook.
    pub stderr: String,
    /// Whether the hook was skipped (not run).
    pub skipped: bool,
    /// Reason for skipping, if applicable.
    pub skip_reason: Option<String>,
}

impl HookResult {
    /// Create a successful result.
    pub fn success() -> Self {
        Self {
            success: true,
            exit_code: Some(0),
            stdout: String::new(),
            stderr: String::new(),
            skipped: false,
            skip_reason: None,
        }
    }

    /// Create a skipped result.
    pub fn skipped(reason: impl Into<String>) -> Self {
        Self {
            success: true,
            exit_code: None,
            stdout: String::new(),
            stderr: String::new(),
            skipped: true,
            skip_reason: Some(reason.into()),
        }
    }

    /// Create a failed result.
    pub fn failed(exit_code: i32, stdout: String, stderr: String) -> Self {
        Self {
            success: false,
            exit_code: Some(exit_code),
            stdout,
            stderr,
            skipped: false,
            skip_reason: None,
        }
    }
}

/// Callback for prompting the user for permission.
pub type PromptCallback = Box<dyn Fn(&str) -> bool>;

/// Hook executor that manages hook discovery and execution.
pub struct HookExecutor {
    config: HooksConfig,
    trust_db: TrustDatabase,
    prompt_callback: Option<PromptCallback>,
}

impl HookExecutor {
    /// Create a new hook executor with the given configuration.
    pub fn new(config: HooksConfig) -> Result<Self> {
        let trust_db = TrustDatabase::load().unwrap_or_default();
        Ok(Self {
            config,
            trust_db,
            prompt_callback: None,
        })
    }

    /// Create a new hook executor with a custom trust database.
    pub fn with_trust_db(config: HooksConfig, trust_db: TrustDatabase) -> Self {
        Self {
            config,
            trust_db,
            prompt_callback: None,
        }
    }

    /// Set a callback for prompting the user.
    pub fn with_prompt_callback(mut self, callback: PromptCallback) -> Self {
        self.prompt_callback = Some(callback);
        self
    }

    /// Execute a hook with the given context.
    ///
    /// This method handles:
    /// 1. Checking if hooks are enabled
    /// 2. Trying YAML config first (if `daft.yml` exists and defines this hook)
    /// 3. Falling back to legacy script execution
    /// 4. Checking trust level for the repository
    /// 5. Handling success/failure based on fail mode
    pub fn execute(&self, ctx: &HookContext, output: &mut dyn Output) -> Result<HookResult> {
        // Check if hooks are globally enabled
        if !self.config.enabled {
            return Ok(HookResult::skipped("Hooks are globally disabled"));
        }

        // Check if this specific hook is enabled
        let hook_config = self.config.get_hook_config(ctx.hook_type);
        if !hook_config.enabled {
            return Ok(HookResult::skipped(format!(
                "{} hook is disabled",
                ctx.hook_type
            )));
        }

        // Determine the worktree to read hooks from
        let hook_source_worktree = self.get_hook_source_worktree(ctx);

        // Try YAML config first
        match self.try_yaml_hook(ctx, &hook_source_worktree, hook_config, output) {
            Ok(Some(result)) => return Ok(result),
            Ok(None) => {} // No YAML config or no definition for this hook — fall through to legacy
            Err(e) => {
                output.warning(&format!(
                    "Error loading YAML config, falling back to script hooks: {e}"
                ));
            }
        }

        // Fallback: legacy script execution
        self.execute_legacy(ctx, hook_config, &hook_source_worktree, output)
    }

    /// Try to execute a hook via YAML configuration.
    ///
    /// Returns `Ok(Some(result))` if YAML config exists and defines this hook.
    /// Returns `Ok(None)` if no YAML config or no definition for this hook type.
    fn try_yaml_hook(
        &self,
        ctx: &HookContext,
        hook_source_worktree: &Path,
        hook_config: &HookConfig,
        output: &mut dyn Output,
    ) -> Result<Option<HookResult>> {
        let yaml_config = match yaml_config_loader::load_merged_config(hook_source_worktree)? {
            Some(config) => config,
            None => {
                return Ok(None);
            }
        };

        let hook_name = ctx.hook_type.yaml_name();

        let hook_def = match yaml_config.hooks.get(hook_name) {
            Some(def) => def,
            None => {
                return Ok(None);
            }
        };

        // Check trust level
        let trust_level = self.trust_db.get_trust_level(&ctx.git_dir);
        match trust_level {
            TrustLevel::Deny => {
                output.debug(&format!(
                    "Skipping {hook_name} YAML hooks: repository not trusted"
                ));
                return Ok(Some(HookResult::skipped("Repository not trusted")));
            }
            TrustLevel::Prompt => {
                let prompt_msg =
                    format!("Repository has YAML hook config for '{hook_name}'. Execute?");
                if let Some(ref callback) = self.prompt_callback {
                    if !callback(&prompt_msg) {
                        return Ok(Some(HookResult::skipped("User declined hook execution")));
                    }
                } else {
                    output.warning(&format!(
                        "YAML hooks exist but no permission callback configured. Skipping {hook_name}."
                    ));
                    return Ok(Some(HookResult::skipped("No permission callback")));
                }
            }
            TrustLevel::Allow => {}
        }

        let source_dir = yaml_config.source_dir.as_deref().unwrap_or(".daft");
        let rc = yaml_config.rc.as_deref();

        let env = HookEnvironment::from_context(ctx);
        let working_dir = env.working_directory(ctx);

        let result = yaml_executor::execute_yaml_hook_with_rc(
            hook_name,
            hook_def,
            ctx,
            output,
            source_dir,
            working_dir,
            rc,
        )?;

        if !result.success && !result.skipped {
            return Ok(Some(self.handle_hook_failure(
                ctx.hook_type,
                hook_config,
                result,
                output,
            )?));
        }

        Ok(Some(result))
    }

    /// Execute legacy script-based hooks.
    fn execute_legacy(
        &self,
        ctx: &HookContext,
        hook_config: &HookConfig,
        hook_source_worktree: &Path,
        output: &mut dyn Output,
    ) -> Result<HookResult> {
        // Discover hooks (handles deprecated filename resolution)
        let discovery = find_hooks(ctx.hook_type, hook_source_worktree, &self.config);

        // Emit deprecation warnings
        for warning in &discovery.deprecation_warnings {
            if warning.new_name_also_exists {
                output.warning(&format!(
                    "Both '{}' and '{}' exist in '{}'. Using '{}'; remove '{}' or run 'git daft hooks migrate'.",
                    warning.new_name,
                    warning.old_name,
                    warning.path.parent().unwrap_or(warning.path.as_path()).display(),
                    warning.new_name,
                    warning.old_name,
                ));
            } else {
                output.warning(&format!(
                    "Hook '{}' uses deprecated name '{}'. Rename to '{}' or run 'git daft hooks migrate'. \
                     Deprecated names will stop working in daft v{}.",
                    warning.path.display(),
                    warning.old_name,
                    warning.new_name,
                    DEPRECATED_HOOK_REMOVAL_VERSION
                ));
            }
        }

        if discovery.hooks.is_empty() {
            if !discovery.deprecation_warnings.is_empty() {
                return Ok(HookResult::skipped(
                    "Deprecated hook files found but not executed. Run 'git daft hooks migrate' to rename them.",
                ));
            }
            output.debug(&format!("No {} hooks found", ctx.hook_type));
            return Ok(HookResult::skipped("No hook files found"));
        }

        // Check trust level
        let trust_level = self.trust_db.get_trust_level(&ctx.git_dir);

        let has_project_hooks = discovery
            .hooks
            .iter()
            .any(|h| h.starts_with(hook_source_worktree));

        if has_project_hooks {
            match trust_level {
                TrustLevel::Deny => {
                    output.debug(&format!(
                        "Skipping {} hooks: repository not trusted",
                        ctx.hook_type
                    ));
                    return Ok(HookResult::skipped("Repository not trusted"));
                }
                TrustLevel::Prompt => {
                    if !self.prompt_for_permission(ctx, &discovery.hooks, output) {
                        return Ok(HookResult::skipped("User declined hook execution"));
                    }
                }
                TrustLevel::Allow => {
                    // Proceed without prompting
                }
            }
        }

        // Execute all hooks in order
        output.step(&format!("Running {} hook...", ctx.hook_type));

        let env = HookEnvironment::from_context(ctx);
        let working_dir = env.working_directory(ctx);

        for hook_path in &discovery.hooks {
            let result = self.execute_hook_file(hook_path, &env, working_dir, output)?;

            if !result.success {
                return self.handle_hook_failure(ctx.hook_type, hook_config, result, output);
            }
        }

        output.step(&format!("{} hook completed successfully", ctx.hook_type));
        Ok(HookResult::success())
    }

    /// Get the worktree path to read hooks from based on hook type.
    fn get_hook_source_worktree(&self, ctx: &HookContext) -> PathBuf {
        match ctx.hook_type {
            // Pre-create: target doesn't exist yet, use source
            HookType::PreCreate => ctx.source_worktree.clone(),
            // Post-create/clone/init: target now exists, use it
            HookType::PostCreate | HookType::PostClone | HookType::PostInit => {
                ctx.worktree_path.clone()
            }
            // Pre-remove: target still exists, use it
            HookType::PreRemove => ctx.worktree_path.clone(),
            // Post-remove: target is gone, use source (current worktree)
            HookType::PostRemove => ctx.source_worktree.clone(),
        }
    }

    /// Prompt the user for permission to run hooks.
    fn prompt_for_permission(
        &self,
        ctx: &HookContext,
        hooks: &[PathBuf],
        output: &mut dyn Output,
    ) -> bool {
        if let Some(ref callback) = self.prompt_callback {
            let hook_list: Vec<String> = hooks
                .iter()
                .filter_map(|p| p.file_name())
                .filter_map(|n| n.to_str())
                .map(String::from)
                .collect();

            let prompt = format!(
                "Repository has {} hooks: {}. Execute?",
                ctx.hook_type,
                hook_list.join(", ")
            );

            callback(&prompt)
        } else {
            // Default: don't execute without explicit permission
            output.warning(&format!(
                "Hooks exist but no permission callback configured. Skipping {} hooks.",
                ctx.hook_type
            ));
            false
        }
    }

    /// Execute a single hook file.
    fn execute_hook_file(
        &self,
        hook_path: &Path,
        env: &HookEnvironment,
        working_dir: &Path,
        output: &mut dyn Output,
    ) -> Result<HookResult> {
        output.debug(&format!("Executing hook: {}", hook_path.display()));

        let mut cmd = Command::new(hook_path);
        cmd.current_dir(working_dir);
        cmd.envs(env.vars());
        cmd.stdout(Stdio::piped());
        cmd.stderr(Stdio::piped());

        let mut child = cmd
            .spawn()
            .with_context(|| format!("Failed to spawn hook: {}", hook_path.display()))?;

        // Capture output while streaming to user
        let stdout_handle = child.stdout.take();
        let stderr_handle = child.stderr.take();

        let mut stdout_content = String::new();
        let mut stderr_content = String::new();

        // Read stdout
        if let Some(stdout) = stdout_handle {
            let reader = BufReader::new(stdout);
            for line in reader.lines().map_while(Result::ok) {
                output.raw(&format!("  {line}\n"));
                stdout_content.push_str(&line);
                stdout_content.push('\n');
            }
        }

        // Read stderr
        if let Some(stderr) = stderr_handle {
            let reader = BufReader::new(stderr);
            for line in reader.lines().map_while(Result::ok) {
                output.raw(&format!("  {line}\n"));
                stderr_content.push_str(&line);
                stderr_content.push('\n');
            }
        }

        // Wait for completion with timeout
        let timeout = Duration::from_secs(self.config.timeout_seconds as u64);
        let status = wait_with_timeout(&mut child, timeout)
            .with_context(|| format!("Hook execution failed: {}", hook_path.display()))?;

        let exit_code = status.code().unwrap_or(-1);

        if status.success() {
            Ok(HookResult {
                success: true,
                exit_code: Some(exit_code),
                stdout: stdout_content,
                stderr: stderr_content,
                skipped: false,
                skip_reason: None,
            })
        } else {
            Ok(HookResult::failed(
                exit_code,
                stdout_content,
                stderr_content,
            ))
        }
    }

    /// Handle a hook failure based on the fail mode.
    fn handle_hook_failure(
        &self,
        hook_type: HookType,
        config: &HookConfig,
        result: HookResult,
        output: &mut dyn Output,
    ) -> Result<HookResult> {
        let exit_code = result.exit_code.unwrap_or(-1);

        match config.fail_mode {
            FailMode::Abort => {
                output.error(&format!(
                    "{} hook failed with exit code {}",
                    hook_type, exit_code
                ));
                if !result.stderr.is_empty() {
                    output.error(&format!("Hook stderr: {}", result.stderr.trim()));
                }
                anyhow::bail!("{} hook failed with exit code {}", hook_type, exit_code);
            }
            FailMode::Warn => {
                output.warning(&format!(
                    "{} hook failed with exit code {} (continuing anyway)",
                    hook_type, exit_code
                ));
                if !result.stderr.is_empty() {
                    output.warning(&format!("Hook stderr: {}", result.stderr.trim()));
                }
                Ok(result)
            }
        }
    }

    /// Check if hooks exist for a worktree and display a notice if untrusted.
    pub fn check_hooks_notice(
        &self,
        worktree_path: &Path,
        git_dir: &Path,
        output: &mut dyn Output,
    ) {
        let hooks = list_hooks(worktree_path);
        if hooks.is_empty() {
            return;
        }

        let trust_level = self.trust_db.get_trust_level(git_dir);
        if trust_level == TrustLevel::Deny {
            output.warning("This repository contains hooks in .daft/hooks/:");
            for hook in &hooks {
                output.list_item(hook.filename());
            }
            output.warning("");
            output.warning("Hooks were NOT executed. To enable hooks for this repository:");
            output.warning("  git daft hooks trust");
        }
    }

    /// Get the trust level for a repository.
    pub fn get_trust_level(&self, git_dir: &Path) -> TrustLevel {
        self.trust_db.get_trust_level(git_dir)
    }

    /// Trust a repository.
    pub fn trust_repository(&mut self, git_dir: &Path, level: TrustLevel) -> Result<()> {
        self.trust_db.set_trust_level(git_dir, level);
        self.trust_db.save()
    }

    /// Untrust a repository.
    pub fn untrust_repository(&mut self, git_dir: &Path) -> Result<()> {
        self.trust_db.remove_trust(git_dir);
        self.trust_db.save()
    }
}

/// Wait for a child process with a timeout.
fn wait_with_timeout(
    child: &mut std::process::Child,
    timeout: Duration,
) -> Result<std::process::ExitStatus> {
    use std::thread;
    use std::time::Instant;

    let start = Instant::now();
    let poll_interval = Duration::from_millis(100);

    loop {
        match child.try_wait()? {
            Some(status) => return Ok(status),
            None => {
                if start.elapsed() >= timeout {
                    // Kill the process
                    child.kill().ok();
                    anyhow::bail!("Hook execution timed out after {:?}", timeout);
                }
                thread::sleep(poll_interval);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::hooks::PROJECT_HOOKS_DIR;
    use crate::output::TestOutput;
    use std::fs;
    use std::os::unix::fs::PermissionsExt;
    use tempfile::tempdir;

    fn create_test_hook(dir: &Path, hook_name: &str, content: &str) -> PathBuf {
        let hooks_dir = dir.join(PROJECT_HOOKS_DIR);
        fs::create_dir_all(&hooks_dir).unwrap();
        let hook_path = hooks_dir.join(hook_name);
        fs::write(&hook_path, content).unwrap();

        // Make executable
        #[cfg(unix)]
        {
            let mut perms = fs::metadata(&hook_path).unwrap().permissions();
            perms.set_mode(0o755);
            fs::set_permissions(&hook_path, perms).unwrap();
        }

        hook_path
    }

    #[test]
    fn test_hook_result_success() {
        let result = HookResult::success();
        assert!(result.success);
        assert!(!result.skipped);
        assert_eq!(result.exit_code, Some(0));
    }

    #[test]
    fn test_hook_result_skipped() {
        let result = HookResult::skipped("test reason");
        assert!(result.success);
        assert!(result.skipped);
        assert_eq!(result.skip_reason, Some("test reason".to_string()));
    }

    #[test]
    fn test_hook_result_failed() {
        let result = HookResult::failed(1, "out".to_string(), "err".to_string());
        assert!(!result.success);
        assert!(!result.skipped);
        assert_eq!(result.exit_code, Some(1));
        assert_eq!(result.stdout, "out");
        assert_eq!(result.stderr, "err");
    }

    #[test]
    fn test_executor_hooks_disabled() {
        let temp_dir = tempdir().unwrap();
        let worktree = temp_dir.path().join("main");
        fs::create_dir_all(&worktree).unwrap();

        create_test_hook(&worktree, "worktree-post-create", "#!/bin/bash\necho test");

        let config = HooksConfig {
            enabled: false,
            ..Default::default()
        };

        let executor = HookExecutor::with_trust_db(config, TrustDatabase::default());
        let mut output = TestOutput::default();

        let ctx = HookContext::new(
            HookType::PostCreate,
            "checkout",
            temp_dir.path(),
            temp_dir.path().join(".git"),
            "origin",
            &worktree,
            &worktree,
            "main",
        );

        let result = executor.execute(&ctx, &mut output).unwrap();
        assert!(result.skipped);
        assert_eq!(
            result.skip_reason,
            Some("Hooks are globally disabled".to_string())
        );
    }

    #[test]
    fn test_executor_no_hooks() {
        let temp_dir = tempdir().unwrap();
        let worktree = temp_dir.path().join("main");
        fs::create_dir_all(&worktree).unwrap();

        let config = HooksConfig::default();
        let executor = HookExecutor::with_trust_db(config, TrustDatabase::default());
        let mut output = TestOutput::default();

        let ctx = HookContext::new(
            HookType::PostCreate,
            "checkout",
            temp_dir.path(),
            temp_dir.path().join(".git"),
            "origin",
            &worktree,
            &worktree,
            "main",
        );

        let result = executor.execute(&ctx, &mut output).unwrap();
        assert!(result.skipped);
        assert_eq!(result.skip_reason, Some("No hook files found".to_string()));
    }

    #[test]
    fn test_executor_untrusted_repo() {
        let temp_dir = tempdir().unwrap();
        let worktree = temp_dir.path().join("main");
        fs::create_dir_all(&worktree).unwrap();

        create_test_hook(&worktree, "worktree-post-create", "#!/bin/bash\necho test");

        let config = HooksConfig::default();
        let executor = HookExecutor::with_trust_db(config, TrustDatabase::default());
        let mut output = TestOutput::default();

        let ctx = HookContext::new(
            HookType::PostCreate,
            "checkout",
            temp_dir.path(),
            temp_dir.path().join(".git"),
            "origin",
            &worktree,
            &worktree,
            "main",
        );

        let result = executor.execute(&ctx, &mut output).unwrap();
        assert!(result.skipped);
        assert_eq!(
            result.skip_reason,
            Some("Repository not trusted".to_string())
        );
    }

    #[test]
    fn test_executor_trusted_repo() {
        let temp_dir = tempdir().unwrap();
        let worktree = temp_dir.path().join("main");
        fs::create_dir_all(&worktree).unwrap();

        create_test_hook(
            &worktree,
            "worktree-post-create",
            "#!/bin/bash\necho 'hook executed'",
        );

        let config = HooksConfig::default();
        let mut trust_db = TrustDatabase::default();
        trust_db.set_trust_level(&temp_dir.path().join(".git"), TrustLevel::Allow);

        let executor = HookExecutor::with_trust_db(config, trust_db);
        let mut output = TestOutput::default();

        let ctx = HookContext::new(
            HookType::PostCreate,
            "checkout",
            temp_dir.path(),
            temp_dir.path().join(".git"),
            "origin",
            &worktree,
            &worktree,
            "main",
        );

        let result = executor.execute(&ctx, &mut output).unwrap();
        assert!(result.success);
        assert!(!result.skipped);
    }
}
