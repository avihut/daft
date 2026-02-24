use crate::{
    core::{worktree::flow_adopt, OutputSink},
    git::should_show_gitoxide_notice,
    hooks::{HookContext, HookExecutor, HookType, HooksConfig, TrustLevel},
    logging::init_logging,
    output::{CliOutput, Output, OutputConfig},
    settings::DaftSettings,
    utils::*,
};
use anyhow::Result;
use clap::Parser;

#[derive(Parser)]
#[command(name = "git-worktree-flow-adopt")]
#[command(version = crate::VERSION)]
#[command(about = "Convert a traditional repository to worktree-based layout")]
#[command(long_about = r#"
WHAT THIS COMMAND DOES

Converts your existing Git repository from the traditional layout to daft's
worktree-based layout. After conversion:

  Before:                    After:
  my-project/                my-project/
  ├── .git/                  ├── .git/        (bare repository)
  ├── src/                   └── main/        (worktree)
  └── README.md                  ├── src/
                                 └── README.md

Your uncommitted changes (staged and unstaged) are preserved in the new
worktree. The command is safe to run - if anything fails, your repository
is restored to its original state.

ABOUT THE WORKTREE WORKFLOW

The worktree workflow eliminates Git branch switching friction by giving
each branch its own directory. Instead of switching branches within a
single directory, you navigate between directories - each containing
a different branch.

BENEFITS

- No more stashing: Each branch has its own working directory
- Parallel development: Work on multiple branches simultaneously
- Persistent context: Each worktree keeps its own IDE state, terminal
  history, and environment (.envrc, node_modules, etc.)
- Instant switching: Just cd to another directory
- Safe experimentation: Changes in one worktree never affect another

HOW TO WORK WITH IT

After adopting, use these commands:

  daft go <branch>
      Check out an existing branch into a new worktree

  daft start <new-branch>
      Create a new branch and worktree from current branch

  daft start <new-branch> main
      Create a new branch from a specific base branch

  daft prune
      Clean up worktrees for merged/deleted branches

Your directory structure grows as you work:

  my-project/
  ├── .git/
  ├── main/              # Default branch
  ├── feature/auth/      # Feature branch
  └── bugfix/login/      # Bugfix branch

REVERTING

To convert back to a traditional layout, use git-worktree-flow-eject(1).
"#)]
pub struct Args {
    #[arg(help = "Path to the repository to convert (defaults to current directory)")]
    repository_path: Option<std::path::PathBuf>,

    #[arg(
        short = 'q',
        long = "quiet",
        help = "Operate quietly; suppress progress reporting"
    )]
    quiet: bool,

    #[arg(
        short = 'v',
        long = "verbose",
        help = "Be verbose; show detailed progress"
    )]
    verbose: bool,

    #[arg(
        long = "trust-hooks",
        help = "Trust the repository and allow hooks to run without prompting"
    )]
    trust_hooks: bool,

    #[arg(long = "no-hooks", help = "Do not run any hooks from the repository")]
    no_hooks: bool,

    #[arg(
        long = "dry-run",
        help = "Show what would be done without making any changes"
    )]
    dry_run: bool,
}

pub fn run() -> Result<()> {
    let args = Args::parse_from(crate::get_clap_args("git-worktree-flow-adopt"));

    init_logging(args.verbose);

    if args.trust_hooks && args.no_hooks {
        anyhow::bail!("--trust-hooks and --no-hooks cannot be used together.");
    }

    let settings = DaftSettings::load_global()?;

    let config = OutputConfig::with_autocd(args.quiet, args.verbose, settings.autocd);
    let mut output = CliOutput::new(config);

    let original_dir = get_current_directory()?;

    if let Err(e) = run_adopt(&args, &settings, &mut output) {
        change_directory(&original_dir).ok();
        return Err(e);
    }

    Ok(())
}

fn run_adopt(args: &Args, settings: &DaftSettings, output: &mut dyn Output) -> Result<()> {
    let params = flow_adopt::AdoptParams {
        repository_path: args.repository_path.clone(),
        dry_run: args.dry_run,
        use_gitoxide: settings.use_gitoxide,
    };

    if should_show_gitoxide_notice(settings.use_gitoxide) {
        output.warning("[experimental] Using gitoxide backend for git operations");
    }

    if !params.dry_run {
        output.start_spinner("Converting to worktree layout...");
    }
    let result = {
        let mut sink = OutputSink(output);
        flow_adopt::execute(&params, &mut sink)?
    };
    output.finish_spinner();

    if result.dry_run {
        output.result(&format!(
            "Would convert to worktree layout with branch '{}' at '{}'",
            result.current_branch,
            result.worktree_path.display()
        ));
        return Ok(());
    }

    // Run post-adopt hook
    run_post_adopt_hook(args, &result, output)?;

    output.result(&format!(
        "Converted to worktree layout. Working directory: '{}/{}'",
        result.repo_display_name, result.current_branch
    ));

    output.cd_path(&get_current_directory()?);

    Ok(())
}

fn run_post_adopt_hook(
    args: &Args,
    result: &flow_adopt::AdoptResult,
    output: &mut dyn Output,
) -> Result<()> {
    if args.no_hooks {
        output.step("Skipping hooks (--no-hooks flag)");
        return Ok(());
    }

    let hooks_config = HooksConfig::default();
    let mut executor = HookExecutor::new(hooks_config)?;

    if args.trust_hooks {
        output.step("Trusting repository for hooks (--trust-hooks flag)");
        executor.trust_repository(&result.git_dir, TrustLevel::Allow)?;
    }

    let ctx = HookContext::new(
        HookType::PostClone,
        "adopt",
        &result.project_root,
        &result.git_dir,
        &result.remote_name,
        &result.worktree_path,
        &result.worktree_path,
        &result.current_branch,
    )
    .with_new_branch(false);

    let hook_result = executor.execute(&ctx, output)?;

    if hook_result.skipped {
        if let Some(reason) = &hook_result.skip_reason {
            if reason == "Repository not trusted" {
                executor.check_hooks_notice(&result.worktree_path, &result.git_dir, output);
            }
        }
    }

    Ok(())
}
