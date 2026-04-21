use crate::{
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
#[command(name = "git-worktree-exec")]
#[command(version = crate::VERSION)]
#[command(about = "Run a command across one or more worktrees")]
#[command(long_about = r#"
Runs one or more commands against one or more selected worktrees without
changing the current directory.

Targets may be given as positional branch or worktree-directory names, or
globs against branch names (e.g. 'feat/*'). Use --all to target every
worktree in the repository. Positionals and --all are mutually exclusive.

Commands are expressed either as a literal argv after --, or as one or
more -x shell strings. The two forms are mutually exclusive. Multiple -x
values run sequentially per worktree; a failure stops that worktree but
does not stop other worktrees.

When a single worktree is targeted, stdio is fully inherited, making
interactive programs (claude, vim, fzf) work the same as if you had cd'd
into the worktree first.
"#)]
#[command(after_help = r#"EXAMPLES:
    Run a single command across all worktrees:
        daft exec --all -- npm test

    Run on specific branches (glob and exact mix):
        daft exec feat/auth 'feat/ui-*' -- cargo build

    Sequential with fail-fast:
        daft exec --all --sequential -- pnpm lint

    Pipeline of commands per worktree:
        daft exec --all -x 'mise install' -x 'pnpm build' -x 'pnpm test'

    Pass-through to an interactive program (single target):
        daft exec feat/auth -- claude

    Live "windows" output (like hooks):
        daft exec --all -v -- cargo test
"#)]
pub struct Args {
    #[arg(help = "Target worktree(s) by branch name, directory name, or glob")]
    pub targets: Vec<String>,

    #[arg(
        long = "all",
        conflicts_with = "targets",
        help = "Target every worktree in the repository"
    )]
    pub all: bool,

    #[arg(
        short = 'x',
        long = "exec",
        value_name = "CMD",
        help = "Shell command to run (repeatable); runs via $SHELL -c"
    )]
    pub exec: Vec<String>,

    #[arg(
        long = "sequential",
        conflicts_with = "keep_going",
        help = "Run worktrees one at a time and stop on first failure"
    )]
    pub sequential: bool,

    #[arg(
        long = "keep-going",
        help = "Run worktrees one at a time and continue through failures"
    )]
    pub keep_going: bool,

    #[arg(
        short,
        long,
        help = "Show hook-style live windows instead of the list-mode table"
    )]
    pub verbose: bool,

    /// Trailing command vector after `--`. Mutually exclusive with `-x`.
    #[arg(last = true, value_name = "CMD")]
    pub trailing: Vec<String>,
}

pub fn run() -> Result<()> {
    let _args = Args::parse_from(crate::get_clap_args("git-worktree-exec"));

    init_logging(_args.verbose);

    if !is_git_repository()? {
        anyhow::bail!("Not inside a Git repository");
    }

    let settings = DaftSettings::load()?;
    let config = OutputConfig::new(false, _args.verbose);
    let mut output = CliOutput::new(config);

    let wt_config = WorktreeConfig {
        remote_name: settings.remote.clone(),
        quiet: false,
    };
    let _git = GitCommand::new(wt_config.quiet).with_gitoxide(settings.use_gitoxide);
    let _project_root = get_project_root()?;

    if should_show_gitoxide_notice(settings.use_gitoxide) {
        output.warning("[experimental] Using gitoxide backend for git operations");
    }

    anyhow::bail!("daft exec is not yet implemented")
}
