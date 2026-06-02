//! Converts hook inputs into generic [`JobSpec`] values.
//!
//! This is the boundary between the hooks input layer (YAML `JobDef`,
//! legacy script paths) and the format-agnostic executor. All
//! hook-specific resolution (command, environment, working directory,
//! RC-file sourcing, template substitution) happens here so that the
//! executor never needs to know about hook configuration details.

use super::config_merge::merge_log_configs;
use crate::executor::{JobSpec, LogConfig};
use crate::hooks::environment::{HookContext, HookEnvironment};
use crate::hooks::yaml_config::JobDef;
use std::collections::HashMap;
use std::path::{Path, PathBuf};

/// A job that was declared in YAML but filtered out before execution.
///
/// Produced by `yaml_jobs_to_specs` alongside the kept `JobSpec`s so the
/// caller can record a skipped-job entry in the log store.
#[derive(Debug, Clone)]
pub struct SkippedJob {
    pub name: String,
    pub background: bool,
    pub reason: String,
}

/// Resolve a job's effective `background:` flag: the job's own setting wins,
/// then the hook-level default, then `false`. Centralizes the precedence rule
/// shared by kept-job spec conversion ([`yaml_jobs_to_specs`]) and the
/// `--skip-hooks` skip-attribution paths so it lives in exactly one place.
pub fn resolve_background(job_background: Option<bool>, hook_background: Option<bool>) -> bool {
    job_background.or(hook_background).unwrap_or(false)
}

/// Parsed `--skip-hooks` selectors.
///
/// Built by [`parse_skip_selectors`] from the raw CLI tokens and carried on
/// [`crate::hooks::yaml_executor::JobFilter`]. Drives the exclude side of hook
/// filtering: [`compute_skip_cascade`] consumes `names`/`tags`, while `all`
/// short-circuits the whole fire and `raw` is retained for no-match warnings.
#[derive(Debug, Default, Clone)]
pub struct SkipSelectors {
    /// `all` / `*` selector: skip every job (short-circuit the hook fire).
    pub all: bool,
    /// Job names to skip (from bare `<name>` and `job:<name>` selectors).
    pub names: Vec<String>,
    /// Tags to skip (from `tag:<tag>` selectors).
    pub tags: Vec<String>,
    /// Hook types to skip wholesale (from bare hook-type names like
    /// `worktree-post-create`), parsed into typed [`crate::hooks::HookType`]
    /// values. Matched against the current fire's hook name in the executor
    /// (via [`crate::hooks::HookType::yaml_name`]), where it short-circuits the
    /// whole hook (like `all`, but scoped to one hook type). Does NOT feed
    /// [`compute_skip_cascade`] — it is a hook-level, not job-level, selector.
    pub hook_types: Vec<crate::hooks::HookType>,
    /// Original selector tokens, retained for no-match warning attribution.
    pub raw: Vec<String>,
}

impl SkipSelectors {
    /// True when no selectors were supplied (the common case — no
    /// `--skip-hooks` flag). Lets callers cheaply bypass the exclude path.
    pub fn is_empty(&self) -> bool {
        !self.all && self.names.is_empty() && self.tags.is_empty() && self.hook_types.is_empty()
    }
}

/// Parse raw `--skip-hooks` tokens into a [`SkipSelectors`].
///
/// Precedence:
/// - `tag:<tag>` → tag selector
/// - `job:<name>` → name selector (escape hatch; `job:all` is the job literally
///   named `all`, NOT the wildcard; `job:worktree-post-create` is the job
///   literally named after a hook type, NOT the hook-type selector)
/// - `all` / `*` → wildcard (`all = true`)
/// - a canonical hook-type name (`worktree-post-create`, `post-clone`, …) →
///   hook-type selector (skip that whole hook). Only the canonical
///   [`crate::hooks::HookType::yaml_name`] spellings are reserved; deprecated
///   short forms (`post-create`) are treated as job names.
/// - anything else → bare name selector
pub fn parse_skip_selectors(selectors: &[String]) -> SkipSelectors {
    use crate::hooks::HookType;
    let mut out = SkipSelectors {
        raw: selectors.to_vec(),
        ..Default::default()
    };
    for sel in selectors {
        let s = sel.trim();
        if let Some(tag) = s.strip_prefix("tag:") {
            out.tags.push(tag.to_string());
        } else if let Some(name) = s.strip_prefix("job:") {
            out.names.push(name.to_string());
        } else if s == "all" || s == "*" {
            out.all = true;
        } else if let Some(ht) = HookType::from_yaml_name(s) {
            out.hook_types.push(ht);
        } else {
            out.names.push(s.to_string());
        }
    }
    out
}

/// Why a job was excluded by `--skip-hooks`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SkipCause {
    /// Direct selector match (job name or tag).
    Requested,
    /// Cascade: this job `needs:` an excluded job. Holds the *immediate*
    /// excluded dependency (not the cascade root) so the rendered reason reads
    /// `depends on <that dep> (skipped)`.
    DependsOn(String),
}

impl SkipCause {
    /// Render the user-facing skip reason for this cause.
    pub fn reason(&self) -> String {
        match self {
            SkipCause::Requested => "requested (--skip-hooks)".to_string(),
            SkipCause::DependsOn(dep) => format!("depends on {dep} (skipped)"),
        }
    }
}

/// Result of [`compute_skip_cascade`]: which jobs are excluded (and why), plus
/// selector tokens that matched nothing (for warnings).
#[derive(Debug, Default)]
pub struct SkipCascade {
    /// Excluded job name → cause. Membership lookup only; iterate the original
    /// `jobs` slice in declaration order when ordered output is needed.
    pub excluded: std::collections::HashMap<String, SkipCause>,
    /// Selector tokens (`<name>` / `tag:<tag>`) that matched no job.
    pub unmatched: Vec<String>,
}

/// Compute the set of jobs excluded by `--skip-hooks`, with attribution.
///
/// Operates on [`JobDef`]s while `needs:` is still intact — call BEFORE
/// [`yaml_jobs_to_specs`]. Does NOT handle the `all` selector; the caller
/// short-circuits that earlier (see `execute_yaml_hook_with_rc`).
///
/// Algorithm:
/// 1. **Direct pass** (declaration order): a named job is `Requested` if its
///    name is in `skip.names` OR any of its tags is in `skip.tags`.
/// 2. **Cascade pass** (BFS over reverse-`needs`): any job that `needs:` an
///    already-excluded job becomes `DependsOn(<that immediate dep>)`. The BFS
///    is seeded and scanned in declaration order so the immediate-parent
///    attribution is deterministic (e.g. the diamond case picks the
///    first-declared excluded dependency).
/// 3. **Unmatched**: each `skip.names` entry matching no `job.name`, and each
///    `skip.tags` entry present on no job, is collected for a warning.
///
/// Only named jobs participate (matching and cascade are name-keyed); unnamed
/// and `group:` jobs are out of scope here — groups carry their own skip
/// handling in [`yaml_jobs_to_specs`].
pub fn compute_skip_cascade(jobs: &[JobDef], skip: &SkipSelectors) -> SkipCascade {
    use std::collections::{HashMap, VecDeque};

    let mut excluded: HashMap<String, SkipCause> = HashMap::new();

    // ── Direct pass (declaration order) ──
    let mut seeds: VecDeque<String> = VecDeque::new();
    for job in jobs {
        let Some(name) = job.name.as_deref() else {
            continue;
        };
        let name_match = skip.names.iter().any(|n| n == name);
        let tag_match = job
            .tags
            .as_ref()
            .is_some_and(|tags| tags.iter().any(|t| skip.tags.contains(t)));
        if name_match || tag_match {
            excluded
                .entry(name.to_string())
                .or_insert(SkipCause::Requested);
            seeds.push_back(name.to_string());
        }
    }

    // ── Cascade pass (BFS over reverse-`needs`) ──
    while let Some(dep) = seeds.pop_front() {
        // Scan jobs in declaration order so the first dependent discovered keeps
        // a stable immediate-parent attribution.
        for job in jobs {
            let Some(name) = job.name.as_deref() else {
                continue;
            };
            if excluded.contains_key(name) {
                continue;
            }
            let needs_dep = job
                .needs
                .as_ref()
                .is_some_and(|needs| needs.iter().any(|n| n == &dep));
            if needs_dep {
                excluded.insert(name.to_string(), SkipCause::DependsOn(dep.clone()));
                seeds.push_back(name.to_string());
            }
        }
    }

    // ── Unmatched selectors (for warnings) ──
    let mut unmatched = Vec::new();
    for n in &skip.names {
        let matched = jobs.iter().any(|j| j.name.as_deref() == Some(n.as_str()));
        if !matched {
            unmatched.push(n.clone());
        }
    }
    for t in &skip.tags {
        let matched = jobs.iter().any(|j| {
            j.tags
                .as_ref()
                .is_some_and(|tags| tags.iter().any(|tag| tag == t))
        });
        if !matched {
            unmatched.push(format!("tag:{t}"));
        }
    }

    SkipCascade {
        excluded,
        unmatched,
    }
}

/// Passthrough context for [`yaml_jobs_to_specs`]: values sourced from the
/// outer hook execution but threaded unchanged into every job spec.
///
/// Tests pass `&JobAdapterContext::default()` since they exercise spec
/// translation in isolation; production wraps the relevant hook-level
/// values from `execute_yaml_hook_with_rc`.
#[derive(Debug, Default, Clone, Copy)]
pub struct JobAdapterContext<'a> {
    /// Path to an RC file whose `source` command is prepended to every job
    /// command — opt-in shell setup like `~/.bashrc`.
    pub rc: Option<&'a str>,
    /// Hook-level `background:` default applied when a job doesn't set its
    /// own.
    pub hook_background: Option<bool>,
    /// Top-level `log:` config from `daft.yml`, merged into each job's
    /// `log_config` so cleanup policies inherit repo-wide defaults.
    pub repo_log: Option<&'a LogConfig>,
}

/// Convert YAML job definitions into format-agnostic [`JobSpec`] values.
///
/// Each [`JobDef`] is resolved into a concrete command string, merged
/// environment, and working directory. This function handles:
/// - Command resolution (run, script, runner, args, platform-specific variants)
/// - Environment variable merging (hook env + per-job env)
/// - Working directory resolution (job `root` relative to base working dir)
/// - RC file sourcing (prepends `source <rc> &&` to commands)
/// - Template variable substitution
///
/// Jobs with platform-specific `run` maps that have no entry for the
/// current OS are silently excluded.
///
/// **Note:** Group jobs (`job.group.is_some()`) are skipped in this
/// initial implementation. They will be handled when the yaml_executor
/// is fully migrated to the generic executor.
pub fn yaml_jobs_to_specs(
    jobs: &[JobDef],
    ctx: &HookContext,
    hook_env: &HashMap<String, String>,
    source_dir: &str,
    working_dir: &Path,
    adapter: &JobAdapterContext<'_>,
) -> (Vec<JobSpec>, Vec<SkippedJob>) {
    let rc = adapter.rc;
    let hook_background = adapter.hook_background;
    let repo_log = adapter.repo_log;
    let mut kept: Vec<JobSpec> = Vec::new();
    let mut skipped: Vec<SkippedJob> = Vec::new();

    for job in jobs {
        let name = job.name.clone().unwrap_or_else(|| "(unnamed)".to_string());
        let declared_background = resolve_background(job.background, hook_background);

        if job.group.is_some() {
            skipped.push(SkippedJob {
                name,
                background: declared_background,
                reason: "skip: group jobs are not yet supported by the generic executor"
                    .to_string(),
            });
            continue;
        }

        if super::yaml_executor::is_platform_skip(job) {
            skipped.push(SkippedJob {
                name,
                background: declared_background,
                reason: format!(
                    "skip: platform-specific run has no entry for {}",
                    std::env::consts::OS
                ),
            });
            continue;
        }

        if let Some(ref skip) = job.skip
            && let Some(info) = super::conditions::should_skip(skip, working_dir)
        {
            skipped.push(SkippedJob {
                name,
                background: declared_background,
                reason: info.reason,
            });
            continue;
        }

        if let Some(ref only) = job.only
            && let Some(info) = super::conditions::should_only_skip(only, working_dir)
        {
            skipped.push(SkippedJob {
                name,
                background: declared_background,
                reason: info.reason,
            });
            continue;
        }

        let cmd = super::yaml_executor::resolve_command(job, ctx, Some(&name), source_dir);

        let cmd = match rc {
            Some(rc_path) => format!("source {rc_path} && {cmd}"),
            None => cmd,
        };

        let mut env = hook_env.clone();
        if let Some(ref job_env) = job.env {
            env.extend(job_env.clone());
        }

        let wd = if let Some(ref root) = job.root {
            working_dir.join(root)
        } else {
            working_dir.to_path_buf()
        };

        kept.push(JobSpec {
            name,
            command: cmd,
            working_dir: wd,
            env,
            description: job.description.clone(),
            needs: job.needs.clone().unwrap_or_default(),
            interactive: job.interactive == Some(true),
            fail_text: job.fail_text.clone(),
            timeout: JobSpec::DEFAULT_TIMEOUT,
            background: declared_background,
            background_output: job.background_output.clone(),
            log_config: merge_job_log(job.log.clone(), repo_log),
            tags: job.tags.clone().unwrap_or_default(),
        });
    }

    // Remove needs: references to skipped jobs so dependent jobs don't
    // fail DAG construction. A skipped dependency is vacuously satisfied —
    // it was never going to run, and we shouldn't block dependents on it.
    let skipped_names: std::collections::HashSet<&str> =
        skipped.iter().map(|s| s.name.as_str()).collect();
    for spec in &mut kept {
        spec.needs.retain(|n| !skipped_names.contains(n.as_str()));
    }

    (kept, skipped)
}

/// Convert legacy hook script paths into [`JobSpec`] values.
///
/// Each script path becomes a single job that runs the script directly
/// (not via `sh -c`). The environment includes all daft hook variables.
pub fn scripts_to_specs(
    hook_paths: &[PathBuf],
    env: &HookEnvironment,
    working_dir: &Path,
) -> Vec<JobSpec> {
    hook_paths
        .iter()
        .map(|path| {
            let name = path
                .file_name()
                .map(|f| f.to_string_lossy().into_owned())
                .unwrap_or_else(|| "(unknown)".to_string());

            JobSpec {
                name,
                command: path.to_string_lossy().into_owned(),
                working_dir: working_dir.to_path_buf(),
                env: env.vars().clone(),
                ..Default::default()
            }
        })
        .collect()
}

/// Merge a per-job [`LogConfig`] with the repo-level default, with per-job
/// fields taking precedence. Either or both may be `None`; the result is
/// `None` only when both are.
///
/// Used by [`yaml_jobs_to_specs`] so top-level `log:` defaults in `daft.yml`
/// (e.g. `max_total_size`, `keep_last`, `stale_running_after`) flow into
/// each job's `log_config` and reach `build_repo_policy`.
fn merge_job_log(per_job: Option<LogConfig>, repo: Option<&LogConfig>) -> Option<LogConfig> {
    match (per_job, repo) {
        (None, None) => None,
        (Some(j), None) => Some(j),
        (None, Some(r)) => Some(r.clone()),
        (Some(j), Some(r)) => Some(merge_log_configs(j, r.clone())),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::hooks::HookType;
    use crate::hooks::yaml_config::{GroupDef, RunCommand};
    use std::collections::HashMap;

    fn make_ctx() -> HookContext {
        HookContext::new(
            HookType::PostCreate,
            "checkout",
            "/project",
            "/project/.git",
            "origin",
            "/project/main",
            "/project/feature/new",
            "feature/new",
        )
    }

    // ── yaml_jobs_to_specs ──────────────────────────────────────────────

    #[test]
    fn simple_job_maps_all_fields() {
        let ctx = make_ctx();
        let mut job_env = HashMap::new();
        job_env.insert("MY_VAR".into(), "hello".into());

        let jobs = vec![JobDef {
            name: Some("install".into()),
            run: Some(RunCommand::Simple("npm install".into())),
            description: Some("Install deps".into()),
            needs: Some(vec!["fetch".into()]),
            interactive: Some(true),
            fail_text: Some("install failed".into()),
            env: Some(job_env.clone()),
            root: None,
            ..Default::default()
        }];

        let hook_env = HashMap::new();
        let (specs, _) = yaml_jobs_to_specs(
            &jobs,
            &ctx,
            &hook_env,
            ".daft",
            Path::new("/project"),
            &JobAdapterContext::default(),
        );

        assert_eq!(specs.len(), 1);
        let s = &specs[0];
        assert_eq!(s.name, "install");
        assert_eq!(s.command, "npm install");
        assert_eq!(s.description.as_deref(), Some("Install deps"));
        assert_eq!(s.needs, vec!["fetch"]);
        assert!(s.interactive);
        assert_eq!(s.fail_text.as_deref(), Some("install failed"));
        assert_eq!(s.env.get("MY_VAR").unwrap(), "hello");
        assert_eq!(s.working_dir, PathBuf::from("/project"));
        assert_eq!(s.timeout, JobSpec::DEFAULT_TIMEOUT);
    }

    #[test]
    fn platform_skip_excludes_job() {
        let ctx = make_ctx();
        // Create a platform-specific run map with only an OS that is NOT the
        // current one. On macOS the current OS is "macos", on Linux it is
        // "linux". We build a map that contains only the *other* OS so the
        // job is always skipped regardless of which platform runs the test.
        let other_os = if cfg!(target_os = "macos") {
            crate::hooks::yaml_config::TargetOs::Linux
        } else if cfg!(target_os = "linux") {
            crate::hooks::yaml_config::TargetOs::Macos
        } else {
            // Windows or other — just use Linux as the non-matching OS.
            crate::hooks::yaml_config::TargetOs::Linux
        };

        let mut platform_map = HashMap::new();
        platform_map.insert(
            other_os,
            crate::hooks::yaml_config::PlatformRunCommand::Simple("echo other".into()),
        );

        let jobs = vec![JobDef {
            name: Some("os-specific".into()),
            run: Some(RunCommand::Platform(platform_map)),
            ..Default::default()
        }];

        let (kept, skipped) = yaml_jobs_to_specs(
            &jobs,
            &ctx,
            &HashMap::new(),
            ".daft",
            Path::new("/tmp"),
            &JobAdapterContext::default(),
        );
        assert!(
            kept.is_empty(),
            "platform-mismatched job should be excluded"
        );
        assert_eq!(skipped.len(), 1);
        assert_eq!(skipped[0].name, "os-specific");
        assert!(skipped[0].reason.contains("platform"));
    }

    #[test]
    fn rc_file_prepended_to_command() {
        let ctx = make_ctx();
        let jobs = vec![JobDef {
            name: Some("build".into()),
            run: Some(RunCommand::Simple("cargo build".into())),
            ..Default::default()
        }];

        let (specs, _) = yaml_jobs_to_specs(
            &jobs,
            &ctx,
            &HashMap::new(),
            ".daft",
            Path::new("/project"),
            &JobAdapterContext {
                rc: Some("~/.bashrc"),
                ..Default::default()
            },
        );

        assert_eq!(specs.len(), 1);
        assert_eq!(specs[0].command, "source ~/.bashrc && cargo build");
    }

    #[test]
    fn working_dir_resolved_from_job_root() {
        let ctx = make_ctx();
        let jobs = vec![JobDef {
            name: Some("test".into()),
            run: Some(RunCommand::Simple("cargo test".into())),
            root: Some("packages/core".into()),
            ..Default::default()
        }];

        let (specs, _) = yaml_jobs_to_specs(
            &jobs,
            &ctx,
            &HashMap::new(),
            ".daft",
            Path::new("/project"),
            &JobAdapterContext::default(),
        );

        assert_eq!(specs.len(), 1);
        assert_eq!(
            specs[0].working_dir,
            PathBuf::from("/project/packages/core")
        );
    }

    #[test]
    fn env_merging_job_wins() {
        let ctx = make_ctx();
        let mut hook_env = HashMap::new();
        hook_env.insert("SHARED".into(), "from-hook".into());
        hook_env.insert("HOOK_ONLY".into(), "yes".into());

        let mut job_env = HashMap::new();
        job_env.insert("SHARED".into(), "from-job".into());
        job_env.insert("JOB_ONLY".into(), "yes".into());

        let jobs = vec![JobDef {
            name: Some("test".into()),
            run: Some(RunCommand::Simple("echo test".into())),
            env: Some(job_env),
            ..Default::default()
        }];

        let (specs, _) = yaml_jobs_to_specs(
            &jobs,
            &ctx,
            &hook_env,
            ".daft",
            Path::new("/project"),
            &JobAdapterContext::default(),
        );

        assert_eq!(specs.len(), 1);
        let env = &specs[0].env;
        assert_eq!(env.get("SHARED").unwrap(), "from-job", "job env should win");
        assert_eq!(env.get("HOOK_ONLY").unwrap(), "yes");
        assert_eq!(env.get("JOB_ONLY").unwrap(), "yes");
    }

    #[test]
    fn needs_mapping() {
        let ctx = make_ctx();
        let jobs = vec![
            JobDef {
                name: Some("a".into()),
                run: Some(RunCommand::Simple("true".into())),
                ..Default::default()
            },
            JobDef {
                name: Some("b".into()),
                run: Some(RunCommand::Simple("true".into())),
                needs: Some(vec!["a".into()]),
                ..Default::default()
            },
        ];

        let (specs, _) = yaml_jobs_to_specs(
            &jobs,
            &ctx,
            &HashMap::new(),
            ".daft",
            Path::new("/tmp"),
            &JobAdapterContext::default(),
        );

        assert_eq!(specs.len(), 2);
        assert!(specs[0].needs.is_empty());
        assert_eq!(specs[1].needs, vec!["a"]);
    }

    #[test]
    fn unnamed_job_gets_default_name() {
        let ctx = make_ctx();
        let jobs = vec![JobDef {
            run: Some(RunCommand::Simple("echo hi".into())),
            ..Default::default()
        }];

        let (specs, _) = yaml_jobs_to_specs(
            &jobs,
            &ctx,
            &HashMap::new(),
            ".daft",
            Path::new("/tmp"),
            &JobAdapterContext::default(),
        );

        assert_eq!(specs.len(), 1);
        assert_eq!(specs[0].name, "(unnamed)");
    }

    #[test]
    fn group_jobs_are_skipped() {
        let ctx = make_ctx();
        let jobs = vec![
            JobDef {
                name: Some("normal".into()),
                run: Some(RunCommand::Simple("echo ok".into())),
                ..Default::default()
            },
            JobDef {
                name: Some("grouped".into()),
                group: Some(GroupDef {
                    parallel: Some(true),
                    jobs: Some(vec![JobDef {
                        name: Some("inner".into()),
                        run: Some(RunCommand::Simple("echo inner".into())),
                        ..Default::default()
                    }]),
                    ..Default::default()
                }),
                ..Default::default()
            },
        ];

        let (kept, skipped) = yaml_jobs_to_specs(
            &jobs,
            &ctx,
            &HashMap::new(),
            ".daft",
            Path::new("/tmp"),
            &JobAdapterContext::default(),
        );

        assert_eq!(kept.len(), 1, "group job should be excluded");
        assert_eq!(kept[0].name, "normal");
        assert_eq!(skipped.len(), 1);
        assert_eq!(skipped[0].name, "grouped");
        assert!(skipped[0].reason.contains("group"));
    }

    #[test]
    fn platform_skip_produces_skipped_job_entry() {
        use crate::hooks::yaml_config::{PlatformRunCommand, TargetOs};
        let mut run_map = HashMap::new();
        let other_os = if cfg!(target_os = "macos") {
            TargetOs::Linux
        } else {
            TargetOs::Macos
        };
        run_map.insert(other_os, PlatformRunCommand::Simple("echo other".into()));

        let jobs = vec![JobDef {
            name: Some("platform-only".to_string()),
            run: Some(RunCommand::Platform(run_map)),
            ..Default::default()
        }];

        let ctx = make_ctx();
        let env = HashMap::new();
        let (kept, skipped) = yaml_jobs_to_specs(
            &jobs,
            &ctx,
            &env,
            "/src",
            std::path::Path::new("/work"),
            &JobAdapterContext::default(),
        );

        assert!(kept.is_empty());
        assert_eq!(skipped.len(), 1);
        assert_eq!(skipped[0].name, "platform-only");
        assert!(skipped[0].reason.contains("platform"));
    }

    #[test]
    fn group_jobs_produce_skipped_job_entry() {
        let jobs = vec![JobDef {
            name: Some("my-group".to_string()),
            group: Some(GroupDef::default()),
            ..Default::default()
        }];

        let ctx = make_ctx();
        let env = HashMap::new();
        let (kept, skipped) = yaml_jobs_to_specs(
            &jobs,
            &ctx,
            &env,
            "/src",
            std::path::Path::new("/work"),
            &JobAdapterContext::default(),
        );

        assert!(kept.is_empty());
        assert_eq!(skipped.len(), 1);
        assert_eq!(skipped[0].name, "my-group");
        assert!(skipped[0].reason.contains("group"));
    }

    #[test]
    fn per_job_skip_true_produces_skipped_entry() {
        use crate::hooks::yaml_config::{JobDef, SkipCondition};
        let jobs = vec![JobDef {
            name: Some("gated".to_string()),
            run: Some(crate::hooks::yaml_config::RunCommand::Simple(
                "echo gated".to_string(),
            )),
            skip: Some(SkipCondition::Bool(true)),
            ..Default::default()
        }];

        let ctx = make_ctx();
        let env = HashMap::new();
        let (kept, skipped) = yaml_jobs_to_specs(
            &jobs,
            &ctx,
            &env,
            "/src",
            std::path::Path::new("/work"),
            &JobAdapterContext::default(),
        );

        assert!(kept.is_empty());
        assert_eq!(skipped.len(), 1);
        assert_eq!(skipped[0].name, "gated");
        assert_eq!(skipped[0].reason, "skip: true");
    }

    #[test]
    fn per_job_skip_false_keeps_job() {
        use crate::hooks::yaml_config::{JobDef, SkipCondition};
        let jobs = vec![JobDef {
            name: Some("always-runs".to_string()),
            run: Some(crate::hooks::yaml_config::RunCommand::Simple(
                "echo go".to_string(),
            )),
            skip: Some(SkipCondition::Bool(false)),
            ..Default::default()
        }];

        let ctx = make_ctx();
        let env = HashMap::new();
        let (kept, skipped) = yaml_jobs_to_specs(
            &jobs,
            &ctx,
            &env,
            "/src",
            std::path::Path::new("/work"),
            &JobAdapterContext::default(),
        );

        assert_eq!(kept.len(), 1);
        assert!(skipped.is_empty());
    }

    #[test]
    fn kept_spec_needs_are_stripped_of_skipped_dependencies() {
        use crate::hooks::yaml_config::{JobDef, SkipCondition};
        let jobs = vec![
            JobDef {
                name: Some("skipped-dep".to_string()),
                run: Some(crate::hooks::yaml_config::RunCommand::Simple(
                    "echo nope".to_string(),
                )),
                skip: Some(SkipCondition::Bool(true)),
                ..Default::default()
            },
            JobDef {
                name: Some("dependent".to_string()),
                run: Some(crate::hooks::yaml_config::RunCommand::Simple(
                    "echo yes".to_string(),
                )),
                needs: Some(vec!["skipped-dep".to_string()]),
                ..Default::default()
            },
        ];

        let ctx = make_ctx();
        let env = HashMap::new();
        let (kept, skipped) = yaml_jobs_to_specs(
            &jobs,
            &ctx,
            &env,
            "/src",
            std::path::Path::new("/work"),
            &JobAdapterContext::default(),
        );

        assert_eq!(kept.len(), 1);
        assert_eq!(kept[0].name, "dependent");
        assert!(
            kept[0].needs.is_empty(),
            "needs: list should have been stripped of the skipped dependency, but got {:?}",
            kept[0].needs
        );
        assert_eq!(skipped.len(), 1);
        assert_eq!(skipped[0].name, "skipped-dep");
    }

    #[test]
    fn per_job_only_false_produces_skipped_entry() {
        use crate::hooks::yaml_config::{JobDef, OnlyCondition};
        let jobs = vec![JobDef {
            name: Some("conditional".to_string()),
            run: Some(crate::hooks::yaml_config::RunCommand::Simple(
                "echo cond".to_string(),
            )),
            only: Some(OnlyCondition::Bool(false)),
            ..Default::default()
        }];

        let ctx = make_ctx();
        let env = HashMap::new();
        let (kept, skipped) = yaml_jobs_to_specs(
            &jobs,
            &ctx,
            &env,
            "/src",
            std::path::Path::new("/work"),
            &JobAdapterContext::default(),
        );

        assert!(kept.is_empty());
        assert_eq!(skipped.len(), 1);
        assert_eq!(skipped[0].reason, "only: false");
    }

    // ── scripts_to_specs ────────────────────────────────────────────────

    #[test]
    fn test_background_fields_pass_through() {
        use crate::hooks::yaml_config::{BackgroundOutput, LogConfig};

        let jobs = vec![JobDef {
            name: Some("bg-job".to_string()),
            run: Some(RunCommand::Simple("echo hi".to_string())),
            background: Some(true),
            background_output: Some(BackgroundOutput::Silent),
            log: Some(LogConfig {
                retention: Some("14d".to_string()),
                ..Default::default()
            }),
            ..Default::default()
        }];

        let ctx = make_ctx();
        let (specs, _) = yaml_jobs_to_specs(
            &jobs,
            &ctx,
            &HashMap::new(),
            ".daft",
            Path::new("/tmp"),
            &JobAdapterContext::default(),
        );
        assert_eq!(specs.len(), 1);
        assert!(specs[0].background);
        assert_eq!(specs[0].background_output, Some(BackgroundOutput::Silent));
        assert_eq!(
            specs[0].log_config.as_ref().unwrap().retention,
            Some("14d".to_string())
        );
    }

    #[test]
    fn scripts_name_from_filename() {
        let ctx = make_ctx();
        let env = HookEnvironment::from_context(&ctx);
        let paths = vec![
            PathBuf::from("/project/.daft/hooks/worktree-post-create"),
            PathBuf::from("/home/user/.config/daft/hooks/post-clone"),
        ];

        let specs = scripts_to_specs(&paths, &env, Path::new("/project/feature/new"));

        assert_eq!(specs.len(), 2);
        assert_eq!(specs[0].name, "worktree-post-create");
        assert_eq!(specs[1].name, "post-clone");
    }

    #[test]
    fn scripts_command_is_full_path() {
        let ctx = make_ctx();
        let env = HookEnvironment::from_context(&ctx);
        let paths = vec![PathBuf::from("/project/.daft/hooks/post-clone")];

        let specs = scripts_to_specs(&paths, &env, Path::new("/project"));

        assert_eq!(specs.len(), 1);
        assert_eq!(specs[0].command, "/project/.daft/hooks/post-clone");
    }

    #[test]
    fn scripts_working_dir_passed_through() {
        let ctx = make_ctx();
        let env = HookEnvironment::from_context(&ctx);
        let paths = vec![PathBuf::from("/some/hook")];
        let wd = Path::new("/custom/dir");

        let specs = scripts_to_specs(&paths, &env, wd);

        assert_eq!(specs[0].working_dir, PathBuf::from("/custom/dir"));
    }

    #[test]
    fn scripts_env_from_hook_environment() {
        let ctx = make_ctx();
        let env = HookEnvironment::from_context(&ctx);
        let paths = vec![PathBuf::from("/some/hook")];

        let specs = scripts_to_specs(&paths, &env, Path::new("/tmp"));

        assert_eq!(
            specs[0].env.get("DAFT_HOOK").map(String::as_str),
            Some("worktree-post-create")
        );
        assert_eq!(
            specs[0].env.get("DAFT_BRANCH_NAME").map(String::as_str),
            Some("feature/new")
        );
    }

    #[test]
    fn scripts_defaults_for_optional_fields() {
        let ctx = make_ctx();
        let env = HookEnvironment::from_context(&ctx);
        let paths = vec![PathBuf::from("/some/hook")];

        let specs = scripts_to_specs(&paths, &env, Path::new("/tmp"));

        let s = &specs[0];
        assert!(!s.interactive);
        assert!(s.needs.is_empty());
        assert!(s.fail_text.is_none());
        assert!(s.description.is_none());
        assert_eq!(s.timeout, JobSpec::DEFAULT_TIMEOUT);
    }

    // ── repo-level log merge ─────────────────────────────────────────────
    //
    // Top-level `log:` defaults in `daft.yml` (max_total_size, keep_last,
    // stale_running_after, plus retention/max_log_size) must propagate into
    // each job's `log_config`, with per-job blocks taking precedence.

    #[test]
    fn merges_repo_log_into_jobs_without_per_job_log() {
        use crate::executor::LogConfig;

        let jobs = vec![JobDef {
            name: Some("build".to_string()),
            run: Some(RunCommand::Simple("cargo build".to_string())),
            log: None,
            ..Default::default()
        }];

        let repo_log = LogConfig {
            max_total_size: Some("1GB".to_string()),
            keep_last: Some(5),
            stale_running_after: Some("48h".to_string()),
            ..Default::default()
        };

        let ctx = make_ctx();
        let (kept, _) = yaml_jobs_to_specs(
            &jobs,
            &ctx,
            &HashMap::new(),
            ".daft",
            Path::new("/tmp"),
            &JobAdapterContext {
                repo_log: Some(&repo_log),
                ..Default::default()
            },
        );

        assert_eq!(kept.len(), 1);
        let lc = kept[0]
            .log_config
            .as_ref()
            .expect("log_config should be Some when repo-level log is provided");
        assert_eq!(lc.max_total_size.as_deref(), Some("1GB"));
        assert_eq!(lc.keep_last, Some(5));
        assert_eq!(lc.stale_running_after.as_deref(), Some("48h"));
    }

    #[test]
    fn per_job_log_overrides_repo_log() {
        use crate::executor::LogConfig;

        let jobs = vec![JobDef {
            name: Some("build".to_string()),
            run: Some(RunCommand::Simple("cargo build".to_string())),
            log: Some(LogConfig {
                retention: Some("1d".to_string()),
                max_log_size: Some("1MB".to_string()),
                ..Default::default()
            }),
            ..Default::default()
        }];

        let repo_log = LogConfig {
            retention: Some("30d".to_string()),
            max_total_size: Some("1GB".to_string()),
            keep_last: Some(3),
            ..Default::default()
        };

        let ctx = make_ctx();
        let (kept, _) = yaml_jobs_to_specs(
            &jobs,
            &ctx,
            &HashMap::new(),
            ".daft",
            Path::new("/tmp"),
            &JobAdapterContext {
                repo_log: Some(&repo_log),
                ..Default::default()
            },
        );

        assert_eq!(kept.len(), 1);
        let lc = kept[0].log_config.as_ref().unwrap();
        // Per-job overrides win
        assert_eq!(
            lc.retention.as_deref(),
            Some("1d"),
            "per-job retention should override repo-level"
        );
        assert_eq!(lc.max_log_size.as_deref(), Some("1MB"));
        // Repo-level fills in unset per-job fields
        assert_eq!(
            lc.max_total_size.as_deref(),
            Some("1GB"),
            "repo-level max_total_size should fill in"
        );
        assert_eq!(lc.keep_last, Some(3), "repo-level keep_last should fill in");
    }

    #[test]
    fn no_repo_log_keeps_per_job_log_unchanged() {
        use crate::executor::LogConfig;

        let jobs = vec![JobDef {
            name: Some("build".to_string()),
            run: Some(RunCommand::Simple("cargo build".to_string())),
            log: Some(LogConfig {
                retention: Some("7d".to_string()),
                ..Default::default()
            }),
            ..Default::default()
        }];

        let ctx = make_ctx();
        let (kept, _) = yaml_jobs_to_specs(
            &jobs,
            &ctx,
            &HashMap::new(),
            ".daft",
            Path::new("/tmp"),
            &JobAdapterContext::default(),
        );

        assert_eq!(kept.len(), 1);
        let lc = kept[0].log_config.as_ref().unwrap();
        assert_eq!(lc.retention.as_deref(), Some("7d"));
        assert!(lc.max_total_size.is_none());
    }

    #[test]
    fn no_log_at_either_level_yields_none() {
        let jobs = vec![JobDef {
            name: Some("build".to_string()),
            run: Some(RunCommand::Simple("cargo build".to_string())),
            log: None,
            ..Default::default()
        }];

        let ctx = make_ctx();
        let (kept, _) = yaml_jobs_to_specs(
            &jobs,
            &ctx,
            &HashMap::new(),
            ".daft",
            Path::new("/tmp"),
            &JobAdapterContext::default(),
        );

        assert_eq!(kept.len(), 1);
        assert!(kept[0].log_config.is_none());
    }

    // ── parse_skip_selectors ────────────────────────────────────────────

    fn strs(items: &[&str]) -> Vec<String> {
        items.iter().map(|s| s.to_string()).collect()
    }

    #[test]
    fn parse_bare_name_goes_to_names() {
        let s = parse_skip_selectors(&strs(&["lint"]));
        assert!(!s.all);
        assert_eq!(s.names, vec!["lint"]);
        assert!(s.tags.is_empty());
    }

    #[test]
    fn parse_job_prefix_is_escape_hatch() {
        // `job:all` is a literal job name, never the wildcard.
        let s = parse_skip_selectors(&strs(&["job:all"]));
        assert!(!s.all);
        assert_eq!(s.names, vec!["all"]);
    }

    #[test]
    fn parse_wildcard_star_and_all() {
        assert!(parse_skip_selectors(&strs(&["all"])).all);
        assert!(parse_skip_selectors(&strs(&["*"])).all);
    }

    #[test]
    fn parse_tag_prefix() {
        let s = parse_skip_selectors(&strs(&["tag:heavy"]));
        assert_eq!(s.tags, vec!["heavy"]);
        assert!(s.names.is_empty());
    }

    #[test]
    fn parse_mixed_selectors() {
        let s = parse_skip_selectors(&strs(&["tag:heavy", "lint", "job:build"]));
        assert!(!s.all);
        assert_eq!(s.tags, vec!["heavy"]);
        assert_eq!(s.names, vec!["lint", "build"]);
        assert!(s.hook_types.is_empty());
        assert_eq!(s.raw.len(), 3);
    }

    #[test]
    fn parse_hook_type_name_is_reserved() {
        let s = parse_skip_selectors(&strs(&["worktree-post-create"]));
        assert!(!s.all);
        assert_eq!(s.hook_types, vec![crate::hooks::HookType::PostCreate]);
        assert!(s.names.is_empty());
        assert!(s.tags.is_empty());
        // post-clone and the other canonical names are reserved too.
        assert_eq!(
            parse_skip_selectors(&strs(&["post-clone"])).hook_types,
            vec![crate::hooks::HookType::PostClone]
        );
    }

    #[test]
    fn parse_job_prefix_escapes_hook_type_name() {
        // `job:worktree-post-create` is a job literally named after the hook
        // type, NOT the hook-type selector.
        let s = parse_skip_selectors(&strs(&["job:worktree-post-create"]));
        assert!(s.hook_types.is_empty());
        assert_eq!(s.names, vec!["worktree-post-create"]);
    }

    #[test]
    fn parse_deprecated_hook_short_form_is_a_job_name() {
        // Deprecated short forms (`post-create`) are NOT reserved — only the
        // canonical yaml_name spellings are. A bare `post-create` is a job name.
        let s = parse_skip_selectors(&strs(&["post-create"]));
        assert!(s.hook_types.is_empty());
        assert_eq!(s.names, vec!["post-create"]);
    }

    // ── compute_skip_cascade ────────────────────────────────────────────

    /// The issue's worked-example graph:
    ///   install            (no needs)
    ///   build  needs install, tags [heavy]
    ///   test   needs build
    ///   lint   needs install
    fn worked_example_jobs() -> Vec<JobDef> {
        vec![
            JobDef {
                name: Some("install".into()),
                ..Default::default()
            },
            JobDef {
                name: Some("build".into()),
                needs: Some(vec!["install".into()]),
                tags: Some(vec!["heavy".into()]),
                ..Default::default()
            },
            JobDef {
                name: Some("test".into()),
                needs: Some(vec!["build".into()]),
                ..Default::default()
            },
            JobDef {
                name: Some("lint".into()),
                needs: Some(vec!["install".into()]),
                ..Default::default()
            },
        ]
    }

    #[test]
    fn cascade_name_seed_pulls_transitive_dependents() {
        let jobs = worked_example_jobs();
        let skip = parse_skip_selectors(&strs(&["install"]));
        let c = compute_skip_cascade(&jobs, &skip);

        assert_eq!(c.excluded.get("install"), Some(&SkipCause::Requested));
        assert_eq!(
            c.excluded.get("build"),
            Some(&SkipCause::DependsOn("install".into()))
        );
        assert_eq!(
            c.excluded.get("test"),
            Some(&SkipCause::DependsOn("build".into()))
        );
        assert_eq!(
            c.excluded.get("lint"),
            Some(&SkipCause::DependsOn("install".into()))
        );
        assert_eq!(c.excluded.len(), 4);
        assert!(c.unmatched.is_empty());
    }

    #[test]
    fn cascade_tag_seed_drops_only_subtree() {
        let jobs = worked_example_jobs();
        let skip = parse_skip_selectors(&strs(&["tag:heavy"]));
        let c = compute_skip_cascade(&jobs, &skip);

        // build matches tag:heavy directly; test depends on build.
        assert_eq!(c.excluded.get("build"), Some(&SkipCause::Requested));
        assert_eq!(
            c.excluded.get("test"),
            Some(&SkipCause::DependsOn("build".into()))
        );
        // install and lint are untouched (upstream / sibling).
        assert!(!c.excluded.contains_key("install"));
        assert!(!c.excluded.contains_key("lint"));
        assert_eq!(c.excluded.len(), 2);
    }

    #[test]
    fn cascade_immediate_parent_attribution() {
        // test reports its immediate excluded dep (build), not the root (install).
        let jobs = worked_example_jobs();
        let skip = parse_skip_selectors(&strs(&["install"]));
        let c = compute_skip_cascade(&jobs, &skip);
        assert_eq!(
            c.excluded.get("test"),
            Some(&SkipCause::DependsOn("build".into()))
        );
    }

    #[test]
    fn cascade_diamond_is_deterministic() {
        // `app` needs two directly-excluded jobs; declaration-order-first wins.
        let jobs = vec![
            JobDef {
                name: Some("a".into()),
                ..Default::default()
            },
            JobDef {
                name: Some("b".into()),
                ..Default::default()
            },
            JobDef {
                name: Some("app".into()),
                needs: Some(vec!["b".into(), "a".into()]),
                ..Default::default()
            },
        ];
        let skip = parse_skip_selectors(&strs(&["a", "b"]));
        let c = compute_skip_cascade(&jobs, &skip);
        // Seeds enqueued in declaration order: a, then b. `a` is processed
        // first and claims `app`.
        assert_eq!(
            c.excluded.get("app"),
            Some(&SkipCause::DependsOn("a".into()))
        );
    }

    #[test]
    fn cascade_unmatched_selectors_collected() {
        let jobs = worked_example_jobs();
        let skip = parse_skip_selectors(&strs(&["ghost", "tag:nope"]));
        let c = compute_skip_cascade(&jobs, &skip);
        assert!(c.excluded.is_empty());
        assert!(c.unmatched.contains(&"ghost".to_string()));
        assert!(c.unmatched.contains(&"tag:nope".to_string()));
    }

    #[test]
    fn cascade_upstream_is_untouched() {
        // Skipping a leaf (test) must not remove its upstream deps.
        let jobs = worked_example_jobs();
        let skip = parse_skip_selectors(&strs(&["test"]));
        let c = compute_skip_cascade(&jobs, &skip);
        assert_eq!(c.excluded.get("test"), Some(&SkipCause::Requested));
        assert_eq!(c.excluded.len(), 1);
        assert!(!c.excluded.contains_key("install"));
        assert!(!c.excluded.contains_key("build"));
        assert!(!c.excluded.contains_key("lint"));
    }

    #[test]
    fn cause_reason_strings() {
        assert_eq!(SkipCause::Requested.reason(), "requested (--skip-hooks)");
        assert_eq!(
            SkipCause::DependsOn("build".into()).reason(),
            "depends on build (skipped)"
        );
    }
}
