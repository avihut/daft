//! Hook execution logic.
//!
//! This module provides the `HookExecutor` which handles discovering,
//! validating, and executing hooks with proper security checks.

use super::trust_skip::{self, SkipSource};
use super::yaml_config_loader;
use super::yaml_executor::{self, JobFilter};
use super::{
    DEPRECATED_HOOK_REMOVAL_VERSION, FailMode, HookConfig, HookContext, HookEnvironment, HookType,
    HooksConfig, TrustDatabase, TrustLevel, find_hooks,
};
use crate::executor::presenter::JobPresenter;
use crate::output::Output;
use crate::store::models::invocation::SKIP_REASON_PROMPT_UNAVAILABLE;
use anyhow::Result;
use std::path::{Path, PathBuf};
use std::sync::Arc;

/// Resolve a hook's effective fail mode from the two sources that can set it.
///
/// Precedence is **git wins**: a git-config `daft.hooks.<hook>.failMode`
/// (recorded in `hook_config.fail_mode_from_git`) overrides a committed
/// `daft.yml fail_mode:`, which in turn overrides the hook-type default. When
/// git did not set the value, `hook_config.fail_mode` still holds that default,
/// so the committed YAML value is used if present and the default otherwise.
fn resolve_fail_mode(hook_config: &HookConfig, yaml: Option<FailMode>) -> FailMode {
    if hook_config.fail_mode_from_git {
        hook_config.fail_mode
    } else {
        yaml.unwrap_or(hook_config.fail_mode)
    }
}

/// Build a warning when an *unparseable* git-config `failMode` is silently
/// overridden by a committed `daft.yml fail_mode:`.
///
/// Under "git wins", a present git value is expected to override the committed
/// one — but a typo (`abrot`) does not parse, so it is ignored and the YAML
/// value wins instead. Left silent, someone who ran `git config … failMode
/// abrot` to restore local gating would believe they had, while a failing
/// gating hook is quietly downgraded to the committed `warn`. Scoped to the
/// exact harm: only fires when a YAML value is actually present to be
/// overridden. Returns `None` (no warning) otherwise.
fn unparsed_git_fail_mode_warning(
    hook_config: &HookConfig,
    yaml: Option<FailMode>,
) -> Option<String> {
    match (&hook_config.fail_mode_git_unparsed, yaml) {
        (Some(bad), Some(yaml_mode)) => Some(format!(
            "Ignoring invalid git config failMode {bad:?} (expected \"abort\" or \"warn\"); \
             using the committed daft.yml fail_mode: {yaml_mode}"
        )),
        _ => None,
    }
}

/// A hook that failed under `FailMode::Abort`, carrying the recorded
/// invocation id out with the failure.
///
/// The id is minted before the jobs run, so it exists on the failing path —
/// but the abort used to `bail!` a plain string and drop it. A failing gate
/// is exactly when a caller most needs to address the recorded jobs
/// (`daft merge --format json` reports it as the verdict's join key into
/// `daft hooks jobs`), so the abort carries it instead. `Display` reproduces
/// the previous message verbatim; nothing user-visible changed.
#[derive(Debug)]
pub struct HookAborted {
    pub hook_type: HookType,
    pub exit_code: i32,
    pub invocation_id: Option<String>,
}

impl HookAborted {
    /// Recover the abort from an error chain, if it is one.
    pub fn from_error(err: &anyhow::Error) -> Option<&HookAborted> {
        err.chain().find_map(|e| e.downcast_ref::<HookAborted>())
    }
}

impl std::fmt::Display for HookAborted {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "{} hook failed with exit code {}",
            self.hook_type, self.exit_code
        )
    }
}

impl std::error::Error for HookAborted {}

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
    /// The log-store invocation id this fire was recorded under, when one
    /// was minted (yaml hooks past the hook-level skip gate). Lets failure
    /// paths print an inspect breadcrumb pointing at the exact invocation.
    pub invocation_id: Option<String>,
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
            invocation_id: None,
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
            invocation_id: None,
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
            invocation_id: None,
            platform_skip: false,
        }
    }

    /// Create a failed result for an execution-preparation error — the config
    /// parsed and named this hook, but the fire could not be set up (invalid
    /// glob pattern, unresolvable `root:` template, failing `files:` command,
    /// broken changed-file source, …).
    ///
    /// Distinct from a config *load* error on purpose: a load error falls
    /// back to legacy script hooks, while a preparation error is a hook
    /// failure routed through the hook's fail mode — a configured gate must
    /// never silently degrade to "no hooks ran".
    pub fn config_error(message: impl Into<String>) -> Self {
        Self {
            success: false,
            exit_code: None,
            stdout: String::new(),
            stderr: message.into(),
            skipped: false,
            skip_reason: None,
            skip_ran_command: false,
            invocation_id: None,
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
            invocation_id: None,
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
            invocation_id: None,
            platform_skip: false,
        }
    }

    /// Stamp the log-store invocation id this result was recorded under.
    pub fn with_invocation(mut self, invocation_id: &str) -> Self {
        self.invocation_id = Some(invocation_id.to_string());
        self
    }
}

/// Callback for prompting the user for permission.
pub type PromptCallback = Box<dyn Fn(&str) -> bool>;

/// Get the worktree path to read hooks from based on hook type.
///
/// For moves, `PostRemove` reads hooks from the new worktree (which already
/// exists at this point) rather than the source worktree (which is gone).
pub(crate) fn get_hook_source_worktree(ctx: &HookContext) -> PathBuf {
    match ctx.hook_type {
        // Pre-create: target doesn't exist yet, use source
        HookType::PreCreate => ctx.source_worktree.clone(),
        // Post-create/clone: target now exists, use it
        HookType::PostCreate | HookType::PostClone => ctx.worktree_path.clone(),
        // Pre-remove: target still exists, use it
        HookType::PreRemove => ctx.worktree_path.clone(),
        // Post-remove: target is gone, use source (current worktree).
        // Exception: during a move, the new worktree already exists at
        // worktree_path, so use that instead.
        HookType::PostRemove => {
            if ctx.is_move {
                ctx.worktree_path.clone()
            } else {
                ctx.source_worktree.clone()
            }
        }
        // Merge hooks read from the target worktree — that's where the
        // merge is (or was) taking place, and also where `daft.yml` is
        // most naturally located (the branch being merged into).
        HookType::PreMerge | HookType::PostMerge => ctx.worktree_path.clone(),
    }
}

/// Pick the display target shown alongside the hook name in the rich
/// hook-box title (e.g. `worktree-pre-remove  on: feature`).
///
/// Worktree-scoped phases get the worktree label they're acting on — the
/// branch, or the dirname for branchless sandbox contexts — so multi-source
/// flows make it obvious which worktree the hooks are touching. Project-
/// scoped phases (`pre-merge` / `post-merge` / `post-clone`) return `None`
/// because the title isn't tied to a single worktree.
pub(crate) fn header_target_for_ctx(ctx: &HookContext) -> Option<&str> {
    match ctx.hook_type {
        HookType::PreCreate | HookType::PostCreate | HookType::PreRemove | HookType::PostRemove => {
            Some(ctx.worktree_label())
        }
        HookType::PreMerge | HookType::PostMerge | HookType::PostClone => None,
    }
}

/// Hook executor that manages hook discovery and execution.
pub struct HookExecutor {
    config: HooksConfig,
    trust_db: TrustDatabase,
    prompt_callback: Option<PromptCallback>,
    bypass_trust: bool,
    job_filter: JobFilter,
    hook_mode: crate::hooks::HookMode,
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
            hook_mode: crate::hooks::HookMode::Auto,
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
            hook_mode: crate::hooks::HookMode::Auto,
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

    /// Apply a `--hooks <mode>` + `--skip-hooks <selectors>` pair.
    ///
    /// The two flags are orthogonal — mode decides *how* the phase runs,
    /// selectors decide *which* jobs run — so every command carrying both
    /// configures the executor through this one call rather than getting the
    /// combination subtly different per site.
    pub fn with_hook_mode(self, mode: crate::hooks::HookMode, skip_hooks: &[String]) -> Self {
        self.with_job_filter(mode.job_filter(skip_hooks))
            .with_hook_execution_mode(mode)
    }

    /// Run `background: true` jobs inline instead of dispatching them to the
    /// coordinator (`--hooks foreground`).
    ///
    /// This is the same promotion `DAFT_NO_BACKGROUND_JOBS` triggers, so it
    /// inherits that path's semantics: a promoted job's failure folds into
    /// the hook outcome, and the default job timeout keeps applying — it
    /// already does on the detached path, so a job that would be killed at
    /// 300s must not start succeeding just because someone is watching.
    pub fn with_hook_execution_mode(mut self, mode: crate::hooks::HookMode) -> Self {
        self.hook_mode = mode;
        self
    }

    /// Replace the job filter between fires.
    ///
    /// `daft merge` needs this: `--only-tag` narrows *the gate*, so it must
    /// apply to `pre-merge` and not to `post-merge`, whose jobs the user
    /// never tagged.
    pub fn set_job_filter(&mut self, filter: JobFilter) {
        self.job_filter = filter;
    }

    /// Replace the hook execution mode between fires.
    ///
    /// `daft merge` needs this too, for a hazard no hook *type* can express:
    /// whether this particular invocation is about to delete the directory
    /// the hook ran in. An ephemeral merge target and `--remove-branch` both
    /// do, so `post-merge` is pinned back to `Auto` for those fires.
    pub fn set_hook_execution_mode(&mut self, mode: crate::hooks::HookMode) {
        self.hook_mode = mode;
    }

    /// Plan-time mirror of [`Self::execute`]'s discovery: whether this hook
    /// phase has anything discoverable to run from `hook_source_worktree` —
    /// a YAML definition, legacy scripts, or deprecated files pending
    /// migration. Deliberately *before* the trust and skip-flag gates:
    /// those decide how a planned row resolves (yellow `↓`, vanish), not
    /// whether work exists. Keep in lockstep with `execute`.
    ///
    /// Only meaningful for hook types whose config source is a live
    /// worktree (`get_hook_source_worktree`); `PreCreate` reads its YAML
    /// from branch content instead and must not be probed this way.
    pub fn hook_phase_has_work(&self, hook_type: HookType, hook_source_worktree: &Path) -> bool {
        if !self.config.enabled {
            return false;
        }
        // A per-hook `enabled: false` skips at run time (`execute`'s second
        // gate) — mirror it here so no row is planned for it either.
        if !self.config.get_hook_config(hook_type).enabled {
            return false;
        }
        // A `Err` from the loader mirrors `execute`: fall through to the
        // legacy discovery (the runtime path warns and does the same).
        if let Ok(Some(yaml)) = yaml_config_loader::load_merged_config(hook_source_worktree)
            && yaml.hooks.contains_key(hook_type.yaml_name())
        {
            return true;
        }
        let discovery = find_hooks(hook_type, hook_source_worktree, &self.config);
        // Deprecated-only discoveries count as work: their runtime skip
        // ("run `daft hooks migrate`") renders visibly and needs its row.
        !discovery.hooks.is_empty() || !discovery.deprecation_warnings.is_empty()
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

        // A hook job whose output turns out to be a hook manager's (a job
        // running lefthook) gains nested child sub-structure (#753). One
        // wrap here covers every lifecycle path — CLI, TUI, and replay —
        // and the raw stream still reaches the parent untouched.
        let presenter = crate::executor::manager_routing::LifecycleRoutingPresenter::wrap_when(
            self.config.output.parse_managers,
            presenter,
        );

        // Check if this specific hook is enabled
        let hook_config = self.config.get_hook_config(ctx.hook_type);
        if !hook_config.enabled {
            return Ok(HookResult::skipped(format!(
                "{} hook is disabled",
                ctx.hook_type
            )));
        }

        // Determine the worktree to read hooks from
        let hook_source_worktree = get_hook_source_worktree(ctx);

        // Try YAML config first. `try_yaml_hook` returns:
        // * `Ok(Some((result, fail_mode)))` when the YAML hook was run
        //   (including failed runs — the caller translates those to Err-or-warn
        //   based on the resolved fail mode below).
        // * `Ok(None)` when no YAML config applies to this hook type.
        // * `Err(_)` only when YAML loading/parsing itself failed — an
        //   infrastructure error that we treat as "fall back to legacy"
        //   rather than a hook-semantic failure.
        match self.try_yaml_hook(ctx, &hook_source_worktree, hook_config, output, &presenter) {
            Ok(Some((result, fail_mode))) => {
                // The YAML hook was invoked. If the hook itself failed
                // (exit != 0) and was not skipped, translate per its
                // resolved fail mode — Abort bails, Warn logs and
                // returns a success-ish HookResult so the caller can
                // continue. Skipped or successful results pass through
                // unchanged.
                if !result.success && !result.skipped {
                    return self.handle_hook_failure(ctx.hook_type, fail_mode, result, output);
                }
                return Ok(result);
            }
            Ok(None) => {} // No YAML config or no definition for this hook — fall through to legacy
            Err(e) => {
                output.warning(&format!(
                    "Error loading YAML config, falling back to script hooks: {e}"
                ));
            }
        }

        // `--hooks off` / `--skip-hooks all` must skip script hooks too. The
        // selector plumbing lives in the YAML executor, which legacy never
        // reaches, so a whole-fire opt-out has to be honored here or the
        // flag's own help text ("off skips the phase") is false for every
        // repo still on `.daft/hooks/*` scripts.
        //
        // Gated here rather than at the top of `execute` on purpose: the YAML
        // path renders attributed per-job skips for the same selectors, and
        // short-circuiting earlier would throw that away.
        //
        // Only the whole-fire selectors apply. Partial ones (job names, tags)
        // cannot: a script hook is one opaque file with no job names to match.
        // `foreground` needs nothing — `scripts_to_specs` never sets
        // `background`, so legacy already runs everything inline — and
        // `background` has no coordinator path to detach into, which
        // `docs/hooks/yaml-reference.md` states.
        if self.user_requested_skip(ctx.hook_type) {
            return Ok(HookResult::skipped(format!(
                "{} script hooks skipped by request",
                ctx.hook_type
            )));
        }

        // Fallback: legacy script execution
        self.execute_legacy(ctx, hook_config, &hook_source_worktree, output, presenter)
    }

    /// Try to execute a hook via YAML configuration.
    ///
    /// Returns `Ok(Some((result, fail_mode)))` if YAML config exists and defines this
    /// hook — including failed runs. Failure translation (Abort-vs-Warn)
    /// is the caller's responsibility via `handle_hook_failure`.
    /// Returns `Ok(None)` if no YAML config or no definition for this
    /// hook type. `Err` signals a YAML load/parse error, not a hook
    /// invocation failure.
    fn try_yaml_hook(
        &self,
        ctx: &HookContext,
        hook_source_worktree: &Path,
        hook_config: &HookConfig,
        output: &mut dyn Output,
        presenter: &Arc<dyn JobPresenter>,
    ) -> Result<Option<(HookResult, FailMode)>> {
        let yaml_config = if ctx.hook_type == HookType::PreCreate {
            // For PreCreate, the target worktree doesn't exist yet.
            // Load config from the target branch via git show, falling back
            // to the base branch and then the default branch.
            match yaml_config_loader::load_config_from_branch(
                &ctx.git_dir,
                &ctx.branch_name,
                ctx.base_branch.as_deref(),
            )? {
                Some(config) => config,
                None => {
                    return Ok(None);
                }
            }
        } else {
            match yaml_config_loader::load_merged_config(hook_source_worktree)? {
                Some(config) => config,
                None => {
                    return Ok(None);
                }
            }
        };

        let hook_name = ctx.hook_type.yaml_name();

        let hook_def = match yaml_config.hooks.get(hook_name) {
            Some(def) => def,
            None => {
                return Ok(None);
            }
        };

        // Resolve the effective fail mode now, while both the git-derived
        // `hook_config` and the parsed `hook_def` are in scope. Threaded through
        // every `Ok(Some(...))` exit below so the caller can translate a failure
        // per the resolved mode.
        let effective_fail_mode = resolve_fail_mode(hook_config, hook_def.fail_mode);

        // Surface an unparseable git `failMode` that is being silently
        // overridden by the committed value, so the misconfiguration is visible
        // rather than a quiet gating downgrade.
        if let Some(msg) = unparsed_git_fail_mode_warning(hook_config, hook_def.fail_mode) {
            output.warning(&msg);
        }

        // Check trust level (unless bypassed by explicit invocation)
        if !self.bypass_trust {
            let trust_level = self.get_verified_trust_level(&ctx.git_dir, output);
            match trust_level {
                TrustLevel::Deny => {
                    if !self.user_requested_skip(ctx.hook_type) {
                        let configured_hooks: Vec<String> = yaml_config
                            .hooks
                            .keys()
                            .filter(|name| HookType::from_yaml_name(name).is_some())
                            .cloned()
                            .collect();
                        trust_skip::notify_and_record(
                            ctx,
                            SkipSource::Yaml { configured_hooks },
                            output,
                        );
                    }
                    output.debug(&format!(
                        "Skipping {hook_name} YAML hooks: repository not trusted"
                    ));
                    return Ok(Some((
                        HookResult::skipped("Repository not trusted"),
                        effective_fail_mode,
                    )));
                }
                TrustLevel::Prompt => {
                    let prompt_msg =
                        format!("Repository has YAML hook config for '{hook_name}'. Execute?");
                    if let Some(ref callback) = self.prompt_callback {
                        if !callback(&prompt_msg) {
                            return Ok(Some((
                                HookResult::skipped("User declined hook execution"),
                                effective_fail_mode,
                            )));
                        }
                    } else {
                        output.warning(&format!(
                            "Repository trust is set to 'prompt' but no interactive prompt is available — skipping {hook_name}. Run '{}' to allow hooks.",
                            crate::daft_cmd("hooks trust")
                        ));
                        trust_skip::record_skip(ctx, SKIP_REASON_PROMPT_UNAVAILABLE);
                        return Ok(Some((
                            HookResult::skipped("No permission callback"),
                            effective_fail_mode,
                        )));
                    }
                }
                TrustLevel::Allow => {}
            }
        }

        // The trust gate passed (Allow, prompt accepted, or explicit bypass):
        // any "skipped while untrusted" record for this (hook, branch) pair
        // is now stale — the upcoming fire supersedes it regardless of how
        // that fire ends (failure and `skip:` conditions are post-trust
        // outcomes, captured by job records instead).
        trust_skip::clear_skips(ctx);

        let source_dir = yaml_config.source_dir.as_deref().unwrap_or(".daft");
        let rc = yaml_config.rc.as_deref();

        // Derived per-worktree env values (#388): ride the context's
        // extra_env so DAFT_* injection, job process env, and template
        // lookups all inherit them. Extends (never replaces) — DAFT_MERGE_*
        // entries a merge command threaded in survive. Legacy script hooks
        // (execute_legacy) deliberately get no injection: declaring env:
        // implies a daft.yml, whose hooks run through this path.
        let ctx_with_derived;
        let ctx = {
            let (derived, warnings) = super::environment::derived_injection(&yaml_config, ctx);
            for warning in &warnings {
                output.warning(warning);
            }
            if derived.is_empty() {
                ctx
            } else {
                ctx_with_derived = ctx.clone().with_derived_env(derived);
                &ctx_with_derived
            }
        };

        let env = HookEnvironment::from_context(ctx);
        let working_dir = env.working_directory(ctx);

        let cfg = yaml_executor::HookExecutionContext {
            source_dir,
            working_dir,
            rc,
            filter: &self.job_filter,
            presenter,
            repo_log: yaml_config.log.as_ref(),
            // Lifecycle hooks keep the 300s job timeout and are never
            // cancel-flag-driven; the trigger label follows the hook default.
            default_job_timeout: Some(crate::executor::JobSpec::DEFAULT_TIMEOUT),
            cancel: None,
            trigger_label: None,
            hook_mode: self.hook_mode,
        };
        // An Err from the yaml executor here is an execution-preparation
        // failure (invalid glob, unresolvable root: template, failing files:
        // command, …), NOT a config-load error: the config parsed and named
        // this hook. Propagating it as Err would hit the outer dispatch's
        // load-error arm and silently fall back to legacy scripts — skipping
        // a configured gate. Fold it into a failed HookResult instead so it
        // flows through the same fail-mode translation as a failing job.
        let result = match yaml_executor::execute_yaml_hook_with_rc(
            hook_name, hook_def, ctx, output, &cfg,
        ) {
            Ok(result) => result,
            Err(e) => HookResult::config_error(format!("{e:#}")),
        };

        // Return the raw result plus the resolved fail mode — failure
        // translation (Abort → Err, Warn → logged-and-continue) is the caller's
        // responsibility via `handle_hook_failure` in `execute`. Doing it here
        // would misclassify Abort-mode hook failures as "YAML config load error"
        // at the outer dispatch and silently fall back to legacy scripts.
        Ok(Some((result, effective_fail_mode)))
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
                    "Both '{}' and '{}' exist in '{}'. Using '{}'; remove '{}' or run '{}'.",
                    warning.new_name,
                    warning.old_name,
                    warning
                        .path
                        .parent()
                        .unwrap_or(warning.path.as_path())
                        .display(),
                    warning.new_name,
                    warning.old_name,
                    crate::daft_cmd("hooks migrate"),
                ));
            } else {
                output.warning(&format!(
                    "Hook '{}' uses deprecated name '{}'. Rename to '{}' or run '{}'. \
                     Deprecated names will stop working in daft v{}.",
                    warning.path.display(),
                    warning.old_name,
                    warning.new_name,
                    crate::daft_cmd("hooks migrate"),
                    DEPRECATED_HOOK_REMOVAL_VERSION
                ));
            }
        }

        if discovery.hooks.is_empty() {
            if !discovery.deprecation_warnings.is_empty() {
                return Ok(HookResult::skipped(format!(
                    "Deprecated hook files found but not executed. Run '{}' to rename them.",
                    crate::daft_cmd("hooks migrate")
                )));
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
                        if !self.user_requested_skip(ctx.hook_type) {
                            let hook_files: Vec<String> = discovery
                                .hooks
                                .iter()
                                .filter(|h| h.starts_with(hook_source_worktree))
                                .filter_map(|p| p.file_name())
                                .filter_map(|n| n.to_str())
                                .map(String::from)
                                .collect();
                            trust_skip::notify_and_record(
                                ctx,
                                SkipSource::Scripts { hook_files },
                                output,
                            );
                        }
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

        // Trust gate passed (or only user-level hooks are involved): drop any
        // stale "skipped while untrusted" record for this (hook, branch).
        trust_skip::clear_skips(ctx);

        // Clear any active spinner — the presenter writes directly to stderr.
        output.finish_spinner();

        let env = HookEnvironment::from_context(ctx);
        let working_dir = env.working_directory(ctx);

        // Convert legacy hook paths to generic JobSpecs
        let specs =
            crate::hooks::job_adapter::scripts_to_specs(&discovery.hooks, &env, working_dir);

        // Use presenter for header and execution
        let hook_type_name = ctx.hook_type.yaml_name();
        let header_target = header_target_for_ctx(ctx);
        presenter.on_phase_start(hook_type_name, header_target);
        let hook_start = std::time::Instant::now();

        // Execute via the generic runner (Piped mode = stop on first failure)
        let results = crate::executor::runner::run_jobs(
            &specs,
            crate::executor::ExecutionMode::Piped,
            &presenter,
            None,
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
            return self.handle_hook_failure(
                ctx.hook_type,
                hook_config.fail_mode,
                hook_result,
                output,
            );
        }

        Ok(HookResult::success())
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
                "Repository trust is set to 'prompt' but no interactive prompt is available — skipping {} hooks. Run '{}' to allow hooks.",
                ctx.hook_type,
                crate::daft_cmd("hooks trust")
            ));
            trust_skip::record_skip(ctx, SKIP_REASON_PROMPT_UNAVAILABLE);
            false
        }
    }

    /// Handle a hook failure based on the resolved fail mode.
    fn handle_hook_failure(
        &self,
        hook_type: HookType,
        fail_mode: FailMode,
        result: HookResult,
        output: &mut dyn Output,
    ) -> Result<HookResult> {
        let exit_code = result.exit_code.unwrap_or(-1);

        // Breadcrumb into the recorded invocation: the listing (newest
        // first, hook-filtered) plus the per-job log drill-down.
        let inspect = result.invocation_id.as_ref().map(|_| {
            format!(
                "inspect: {}",
                crate::daft_cmd(&format!(
                    "hooks jobs --last --hook {}",
                    hook_type.yaml_name()
                ))
            )
        });

        match fail_mode {
            FailMode::Abort => {
                output.error(&format!(
                    "{} hook failed with exit code {}",
                    hook_type, exit_code
                ));
                if !result.stderr.is_empty() {
                    output.error(&format!("Hook stderr: {}", result.stderr.trim()));
                }
                if let Some(inspect) = inspect {
                    output.info(&inspect);
                }
                Err(anyhow::Error::new(HookAborted {
                    hook_type,
                    exit_code,
                    invocation_id: result.invocation_id.clone(),
                }))
            }
            FailMode::Warn => {
                output.warning(&format!(
                    "{} hook failed with exit code {} (continuing anyway)",
                    hook_type, exit_code
                ));
                if !result.stderr.is_empty() {
                    output.warning(&format!("Hook stderr: {}", result.stderr.trim()));
                }
                if let Some(inspect) = inspect {
                    output.info(&inspect);
                }
                Ok(result)
            }
        }
    }

    /// Whether the user explicitly asked to skip this whole hook fire
    /// (`--skip-hooks all` or a hook-type selector naming it). An explicit
    /// opt-out must not trigger the untrusted-hook notice or a replay
    /// record: the hooks were not going to run regardless of trust. Partial
    /// selectors (job names, tags) do NOT suppress — the remaining jobs
    /// would have run if trusted, so the trust skip is still surprising.
    fn user_requested_skip(&self, hook_type: HookType) -> bool {
        self.job_filter.skip.all || self.job_filter.skip.hook_types.contains(&hook_type)
    }

    /// Get the trust level for a repository.
    pub fn get_trust_level(&self, git_dir: &Path) -> TrustLevel {
        self.trust_db.get_trust_level(git_dir)
    }

    /// Trust a repository.
    ///
    /// Persists under the registry lock (`update`), then mirrors the change into
    /// the cached `trust_db` so this executor's subsequent hook-execution reads
    /// see the grant. The cached copy is loaded once in `new()` and must not be
    /// saved wholesale — that is the load-once/save-many lost-update bug (#666).
    pub fn trust_repository(&mut self, git_dir: &Path, level: TrustLevel) -> Result<()> {
        TrustDatabase::update(|db| {
            db.set_trust_level(git_dir, level);
            Ok(())
        })?;
        self.trust_db.set_trust_level(git_dir, level);
        Ok(())
    }

    /// Trust a repository with a fingerprint (remote URL).
    pub fn trust_repository_with_fingerprint(
        &mut self,
        git_dir: &Path,
        level: TrustLevel,
        fingerprint: String,
    ) -> Result<()> {
        TrustDatabase::update(|db| {
            db.set_trust_level_with_fingerprint(git_dir, level, fingerprint.clone());
            Ok(())
        })?;
        self.trust_db
            .set_trust_level_with_fingerprint(git_dir, level, fingerprint);
        Ok(())
    }

    /// Untrust a repository.
    pub fn untrust_repository(&mut self, git_dir: &Path) -> Result<()> {
        TrustDatabase::update(|db| {
            db.remove_trust(git_dir);
            Ok(())
        })?;
        self.trust_db.remove_trust(git_dir);
        Ok(())
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
                output.warning(&format!(
                    "A different repository may now be at this path. \
                     Run '{}' to re-trust.",
                    crate::daft_cmd("hooks trust")
                ));
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
    #[cfg(unix)]
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
    fn resolve_fail_mode_git_beats_yaml_beats_default() {
        // PostCreate defaults to Abort; git has not set the value.
        let mut cfg = HookConfig::new(HookType::PostCreate);
        assert_eq!(cfg.fail_mode, FailMode::Abort);
        assert!(!cfg.fail_mode_from_git);

        // git unset + yaml set → the committed daft.yml value wins over the default.
        assert_eq!(
            resolve_fail_mode(&cfg, Some(FailMode::Warn)),
            FailMode::Warn
        );
        // git unset + yaml unset → the hook-type default.
        assert_eq!(resolve_fail_mode(&cfg, None), FailMode::Abort);

        // git set (even to the type default) → git config wins over daft.yml.
        cfg.fail_mode = FailMode::Abort;
        cfg.fail_mode_from_git = true;
        assert_eq!(
            resolve_fail_mode(&cfg, Some(FailMode::Warn)),
            FailMode::Abort
        );
        assert_eq!(resolve_fail_mode(&cfg, None), FailMode::Abort);
    }

    #[test]
    fn unparsed_git_fail_mode_warns_only_when_overriding_a_committed_value() {
        let mut cfg = HookConfig::new(HookType::PreMerge);

        // No unparseable git value recorded → never warns.
        assert!(unparsed_git_fail_mode_warning(&cfg, Some(FailMode::Warn)).is_none());
        assert!(unparsed_git_fail_mode_warning(&cfg, None).is_none());

        // A present-but-unparseable git value (e.g. a `failMode abrot` typo)...
        cfg.fail_mode_git_unparsed = Some("abrot".to_string());

        // ...with a committed daft.yml value being silently overridden → warn,
        // naming the bad value. This is the precise harm: the user believes
        // their git override re-gated the hook, but the committed value wins.
        let msg = unparsed_git_fail_mode_warning(&cfg, Some(FailMode::Warn))
            .expect("must warn when a committed value silently wins over a bad git value");
        assert!(
            msg.contains("abrot"),
            "warning should name the bad value: {msg}"
        );

        // ...but with no committed value nothing is being overridden (the
        // default applies either way), so stay quiet to avoid noise.
        assert!(unparsed_git_fail_mode_warning(&cfg, None).is_none());
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
    fn hook_phase_has_work_mirrors_discovery() {
        let temp_dir = tempdir().unwrap();
        let worktree = temp_dir.path().join("main");
        fs::create_dir_all(&worktree).unwrap();
        // Pin the user hooks dir inside the tempdir so the developer's real
        // user-level hooks can't leak into the probe.
        let config = HooksConfig {
            user_directory: temp_dir.path().join("user-hooks"),
            ..Default::default()
        };
        let executor = HookExecutor::with_trust_db(config, TrustDatabase::default());

        // Nothing on disk: no work for any phase.
        assert!(!executor.hook_phase_has_work(HookType::PreRemove, &worktree));
        assert!(!executor.hook_phase_has_work(HookType::PostRemove, &worktree));

        // A YAML definition counts, and only for its own hook type.
        fs::write(
            worktree.join("daft.yml"),
            "hooks:\n  worktree-pre-remove:\n    jobs:\n      - name: a\n        run: \"true\"\n",
        )
        .unwrap();
        assert!(executor.hook_phase_has_work(HookType::PreRemove, &worktree));
        assert!(!executor.hook_phase_has_work(HookType::PostRemove, &worktree));

        // A per-hook `enabled: false` mirrors `execute`'s second gate: the
        // phase skips at run time, so it must plan no row either.
        let mut disabled_config = HooksConfig {
            user_directory: temp_dir.path().join("user-hooks"),
            ..Default::default()
        };
        disabled_config.worktree_pre_remove.enabled = false;
        let disabled = HookExecutor::with_trust_db(disabled_config, TrustDatabase::default());
        assert!(!disabled.hook_phase_has_work(HookType::PreRemove, &worktree));

        // Legacy scripts count too (the executor's fallback path).
        create_test_hook(&worktree, "worktree-post-remove", "#!/bin/bash\ntrue");
        assert!(executor.hook_phase_has_work(HookType::PostRemove, &worktree));
    }

    #[test]
    fn hook_phase_has_work_counts_deprecated_files_and_respects_global_disable() {
        let temp_dir = tempdir().unwrap();
        let worktree = temp_dir.path().join("main");
        fs::create_dir_all(&worktree).unwrap();

        // Deprecated-only discovery is work: its runtime skip ("run daft
        // hooks migrate") renders visibly and needs its planned row.
        create_test_hook(&worktree, "pre-remove", "#!/bin/bash\ntrue");
        let user_dir = temp_dir.path().join("user-hooks");
        let config = HooksConfig {
            user_directory: user_dir.clone(),
            ..Default::default()
        };
        let executor = HookExecutor::with_trust_db(config, TrustDatabase::default());
        assert!(executor.hook_phase_has_work(HookType::PreRemove, &worktree));

        // Globally disabled: nothing runs, so nothing is planned.
        let disabled = HooksConfig {
            enabled: false,
            user_directory: user_dir,
            ..Default::default()
        };
        let executor = HookExecutor::with_trust_db(disabled, TrustDatabase::default());
        assert!(!executor.hook_phase_has_work(HookType::PreRemove, &worktree));
    }

    /// Build a context whose git dir exists (so the skip record can compute
    /// a repo id) and whose state writes land in the test's tempdir.
    fn test_ctx_with_state(
        temp_dir: &Path,
        worktree: &Path,
        hook_type: HookType,
        branch: &str,
    ) -> HookContext {
        let git_dir = temp_dir.join(".git");
        fs::create_dir_all(&git_dir).unwrap();
        HookContext::new(
            hook_type, "checkout", temp_dir, &git_dir, "origin", worktree, worktree, branch,
        )
        .with_state_dir(temp_dir.join("state"))
    }

    /// Skip rows recorded for the context's repo, via the same store the
    /// production write path uses.
    fn skip_rows(ctx: &HookContext) -> Vec<crate::store::models::InvocationRow> {
        use crate::coordinator::ports::JobsStorePort;
        let repo_hash =
            crate::core::repo_identity::compute_repo_id_from_common_dir(&ctx.git_dir).unwrap();
        let state = ctx.state_dir.as_ref().unwrap();
        let base = state.join("jobs").join(&repo_hash);
        if !base.join("coordinator.db").exists() {
            return Vec::new();
        }
        let store = crate::coordinator::adapters::SqliteJobsStore::for_repo_base(&base).unwrap();
        store.list_skipped_invocations(&repo_hash).unwrap()
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

        let ctx = test_ctx_with_state(temp_dir.path(), &worktree, HookType::PostCreate, "main");

        let presenter = NullPresenter::arc();
        let result = executor.execute(&ctx, &mut output, presenter).unwrap();
        assert!(result.skipped);
        assert_eq!(
            result.skip_reason,
            Some("Repository not trusted".to_string())
        );

        // The Deny arm emits the notice (once) and records the skip.
        let notices = output.notices();
        assert_eq!(notices.len(), 1);
        assert!(notices[0].contains("worktree-post-create"));
        let rows = skip_rows(&ctx);
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].hook_type, "worktree-post-create");
        assert_eq!(rows[0].worktree, "main");
        assert_eq!(rows[0].skip_reason.as_deref(), Some("untrusted"));
    }

    #[test]
    fn test_executor_untrusted_yaml_config_warns_and_records() {
        let temp_dir = tempdir().unwrap();
        let worktree = temp_dir.path().join("main");
        fs::create_dir_all(&worktree).unwrap();
        fs::write(
            worktree.join("daft.yml"),
            "hooks:\n  worktree-post-create:\n    jobs:\n      - name: setup\n        run: echo hi\n",
        )
        .unwrap();

        let config = HooksConfig::default();
        let executor = HookExecutor::with_trust_db(config, TrustDatabase::default());
        let mut output = TestOutput::default();

        let ctx = test_ctx_with_state(temp_dir.path(), &worktree, HookType::PostCreate, "main");

        let presenter = NullPresenter::arc();
        let result = executor.execute(&ctx, &mut output, presenter).unwrap();
        assert!(result.skipped);
        assert_eq!(
            result.skip_reason,
            Some("Repository not trusted".to_string())
        );

        let notices = output.notices();
        assert_eq!(notices.len(), 1);
        assert!(notices[0].contains("daft.yml"));
        assert!(notices[0].contains("worktree-post-create"));
        let rows = skip_rows(&ctx);
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].skip_reason.as_deref(), Some("untrusted"));
    }

    #[test]
    fn test_executor_user_requested_skip_suppresses_notice_and_record() {
        let temp_dir = tempdir().unwrap();
        let worktree = temp_dir.path().join("main");
        fs::create_dir_all(&worktree).unwrap();

        create_test_hook(&worktree, "worktree-post-create", "#!/bin/bash\necho test");

        for selector in ["all", "worktree-post-create"] {
            let config = HooksConfig::default();
            let executor = HookExecutor::with_trust_db(config, TrustDatabase::default())
                .with_job_filter(JobFilter::skipping(&[selector.to_string()]));
            let mut output = TestOutput::default();

            let ctx = test_ctx_with_state(temp_dir.path(), &worktree, HookType::PostCreate, "main");

            let presenter = NullPresenter::arc();
            let result = executor.execute(&ctx, &mut output, presenter).unwrap();
            assert!(result.skipped, "selector {selector}: still trust-skipped");
            assert!(
                output.notices().is_empty() && output.warnings().is_empty(),
                "selector {selector}: explicit opt-out must not notify"
            );
            assert!(
                skip_rows(&ctx).is_empty(),
                "selector {selector}: explicit opt-out must not record"
            );
        }
    }

    #[test]
    fn test_executor_bypass_trust_neither_warns_nor_records() {
        let temp_dir = tempdir().unwrap();
        let worktree = temp_dir.path().join("main");
        fs::create_dir_all(&worktree).unwrap();

        create_test_hook(&worktree, "worktree-post-create", "#!/bin/bash\necho test");

        let config = HooksConfig::default();
        let executor =
            HookExecutor::with_trust_db(config, TrustDatabase::default()).with_bypass_trust(true);
        let mut output = TestOutput::default();

        let ctx = test_ctx_with_state(temp_dir.path(), &worktree, HookType::PostCreate, "main");

        let presenter = NullPresenter::arc();
        let result = executor.execute(&ctx, &mut output, presenter).unwrap();
        assert!(result.success);
        assert!(output.notices().is_empty() && output.warnings().is_empty());
        assert!(skip_rows(&ctx).is_empty());
    }

    #[test]
    fn test_executor_trust_pass_clears_recorded_skip() {
        let temp_dir = tempdir().unwrap();
        let worktree = temp_dir.path().join("main");
        fs::create_dir_all(&worktree).unwrap();

        create_test_hook(&worktree, "worktree-post-create", "#!/bin/bash\necho test");

        let ctx = test_ctx_with_state(temp_dir.path(), &worktree, HookType::PostCreate, "main");
        let presenter = NullPresenter::arc();

        // First run untrusted: records the skip.
        let untrusted =
            HookExecutor::with_trust_db(HooksConfig::default(), TrustDatabase::default());
        let mut output = TestOutput::default();
        untrusted
            .execute(&ctx, &mut output, presenter.clone())
            .unwrap();
        assert_eq!(skip_rows(&ctx).len(), 1);

        // Then trust and run again: the passing gate clears the record.
        let mut trust_db = TrustDatabase::default();
        trust_db.set_trust_level(&ctx.git_dir, TrustLevel::Allow);
        let trusted = HookExecutor::with_trust_db(HooksConfig::default(), trust_db);
        let mut output = TestOutput::default();
        let result = trusted.execute(&ctx, &mut output, presenter).unwrap();
        assert!(result.success);
        assert!(skip_rows(&ctx).is_empty(), "trust pass clears the record");
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

        // Build the context first: it creates the git dir, which must exist
        // before set_trust_level so both sides canonicalize identically.
        let ctx = test_ctx_with_state(temp_dir.path(), &worktree, HookType::PostCreate, "main");

        let config = HooksConfig::default();
        let mut trust_db = TrustDatabase::default();
        trust_db.set_trust_level(&ctx.git_dir, TrustLevel::Allow);

        let executor = HookExecutor::with_trust_db(config, trust_db);
        let mut output = TestOutput::default();

        let presenter = NullPresenter::arc();
        let result = executor.execute(&ctx, &mut output, presenter).unwrap();
        assert!(result.success);
        assert!(!result.skipped);
    }

    #[test]
    fn post_create_failure_aborts_by_default() {
        let temp_dir = tempdir().unwrap();
        let worktree = temp_dir.path().join("main");
        fs::create_dir_all(&worktree).unwrap();

        create_test_hook(&worktree, "worktree-post-create", "#!/bin/bash\nexit 1");

        let ctx = test_ctx_with_state(temp_dir.path(), &worktree, HookType::PostCreate, "main");

        let mut trust_db = TrustDatabase::default();
        trust_db.set_trust_level(&ctx.git_dir, TrustLevel::Allow);
        let executor = HookExecutor::with_trust_db(HooksConfig::default(), trust_db);
        let mut output = TestOutput::default();

        // #765: a failed post-create aborts (Err) under the default config,
        // so the creation command skips its `-x` tail and exits non-zero.
        let err = executor
            .execute(&ctx, &mut output, NullPresenter::arc())
            .expect_err("post-create failure must abort by default");
        assert!(
            err.to_string().contains("worktree-post-create hook failed"),
            "unexpected abort message: {err}"
        );
    }

    #[test]
    fn post_create_failure_warn_mode_continues() {
        let temp_dir = tempdir().unwrap();
        let worktree = temp_dir.path().join("main");
        fs::create_dir_all(&worktree).unwrap();

        create_test_hook(&worktree, "worktree-post-create", "#!/bin/bash\nexit 1");

        let ctx = test_ctx_with_state(temp_dir.path(), &worktree, HookType::PostCreate, "main");

        let mut config = HooksConfig::default();
        config.worktree_post_create.fail_mode = FailMode::Warn;
        let mut trust_db = TrustDatabase::default();
        trust_db.set_trust_level(&ctx.git_dir, TrustLevel::Allow);
        let executor = HookExecutor::with_trust_db(config, trust_db);
        let mut output = TestOutput::default();

        // `failMode=warn` is the #765 opt-out: the failure is reported but
        // the run returns Ok so the creation command continues (runs `-x`,
        // exits 0).
        let result = executor
            .execute(&ctx, &mut output, NullPresenter::arc())
            .expect("warn mode must not abort");
        assert!(!result.success);
        assert!(!result.skipped);
        assert!(
            output
                .warnings()
                .iter()
                .any(|w| w.contains("continuing anyway")),
            "warn mode should announce it is continuing: {:?}",
            output.warnings()
        );
    }

    #[test]
    fn test_get_hook_source_worktree_post_remove_non_move_uses_source() {
        let ctx = HookContext::new(
            HookType::PostRemove,
            "rename",
            PathBuf::from("/project"),
            PathBuf::from("/project/.git"),
            "origin",
            PathBuf::from("/project/source"),
            PathBuf::from("/project/old-wt"),
            "feat/old",
        );
        // Non-move: PostRemove should use source_worktree
        assert_eq!(
            get_hook_source_worktree(&ctx),
            PathBuf::from("/project/source")
        );
    }

    #[test]
    fn test_get_hook_source_worktree_post_remove_move_uses_worktree_path() {
        let ctx = HookContext {
            is_move: true,
            ..HookContext::new(
                HookType::PostRemove,
                "rename",
                PathBuf::from("/project"),
                PathBuf::from("/project/.git"),
                "origin",
                PathBuf::from("/project/source"),
                PathBuf::from("/project/new-wt"),
                "feat/new",
            )
        };
        // Move: PostRemove should use worktree_path (the new location)
        assert_eq!(
            get_hook_source_worktree(&ctx),
            PathBuf::from("/project/new-wt")
        );
    }
}
