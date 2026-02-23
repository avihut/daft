use crate::{
    core::{worktree::rename, OutputSink},
    is_git_repository,
    logging::init_logging,
    output::{CliOutput, Output, OutputConfig},
    settings::DaftSettings,
    CD_FILE_ENV,
};
use anyhow::Result;
use clap::Parser;

#[derive(Parser)]
#[command(name = "git-worktree-rename")]
#[command(version = crate::VERSION)]
#[command(about = "Rename a branch and move its worktree")]
#[command(long_about = r#"
Renames a local branch and moves its associated worktree directory to match
the new branch name. If the branch has a remote tracking branch, the remote
branch is also renamed (push new name, delete old name) unless --no-remote
is specified.

The source can be specified as a branch name or a path to an existing
worktree (absolute or relative).

If you are currently inside the worktree being renamed, the shell is
redirected to the new worktree location after the rename completes.

Empty parent directories left behind by the move are automatically cleaned up.
"#)]
pub struct Args {
    #[arg(help = "Source branch name or worktree path")]
    source: String,

    #[arg(help = "New branch name")]
    new_branch: String,

    #[arg(long, help = "Skip remote branch rename")]
    no_remote: bool,

    #[arg(long, help = "Preview changes without executing")]
    dry_run: bool,

    #[arg(short, long, help = "Suppress progress output")]
    quiet: bool,

    #[arg(short, long, help = "Show detailed progress")]
    verbose: bool,
}

pub fn run() -> Result<()> {
    let args = Args::parse_from(crate::get_clap_args("git-worktree-rename"));

    init_logging(args.verbose);

    if !is_git_repository()? {
        anyhow::bail!("Not inside a Git repository");
    }

    let settings = DaftSettings::load()?;
    let config = OutputConfig::with_autocd(args.quiet, args.verbose, settings.autocd);
    let mut output = CliOutput::new(config);

    run_rename(&mut output, &settings, &args)?;
    Ok(())
}

fn run_rename(output: &mut dyn Output, settings: &DaftSettings, args: &Args) -> Result<()> {
    let params = rename::RenameParams {
        source: args.source.clone(),
        new_branch: args.new_branch.clone(),
        no_remote: args.no_remote,
        dry_run: args.dry_run,
        use_gitoxide: settings.use_gitoxide,
        is_quiet: output.is_quiet(),
        remote_name: settings.remote.clone(),
        multi_remote_enabled: settings.multi_remote_enabled,
        multi_remote_default: settings.multi_remote_default.clone(),
    };

    let result = {
        let mut sink = OutputSink(output);
        rename::execute(&params, &mut sink)?
    };

    // Render result
    if result.dry_run {
        output.info("Dry run complete. No changes were made.");
    } else {
        let mut summary_parts = Vec::new();
        if result.branch_renamed {
            summary_parts.push(format!(
                "branch '{}' -> '{}'",
                result.old_branch, result.new_branch
            ));
        }
        if result.worktree_moved {
            summary_parts.push(format!(
                "worktree '{}' -> '{}'",
                result.old_path.display(),
                result.new_path.display()
            ));
        }
        if result.remote_renamed {
            summary_parts.push("remote branch updated".to_string());
        }

        output.success(&format!("Renamed: {}", summary_parts.join(", ")));

        for warning in &result.warnings {
            output.warning(warning);
        }
    }

    // Handle cd_target via DAFT_CD_FILE
    if let Some(ref cd_target) = result.cd_target {
        if let Ok(cd_file) = std::env::var(CD_FILE_ENV) {
            std::fs::write(&cd_file, cd_target.to_string_lossy().as_bytes()).ok();
        }
    }

    Ok(())
}
