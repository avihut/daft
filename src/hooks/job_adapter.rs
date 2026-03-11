//! Converts hook inputs into generic [`JobSpec`] values.
//!
//! This is the boundary between the hooks input layer (YAML `JobDef`,
//! legacy script paths) and the format-agnostic executor. All
//! hook-specific resolution (command, environment, working directory,
//! RC-file sourcing, template substitution) happens here so that the
//! executor never needs to know about hook configuration details.

use crate::executor::JobSpec;
use crate::hooks::environment::{HookContext, HookEnvironment};
use crate::hooks::yaml_config::JobDef;
use std::collections::HashMap;
use std::path::{Path, PathBuf};

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
    rc: Option<&str>,
) -> Vec<JobSpec> {
    jobs.iter()
        .filter_map(|job| {
            // Skip group jobs — they contain nested sub-jobs that require
            // recursive handling. This will be wired up when the
            // yaml_executor is fully migrated to the generic executor.
            if job.group.is_some() {
                return None;
            }

            // Skip jobs with a platform-specific run map that has no
            // entry for the current OS.
            if super::yaml_executor::is_platform_skip(job) {
                return None;
            }

            let name = job.name.clone().unwrap_or_else(|| "(unnamed)".to_string());

            let cmd = super::yaml_executor::resolve_command(job, ctx, Some(&name), source_dir);

            // Prepend RC file sourcing if configured.
            let cmd = match rc {
                Some(rc_path) => format!("source {rc_path} && {cmd}"),
                None => cmd,
            };

            // Build merged environment: hook-level env first, then
            // per-job env (job wins on conflict).
            let mut env = hook_env.clone();
            if let Some(ref job_env) = job.env {
                env.extend(job_env.clone());
            }

            // Resolve working directory.
            let wd = if let Some(ref root) = job.root {
                working_dir.join(root)
            } else {
                working_dir.to_path_buf()
            };

            Some(JobSpec {
                name,
                command: cmd,
                working_dir: wd,
                env,
                description: job.description.clone(),
                needs: job.needs.clone().unwrap_or_default(),
                interactive: job.interactive == Some(true),
                fail_text: job.fail_text.clone(),
                timeout: JobSpec::DEFAULT_TIMEOUT,
            })
        })
        .collect()
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::hooks::yaml_config::{GroupDef, RunCommand};
    use crate::hooks::HookType;
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
        let specs =
            yaml_jobs_to_specs(&jobs, &ctx, &hook_env, ".daft", Path::new("/project"), None);

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

        let specs = yaml_jobs_to_specs(
            &jobs,
            &ctx,
            &HashMap::new(),
            ".daft",
            Path::new("/tmp"),
            None,
        );
        assert!(
            specs.is_empty(),
            "platform-mismatched job should be excluded"
        );
    }

    #[test]
    fn rc_file_prepended_to_command() {
        let ctx = make_ctx();
        let jobs = vec![JobDef {
            name: Some("build".into()),
            run: Some(RunCommand::Simple("cargo build".into())),
            ..Default::default()
        }];

        let specs = yaml_jobs_to_specs(
            &jobs,
            &ctx,
            &HashMap::new(),
            ".daft",
            Path::new("/project"),
            Some("~/.bashrc"),
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

        let specs = yaml_jobs_to_specs(
            &jobs,
            &ctx,
            &HashMap::new(),
            ".daft",
            Path::new("/project"),
            None,
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

        let specs =
            yaml_jobs_to_specs(&jobs, &ctx, &hook_env, ".daft", Path::new("/project"), None);

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

        let specs = yaml_jobs_to_specs(
            &jobs,
            &ctx,
            &HashMap::new(),
            ".daft",
            Path::new("/tmp"),
            None,
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

        let specs = yaml_jobs_to_specs(
            &jobs,
            &ctx,
            &HashMap::new(),
            ".daft",
            Path::new("/tmp"),
            None,
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

        let specs = yaml_jobs_to_specs(
            &jobs,
            &ctx,
            &HashMap::new(),
            ".daft",
            Path::new("/tmp"),
            None,
        );

        assert_eq!(specs.len(), 1, "group job should be excluded");
        assert_eq!(specs[0].name, "normal");
    }

    // ── scripts_to_specs ────────────────────────────────────────────────

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
}
