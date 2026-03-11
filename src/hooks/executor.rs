//! Hook execution logic.
//!
//! This module provides the `HookExecutor` which handles discovering,
//! validating, and executing hooks with proper security checks.

use super::yaml_config_loader;
use super::yaml_executor::{self, JobFilter};
use super::{
    find_hooks, list_hooks, FailMode, HookConfig, HookContext, HookEnvironment, HookType,
    HooksConfig, TrustDatabase, TrustLevel, DEPRECATED_HOOK_REMOVAL_VERSION,
};
use crate::executor::presenter::JobPresenter;
use crate::output::Output;
use anyhow::Result;
use std::path::{Path, PathBuf};
use std::sync::Arc;

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
    /// Whether the skip evaluation involved running a command check.
    pub skip_ran_command: bool,
    /// Whether the skip was due to a platform mismatch (OS-keyed run with no matching variant).
    /// Platform skips are completely silent — no output, not even a skip message.
    pub platform_skip: bool,
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
            skip_ran_command: false,
            platform_skip: false,
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
            skip_ran_command: false,
            platform_skip: false,
        }
    }

    /// Create a skipped result where the skip check ran a command.
    pub fn skipped_after_command(reason: impl Into<String>) -> Self {
        Self {
            success: true,
            exit_code: None,
            stdout: String::new(),
            stderr: String::new(),
            skipped: true,
            skip_reason: Some(reason.into()),
            skip_ran_command: true,
            platform_skip: false,
        }
    }

    /// Create a result for a platform skip (OS-keyed run with no matching variant).
    ///
    /// Platform skips are completely silent — no output, not even a skip message.
    /// They still count as "satisfied" for dependency purposes.
    pub fn platform_skipped() -> Self {
        Self {
            success: true,
            exit_code: None,
            stdout: String::new(),
            stderr: String::new(),
            skipped: true,
            skip_reason: Some("platform skip".to_string()),
            skip_ran_command: false,
            platform_skip: true,
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
            skip_ran_command: false,
            platform_skip: false,
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
    bypass_trust: bool,
    job_filter: JobFilter,
}

impl HookExecutor {
    /// Create a new hook executor with the given configuration.
    pub fn new(config: HooksConfig) -> Result<Self> {
        let trust_db = TrustDatabase::load().unwrap_or_default();
        Ok(Self {
            config,
            trust_db,
            prompt_callback: None,
            bypass_trust: false,
            job_filter: JobFilter::default(),
        })
    }

    /// Create a new hook executor with a custom trust database.
    pub fn with_trust_db(config: HooksConfig, trust_db: TrustDatabase) -> Self {
        Self {
            config,
            trust_db,
            prompt_callback: None,
            bypass_trust: false,
            job_filter: JobFilter::default(),
        }
    }

    /// Set a callback for prompting the user.
    pub fn with_prompt_callback(mut self, callback: PromptCallback) -> Self {
        self.prompt_callback = Some(callback);
        self
    }

    /// Bypass trust checks during execution.
    ///
    /// Used by `hooks run` where the user is explicitly invoking a hook.
    pub fn with_bypass_trust(mut self, bypass: bool) -> Self {
        self.bypass_trust = bypass;
        self
    }

    /// Set a job filter to restrict which jobs are executed.
    pub fn with_job_filter(mut self, filter: JobFilter) -> Self {
        self.job_filter = filter;
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
    pub fn execute(
        &self,
        ctx: &HookContext,
        output: &mut dyn Output,
        presenter: Arc<dyn JobPresenter>,
    ) -> Result<HookResult> {
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
        match self.try_yaml_hook(ctx, &hook_source_worktree, hook_config, output, &presenter) {
            Ok(Some(result)) => return Ok(result),
            Ok(None) => {} // No YAML config or no definition for this hook — fall through to legacy
            Err(e) => {
                output.warning(&format!(
                    "Error loading YAML config, falling back to script hooks: {e}"
                ));
            }
        }

        // Fallback: legacy script execution
        self.execute_legacy(ctx, hook_config, &hook_source_worktree, output, presenter)
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
        _presenter: &Arc<dyn JobPresenter>,
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

        // Check trust level (unless bypassed by explicit invocation)
        if !self.bypass_trust {
            let trust_level = self.get_verified_trust_level(&ctx.git_dir, output);
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
            &self.config.output,
            &self.job_filter,
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
        presenter: Arc<dyn JobPresenter>,
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

        // Check trust level (unless bypassed by explicit invocation)
        if !self.bypass_trust {
            let trust_level = self.get_verified_trust_level(&ctx.git_dir, output);

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
        }

        // Clear any active spinner — the presenter writes directly to stderr.
        output.finish_spinner();

        let env = HookEnvironment::from_context(ctx);
        let working_dir = env.working_directory(ctx);

        // Convert legacy hook paths to generic JobSpecs
        let specs =
            crate::hooks::job_adapter::scripts_to_specs(&discovery.hooks, &env, working_dir);

        // Use presenter for header and execution
        let hook_type_name = ctx.hook_type.yaml_name();
        presenter.on_phase_start(hook_type_name);
        let hook_start = std::time::Instant::now();

        // Execute via the generic runner (Piped mode = stop on first failure)
        let results = crate::executor::runner::run_jobs(
            &specs,
            crate::executor::ExecutionMode::Piped,
            &presenter,
        )?;

        presenter.on_phase_complete(hook_start.elapsed());

        // Check results for failure
        let any_failed = results
            .iter()
            .any(|r| r.status == crate::executor::NodeStatus::Failed);
        if any_failed {
            let failed = results
                .iter()
                .find(|r| r.status == crate::executor::NodeStatus::Failed)
                .unwrap();
            let hook_result = HookResult::failed(
                failed.exit_code.unwrap_or(-1),
                failed.stdout.clone(),
                failed.stderr.clone(),
            );
            return self.handle_hook_failure(ctx.hook_type, hook_config, hook_result, output);
        }

        Ok(HookResult::success())
    }

    /// Get the worktree path to read hooks from based on hook type.
    fn get_hook_source_worktree(&self, ctx: &HookContext) -> PathBuf {
        match ctx.hook_type {
            // Pre-create: target doesn't exist yet, use source
            HookType::PreCreate => ctx.source_worktree.clone(),
            // Post-create/clone: target now exists, use it
            HookType::PostCreate | HookType::PostClone => ctx.worktree_path.clone(),
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

    /// Trust a repository with a fingerprint (remote URL).
    pub fn trust_repository_with_fingerprint(
        &mut self,
        git_dir: &Path,
        level: TrustLevel,
        fingerprint: String,
    ) -> Result<()> {
        self.trust_db
            .set_trust_level_with_fingerprint(git_dir, level, fingerprint);
        self.trust_db.save()
    }

    /// Untrust a repository.
    pub fn untrust_repository(&mut self, git_dir: &Path) -> Result<()> {
        self.trust_db.remove_trust(git_dir);
        self.trust_db.save()
    }

    /// Get the effective trust level, considering fingerprint verification.
    ///
    /// If a trust entry has a stored fingerprint (remote URL), the current
    /// remote URL is checked against it. On mismatch, the level is downgraded
    /// to `Prompt` and a warning is emitted.
    ///
    /// Entries without a fingerprint (created before this feature) are treated
    /// as valid without verification.
    fn get_verified_trust_level(&self, git_dir: &Path, output: &mut dyn Output) -> TrustLevel {
        let entry = match self.trust_db.get_trust_entry(git_dir) {
            Some(entry) => entry,
            None => {
                // No explicit entry — fall through to pattern matching / default
                return self.trust_db.get_trust_level(git_dir);
            }
        };

        // If no fingerprint stored, this is a legacy entry — trust it as-is
        let stored_fingerprint = match &entry.fingerprint {
            Some(fp) => fp,
            None => return entry.level,
        };

        // Get the current remote URL from the repo
        let current_url = super::get_remote_url_for_git_dir(git_dir);

        match current_url {
            Some(ref url) if url == stored_fingerprint => {
                // Fingerprint matches — trust level is valid
                entry.level
            }
            Some(ref url) => {
                // Fingerprint mismatch — different repo at same path
                output.warning(&format!(
                    "Trust fingerprint mismatch for {}",
                    git_dir.display()
                ));
                output.warning(&format!("  Trusted remote: {stored_fingerprint}"));
                output.warning(&format!("  Current remote: {url}"));
                output.warning(
                    "A different repository may now be at this path. \
                     Run 'git daft hooks trust' to re-trust.",
                );
                TrustLevel::Prompt
            }
            None => {
                // Can't determine remote URL — don't penalize
                entry.level
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::executor::presenter::NullPresenter;
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

        let presenter = NullPresenter::arc();
        let result = executor.execute(&ctx, &mut output, presenter).unwrap();
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

        let presenter = NullPresenter::arc();
        let result = executor.execute(&ctx, &mut output, presenter).unwrap();
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

        let presenter = NullPresenter::arc();
        let result = executor.execute(&ctx, &mut output, presenter).unwrap();
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

        let presenter = NullPresenter::arc();
        let result = executor.execute(&ctx, &mut output, presenter).unwrap();
        assert!(result.success);
        assert!(!result.skipped);
    }
}
