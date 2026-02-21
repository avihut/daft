use super::{styled_trust_level, HooksRunArgs};
use crate::hooks::yaml_executor::JobFilter;
use crate::hooks::{
    yaml_config, yaml_config_loader, HookExecutor, HookType, HooksConfig, TrustDatabase, TrustLevel,
};
use crate::styles::{bold, cyan, dim};
use crate::{get_current_branch, get_current_worktree_path, get_git_common_dir, get_project_root};
use anyhow::{Context, Result};

/// Run a hook manually.
pub(super) fn cmd_run(args: &HooksRunArgs) -> Result<()> {
    use crate::hooks::yaml_config_loader::get_effective_jobs;
    use crate::hooks::HookContext;

    // Resolve worktree context
    let worktree_path = get_current_worktree_path()
        .context("Not in a git worktree. Run this command from within a worktree directory.")?;

    // Load YAML config (needed for both listing and execution)
    let yaml_config = yaml_config_loader::load_merged_config(&worktree_path)
        .context("Failed to load YAML config")?;
    let yaml_config = match yaml_config {
        Some(c) => c,
        None => {
            anyhow::bail!("No daft.yml found in this worktree");
        }
    };

    // If no hook type specified, list available hooks
    let hook_type_str = match args.hook_type {
        Some(ref s) => s.clone(),
        None => {
            return cmd_run_list_hooks(&yaml_config);
        }
    };

    // Parse hook type
    let hook_type = HookType::from_yaml_name(&hook_type_str).ok_or_else(|| {
        anyhow::anyhow!(
            "Unknown hook type: '{}'\nValid hook types: {}",
            hook_type_str,
            yaml_config::KNOWN_HOOK_NAMES.join(", ")
        )
    })?;

    let git_dir = get_git_common_dir().context("Could not determine git directory")?;
    let project_root = get_project_root().context("Could not determine project root")?;
    let branch_name = get_current_branch().unwrap_or_else(|_| "HEAD".to_string());

    let hook_name = hook_type.yaml_name();
    let hook_def = yaml_config.hooks.get(hook_name).ok_or_else(|| {
        let mut names: Vec<&str> = yaml_config.hooks.keys().map(|s| s.as_str()).collect();
        names.sort();
        if names.is_empty() {
            anyhow::anyhow!("No hooks defined in daft.yml")
        } else {
            anyhow::anyhow!(
                "Hook '{}' is not defined in daft.yml\nConfigured hooks: {}",
                hook_name,
                names.join(", ")
            )
        }
    })?;

    // Check trust level and show hint if not trusted
    let trust_db = TrustDatabase::load().unwrap_or_default();
    let trust_level = trust_db.get_trust_level(&git_dir);
    if trust_level != TrustLevel::Allow {
        println!(
            "{} this repository is not in your trust list ({}).",
            dim("Note:"),
            styled_trust_level(trust_level)
        );
        println!(
            "  {} run `{}` to allow hooks to run automatically.",
            dim("Tip:"),
            cyan("git daft hooks trust")
        );
        println!();
    }

    // Build job filter
    let filter = JobFilter {
        only_job_name: args.job.clone(),
        only_tags: args.tag.clone(),
    };

    // Dry-run: preview jobs without executing
    if args.dry_run {
        let mut jobs = get_effective_jobs(hook_def);

        // Apply exclude_tags from hook definition
        if let Some(ref exclude_tags) = hook_def.exclude_tags {
            jobs.retain(|job| {
                if let Some(ref tags) = job.tags {
                    !tags.iter().any(|t| exclude_tags.contains(t))
                } else {
                    true
                }
            });
        }

        // Apply inclusion filters
        if let Some(ref name) = filter.only_job_name {
            jobs.retain(|j| j.name.as_deref() == Some(name.as_str()));
            if jobs.is_empty() {
                anyhow::bail!("No job named '{}' found in hook '{}'", name, hook_name);
            }
        }
        if !filter.only_tags.is_empty() {
            jobs.retain(|job| {
                job.tags
                    .as_ref()
                    .is_some_and(|tags| tags.iter().any(|t| filter.only_tags.contains(t)))
            });
            if jobs.is_empty() {
                anyhow::bail!(
                    "No jobs matching tags {:?} in hook '{}'",
                    filter.only_tags,
                    hook_name
                );
            }
        }

        // Sort by priority
        jobs.sort_by_key(|j| j.priority.unwrap_or(0));

        if jobs.is_empty() {
            println!("{}", dim("No jobs to run."));
            return Ok(());
        }

        let job_count = jobs.len();
        let job_word = if job_count == 1 { "job" } else { "jobs" };
        println!(
            "{} {} ({} {})",
            bold("Hook:"),
            cyan(hook_name),
            job_count,
            job_word
        );
        println!();

        for (i, job) in jobs.iter().enumerate() {
            let name = job.name.as_deref().unwrap_or("(unnamed)");
            println!("  {}. {}", i + 1, bold(name));

            if let Some(ref desc) = job.description {
                println!("     {}", dim(desc));
            }

            if let Some(ref os) = job.os {
                let os_list: Vec<&str> = os.as_slice().iter().map(|o| o.as_str()).collect();
                println!("     {}: {}", dim("os"), os_list.join(", "));
            }

            if let Some(ref arch) = job.arch {
                let arch_list: Vec<&str> = arch.as_slice().iter().map(|a| a.as_str()).collect();
                println!("     {}: {}", dim("arch"), arch_list.join(", "));
            }

            if let Some(ref run) = job.run {
                println!("     {}: {}", dim("run"), run);
            } else if let Some(ref script) = job.script {
                let runner_str = job
                    .runner
                    .as_ref()
                    .map(|r| format!("{r} "))
                    .unwrap_or_default();
                println!("     {}: {}{}", dim("script"), runner_str, script);
            } else if job.group.is_some() {
                println!("     {}", dim("(group)"));
            }

            if let Some(ref needs) = job.needs {
                if !needs.is_empty() {
                    println!("     {}: [{}]", dim("needs"), needs.join(", "));
                }
            }

            if let Some(ref tags) = job.tags {
                if !tags.is_empty() {
                    println!("     {}: [{}]", dim("tags"), tags.join(", "));
                }
            }

            if i + 1 < job_count {
                println!();
            }
        }

        return Ok(());
    }

    // Build HookContext for execution
    let ctx = HookContext::new(
        hook_type,
        "hooks-run",
        &project_root,
        &git_dir,
        "origin",
        &worktree_path,
        &worktree_path,
        &branch_name,
    );

    let mut hooks_config = HooksConfig::default();
    if args.verbose {
        hooks_config.output.verbose = true;
    }
    let executor = HookExecutor::new(hooks_config)?
        .with_bypass_trust(true)
        .with_job_filter(filter);

    let mut output = crate::output::CliOutput::default_output();
    let result = executor.execute(&ctx, &mut output)?;

    if result.skipped {
        if let Some(reason) = result.skip_reason {
            println!("{}", dim(&format!("Skipped: {reason}")));
        }
    } else if !result.success {
        std::process::exit(result.exit_code.unwrap_or(1));
    }

    Ok(())
}

/// List available hooks when `hooks run` is invoked with no arguments.
fn cmd_run_list_hooks(config: &yaml_config::YamlConfig) -> Result<()> {
    use crate::hooks::yaml_config_loader::get_effective_jobs;

    if config.hooks.is_empty() {
        println!("{}", dim("No hooks defined in daft.yml."));
        return Ok(());
    }

    let mut names: Vec<&String> = config.hooks.keys().collect();
    names.sort();

    println!("{}", bold("Available hooks:"));
    println!();

    for name in &names {
        let hook_def = &config.hooks[*name];
        let jobs = get_effective_jobs(hook_def);
        let job_count = jobs.len();
        let job_word = if job_count == 1 { "job" } else { "jobs" };
        println!("  {} ({} {})", cyan(name), job_count, job_word);
    }

    println!();
    println!(
        "Run a hook with: {}",
        cyan("git daft hooks run <hook-type>")
    );

    Ok(())
}
