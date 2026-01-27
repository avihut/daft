use anyhow::Result;
use clap::Parser;
use daft::{
    get_project_root,
    git::GitCommand,
    is_git_repository, log_error, log_info, log_warning,
    logging::init_logging,
    output::{CliOutput, Output, OutputConfig},
    settings::DaftSettings,
    utils::*,
    WorktreeConfig,
};
use std::path::PathBuf;

#[derive(Parser)]
#[command(name = "git-worktree-carry")]
#[command(version = daft::VERSION)]
#[command(about = "Transfer uncommitted changes to other worktrees")]
#[command(long_about = r#"
Transfers uncommitted changes (staged, unstaged, and untracked files) from
the current worktree to one or more target worktrees.

When a single target is specified without --copy, changes are moved: they
are applied to the target worktree and removed from the source. When --copy
is specified or multiple targets are given, changes are copied: they are
applied to all targets while remaining in the source worktree.

Targets may be specified by worktree directory name or by branch name. If
both a worktree and a branch have the same name, the worktree takes
precedence.

After transferring changes, the working directory is changed to the last
target worktree (or the only target, if just one was specified).
"#)]
pub struct Args {
    #[arg(
        required = true,
        help = "Target worktree(s) by directory name or branch name"
    )]
    targets: Vec<String>,

    #[arg(
        short = 'c',
        long = "copy",
        help = "Copy changes instead of moving; changes remain in the source worktree"
    )]
    copy: bool,

    #[arg(short, long, help = "Be verbose; show detailed progress")]
    verbose: bool,
}

pub fn run() -> Result<()> {
    let args = Args::parse_from(daft::get_clap_args("git-worktree-carry"));

    init_logging(args.verbose);

    if !is_git_repository()? {
        anyhow::bail!("Not inside a Git repository");
    }

    // Load settings from git config
    let settings = DaftSettings::load()?;

    let config = OutputConfig::with_autocd(false, args.verbose, settings.autocd);
    let mut output = CliOutput::new(config);

    run_carry(&args, &settings, &mut output)
}

fn run_carry(args: &Args, settings: &DaftSettings, output: &mut dyn Output) -> Result<()> {
    let config = WorktreeConfig {
        remote_name: settings.remote.clone(),
        quiet: false,
    };
    let git = GitCommand::new(config.quiet);

    // Get the current worktree path before we start
    let source_worktree = git.get_current_worktree_path()?;
    let project_root = get_project_root()?;

    // Check for uncommitted changes
    if !git.has_uncommitted_changes()? {
        println!("No uncommitted changes to carry.");
        return Ok(());
    }

    // Resolve all targets upfront (fail fast if any are invalid)
    let mut resolved_targets: Vec<PathBuf> = Vec::new();
    let mut errors: Vec<String> = Vec::new();

    for target in &args.targets {
        match git.resolve_worktree_path(target, &project_root) {
            Ok(path) => {
                // Check if target is the current worktree
                if path == source_worktree {
                    log_warning!("Skipping '{}': already in this worktree", target);
                    continue;
                }
                resolved_targets.push(path);
            }
            Err(e) => {
                errors.push(format!("'{}': {}", target, e));
            }
        }
    }

    // If there are errors, report them and bail
    if !errors.is_empty() {
        for error in &errors {
            log_error!("Failed to resolve target {}", error);
        }
        anyhow::bail!(
            "Failed to resolve {} target(s). No changes were made.",
            errors.len()
        );
    }

    // If no valid targets remain, exit
    if resolved_targets.is_empty() {
        println!("No valid targets to carry changes to.");
        return Ok(());
    }

    // Determine copy mode: explicit --copy flag OR multiple targets
    let copy_mode = args.copy || resolved_targets.len() > 1;

    // Stash the changes
    println!("--> Stashing uncommitted changes...");
    git.stash_push_with_untracked("daft: carry changes")?;

    // Track successes and failures for multi-target
    let mut successes: Vec<String> = Vec::new();
    let mut failures: Vec<String> = Vec::new();

    // Apply to each target
    for target_path in &resolved_targets {
        let target_name = target_path
            .file_name()
            .and_then(|n| n.to_str())
            .unwrap_or("unknown");

        println!("--> Applying changes to '{}'...", target_name);

        // Change to target directory
        if let Err(e) = change_directory(target_path) {
            log_error!("Failed to change to '{}': {}", target_name, e);
            failures.push(target_name.to_string());
            continue;
        }

        // Apply stash (not pop, to preserve for next target)
        if let Err(e) = git.stash_apply() {
            log_error!("Failed to apply changes to '{}': {}", target_name, e);
            eprintln!(
                "    Possible conflicts. Resolve with: cd {} && git stash apply",
                target_path.display()
            );
            failures.push(target_name.to_string());
        } else {
            log_info!("Changes applied to '{}'", target_name);
            successes.push(target_name.to_string());
        }
    }

    // Handle stash cleanup based on mode
    if copy_mode {
        // Return to source and restore changes
        println!("--> Restoring changes in source worktree...");
        change_directory(&source_worktree)?;
        if let Err(e) = git.stash_pop() {
            log_error!("Failed to restore stashed changes: {}", e);
            eprintln!("Warning: Your changes are still in the stash. Run 'git stash pop' to restore them.");
        }
    } else {
        // Move mode: drop the stash since we moved the changes
        if let Err(e) = git.stash_drop() {
            log_warning!("Failed to drop stash: {}", e);
        }
    }

    // Change to the last target worktree
    let last_target = resolved_targets.last().expect("at least one target");
    change_directory(last_target)?;

    // Print summary
    println!("---");
    if failures.is_empty() {
        if copy_mode {
            if successes.len() == 1 {
                println!(
                    "Done! Changes copied to '{}'. Now in {}",
                    successes[0],
                    last_target.display()
                );
            } else {
                println!(
                    "Done! Changes copied to {} worktrees. Now in {}",
                    successes.len(),
                    last_target.display()
                );
            }
        } else {
            println!("Done! Now in {}", last_target.display());
        }
    } else {
        eprintln!(
            "Completed with {} success(es) and {} failure(s).",
            successes.len(),
            failures.len()
        );
        if !failures.is_empty() {
            eprintln!("Failed targets: {}", failures.join(", "));
        }
        eprintln!(
            "Stash preserved for recovery. Now in {}",
            last_target.display()
        );
    }

    output.cd_path(&get_current_directory()?);

    Ok(())
}
