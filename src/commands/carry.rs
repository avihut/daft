use crate::{
    core::{worktree::carry, OutputSink},
    get_project_root,
    git::{should_show_gitoxide_notice, GitCommand},
    is_git_repository,
    logging::init_logging,
    output::{CliOutput, Output, OutputConfig},
    settings::DaftSettings,
    WorktreeConfig,
};
use anyhow::Result;
use clap::Parser;

#[derive(Parser)]
#[command(name = "git-worktree-carry")]
#[command(version = crate::VERSION)]
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
    let args = Args::parse_from(crate::get_clap_args("git-worktree-carry"));

    init_logging(args.verbose);

    if !is_git_repository()? {
        anyhow::bail!("Not inside a Git repository");
    }

    let settings = DaftSettings::load()?;
    let config = OutputConfig::with_autocd(false, args.verbose, settings.autocd);
    let mut output = CliOutput::new(config);

    let wt_config = WorktreeConfig {
        remote_name: settings.remote.clone(),
        quiet: false,
    };
    let git = GitCommand::new(wt_config.quiet).with_gitoxide(settings.use_gitoxide);
    let project_root = get_project_root()?;

    let params = carry::CarryParams {
        targets: args.targets,
        copy: args.copy,
    };

    if should_show_gitoxide_notice(settings.use_gitoxide) {
        output.warning("[experimental] Using gitoxide backend for git operations");
    }

    output.start_spinner("Carrying changes...");
    let result = {
        let mut sink = OutputSink(&mut output);
        carry::execute(&params, &git, &project_root, &mut sink)?
    };
    output.finish_spinner();

    render_carry_result(&result, &mut output);
    output.cd_path(&result.cd_target);

    Ok(())
}

fn render_carry_result(result: &carry::CarryResult, output: &mut dyn Output) {
    if result.no_changes {
        output.info("No uncommitted changes to carry.");
        return;
    }

    if !result.resolution_errors.is_empty() {
        for error in &result.resolution_errors {
            output.error(&format!("Failed to resolve target {}", error));
        }
        output.error(&format!(
            "Failed to resolve {} target(s). No changes were made.",
            result.resolution_errors.len()
        ));
        return;
    }

    if result.no_valid_targets {
        output.info("No valid targets to carry changes to.");
        return;
    }

    if result.failures.is_empty() {
        if result.copy_mode {
            if result.successes.len() == 1 {
                output.result(&format!(
                    "Done! Changes copied to '{}'. Now in {}",
                    result.successes[0].name,
                    result.cd_target.display()
                ));
            } else {
                output.result(&format!(
                    "Done! Changes copied to {} worktrees. Now in {}",
                    result.successes.len(),
                    result.cd_target.display()
                ));
            }
        } else {
            output.result(&format!("Done! Now in {}", result.cd_target.display()));
        }
    } else {
        output.error(&format!(
            "Completed with {} success(es) and {} failure(s).",
            result.successes.len(),
            result.failures.len()
        ));
        for failure in &result.failures {
            output.error(&format!("  {}: {}", failure.name, failure.error));
        }
        if result.stash_preserved {
            output.warning(&format!(
                "Stash preserved for recovery. Now in {}",
                result.cd_target.display()
            ));
        }
    }
}
